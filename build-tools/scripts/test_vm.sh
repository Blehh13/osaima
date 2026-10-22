#!/usr/bin/env bash
#
# test_vm.sh — smoke-test a running Interstellar OS VM (see run_vm.sh) over SSH:
# distro identity, OSAIMA services, local LLM, voice, the scheduler, the security gate
# and the audit log. Exits non-zero when any check fails.
set -uo pipefail

OUT="${OUT:-$(realpath "$(dirname "$0")/../..")/out/vm}"
SSH=(ssh -q -p 2222 -i "$OUT/id_osaima" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null root@127.0.0.1)

echo "Waiting for SSH..."
for _ in $(seq 1 60); do "${SSH[@]}" true 2>/dev/null && break; sleep 3; done

"${SSH[@]}" 'bash -s' <<'REMOTE'
pass=0; fail=0
check() { if eval "$2" >/dev/null 2>&1; then echo "  PASS  $1"; pass=$((pass+1)); else echo "  FAIL  $1"; fail=$((fail+1)); fi; }
uid=$(id -u osaima)
as_user() { runuser -u osaima -- env XDG_RUNTIME_DIR=/run/user/$uid "$@"; }

echo "== System"
grep PRETTY_NAME /etc/os-release
check "identifies as Interstellar OS" "grep -qi interstellar /etc/os-release"
for s in osaima-llm osaima-router osaima-mcp-daemon osaima-voice; do
	check "$s.service active" "as_user systemctl --user is-active $s"
done
echo "== Desktop (sway session, top bar, assistant window)"
sock=""; for _ in $(seq 1 30); do sock=$(ls /run/user/$uid/sway-ipc.*.sock 2>/dev/null | head -1); [ -n "$sock" ] && break; sleep 2; done
check "sway running for osaima" "pgrep -u osaima -x sway"
check "top bar (waybar) running" "pgrep -u osaima -x waybar"
for _ in $(seq 1 30); do swaymsg -s "$sock" -t get_tree 2>/dev/null | grep -q 'OSAIMA Assistant' && break; sleep 2; done
check "assistant is a real window" "swaymsg -s '$sock' -t get_tree | grep -q 'OSAIMA Assistant'"
echo "== Kernel enforcement"
check "Landlock enabled in this kernel" "grep -q landlock /sys/kernel/security/lsm"
check "osaima-jail: Landlock fully enforced + seccomp on" "as_user osaima-jail -- true 2>&1 | grep -q 'landlock=full seccomp=on'"

echo "== Scheduler (osaima-schedd)"
schedd_log() { journalctl -u osaima-schedd -b --no-pager -o cat 2>/dev/null; }
check "osaima-schedd.service active" "systemctl is-active osaima-schedd"
schedd_log | grep -m1 "^osaima-schedd:"
check "scheduler reads the eBPF telemetry" "schedd_log | grep -q 'on eBPF data'"
check "scheduler uses the trained model" "schedd_log | grep -q '^osaima-schedd: trained'"
# a program of the user that only computes should be judged batch and given a worse priority
# matched by its arguments: Gentoo's python3 is a wrapper that runs python3.N
hog="^[^ ]*python[0-9.]* -c while True: pass$"
as_user timeout 120 python3 -c 'while True: pass' > /dev/null 2>&1 &
hog_nice=""
for _ in $(seq 1 30); do
	p=$(pgrep -u osaima -f "$hog" | head -1)
	[ -n "$p" ] && hog_nice=$(ps -o ni= -p "$p" | tr -d ' ') && [ "${hog_nice:-0}" -gt 0 ] 2>/dev/null && break
	sleep 2
done
check "a CPU hog of the user gets a worse priority (nice ${hog_nice:-?})" "[ '${hog_nice:-0}' -gt 0 ]"
schedd_log | grep -- '->' | tail -3
pkill -u osaima -f "$hog"

echo "== Waiting for the model to load"
for _ in $(seq 1 90); do curl -sf http://127.0.0.1:8080/v1/models >/dev/null && break; sleep 2; done
check "llama-server answering on localhost" "curl -sf http://127.0.0.1:8080/v1/models"

as_user python3 - <<'PY'
import json, os, socket, time
sock = os.path.join(os.environ["XDG_RUNTIME_DIR"], "osaima", "mcp.sock")
def rpc(method, params=None, path=sock):
    s = socket.socket(socket.AF_UNIX); s.settimeout(300); s.connect(path)
    s.sendall((json.dumps({"jsonrpc": "2.0", "id": "1", "method": method, "params": params}) + "\n").encode())
    buf = b""
    while not buf.endswith(b"\n"): buf += s.recv(65536)
    return json.loads(buf)
failed = 0
def ok(c, name):
    global failed
    failed += not c
    print(("  PASS  " if c else "  FAIL  ") + name)

print("== Voice (on the device)")
try:
    vs = rpc("voice.status", path=os.path.join(os.environ["XDG_RUNTIME_DIR"], "osaima", "voice.sock"))["result"]
    ok(vs["enabled"], "voice service answers")
    ok(vs["can_speak"], f"can speak with {vs['tts_voice']} ({'; '.join(vs['problems']) or 'no problems'})")
    ok(vs["can_listen"], f"can listen with {vs['stt_model']}")
except Exception as e:
    ok(False, f"voice service answers ({e})")

print("== Local LLM (no cloud)")
st = rpc("system.llm_status")["result"]
ok(st["local"]["reachable"] and not st["cloud_fallbacks"], f"local model {st['local'].get('models')}, cloud fallbacks {st['cloud_fallbacks']}")
for q, want in [("open the terminal", ("open_app",)), ("set a timer for 2 minutes", ("timer",)),
                ("delete my Downloads folder", ("files", "execute_command", "task", "unknown"))]:
    t = time.time(); resp = rpc("system.classify_intent", {"query": q}); r = resp.get("result") or {}
    detail = f" error: {resp['error'].get('message', '')[:120]}" if "error" in resp else ""
    line = f"{q!r} -> {r.get('intent')} in {time.time()-t:.1f}s via {r.get('source')}{detail}"
    if want[0] in ("open_app", "timer") or "error" in resp:
        ok(r.get("intent") in want, line)
    else:
        # an ambiguous request goes to the LLM, whose answer varies from run to run; it must
        # answer, but the quality of its answers is measured by the 157-case eval, not here
        good = r.get("intent") in want
        print(("  PASS  " if good else "  WARN  ") + line + ("" if good else " (LLM answer varies; see the eval)"))
    if want[0] in ("open_app", "timer"):  # everyday commands must not need the LLM
        ok(r.get("source") == "fast" and time.time() - t < 1, f"{q!r} answered on the fast path")

print("== Security gate + sandbox")
ex = lambda c, **k: rpc("system.execute_command", {"command": c, **k})
r = ex("uname -sr"); ok("result" in r and r["result"]["sandbox"] == "bubblewrap+landlock+seccomp", f"read-only runs in {r.get('result', {}).get('sandbox')}")
r = ex("env | grep -ciE 'dbus|wayland|api_key' || true", approved=True, typed="confirm")
ok(r.get("result", {}).get("output", "").strip().startswith("0"), "approved command gets no session/bus/key variables")
ok("error" not in rpc("system.tasks"), "agent task service answers")
w = rpc("system.os_action", {"action": "window", "op": "list"})
ok("result" in w and any("OSAIMA" in (x.get("title") or "") for x in w["result"]["result"]["windows"]), "daemon controls real windows (sway IPC)")
ok("error" in ex("mkdir ~/x_gate"), "reversible refused without approval")
ok("error" in ex("rm -rf ~/x_gate", approved=True), "irreversible refused without typed phrase")
r = ex("echo x | tee /etc/osaima_probe", approved=True, typed="confirm")
ok(not os.path.exists("/etc/osaima_probe"), "sandbox blocks system writes even when confirmed")
v = rpc("system.audit_verify")["result"]
ok(v["ok"] and v["records"] > 0, f"audit chain intact ({v['records']} records)")
raise SystemExit(min(failed, 1))
PY
py=$?
echo "== $pass shell checks passed, $fail failed (Python checks listed above)"
[ "$fail" -eq 0 ] && [ "$py" -eq 0 ]
REMOTE
inside=$?

# From outside, as anyone on the network would: a password login must not even be offered
# (the test user's password is published).
echo "== SSH from outside"
answer=$(ssh -p 2222 -o BatchMode=yes -o PubkeyAuthentication=no -o StrictHostKeyChecking=no \
	-o UserKnownHostsFile=/dev/null osaima@127.0.0.1 true 2>&1)
if echo "$answer" | grep -qF "Permission denied (publickey)."; then
	echo "  PASS  SSH offers key logins only"; outside=0
else
	echo "  FAIL  SSH offers more than keys: $answer"; outside=1
fi
[ "$inside" -eq 0 ] && [ "$outside" -eq 0 ]
