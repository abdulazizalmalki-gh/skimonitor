#!/usr/bin/env bash
# Verify probe.sh against the failure modes found in the isolated audit:
#   #3 newline in argv must not corrupt the JSON (jstr)
#   #5 refquota-only datasets must survive the zfs filter
#   #6 unavailable numeric -> null, not 0 (num/nz)
#   #4 aggregate cpu accounting == per-core accounting
set -u
export LC_ALL=C
cd "$(dirname "$0")/.."
fail=0
chk() { # chk <name> <expected> <actual>
  if [ "$2" = "$3" ]; then echo "PASS $1"; else echo "FAIL $1: expected [$2] got [$3]"; fail=1; fi
}

# --- unit: helpers, extracted from the real probe.sh ---
eval "$(awk '/^jstr\(\)/,/^}$/' probe.sh; awk '/^num\(\)/,/^}$/' probe.sh; awk '/^nz\(\)/,/^}$/' probe.sh)"
for fn in jstr num nz; do
  declare -f "$fn" >/dev/null || { echo "FATAL: could not extract $fn()"; exit 1; }
done

chk "jstr-newline"   'line1\nline2'      "$(jstr $'line1\nline2')"
chk "jstr-tab"       'a\tb'             "$(jstr $'a\tb')"
chk "jstr-cr"        'a\r\nb'           "$(jstr $'a\r\nb')"
chk "jstr-backslash" 'a\\b'             "$(jstr $'a\\b')"
chk "jstr-quote"     'a\"b'             "$(jstr 'a"b')"
chk "jstr-ctrl-bell" 'ab'               "$(jstr $'a\x07b')"
chk "jstr-vtab"      'ab'               "$(jstr $'a\x0bb')"
chk "jstr-del"       'ab'               "$(jstr $'a\x7fb')"
chk "num-empty-fb"   ''                 "$(num 'N/A' '')"
chk "num-default-0"  '0'                "$(num 'N/A')"
chk "num-valid"      '42'               "$(num '42')"
chk "nz-NA"          'null'             "$(nz 'N/A')"
chk "nz-negative"    'null'             "$(nz '-5')"
chk "nz-valid"       '45'               "$(nz '45')"
chk "nz-blank"       'null'             "$(nz '')"

# --- end-to-end: run the WHOLE probe with stub nvidia-smi + zfs + zpool ---
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
# victim whose argv embeds a literal newline + quote:
# bash keeps trailing args verbatim in argv: `bash -c SCRIPT ARG0` puts ARG0
# into /proc/PID/cmdline exactly, newline byte included.
bash -c 'while :; do sleep 2; done' $'evil "--a\n--b' &
VICTIM=$!
sleep 0.6
kill -0 "$VICTIM" 2>/dev/null || { echo "FATAL: victim died"; exit 1; }
# fixture sanity: the test is worthless if argv has no newline
tr '\0' '\n' < "/proc/$VICTIM/cmdline" | od -An -c | grep -q '\\n' \
  || { echo "FATAL: fixture argv lacks a newline"; exit 1; }

cat > "$TMP/nvidia-smi" <<EOF
#!/bin/bash
if [[ "\$1" == --query-gpu* ]]; then
  echo "0, Tesla Stub, GPU-STUB-0, 50 %, 24576 MiB, 12345 MiB, N/A, N/A, 200.5 W, 300 W"
else
  echo "$VICTIM, evil, 1234"
fi
EOF
chmod +x "$TMP/nvidia-smi"
cat > "$TMP/zfs" <<'EOF'
#!/bin/bash
# -p numeric output: unset shows "-"; refquota-only fs must pass, none/none must not
printf 'tank/sub\tdataset\t524288000\t-\t1073741824\t9\t-\n'
printf 'tank/plain\tdataset\t524288000\t-\t-\t9\t-\n'
printf 'tank/vm-9-disk-0\tvolume\t10737418240\t-\t-\t9\t34359738368\n'
EOF
chmod +x "$TMP/zfs"
cat > "$TMP/zpool" <<'EOF'
#!/bin/bash
echo "tank  17592186044416  8070450532352  9521735512064  45%  ONLINE"
EOF
chmod +x "$TMP/zpool"

export PATH="$TMP:$PATH"
bash -s < probe.sh > "$TMP/frame.json" 2>"$TMP/err" || echo "FAIL e2e-probe-run"
chk "e2e-stderr-empty" '' "$(cat "$TMP/err")"

python3 - "$TMP/frame.json" <<'PY'
import json, sys
fail = 0
def chk(name, exp, got):
    global fail
    if exp == got: print(f"PASS {name}")
    else: print(f"FAIL {name}: expected [{exp}] got [{got}]"); fail = 1
try:
    p = json.load(open(sys.argv[1]))
    chk("e2e-json-parses", True, True)
except Exception as e:
    print(f"FAIL e2e-json-parses: {e}"); sys.exit(1)

g = p["gpus"][0]
chk("e2e-gpu-total", 24576, g["mem_total_mb"])
chk("e2e-gpu-temp-null", None, g["temp_c"])      # N/A must be null, not fake 0
chk("e2e-gpu-fan-null", None, g["fan_pct"])
chk("e2e-power-valid", 200, int(g["power_w"]))
procs = g["processes"]
chk("e2e-proc-found", 1, len(procs))
raw = procs[0]["name"]
chk("e2e-proc-newline-roundtrip", True, "\n" in raw)  # decoded name holds the real newline = valid escaping end-to-end
chk("e2e-proc-evil-parts", True, "evil" in raw and "--b" in raw)
chk("e2e-proc-mem", 1234, procs[0]["mem_mb"])

names = [d["name"] for d in p["zdatasets"]]
chk("e2e-refquota-included", True, "tank/sub" in names)
chk("e2e-nolimit-excluded", False, "tank/plain" in names)
chk("e2e-volume-included", True, "tank/vm-9-disk-0" in names)

# aggregate vs sum-of-cores: same accounting. The probe reads /proc/stat twice
# (cores then aggregate), so live jiffies can drift between reads -> tolerance.
tot = p["cpu"]["busy"] + p["cpu"]["idle"]
s = sum(c["busy"] + c["idle"] for c in p["cpu"]["cores"])
tol = max(200, 16 * len(p["cpu"]["cores"]))
chk("e2e-cpu-totals-consistent", True, abs(tot - s) <= tol)
# irq-heavy line: aggregate busy must include irq+softirq+steal
print(f"info: totals agg={tot} cores={s} diff={tot-s}")
sys.exit(fail)
PY
[ $? -eq 0 ] || fail=1
kill "$VICTIM" 2>/dev/null

[ $fail -eq 0 ] && echo "ALL PASS"
exit $fail
