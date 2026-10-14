#!/usr/bin/env bash
# skimonitor remote probe — runs ON THE TARGET via `ssh ... bash -s`.
# Requires only bash + coreutils (no jq/python). Prints exactly ONE JSON object.
# All counters are raw; the caller diffs consecutive samples for rates.
set -u
export LC_ALL=C

jstr() { # $1 -> JSON-safe string. Escapes \ and ", encodes tab/newline/CR as
  # \t \n \r, drops remaining C0 controls + DEL. Pure bash: sed is line-based
  # and would pass an embedded newline straight through the JSON string.
  local s=$1 c
  s=${s//\\/\\\\}
  s=${s//\"/\\\"}
  s=${s//$'\t'/\\t}
  s=${s//$'\n'/\\n}
  s=${s//$'\r'/\\r}
  for c in $'\001' $'\002' $'\003' $'\004' $'\005' $'\006' $'\007' $'\010' \
           $'\013' $'\014' $'\016' $'\017' $'\020' $'\021' $'\022' $'\023' \
           $'\024' $'\025' $'\026' $'\027' $'\030' $'\031' $'\032' $'\033' \
           $'\034' $'\035' $'\036' $'\037' $'\177'; do
    s=${s//$c/}
  done
  printf '%s' "$s"
}
num() { # $1 -> sanitized numeric, or $2 if given, else 0. Note ${2-0} not ${2:-0}:
  # an explicitly passed empty fallback must stay empty (nz maps it to null).
  local v=${1//[^0-9.eE+-]/}
  [[ "$v" =~ ^-?[0-9]+(\.[0-9]+)?$ ]] && printf '%s' "$v" || printf '%s' "${2-0}"
}
nz() { # numeric-or-null (non-numeric / negative / empty -> null)
  local raw=${1:-} v
  v=$(num "$raw" "")
  [ -z "$v" ] && { printf 'null'; return; }
  case $v in -*) printf 'null'; return;; esac
  printf '%s' "$v"
}

HOST=$(hostname 2>/dev/null || echo unknown)
UPT=$(awk '{print int($1)}' /proc/uptime 2>/dev/null)
KVER=$(uname -r 2>/dev/null)
ARCH=$(uname -m 2>/dev/null)
NOW=$(date +%s 2>/dev/null)

# ---------- CPU per-core busy/idle counters ----------
HZ=$(getconf CLK_TCK 2>/dev/null); HZ=${HZ:-100}
declare -A BUSY IDLE
CPUS=()
while read -r line; do
  [[ $line == cpu[0-9]* ]] || continue
  set -- $line
  id=${1#cpu}
  idle=$(( ${5:-0} + ${6:-0} ))
  busy=$(( ${2:-0} + ${3:-0} + ${4:-0} + ${7:-0} + ${8:-0} + ${9:-0} ))
  BUSY[$id]=$busy; IDLE[$id]=$idle; CPUS+=("$id")
done < /proc/stat
NCPU=${#CPUS[@]}

# aggregate line — SAME accounting as the per-core loop below:
# idle = idle+iowait; busy = user+nice+system+irq+softirq+steal.
# guest/guest_nice are already inside user/nice and must not be added again.
AGG_BUSY=0; AGG_IDLE=0
while read -r line; do
  [[ $line == cpu\ * ]] || continue
  set -- $line
  AGG_IDLE=$(( ${5:-0} + ${6:-0} ))
  AGG_BUSY=$(( ${2:-0} + ${3:-0} + ${4:-0} + ${7:-0} + ${8:-0} + ${9:-0} ))
  break
done < /proc/stat

# core-id topology (physical cores; HT siblings share core_id)
declare -A CPUCORE
for f in /sys/devices/system/cpu/cpu*/topology/core_id; do
  [ -r "$f" ] || continue
  c=$(basename "$(dirname "$(dirname "$f")")"); c=${c#cpu}
  [[ $c == [0-9]* ]] || continue
  CPUCORE[$c]=$(cat "$f" 2>/dev/null)
done

# per-core MHz: sysfs cpufreq first, /proc/cpuinfo 'cpu MHz' as fallback (VMs)
declare -A MHZINFO
while read -r proc mhzv; do MHZINFO[$proc]=$mhzv; done < <(awk '/^processor/{p=$3} /^cpu MHz/{gsub(/\.[0-9]+$/,"",$4); print p, $4}' /proc/cpuinfo 2>/dev/null)

CORES=''
for id in "${CPUS[@]}"; do
  core=${CPUCORE[$id]:-$id}
  mhz=$(awk '{printf "%.0f", $1/1000}' /sys/devices/system/cpu/cpu$id/cpufreq/scaling_cur_freq 2>/dev/null)
  [ -z "$mhz" ] && mhz=${MHZINFO[$id]:-}
  mhz=$(num "${mhz:-0}" 0)
  [ -n "$CORES" ] && CORES+=','
  CORES+="{\"id\":$id,\"core\":$core,\"busy\":${BUSY[$id]},\"idle\":${IDLE[$id]},\"mhz\":$mhz}"
done

MODEL=$(awk -F': +' '/model name|Model/{print $2; exit}' /proc/cpuinfo 2>/dev/null)
[ -z "$MODEL" ] && MODEL=$(awk -F': +' '/Hardware|model/{print $2; exit}' /proc/cpuinfo 2>/dev/null)
MHZ=$(awk '{s+=$1; n++} END{if(n>0) printf "%.0f", s/n/1000; else print 0}' \
      /sys/devices/system/cpu/cpu*/cpufreq/scaling_cur_freq 2>/dev/null)
MHZ=${MHZ:-0}

# ---------- loadavg ----------
L1=0; L5=0; L15=0
{ read -r L1 L5 L15 _; } < /proc/loadavg

# ---------- memory ----------
mk() { awk -v k="$1:" '$1==k{print $2}' /proc/meminfo; }
MT=$(mk MemTotal); MA=$(mk MemAvailable)
MF=$(mk MemFree)
BC=$(mk Buffers); CA=$(mk Cached); SH=$(mk Shmem); SW=$(mk SwapCached)
SR=$(mk SReclaimable)
ST=$(mk SwapTotal); SF=$(mk SwapFree)
for v in MT MA MF BC CA SH SW ST SF SR; do [ -z "${!v}" ] && eval "$v=0"; done
MT=$(num $MT 0); MA=$(num $MA 0); MF=$(num $MF 0)

# ---------- disks (capacity + io counters) ----------
declare -A RSEC WSEC MAJMIN_R MAJMIN_W
while read -r mj mn nm f4 f5 r f7 f8 f9 w _rest; do
  RSEC[$nm]=$r; WSEC[$nm]=$w
  MAJMIN_R[$mj:$mn]=$r; MAJMIN_W[$mj:$mn]=$w
done < /proc/diskstats

DISKS=''
while read -r src fstype size used avail ipct mp _; do
  case $fstype in ext[234]|xfs|btrfs|zfs|f2fs|jfs|vfat|exfat|nilfs2|reiserfs) ;; *) continue ;; esac
  [ -z "$size" ] && continue
  [ "$size" = "0" ] && continue
  pct=${ipct%\%}; [ "$pct" = "-" ] && pct=0
  base=$(basename "$src" 2>/dev/null)
  r=${RSEC[$base]:-}; w=${WSEC[$base]:-}
  if [ -z "$r" ]; then
    mm=$(stat -c '%H:%L' "$src" 2>/dev/null)   # major:minor — works for LVM/multipath
    if [ -n "$mm" ]; then r=${MAJMIN_R[$mm]:-0}; w=${MAJMIN_W[$mm]:-0}; fi
  fi
  if [ -z "$r" ] && [ -n "$base" ]; then
    stem=$(printf '%s' "$base" | sed -e 's/[0-9]*$//' -e 's/p$//')
    r=${RSEC[$stem]:-0}; w=${WSEC[$stem]:-0}
  fi
  r=$(num "${r:-0}" 0); w=$(num "${w:-0}" 0)
  mpj=$(jstr "$mp"); sj=$(jstr "$src"); fj=$(jstr "$fstype")
  [ -n "$DISKS" ] && DISKS+=','
  DISKS+="{\"mount\":\"$mpj\",\"device\":\"$sj\",\"fs\":\"$fj\",\"size\":$(num "$size" 0),\"used\":$(num "$used" 0),\"avail\":$(num "$avail" 0),\"use_pct\":$pct,\"rsec\":$r,\"wsec\":$w}"
done < <(df -PT -B1 2>/dev/null | awk 'NR>1 && $1!="Filesystem"')

# ---------- ZFS pools (df blind spot: zvols + pool-level usage) ----------
ZPOOLS=''
if command -v zpool >/dev/null 2>&1; then
  while read -r nm sz al fr cap st rest; do
    [ -z "$nm" ] && continue
    [ -n "$ZPOOLS" ] && ZPOOLS+=','
    ZPOOLS+="{\"name\":\"$(jstr "$nm")\",\"size\":$(num "${sz//[^0-9]/}" 0),\"alloc\":$(num "${al//[^0-9]/}" 0),\"free\":$(num "${fr//[^0-9]/}" 0),\"cap\":$(num "${cap%\%}" 0),\"state\":\"$(jstr "${st:-UNKNOWN}")\"}"
  done < <(zpool list -p -o name,size,alloc,free,cap,health 2>/dev/null | tail -n +2)
fi

# ---------- ZFS datasets (quota-based truth for subvol/vm zvols) ----------
ZDATS=''
if command -v zfs >/dev/null 2>&1; then
  while IFS=$'\t' read -r nm ty us qt rq av vs; do
    [ -z "$nm" ] && continue
    case $qt in none|'-') qv=-1 ;; 0) qv=-1 ;; *) qv=$(num "${qt//[^0-9]/}" -1) ;; esac
    case $rq in none|'-') rv=-1 ;; 0) rv=-1 ;; *) rv=$(num "${rq//[^0-9]/}" -1) ;; esac
    case $vs in none|'-') vv=-1 ;; 0) vv=-1 ;; *) vv=$(num "${vs//[^0-9]/}" -1) ;; esac
    [ -n "$ZDATS" ] && ZDATS+=','
    ZDATS+="{\"name\":\"$(jstr "$nm")\",\"type\":\"$(jstr "$ty")\",\"used\":$(num "${us//[^0-9]/}" 0),\"quota\":$qv,\"refquota\":$rv,\"volsize\":$vv,\"avail\":$(num "${av//[^0-9]/}" 0)}"
  done < <(zfs list -H -p -o name,type,used,quota,refquota,avail,volsize 2>/dev/null \
           | awk -F'\t' '($2=="volume" || ($4!="none" && $4!="-") || ($5!="none" && $5!="-")) && $1!~/@/ { print }' \
           | sort -t$'\t' -k3,3nr | head -40)
fi

# ---------- network ----------
NETS=''
while read -r name rx tx; do
  case $name in lo|docker*|veth*|br-*|virbr*|tap*|tun*|kube*|flannel*|cali*|cni*|tailscale0) continue ;; esac
  st=$(cat /sys/class/net/$name/operstate 2>/dev/null); st=${st:-unknown}
  sp=$(cat /sys/class/net/$name/speed 2>/dev/null); sp=${sp:--1}
  [ "$sp" = "-1" ] && [ -e /sys/class/net/$name/device/speed ] && sp=$(cat /sys/class/net/$name/device/speed 2>/dev/null || echo -1)
  nj=$(jstr "$name")
  [ -n "$NETS" ] && NETS+=','
  NETS+="{\"name\":\"$nj\",\"rx\":$rx,\"tx\":$tx,\"state\":\"$st\",\"speed\":$(num $sp -1)}"
done < <(awk 'NR>2{gsub(/:/,"",$1); print $1, $2, $10}' /proc/net/dev)

# ---------- temperatures ----------
# priority: coretemp/k10temp/zenpower (max of sensors) -> acpitz/*thermal -> nvme
CT='null'; CS='null'
TS='[]'
pick_max() { # $1 = hwmon dir; echo max temp millideg or empty
  local d=$1 best='' v
  for f in "$d"/temp*_input; do
    [ -r "$f" ] || continue
    v=$(cat "$f" 2>/dev/null) || continue
    [ -z "$v" ] && continue
    { [ -z "$best" ] || [ "$v" -gt "$best" ] 2>/dev/null; } && best=$v
  done
  [ -n "$best" ] && awk -v x="$best" 'BEGIN{printf "%.1f", x/1000}'
}
collect_sensors() { # $1 = hwmon dir -> JSON list of {label,c}
  local d=$1 out='' lbl v
  for f in "$d"/temp*_input; do
    [ -r "$f" ] || continue
    v=$(cat "$f" 2>/dev/null) || continue
    [ -z "$v" ] && continue
    n=${f%_input}; n=${n##*/temp}
    lbl=$(cat "$d/temp${n}_label" 2>/dev/null)
    [ -z "$lbl" ] && lbl="temp$n"
    lbl=$(jstr "$lbl")
    c=$(awk -v x="$v" 'BEGIN{printf "%.1f", x/1000}')
    [ -n "$out" ] && out+=','
    out+="{\"label\":\"$lbl\",\"c\":$c}"
  done
  printf '[%s]' "$out"
}
for want in coretemp k10temp zenpower; do
  for d in /sys/class/hwmon/hwmon*; do
    [ -r "$d/name" ] || continue
    [ "$(cat "$d/name")" = "$want" ] || continue
    t=$(pick_max "$d") || true
    [ -n "$t" ] && { CT=$t; CS="\"$want\""; TS=$(collect_sensors "$d"); break 2; }
  done
done
[ "$CT" = null ] && for d in /sys/class/hwmon/hwmon*; do
  [ -r "$d/name" ] || continue
  lab=$(cat "$d/name")
  case $lab in acpitz*|cpu_thermal|soc_thermal|cpu-thermal)
    t=$(pick_max "$d") || true
    [ -n "$t" ] && { CT=$t; CS=\"$lab\"; break; } ;;
  esac
done
NT='null'
for d in /sys/class/hwmon/hwmon*; do
  [ -r "$d/name" ] || continue
  [ "$(cat "$d/name")" = nvme ] || continue
  t=$(cat "$d/temp1_input" 2>/dev/null) || true
  [ -n "$t" ] && { NT=$(awk -v x=$t 'BEGIN{printf "%.1f", x/1000}'); break; }
done
[ "$NT" = null ] && { NT='null'; }

# ---------- inference engine probing ----------
# A GPU compute process is usually an inference server (llama.cpp, vLLM, SGLang,
# TGI, Strata). For every compute pid: find the port(s) it listens on, probe
# /metrics + /health + /v1/models on 127.0.0.1, classify the engine from the
# payload shape, and cache a compact entry. The stream shell keeps vars between
# the wrapper's 1s re-invocations, so a full cycle runs every ENG_EVERY seconds
# (6s hard budget -> never stalls a frame into the watchdog window). The client
# parses each entry and renders an inference line under the matching GPU procs.
# curl + coreutils only; no temp files, nothing written to disk, read-only GETs.
command -v curl >/dev/null 2>&1 || ENG_OK=0

eng_curl() { # $1 = port, $2 = path, $3 = max-time -> body (empty on fail/HTML)
  local b
  b=$(timeout 4 curl -s -m "$3" "http://127.0.0.1:$1$2" 2>/dev/null) || return 0
  case "$b" in *'<'*|'') return 0 ;; esac
  printf '%s' "$b"
}
pid_socket_inodes() { # $1 = pid -> inodes of its socket fds (empty when the
  # dir belongs to another user — the signal to use the fallback path)
  ls -l "/proc/$1/fd" 2>/dev/null \
    | sed -n 's/.* -> socket:\[\([0-9]*\)\]$/\1/p' | sort -u
}
listen_ino_port() { # -> "<ino> <port>" per LISTEN tcp socket (ino: from ss -e
  # is shown even for other users' pids — ownership resolves via our own fd scan)
  timeout 4 ss -tlnHeH 2>/dev/null | awk '
    { ino=""; loc=""
      for (i = 1; i <= NF; i++) {
        if ($i ~ /^ino:[0-9]+$/) { ino = substr($i, 5) }
        else if (loc == "" && $i ~ /:[0-9]+$/) { n = split($i, a, ":"); loc = a[n] }
      }
      if (ino != "" && loc != "") print ino, loc }'
}
proc_ports() { # $1 = pid -> up to 4 decimal LISTEN ports owned by the pid.
  # Echoes the sentinel X when attribution is impossible non-root (fd dir
  # unreadable or ss lacks -e), telling the caller to use hint fallbacks.
  local pid=$1 inodes got
  inodes=$(pid_socket_inodes "$pid")
  [ -n "$inodes" ] || { printf 'X'; return 0; }
  [ -n "${ENG_HAS_E:-}" ] || { printf 'X'; return 0; }
  got=$(listen_ino_port | awk 'NR==FNR{want[$0]=1;next}
      ($1 in want) && $2+0>0 && $2+0<65536 {print $2}' \
      <(printf '%s\n' "$inodes") - | sort -un | head -4)
  printf '%s' "$got"
}
cmdline_ports() { # $1 = pid -> ports from --port-style flags in the argv
  tr '\0' '\n' < "/proc/$1/cmdline" 2>/dev/null | awk '
    p ~ /(--api-port|--port|--http-port|--grpc-port)$/ && $0 ~ /^[0-9]+$/ { print $0 }
    /^(--api-port|--port|-p)=[0-9]+$/ { split($0, a, "="); print a[2] }
    { p = $0 }' | awk '$1 > 0 && $1 < 65536' | sort -un | head -4
}
netns_ports() { # $1 = pid -> LISTEN ports in the pid's netns (world-readable
  # table; ownership unknown). Server-ish ports first; payload classification
  # is the final gate either way.
  local p out=''
  while read -r _ loc _ st _; do
    [ "$st" = "0A" ] || continue
    p=${loc##*:}
    case $p in *[!0-9A-Fa-f]*) continue ;; esac
    out+="$((16#$p)) "
  done < <(cat "/proc/$1/net/tcp" "/proc/$1/net/tcp6" 2>/dev/null)
  [ -n "$out" ] || return 0
  printf '%s\n' "$out" | tr ' ' '\n' | grep -v '^$' | sort -un | awk '
    { order[++n] = $1 }
    END { for (i = 1; i <= n; i++) if (order[i] == 8000 || order[i] == 8080 || \
          order[i] == 5000 || order[i] == 11434 || (order[i] >= 18000 && order[i] < 19000)) \
            print order[i]
          for (i = 1; i <= n; i++) if (!(order[i] == 8000 || order[i] == 8080 || \
          order[i] == 5000 || order[i] == 11434 || (order[i] >= 18000 && order[i] < 19000))) \
            print order[i] }' | head -4
}
eng_docker_hostports() { # $1 = internal port, $2 = 12-hex cid -> host-published port
  # docker ps {{.Ports}} lines carry '0.0.0.0:18080->8080/tcp'; pull the HOST
  # side of every mapping whose CONTAINER side is $1, for this pid's container.
  local line
  [ -n "$2" ] || return 0
  line=$(printf '%s\n' "${ENG_DOCKER_MAP:-}" | awk -v c="$2" 'index($1, c)==1 {print substr($0, length($1)+2); exit}')
  [ -n "$line" ] || return 0
  printf '%s' "$line" | tr ',' '\n' | grep -oE "[0-9]+->$1(/[a-z0-9]+)?" \
    | cut -d- -f1 | grep -E '^[0-9]+$' | sort -un | head -2
}
eng_docker_map() { # one CLI call, cached ~30s (containers change rarely)
  local now; now=$(date +%s 2>/dev/null || echo 0)
  if [ -z "${ENG_DOCKER_TS:-}" ] || [ $((now - ENG_DOCKER_TS)) -ge 30 ] || [ "${1:-}" = force ]; then
    if command -v docker >/dev/null 2>&1; then
      ENG_DOCKER_MAP=$(timeout 2 docker ps --format '{{.ID}} {{.Ports}}' 2>/dev/null || true)
    else
      ENG_DOCKER_MAP=''
    fi
    ENG_DOCKER_TS=$now
  fi
}
looks_like_server() { # $1 = pid -> 0 when argv/binary names an inference server
  local a
  a=$(tr ' ' ' ' < "/proc/$1/cmdline" 2>/dev/null | head -c 400)
  case $a in
    *llama-server*|*vllm*|*sglang*|*text-generation*|*tgi*|*strata*|*ollama*|\
    *api_server*|*--serve*|*--port*|*--model*) return 0 ;;
  esac
  return 1
}
eng_classify() { # $1 = payload -> engine id ('' = not an engine). ORDER
  # matters: llamacpp: prom must win before any generic prom catch-all.
  case $1 in
    *llamacpp:*)       echo llama-cpp ;;
    *'"live"'*)        echo strata ;;
    *'"prompt_ms"'*)   echo llama-legacy ;;
    *vllm:*)           echo vllm ;;
    *sglang:*)         echo sglang ;;
    *tgi_*)            echo tgi ;;
    *process_cpu*)     echo prom ;;
    *)                 echo '' ;;
  esac
}
eng_trim_prom() { # stdin = metrics text, $1 = kind -> only families we parse
  case $1 in
    llama-cpp) grep -E '^llamacpp:' | head -80 ;;
    vllm) grep -E '^vllm:(gpu_cache_usage_perc|kv_cache_usage_perc|num_requests_(running|waiting|total)|prompt_tokens_total|generation_tokens_total|request_success_total|time_to_first_token_seconds_(sum|count)|request_time_per_output_token_seconds_(sum|count)|inter_token_latency_seconds_(sum|count)|gpu_prefix_cache_hit_rate)' | head -80 ;;
    sglang) grep -E '^sglang:(num_running_reqs|num_queue_reqs|num_requests_total|token_usage|gen_throughput|prompt_tokens_total|generation_tokens_total|time_to_first_token_seconds_(sum|count)|cache_hit_rate)' | head -80 ;;
    tgi) grep -E '^tgi_' | head -80 ;;
    *) head -60 ;;
  esac
}
eng_probe_port() { # $1 = port, $2 = pid, $3 = attr -> engine JSON object or nothing
  local p=$1 pid=$2 attr=$3 hp hm raw kind off
  raw=$(timeout 3 curl -s -m 1.5 "http://127.0.0.1:$p/metrics" 2>/dev/null | head -c 80000)
  kind=''
  off=0
  case "$raw" in
    *does*not*support*metrics*) kind=llama-cpp; off=1; raw='' ;;
    *) kind=$(eng_classify "$raw") ;;
  esac
  hp=$(eng_curl "$p" /health 1.0)
  if [ -z "$kind" ]; then
    # /metrics did not classify: only an engine if /health itself speaks an
    # engine shape — otherwise this port is some unrelated service
    [ -n "$hp" ] || return 0
    kind=$(eng_classify "$hp")
    [ -n "$kind" ] || return 0
    raw=''
  fi
  hm=$(eng_curl "$p" /v1/models 1.0); [ ${#hm} -gt 300 ] && hm=$(printf '%s' "$hm" | head -c 300)
  case $kind in
    strata|llama-legacy) ;;                     # full JSON, parsed client-side
    prom)   raw=$(printf '%s\n' "$raw" | eng_trim_prom prom) ;;
    *)      raw=$(printf '%s\n' "$raw" | eng_trim_prom "$kind") ;;
  esac
  local moff=false; [ "$off" = 1 ] && moff=true
  printf '{"kind":"%s","port":%s,"pid":%s,"attr":"%s","t":%s,"moff":%s,"health":"%s","models":"%s","raw":"%s"}' \
    "$kind" "$p" "$pid" "$attr" "$(date +%s 2>/dev/null || echo 0)" "$moff" \
    "$(jstr "${hp:0:240}")" "$(jstr "${hm:-}")" "$(jstr "$raw")"
}
eng_probe_all() { # one cycle over every GPU compute pid -> engines JSON array
  local pids pid ports p e out start now taken hint
  start=$(date +%s 2>/dev/null || echo 0)
  out=''
  taken=''
  pids=$("$NSMI" --query-compute-apps=pid --format=csv,noheader,nounits 2>/dev/null \
    | tr -d ' ' | grep -E '^[0-9]+$' | sort -un | head -4)
  [ -n "$pids" ] || { printf '[]'; return; }
  for pid in $pids; do
    ports=$(proc_ports "$pid")
    hint=0
    if [ "$ports" = X ]; then
      ports=''
      if looks_like_server "$pid"; then
        hint=1
        eng_docker_map
        cid=$(sed -n 's/.*docker[-.]\([0-9a-f]\{12,64\}\).*/\1/p' \
              "/proc/$pid/cgroup" 2>/dev/null | head -1 | cut -c1-12)
        for ip in $(netns_ports "$pid") $(cmdline_ports "$pid"); do
          # container? translate internal port -> host-published port
          hp=$(eng_docker_hostports "$ip" "${cid:-}")
          [ -n "$hp" ] && ports="$ports $hp" || ports="$ports $ip"
        done
        ports=$(printf '%s\n' $ports | grep -E '^[0-9]+$' | sort -un | head -4)
      fi
    fi
    [ -n "$ports" ] || continue
    for p in $ports; do
      case " $taken " in *" $p "*) continue ;; esac
      now=$(date +%s 2>/dev/null || echo 0)
      [ $((now - start)) -le 4 ] || break   # hard budget: never stall a frame into the watchdog
      e=$(eng_probe_port "$p" "$pid" "$([ "$hint" = 1 ] && echo hint || echo fd)")
      [ -n "$e" ] || continue
      taken="$taken $p"
      [ -n "$out" ] && out+=','
      out+="$e"
      break   # one engine endpoint per process is enough
    done
  done
  printf '[%s]' "$out"
}

# ---------- GPUs ----------
GPUS=''

# does this ss build publish socket inodes with -e? checked once per stream
if [ "${ENG_OK:-1}" = 1 ] && [ -z "${ENG_ECHECK:-}" ]; then
  if listen_ino_port | grep -q .; then ENG_HAS_E=1; fi
  ENG_ECHECK=1
fi

# NVIDIA: nvidia-smi may be missing from a non-login PATH
NSMI=$(command -v nvidia-smi 2>/dev/null || true)
if [ -z "$NSMI" ]; then
  for p in /usr/bin/nvidia-smi /usr/local/bin/nvidia-smi /usr/lib/nvidia/nvidia-smi; do
    [ -x "$p" ] && NSMI="$p" && break
  done
fi
if [ -n "$NSMI" ]; then
  while IFS='|' read -r gi gname guuid gutil gmt gmu gtemp gfan gpow gcap; do
    [ -z "$gi" ] && continue
    gname=$(jstr "$gname"); guuid=$(jstr "$guuid")
    procs=''
    if apps=$("$NSMI" -i "$gi" --query-compute-apps=pid,process_name,used_memory --format=csv,noheader,nounits 2>/dev/null | tr ',' '|'); then
      while IFS='|' read -r pid pname pmem; do
        [ -z "${pid// }" ] && continue
        pid=${pid// }
        # full argv (nvidia-smi's process_name has no args); fall back to it
        cmdline=$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null | sed 's/[[:space:]]*$//')
        [ -z "$cmdline" ] && cmdline=$(echo "$pname" | sed 's/^ *//;s/ *$//')
        pj=$(jstr "$cmdline")
        user=$(ps -o user= -p "$pid" 2>/dev/null | tr -d ' '); user=${user:-?}
        uj=$(jstr "$user")
        [ -n "$procs" ] && procs+=','
        procs+="{\"pid\":$pid,\"name\":\"$pj\",\"user\":\"$uj\",\"mem_mb\":$(num "$pmem" 0)}"
      done <<< "$apps"
    fi
    [ -n "$GPUS" ] && GPUS+=','
    GPUS+="{\"vendor\":\"nvidia\",\"idx\":$gi,\"name\":\"$gname\",\"uuid\":\"$guuid\",\"util\":$(num "$gutil" 0),\"mem_total_mb\":$(num "${gmt//[^0-9]/}" 0),\"mem_used_mb\":$(num "${gmu//[^0-9]/}" 0),\"temp_c\":$(nz "$gtemp"),\"fan_pct\":$(nz "$gfan"),\"power_w\":$(nz "$gpow"),\"power_cap_w\":$(nz "$gcap"),\"processes\":[$procs]}"
  done < <("$NSMI" --query-gpu=index,name,uuid,utilization.gpu,memory.total,memory.used,temperature.gpu,fan.speed,power.draw,power.limit --format=csv,noheader,nounits 2>/dev/null | awk -F', ' -v OFS='|' 'NF>=6{$1=$1; print}')
fi

# AMD (sysfs: /sys/class/drm/card*/device — gpu_busy_percent + vram, no per-proc without root)
if [ -z "$GPUS" ]; then
  for d in /sys/class/drm/card[0-9]*/device; do
    [ -e "$d/gpu_busy_percent" ] || continue
    vend=$(cat "$d/vendor" 2>/dev/null)
    case $vend in 0x1002) ;; *) continue ;; esac
    busy=$(cat "$d/gpu_busy_percent" 2>/dev/null || echo 0)
    vt=$(cat "$d/mem_info_vram_total" 2>/dev/null || echo 0)
    vu=$(cat "$d/mem_info_vram_used" 2>/dev/null || echo 0)
    nm=$(cat "$d/unique" 2>/dev/null)
    card=$(basename "$(dirname "$d")")
    t=$(cat "$d/hwmon"*/temp1_input 2>/dev/null | head -1)
    temp='null'; [ -n "$t" ] && temp=$(awk -v x=$t 'BEGIN{printf "%.1f", x/1000}')
    vtm=$((vt/1048576)); vum=$((vu/1048576))
    [ -n "$GPUS" ] && GPUS+=','
    GPUS+="{\"vendor\":\"amd\",\"idx\":0,\"name\":\"AMD GPU $(jstr "${nm:-$card}")\",\"uuid\":\"\",\"util\":$(num "$busy" 0),\"mem_total_mb\":$vtm,\"mem_used_mb\":$vum,\"temp_c\":$temp,\"fan_pct\":null,\"power_w\":null,\"power_cap_w\":null,\"processes\":[]}"
  done
fi

# PCI presence fallback: hosts without GPU drivers (e.g. Proxmox with cards
# passed through to VMs) still show the hardware — read-only, no nvidia-smi.
if [ -z "$GPUS" ]; then
  gi=0
  for d in /sys/bus/pci/devices/*; do
    cls=$(cat "$d/class" 2>/dev/null)
    vend=$(cat "$d/vendor" 2>/dev/null)
    case $cls in
      0x0302*) ;;                       # 3D controller: always count
      0x0300*)                          # VGA: only nvidia/amd discrete
        case $vend in 0x10de|0x1002) ;; *) continue ;; esac ;;
      *) continue ;;
    esac
    drv=$(basename "$(readlink -f "$d/driver" 2>/dev/null)" 2>/dev/null)
    [ -z "$drv" ] && drv="no driver"
    slot=$(basename "$d"); nm=""
    if command -v lspci >/dev/null 2>&1; then
      raw=$(lspci -s "${slot#0000:}" 2>/dev/null | sed 's/^[^ ]*: *//;s/ (rev [^)]*)//')
      # lspci format: "Vendor [VEN] Product [PRODUCT]" — the last bracket is the product
      nm=$(printf '%s' "$raw" | sed -n 's/.*\[\([^]]*\)\].*/\1/p')
      [ -z "$nm" ] && nm="$raw"
    fi
    case $vend in
      0x10de) vname="NVIDIA" ;; 0x1002) vname="AMD" ;; 0x8086) vname="Intel" ;;
      *) vname="${vend#0x}" ;;
    esac
    dev=$(cat "$d/device" 2>/dev/null)
    [ -z "$nm" ] && nm="$vname GPU device ${dev#0x}"
    [ -n "$GPUS" ] && GPUS+=','
    GPUS+="{\"vendor\":\"pci\",\"idx\":$gi,\"name\":\"$(jstr "$nm [$drv]")\",\"uuid\":\"\",\"util\":0,\"mem_total_mb\":0,\"mem_used_mb\":0,\"temp_c\":null,\"fan_pct\":null,\"power_w\":null,\"power_cap_w\":null,\"processes\":[]}"
    gi=$((gi+1))
    [ $gi -ge 4 ] && break
  done
fi

# ---------- engine cycle (cached; ~6s cadence, 4s hard budget) ----------
# The payload rides only on refresh frames; cache frames carry "engines":null
# so the client keeps the last map without re-shipping ~12KB every second.
ENG_EMIT=0
if [ "${ENG_OK:-1}" = 1 ] && [ -n "${NSMI:-}" ]; then
  _now=$(date +%s 2>/dev/null || echo 0)
  if [ -z "${ENG_TS:-}" ] || [ $((_now - ${ENG_TS:-0})) -ge "${ENG_EVERY:-6}" ]; then
    ENG_JSON=$(eng_probe_all) || ENG_JSON='[]'
    ENG_TS=$_now
    ENG_EMIT=1
  fi
fi

# ---------- emit ----------
printf '{'
printf '"v":1,'
printf '"epoch":%s,' "${NOW:-0}"
printf '"hostname":"%s",' "$(jstr "$HOST")"
printf '"kernel":"%s",' "$(jstr "$KVER")"
printf '"arch":"%s",' "$(jstr "$ARCH")"
printf '"uptime_s":%s,' "${UPT:-0}"
printf '"cpu":{"model":"%s","mhz":%s,"hz":%s,"ncpu":%s,"busy":%s,"idle":%s,"load":[%s,%s,%s],"cores":[%s]},' \
  "$(jstr "$MODEL")" "$MHZ" "$HZ" "$NCPU" "$AGG_BUSY" "$AGG_IDLE" "$L1" "$L5" "$L15" "$CORES"
printf '"mem":{"total_kb":%s,"avail_kb":%s,"free_kb":%s,"buffers_kb":%s,"cached_kb":%s,"sreclaim_kb":%s,"swap_total_kb":%s,"swap_free_kb":%s},' \
  "$MT" "$MA" "$MF" "$BC" "$CA" "$SR" "$ST" "$SF"
printf '"disks":[%s],' "$DISKS"
printf '"zpools":[%s],' "$ZPOOLS"
printf '"zdatasets":[%s],' "$ZDATS"
printf '"nets":[%s],' "$NETS"
printf '"temp":{"cpu_c":%s,"cpu_src":%s,"nvme_c":%s,"sensors":%s},' "$CT" "$CS" "$NT" "$TS"
printf '"gpus":[%s],' "$GPUS"
if [ "$ENG_EMIT" = 1 ]; then printf '"engines":%s' "${ENG_JSON:-[]}"; else printf '"engines":null'; fi
printf '}\n'
