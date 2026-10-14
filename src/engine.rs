//! Engine payload parsers: one per inference engine family, mirroring the
//! target-side classification in probe.sh. Everything is derived from the
//! single captured payload — engine-native gauges/averages first, counter
//! arithmetic second — so an idle server still shows its last real numbers
//! instead of a blinking zero. Unknown field -> None, never a guess.

use crate::model::{EngineEntry, EngineInfo, Metrics, Probe};
use serde_json::Value;

/// one parsed engine kept per pid between probe refresh frames
#[derive(Debug, Clone)]
pub struct AttachedEngine {
    pub pid: u64,
    pub port: u64,
    pub info: EngineInfo,
}

/// probe.sh may emit booleans as `true`/`false` or as `0`/`1` depending on
/// version — accept both instead of dropping the whole frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct Boolish(pub bool);

impl<'de> serde::Deserialize<'de> for Boolish {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = serde_json::Value::deserialize(d)?;
        Ok(Boolish(match v {
            Value::Bool(b) => b,
            Value::Number(n) => n.as_f64().map(|x| x != 0.0).unwrap_or(false),
            Value::String(ref s) => s == "1" || s == "true",
            _ => false,
        }))
    }
}

/// Refresh the per-host engine cache from a probe frame. The probe ships
/// payloads only on its ~6s refresh frames (`"engines": [...]`); cached frames
/// carry `null` and reuse the last map. An explicit empty array means the
/// cycle ran and found nothing — that clears (a dead server must not linger).
pub fn update(cache: &mut Vec<AttachedEngine>, probe: &Probe) {
    let Some(entries) = &probe.engines else {
        return; // cached frame: keep what we have
    };
    // drop engines whose pid is no longer a GPU compute process at all
    let live: Vec<u64> = probe.gpus.iter().flat_map(|g| g.processes.iter().map(|p| p.pid)).collect();
    cache.retain(|e| live.contains(&e.pid));
    for en in entries {
        let info = entry_info(en);
        match cache.iter_mut().find(|e| e.pid == en.pid && e.port == en.port) {
            Some(slot) => slot.info = info,
            None => cache.push(AttachedEngine { pid: en.pid, port: en.port, info }),
        }
    }
    // a pid that lost its engine (port gone / no longer classified) must not
    // keep showing stale numbers
    let mapped: Vec<u64> = entries.iter().map(|e| e.pid).collect();
    cache.retain(|e| mapped.contains(&e.pid) || !live.contains(&e.pid));
}

/// attach cached engine info onto the derived GPU process rows (by pid)
pub fn attach(m: &mut Metrics, cache: &[AttachedEngine]) {
    for g in &mut m.gpus {
        for p in &mut g.procs {
            if let Some(e) = cache.iter().find(|e| e.pid == p.pid) {
                p.engine = Some(e.info.clone());
            }
        }
    }
}

fn entry_info(en: &EngineEntry) -> EngineInfo {
    let mut info = if en.moff.0 {
        let mut i = EngineInfo {
            kind: en.kind.clone(),
            ..Default::default()
        };
        i.note = Some("started without --metrics".into());
        i
    } else {
        parse(&en.kind, &en.health, &en.models, &en.raw)
    };
    info.port = en.port;
    if en.attr == "hint" {
        info.hint = true;
    }
    if info.model.is_none() {
        info.model = model_from(&en.models).or_else(|| model_from(&en.health));
    }
    info
}

pub fn parse(kind: &str, health: &str, models: &str, raw: &str) -> EngineInfo {
    let mut info = EngineInfo {
        kind: kind.to_string(),
        ..Default::default()
    };
    match kind {
        "strata" => parse_strata(raw, &mut info),
        "llama-legacy" => parse_llama_legacy(raw, &mut info),
        "llama-cpp" => parse_llama_prom(raw, &mut info),
        "vllm" => parse_vllm(raw, &mut info),
        "sglang" => parse_sglang(raw, &mut info),
        "tgi" => parse_tgi(raw, &mut info),
        _ => info.note = Some("metrics not understood".into()),
    }
    if info.model.is_none() {
        info.model = model_from(health).or_else(|| model_from(models));
    }
    info
}

/// pull "model": "..." out of a JSON blob (health payload or /v1/models)
fn model_from(text: &str) -> Option<String> {
    if let Some(v) = serde_json::from_str::<Value>(text).ok() {
        if let Some(m) = v.get("model").and_then(|x| x.as_str()) {
            return Some(m.to_string());
        }
        // /v1/models: {"data":[{"id":...}]} or {"models":[...]}
        if let Some(arr) = v.get("data").or_else(|| v.get("models")).and_then(|a| a.as_array()) {
            if let Some(first) = arr.first() {
                let s = first.as_str().map(str::to_string).or_else(|| {
                    first
                        .get("id")
                        .or_else(|| first.get("name"))
                        .and_then(|x| x.as_str())
                        .map(str::to_string)
                })?;
                return Some(s);
            }
        }
    }
    // The probe caps payloads to stay inside the frame budget, so the JSON is
    // often truncated: scan the raw text for the first quoted model-ish value.
    for key in ["\"model\":\"", "\"id\":\"", "\"name\":\""] {
        if let Some(i) = text.find(key) {
            let rest = &text[i + key.len()..];
            if let Some(j) = rest.find('"') {
                let v = &rest[..j];
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

fn fnum(v: &Value, path: &[&str]) -> Option<f64> {
    let mut cur = v;
    for k in path {
        cur = cur.get(k)?;
    }
    cur.as_f64().or_else(|| cur.as_i64().map(|x| x as f64))
}

/// shortest honest model-name trim for the one-row display
pub fn short_model(m: &str) -> String {
    let base = m.rsplit('/').next().unwrap_or(m);
    base.chars().take(24).collect()
}

// ---------------- llama.cpp (prometheus) ----------------
pub fn prom_lines(raw: &str) -> Vec<(String, f64)> {
    // one sample per line; greedy {..} is fine — the value is the last token
    raw.lines()
        .filter_map(|l| {
            let l = l.trim_start();
            if l.is_empty() || l.starts_with('#') {
                return None;
            }
            let (name, rest) = l.split_once(' ')?;
            let val = rest.split_whitespace().next()?;
            Some((name.to_string(), val.parse::<f64>().ok()?))
        })
        .collect()
}

fn prom_get<'a>(lines: &'a [(String, f64)], name: &str) -> Option<f64> {
    // sum across label series (multi-model servers)
    let mut acc = None::<f64>;
    for (n, v) in lines {
        if n == name || n.starts_with(&format!("{name}{{")) {
            acc = Some(acc.unwrap_or(0.0) + *v);
        }
    }
    acc
}

fn parse_llama_prom(raw: &str, info: &mut EngineInfo) {
    let l = prom_lines(raw);
    info.decode_tok_s = prom_get(&l, "llamacpp:predicted_tokens_seconds");
    info.prefill_tok_s = prom_get(&l, "llamacpp:prompt_tokens_seconds");
    info.running = prom_get(&l, "llamacpp:requests_processing");
    info.queued = prom_get(&l, "llamacpp:requests_deferred");
    if let Some(k) = prom_get(&l, "llamacpp:kv_cache_usage_perc") {
        info.kv_pct = Some(k * 100.0);
    }
    info.ctx_tokens = prom_get(&l, "llamacpp:n_tokens_max");
    // lifetime averages as idle fallback
    if let (Some(d), Some(ds)) = (
        prom_get(&l, "llamacpp:tokens_predicted_total"),
        prom_get(&l, "llamacpp:tokens_predicted_seconds_total"),
    ) {
        if ds > 0.0 {
            info.decode_avg = Some(d / ds);
        }
    }
    if let (Some(p), Some(ps)) = (
        prom_get(&l, "llamacpp:prompt_tokens_total"),
        prom_get(&l, "llamacpp:prompt_seconds_total"),
    ) {
        if ps > 0.0 {
            info.prefill_avg = Some(p / ps);
        }
    }
    // speculative decoding counters (added upstream 2026-08; absent on older
    // builds -> no field, no lie)
    if let (Some(accepted), Some(drafted)) = (
        prom_get(&l, "llamacpp:spec_decode_num_accepted_tokens_total"),
        prom_get(&l, "llamacpp:spec_decode_num_draft_tokens_total"),
    ) {
        if drafted > 0.0 {
            info.draft_accept_pct = Some(100.0 * accepted / drafted);
        }
    }
}

// ---------------- llama.cpp (legacy JSON /metrics) ----------------
fn parse_llama_legacy(raw: &str, info: &mut EngineInfo) {
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        info.note = Some("metrics JSON unparsable".into());
        return;
    };
    let u = || fnum(&v, &["util", "predicted_per_second"]);
    let p = || fnum(&v, &["util", "prompt_per_second"]);
    info.decode_tok_s = u();
    info.prefill_tok_s = p();
    if let Some(slots) = v.get("slots").and_then(|s| s.as_array()) {
        // aggregate over busy slots: sums divided by summed ms
        let mut dn = 0.0;
        let mut dms = 0.0;
        let mut pn = 0.0;
        let mut pms = 0.0;
        for s in slots {
            dn += fnum(s, &["predicted_n"]).unwrap_or(0.0);
            dms += fnum(s, &["predicted_ms"]).unwrap_or(0.0);
            pn += fnum(s, &["prompt_n"]).unwrap_or(0.0);
            pms += fnum(s, &["prompt_ms"]).unwrap_or(0.0);
        }
        if info.decode_tok_s.is_none() && dms > 0.0 {
            info.decode_tok_s = Some(dn / (dms / 1000.0));
        }
        if info.prefill_tok_s.is_none() && pms > 0.0 {
            info.prefill_tok_s = Some(pn / (pms / 1000.0));
        }
    }
}

// ---------------- Strata ----------------
fn parse_strata(raw: &str, info: &mut EngineInfo) {
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        info.note = Some("metrics JSON unparsable".into());
        return;
    };
    info.model = v["engine"]["model"].as_str().map(str::to_string);
    info.ctx_tokens = fnum(&v, &["engine", "max_context"]);
    info.decode_tok_s = fnum(&v, &["live", "tok_s"]);
    info.prefill_tok_s = fnum(&v, &["live", "prefill_tok_s_mean"]);
    info.running = fnum(&v, &["live", "running"]);
    info.queued = fnum(&v, &["live", "queued"]);
    info.decode_avg = fnum(&v, &["live", "tok_s_mean"]);
    // last real (finished) request drives the idle-time numbers: the array is
    // newest-first and the head can be an in-flight/cancelled stub
    if let Some(reqs) = v.get("requests").and_then(|r| r.as_array()) {
        if let Some(r) = reqs
            .iter()
            .find(|r| r.get("finish").and_then(Value::as_str).unwrap_or("") != "cancel")
            .or_else(|| reqs.first())
        {
            info.hit_pct = fnum(r, &["hit_rate"]).map(|x| x * 100.0);
            if info.decode_tok_s.is_none() || info.decode_tok_s == Some(0.0) {
                info.decode_avg = info
                    .decode_avg
                    .or_else(|| fnum(r, &["decode_tok_s"]));
            }
        }
    }
    // lifetime totals: averages + draft acceptance + overall cache hit
    let t = &v["totals"];
    if let (Some(pt), Some(du), Some(pm)) = (
        fnum(t, &["prompt_tokens"]),
        fnum(t, &["reused"]),
        fnum(t, &["prompt_ms"]),
    ) {
        if pm > 0.0 && pt >= du {
            info.prefill_avg = Some((pt - du) / (pm / 1000.0));
        }
    }
    if let (Some(ot), Some(dm)) = (fnum(t, &["output_tokens"]), fnum(t, &["decode_ms"])) {
        if dm > 0.0 {
            info.decode_avg = info.decode_avg.or(Some(ot / (dm / 1000.0)));
        }
    }
    if let (Some(do_), Some(da)) = (
        fnum(t, &["drafts_offered"]),
        fnum(t, &["drafts_accepted"]),
    ) {
        if do_ > 0.0 {
            info.draft_accept_pct = Some(100.0 * da / do_);
        }
    }
    if info.hit_pct.is_none() {
        if let (Some(pt), Some(du)) = (fnum(t, &["prompt_tokens"]), fnum(t, &["reused"])) {
            if pt > 0.0 {
                info.hit_pct = Some(100.0 * du / pt);
            }
        }
    }
    match v["live"]["phase"].as_str() {
        Some("prompt") | Some("prefill") => info.note = Some("prefilling".into()),
        _ => {}
    }
}

// ---------------- vLLM ----------------
fn parse_vllm(raw: &str, info: &mut EngineInfo) {
    let l = prom_lines(raw);
    info.running = prom_get(&l, "vllm:num_requests_running");
    info.queued = prom_get(&l, "vllm:num_requests_waiting");
    if let Some(k) = prom_get(&l, "vllm:gpu_cache_usage_perc")
        .or_else(|| prom_get(&l, "vllm:kv_cache_usage_perc"))
    {
        info.kv_pct = Some(k * 100.0);
    }
    if let (Some(sum), Some(cnt)) = (
        prom_get(&l, "vllm:request_time_per_output_token_seconds_sum"),
        prom_get(&l, "vllm:request_time_per_output_token_seconds_count"),
    ) {
        if sum > 0.0 && cnt > 0.0 {
            info.decode_avg = Some(cnt / sum);
        }
    }
    // prompt cache hit rate is exposed as a ready-made gauge on recent builds
    if let Some(h) = prom_get(&l, "vllm:gpu_prefix_cache_hit_rate") {
        info.hit_pct = Some(h * 100.0);
    }
}

// ---------------- SGLang ----------------
fn parse_sglang(raw: &str, info: &mut EngineInfo) {
    let l = prom_lines(raw);
    info.running = prom_get(&l, "sglang:num_running_reqs");
    info.queued = prom_get(&l, "sglang:num_queue_reqs");
    info.decode_tok_s = prom_get(&l, "sglang:gen_throughput");
    if let Some(k) = prom_get(&l, "sglang:token_usage") {
        info.kv_pct = Some(k * 100.0);
    }
    if let Some(h) = prom_get(&l, "sglang:cache_hit_rate") {
        info.hit_pct = Some(h * 100.0);
    }
}

// ---------------- TGI ----------------
fn parse_tgi(raw: &str, info: &mut EngineInfo) {
    let l = prom_lines(raw);
    info.running = prom_get(&l, "tgi_batch_current_size");
    info.queued = prom_get(&l, "tgi_batch_next_size");
    if let Some(k) = prom_get(&l, "tgi_slot_cache_usage_perc") {
        info.kv_pct = Some(if k <= 1.0 { k * 100.0 } else { k });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llama_prom_reads_gauges_and_averages() {
        let raw = "# HELP x\nllamacpp:prompt_tokens_total 282030\n\
                   llamacpp:prompt_seconds_total 240.94\n\
                   llamacpp:tokens_predicted_total 44042\n\
                   llamacpp:tokens_predicted_seconds_total 791.224\n\
                   llamacpp:prompt_tokens_seconds 1170.54\n\
                   llamacpp:predicted_tokens_seconds 55.6631\n\
                   llamacpp:requests_processing 1\n\
                   llamacpp:requests_deferred 0\n\
                   llamacpp:kv_cache_usage_perc 0.12\n\
                   llamacpp:n_tokens_max 11937\n\
                   llamacpp:spec_decode_num_draft_tokens_total 100\n\
                   llamacpp:spec_decode_num_accepted_tokens_total 62\n";
        let i = parse("llama-cpp", "", "", raw);
        assert_eq!(i.decode_tok_s, Some(55.6631));
        assert_eq!(i.prefill_tok_s, Some(1170.54));
        assert_eq!(i.kv_pct, Some(12.0));
        assert_eq!(i.ctx_tokens, Some(11937.0));
        assert_eq!(i.draft_accept_pct, Some(62.0));
        let d = i.decode_avg.unwrap();
        assert!((d - 55.6627).abs() < 0.1);
    }

    #[test]
    fn strata_full_payload() {
        let raw = r#"{"engine":{"model":"qwen-27b","max_context":262144},
            "live":{"state":"idle","queued":0,"phase":null,"tok_s":null,"prefill_tok_s_mean":null,"running":0},
            "requests":[{"finish":"stop","decode_tok_s":108.6,"hit_rate":0.992,"prompt_ms":1004.4,"prompt_tokens":74151,"reused":73968}],
            "totals":{"requests":270,"prompt_tokens":27548362,"reused":27032223,"output_tokens":190472,"prompt_ms":630193.9,"decode_ms":1695440.6,"drafts_offered":155792,"drafts_accepted":114837}}"#;
        let i = parse("strata", r#"{"service":"strata"}"#, "", raw);
        assert_eq!(i.model.as_deref(), Some("qwen-27b"));
        assert_eq!(i.ctx_tokens, Some(262144.0));
        assert_eq!(i.decode_avg, Some(108.6));
        assert_eq!(i.hit_pct, Some(99.2));
        let ap = i.prefill_avg.unwrap();
        assert!((ap - 824.0).abs() < 5.0); // (27548362-27032223)/(630.19s)
        assert_eq!(i.draft_accept_pct, Some(100.0 * 114837.0 / 155792.0));
    }

    #[test]
    fn strata_cancel_stub_head_skipped() {
        let raw = r#"{"engine":{},"live":{"tok_s":null},
            "requests":[{"finish":"cancel","hit_rate":0.0},{"finish":"stop","decode_tok_s":70.0,"hit_rate":0.5}],
            "totals":{}}"#;
        let i = parse("strata", "", "", raw);
        assert_eq!(i.decode_avg, Some(70.0));
        assert_eq!(i.hit_pct, Some(50.0));
    }

    #[test]
    fn vllm_counters_and_kv() {
        let raw = "vllm:num_requests_running 3\nvllm:num_requests_waiting 1\n\
                   vllm:gpu_cache_usage_perc 0.42\n\
                   vllm:request_time_per_output_token_seconds_sum 55.0\n\
                   vllm:request_time_per_output_token_seconds_count 1100.0\n";
        let i = parse("vllm", "", "", raw);
        assert_eq!(i.running, Some(3.0));
        assert_eq!(i.queued, Some(1.0));
        assert_eq!(i.kv_pct, Some(42.0));
        assert_eq!(i.decode_avg, Some(20.0));
    }

    #[test]
    fn unparsable_json_is_a_note_not_a_panic() {
        let i = parse("strata", "", "", "{trunc");
        assert_eq!(i.note.as_deref(), Some("metrics JSON unparsable"));
        assert_eq!(i.decode_tok_s, None);
    }

    #[test]
    fn model_from_v1_models_data() {
        let m = model_from(r#"{"data":[{"id":"demo-lab/embed-v2","object":"model"}]}"#);
        assert_eq!(m.as_deref(), Some("demo-lab/embed-v2"));
    }

    #[test]
    fn prom_lines_skips_comments_and_handles_labels() {
        let l = prom_lines("# help\nmetric_a{model=\"x\"} 1.5 171\nbad line\n");
        assert_eq!(l.len(), 1);
        assert_eq!(prom_get(&l, "metric_a"), Some(1.5));
    }

    #[test]
    fn boolish_accepts_int_and_string_forms() {
        // older probes emitted moff as 0/1; a whole frame must not die on it
        for (txt, want) in [("true", true), ("false", false), ("1", true), ("0", false), ("\"1\"", true)] {
            let j = format!(r#"{{"kind":"x","port":1,"pid":1,"attr":"fd","t":0,"moff":{txt}}}"#);
            let e: crate::model::EngineEntry = serde_json::from_str(&j).unwrap();
            assert_eq!(e.moff.0, want, "{txt}");
        }
    }

    #[test]
    fn model_from_survives_truncated_json() {
        // probe caps /v1/models mid-object: string scan still finds the id
        let trunc = "{\"models\":[{\"name\":\"Qwen3.5-9B.gguf\",\"model\":\"Qwen3.5-9B.gguf\",\"modified_at\":\"";
        assert_eq!(model_from(trunc).as_deref(), Some("Qwen3.5-9B.gguf"));
    }

    #[test]
    fn update_null_keeps_cache_and_attach_lands_on_procs() {
        use crate::model::Probe;
        let mk = |engines: Option<&str>| -> Probe {
            serde_json::from_str(&format!(
                r#"{{"v":1,"epoch":1,"hostname":"h","kernel":"k","arch":"a","uptime_s":1,
                "cpu":{{"model":"m","mhz":0,"hz":0,"ncpu":1,"busy":0,"idle":0,"load":[0,0,0],"cores":[]}},
                "mem":{{"total_kb":1,"avail_kb":1,"free_kb":1,"buffers_kb":1,"cached_kb":1,"sreclaim_kb":1,"swap_total_kb":0,"swap_free_kb":0}},
                "disks":[],"zpools":[],"zdatasets":[],"nets":[],
                "temp":{{"cpu_c":null,"cpu_src":null,"nvme_c":null,"sensors":[]}},
                "gpus":[{{"vendor":"nvidia","idx":0,"name":"X","uuid":"","util":1,"mem_total_mb":1,"mem_used_mb":1,"temp_c":null,"fan_pct":null,"power_w":null,"power_cap_w":null,
                          "processes":[{{"pid":42,"name":"llama-server --port 8080","user":"u","mem_mb":100}}]}}],
                "engines":{}}}"#,
                engines.unwrap_or("null")
            ))
            .unwrap()
        };
        let mut cache = Vec::new();
        // refresh frame
        let one = r#"[{"kind":"llama-cpp","port":8080,"pid":42,"attr":"fd","t":1,"moff":false,
                     "health":"ok","models":"","raw":"llamacpp:predicted_tokens_seconds 41.5\nllamacpp:requests_processing 1\n"}]"#;
        update(&mut cache, &mk(Some(one)));
        assert_eq!(cache.len(), 1);
        // cached frame (null): keeps
        update(&mut cache, &mk(None));
        assert_eq!(cache.len(), 1);
        // explicit empty array: the cycle ran and found nothing -> clears
        update(&mut cache, &mk(Some("[]")));
        assert!(cache.is_empty());
        // attach path end-to-end
        update(&mut cache, &mk(Some(one)));
        let p = mk(Some("[]"));
        let prev = mk(Some("[]"));
        let mut m = crate::model::derive(&p, &prev, 1.0);
        attach(&mut m, &cache);
        let e = m.gpus[0].procs[0].engine.as_ref().unwrap();
        assert_eq!(e.kind, "llama-cpp");
        assert_eq!(e.decode_tok_s, Some(41.5));
    }

    #[test]
    fn short_model_trims_path() {
        assert_eq!(short_model("/models/Qwen3.8-Flash-Next-GSQ-RCO-abliterated-IQ3_S.gguf"),
                   "Qwen3.8-Flash-Next-GSQ-R");
    }
}
