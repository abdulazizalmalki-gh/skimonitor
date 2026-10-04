#!/usr/bin/env bash
# sshscope remote probe — runs ON THE TARGET via `ssh ... bash -s`.
# Requires only bash + coreutils (no jq/python). Prints exactly ONE JSON object.
# All counters are raw; the caller diffs consecutive samples for rates.
set -u
export LC_ALL=C

jstr() { # $1 -> JSON-safe string (escapes \ and ", drops control chars)
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/[[:cntrl:]]//g'
}
num() { # $1 -> sanitized numeric or 0 (also maps -1 sentinel -> 0 via $2=0)
  local v=${1//[^0-9.eE+-]/}
  [[ "$v" =~ ^-?[0-9]+(\.[0-9]+)?$ ]] && printf '%s' "$v" || printf '%s' "${2:-0}"
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

# aggregate line
AGG_BUSY=0; AGG_IDLE=0
while read -r line; do
  [[ $line == cpu\ * ]] || continue
  set -- $line
  AGG_IDLE=$(( ${5:-0} + ${6:-0} + ${7:-0} + ${8:-0} + ${9:-0} + ${10:-0} ))
  AGG_BUSY=$(( ${2:-0} + ${3:-0} + ${4:-0} ))
  break
done < /proc/stat

# core-id topology (physical cores; HT siblings share core_id)
declare -A CPUCORE
for f in /sys/devices/system/cpu/cpu*/topology/core_id; do
  [ -r "$f" ] || continue
  c=$(basename "$(dirname "$f")"); c=${c#cpu}
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
    r=${MAJMIN_R[$mm]:-0}; w=${MAJMIN_W[$mm]:-0}
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

# ---------- GPUs ----------
GPUS=''

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
      nm=$(lspci -s "${slot#0000:}" 2>/dev/null | sed 's/^[^ ]*: *//;s/ (rev [^)]*)//')
      nm=${nm#*controller: }      # keep "NVIDIA ... [[GPU model]]" not "3D controller: ..."
      nm=${nm#VGA compatible controller: }
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
printf '"nets":[%s],' "$NETS"
printf '"temp":{"cpu_c":%s,"cpu_src":%s,"nvme_c":%s,"sensors":%s},' "$CT" "$CS" "$NT" "$TS"
printf '"gpus":[%s]' "$GPUS"
printf '}\n'
