use crate::model::Probe;
use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// One sampled result for a host. `epoch` identifies which stream incarnation
/// produced it, so main can ignore errors from already-replaced streams.
pub struct Sample {
    pub target: String,
    pub result: Result<Probe, String>,
    pub at: Instant,
    pub epoch: u64,
}

const PROBE_SCRIPT: &str = include_str!("../probe.sh");

pub fn is_local(target: &str) -> bool {
    let t = target.trim();
    if t.eq_ignore_ascii_case("local") {
        return true;
    }
    let h = t.rsplit('@').next().unwrap_or(t);
    h == "localhost" || h == "127.0.0.1" || h == "::1"
}

/// Private key files residing in ~/.ssh (non-default names included).
///
/// Plain `ssh` only auto-offers id_rsa/id_ed25519/... plus the agent, so a key
/// saved under a custom filename is never tried unless ~/.ssh/config pins it
/// for that host. We collect every private key we can find and pass
/// `-i` for each so the machine's own keys are always offered, which is exactly
/// what "use the ssh key residing on this machine" should mean.
///
/// Cached after first use; passphrases stay irrelevant (BatchMode).
fn local_identities() -> Vec<String> {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Vec<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let home = std::env::var("HOME").unwrap_or_default();
            let dir = std::path::Path::new(&home).join(".ssh");
            let mut out = Vec::new();
            let rd = match std::fs::read_dir(&dir) {
                Ok(r) => r,
                Err(_) => return out,
            };
            for entry in rd.flatten() {
                let p = entry.path();
                if !p.is_file() {
                    continue;
                }
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                // skip obvious non-keys
                if name.ends_with(".pub")
                    || matches!(
                        name,
                        "config"
                            | "known_hosts"
                            | "known_hosts.old"
                            | "authorized_keys"
                            | "authorized_keys2"
                            | "id"
                            | "environment"
                    )
                {
                    continue;
                }
                if is_private_key_file(&p) {
                    out.push(p.to_string_lossy().into_owned());
                }
            }
            out.sort();
            out
        })
        .clone()
}

/// Peek at the file head for a private-key banner.
fn is_private_key_file(p: &std::path::Path) -> bool {
    use std::io::Read;
    let mut f = match std::fs::File::open(p) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut buf = [0u8; 64];
    let n = match f.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return false,
    };
    let head = String::from_utf8_lossy(&buf[..n]);
    head.starts_with("-----BEGIN") && head.contains("PRIVATE KEY")
}

/// Apply `-i <key>` for every local identity to an ssh Command.
fn with_local_identities(c: &mut Command) {
    for k in local_identities() {
        c.arg("-i").arg(k);
    }
}

/// Build the ssh command: system `ssh`, so it uses the key material already on
/// THIS machine — ssh-agent, ~/.ssh/config (Host aliases, IdentityFile), and the
/// default identity files (~/.ssh/id_ed25519, id_rsa...). BatchMode=yes means
/// it never hangs on a password prompt: key auth or fail fast.
fn ssh_cmd(target: &str) -> Command {
    let mut c = Command::new("ssh");
    c.arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("PreferredAuthentications=publickey")
        .arg("-o")
        .arg("ConnectTimeout=8")
        .arg("-o")
        .arg("ServerAliveInterval=15")
        .arg("-o")
        .arg("ServerAliveCountMax=2")
        .arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .arg("-o")
        .arg("NumberOfPasswordPrompts=0");
    with_local_identities(&mut c);
    c.arg("-T").arg(target);
    c
}

/// Run one probe on a worker thread; the result goes to `tx`.
pub fn spawn_probe(target: String, tx: mpsc::Sender<Sample>, overall_timeout: Duration) {
    let at = Instant::now();
    thread::spawn(move || {
        let result = run_probe_blocking(&target, &target, overall_timeout);
        let _ = tx.send(Sample {
            target,
            result,
            at,
            epoch: 0,
        });
    });
}

/// Wrap the one-shot probe script into a loop that emits one JSON line per
/// `sleep_secs` until killed. The probe body never calls `exit` at top level,
/// so each frame failure just skips to the next tick.
pub fn stream_script(sleep_secs: u64) -> String {
    let body = PROBE_SCRIPT
        .lines()
        .skip(1) // drop shebang
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "sshscope_frame() {{\n{body}\n}}\nwhile :; do\n  sshscope_frame\n  sleep {sleep_secs}\ndone\n"
    )
}

/// Persistent streaming sampler for one target. Spawns the local `ssh` client
/// (same local-key auth path as spawn_probe) with `bash -s` running the stream
/// script, reads stdout line-by-line and sends each parsed Probe on the channel.
/// When the stream dies (ssh exit, killed link, EOF) sends one Err Sample.
/// Returns the worker handle plus a pid channel so the owner can kill -TERM
/// the ssh process on respawn/pause.
pub fn spawn_stream(
    target: String,
    sshdest: String,
    sleep_secs: u64,
    tx: mpsc::Sender<Sample>,
    epoch: u64,
) -> (thread::JoinHandle<()>, mpsc::Receiver<u32>) {
    let (pidtx, pidrx) = mpsc::channel();
    let h = thread::spawn(move || {
        let script = stream_script(sleep_secs);
        let mut child = if is_local(&target) {
            Command::new("bash")
                .arg("-s")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
        } else {
            ssh_cmd_stream(&sshdest)
                .arg("bash -s")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
        };
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(Sample {
                    target,
                    result: Err(format!("spawn: {e}")),
                    at: Instant::now(),
                    epoch,
                });
                return;
            }
        };
        let _ = pidtx.send(child.id());

        // feed the script, close stdin
        if let Some(mut si) = child.stdin.take() {
            let _ = si.write_all(script.as_bytes());
            let _ = si.flush();
        }

        let stderr_tail: std::sync::Arc<std::sync::Mutex<String>> =
            std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        {
            let tail = stderr_tail.clone();
            let mut se = child.stderr.take();
            thread::spawn(move || {
                if let Some(mut se) = se.take() {
                    use std::io::Read;
                    let mut buf = [0u8; 512];
                    let mut acc = String::new();
                    while let Ok(n) = se.read(&mut buf) {
                        if n == 0 {
                            break;
                        }
                        acc.push_str(&String::from_utf8_lossy(&buf[..n]));
                        if acc.len() > 400 {
                            acc.drain(..acc.len() - 400);
                        }
                        let mut t = tail.lock().unwrap();
                        *t = acc.clone();
                    }
                }
            });
        }

        let out = child.stdout.take();
        let mut reason = String::new();
        if let Some(out) = out {
            let reader = std::io::BufReader::new(out);
            for line in reader.lines() {
                let at = Instant::now();
                match line {
                    Ok(l) => {
                        let l = l.trim().to_string();
                        if l.is_empty() || !l.starts_with('{') {
                            continue;
                        }
                        match serde_json::from_str::<crate::model::Probe>(&l) {
                            Ok(p) => {
                                if tx
                                    .send(Sample {
                                        target: target.clone(),
                                        result: Ok(p),
                                        at,
                                        epoch,
                                    })
                                    .is_err()
                                {
                                    reason = "consumer gone".into();
                                    break;
                                }
                            }
                            Err(e) => {
                                reason = format!("bad frame: {e}");
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        reason = format!("read: {e}");
                        break;
                    }
                }
            }
        } else {
            reason = "no stdout".into();
        }

        let _status = child.wait();
        if reason.is_empty() {
            reason = "stream ended".into();
        }
        let tail = stderr_tail.lock().unwrap().clone();
        let tail: String = tail
            .lines()
            .filter(|l| !l.contains("Permanently added"))
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .chars()
            .take(140)
            .collect();
        if !tail.is_empty() {
            reason = format!("{reason}: {tail}");
        }
        let _ = tx.send(Sample {
            target,
            result: Err(reason),
            at: Instant::now(),
            epoch,
        });
    });
    (h, pidrx)
}

/// ssh flags for a long-lived stream: faster dead-link detection than --once.
fn ssh_cmd_stream(target: &str) -> Command {
    let mut c = Command::new("ssh");
    c.arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("PreferredAuthentications=publickey")
        .arg("-o")
        .arg("ConnectTimeout=8")
        .arg("-o")
        .arg("ServerAliveInterval=4")
        .arg("-o")
        .arg("ServerAliveCountMax=3")
        .arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .arg("-o")
        .arg("NumberOfPasswordPrompts=0");
    with_local_identities(&mut c);
    c.arg("-T").arg(target);
    c
}

pub fn run_probe_blocking(target: &str, sshdest: &str, timeout: Duration) -> Result<Probe, String> {
    let mut child = if is_local(target) {
        let mut c = Command::new("bash")
            .arg("-s")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn bash: {e}"))?;
        if let Some(mut si) = c.stdin.take() {
            let _ = si.write_all(PROBE_SCRIPT.as_bytes());
        }
        c
    } else {
        let mut c = ssh_cmd(sshdest)
            .arg("bash -s")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn ssh: {e}"))?;
        // write script then close stdin: EOF triggers execution
        if let Some(mut si) = c.stdin.take() {
            let _ = si.write_all(PROBE_SCRIPT.as_bytes());
            let _ = si.flush();
        }
        c
    };

    // poll with deadline so a hung ssh can't wedge the sampler
    let start = Instant::now();
    let out = loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                break child
                    .wait_with_output()
                    .map_err(|e| format!("read output: {e}"))?
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("timeout after {}s", timeout.as_secs()));
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("wait: {e}")),
        }
    };

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg: String = err
            .lines()
            .filter(|l| !l.contains("Permanently added"))
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .chars()
            .take(160)
            .collect();
        return Err(if msg.is_empty() {
            format!("ssh exit {}", out.status.code().unwrap_or(-1))
        } else {
            msg
        });
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    // probe prints one JSON line; be forgiving about stray blank lines
    let line = stdout
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .ok_or("no JSON from probe")?;
    serde_json::from_str::<Probe>(line.trim()).map_err(|e| format!("bad probe JSON: {e}"))
}
