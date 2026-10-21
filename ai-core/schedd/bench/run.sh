#!/usr/bin/env bash
#
# Does the scheduler daemon help a frame-rate task that competes with CPU hogs?
#
#   sudo bench/run.sh SCHEDD [MODEL.json]
#
# Four runs of bench/frames.py, each against the same crowd of hogs (three per CPU):
#   idle              no hogs: the best case
#   loaded            the hogs, no scheduler daemon
#   loaded+rules      the hogs, the daemon with its built-in rules
#   loaded+model      the hogs, the daemon with the trained model (if one is given)
#
# Everything under test runs as an unprivileged user (uid 1500), and the daemon is told
# to touch only users from that id up, so nothing else on the machine is affected. Needs
# root, to create that user and to let the daemon raise priorities. Prints a markdown table.

set -euo pipefail

SCHEDD=${1:?usage: run.sh SCHEDD [MODEL.json]}
MODEL=${2:-}
HERE=$(cd "$(dirname "$0")" && pwd)
TRAIN=$HERE/../train
USER_NAME=osaima-bench
UID_BENCH=1500
HOGS=$(( $(nproc) * 3 ))
OUT=$(mktemp -d)

[ "$(id -u)" = 0 ] || { echo "run as root" >&2; exit 1; }
id "$USER_NAME" > /dev/null 2>&1 || useradd -u "$UID_BENCH" -M -s /usr/sbin/nologin "$USER_NAME"
as_bench() { setpriv --reuid="$UID_BENCH" --regid="$UID_BENCH" --clear-groups "$@"; }

one_run() {
	local label=$1 mode=$2
	local hog_pids=() daemon=""
	if [ "$mode" != idle ]; then
		for i in $(seq 1 "$HOGS"); do
			as_bench python3 "$TRAIN/workload.py" batch --seed "$i" &
			hog_pids+=("$!")
		done
	fi
	case "$mode" in
		rules) "$SCHEDD" run --min-uid "$UID_BENCH" --interval 1 --hysteresis 2 --source proc > "$OUT/$label.daemon.log" 2>&1 & daemon=$! ;;
		model) "$SCHEDD" run --min-uid "$UID_BENCH" --interval 1 --hysteresis 2 --source proc --model "$MODEL" > "$OUT/$label.daemon.log" 2>&1 & daemon=$! ;;
	esac
	as_bench python3 "$HERE/frames.py" --label "$label" > "$OUT/$label.json"
	if [ -n "$daemon" ]; then kill -TERM "$daemon"; wait "$daemon" || true; fi
	if [ "${#hog_pids[@]}" -gt 0 ]; then kill -KILL "${hog_pids[@]}" 2> /dev/null || true; wait "${hog_pids[@]}" 2> /dev/null || true; fi
	cat "$OUT/$label.json"
}

echo "$(nproc) CPUs, $HOGS CPU hogs, a $(python3 -c 'print(round(6/16.67*100))')% frame task (6 ms of work every 16.7 ms)" >&2
one_run "idle (no hogs)" idle
one_run "loaded, no scheduler" none
one_run "loaded + scheduler (rules)" rules
[ -n "$MODEL" ] && one_run "loaded + scheduler (trained model)" model

python3 - "$OUT" << 'PY'
import glob, json, os, sys
rows = [json.load(open(p)) for p in sorted(glob.glob(os.path.join(sys.argv[1], "*.json")), key=os.path.getmtime)]
print("\n| Run | Frames | Median | 95th | 99th | Worst | Late frames |")
print("|---|---|---|---|---|---|---|")
for r in rows:
    print(f"| {r['label']} | {r['frames']} | {r['p50_ms']} ms | {r['p95_ms']} ms | {r['p99_ms']} ms | {r['max_ms']} ms | {r['late_percent']}% |")
print("\nA frame is late when it finishes more than %s ms after it was due." % rows[0]["period_ms"])
PY
echo
echo "what the daemon did (rules run):"
grep -E "set|judged" "$OUT"/*rules*.daemon.log | head -12 || true
