#!/usr/bin/env bash
#
# test_vm.sh — smoke-test a running Interstellar OS VM (see run_vm.sh) over SSH:
# distro identity, OSAIMA services, local LLM, the security gate and the audit log.
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
for s in osaima-llm osaima-router osaima-mcp-daemon; do
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

echo "== Waiting for the model to load"
for _ in $(seq 1 90); do curl -sf http://127.0.0.1:8080/v1/models >/dev/null && break; sleep 2; done
check "llama-server answering on localhost" "curl -sf http://127.0.0.1:8080/v1/models"

as_user python3 - <<'PY'
import json, os, socket, time
sock = os.path.join(os.environ["XDG_RUNTIME_DIR"], "osaima", "mcp.sock")
def rpc(method, params=None):
    s = socket.socket(socket.AF_UNIX); s.settimeout(300); s.connect(sock)
    s.sendall((json.dumps({"jsonrpc": "2.0", "id": "1", "method": method, "params": params}) + "\n").encode())
    buf = b""
    while not buf.endswith(b"\n"): buf += s.recv(65536)
    return json.loads(buf)
ok = lambda c, name: print(("  PASS  " if c else "  FAIL  ") + name)

print("== Local LLM (no cloud)")
st = rpc("system.llm_status")["result"]
ok(st["local"]["reachable"] and not st["cloud_fallbacks"], f"local model {st['local'].get('models')}, cloud fallbacks {st['cloud_fallbacks']}")
for q, want in [("open the terminal", ("open_app",)), ("set a timer for 2 minutes", ("timer",)),
                ("delete my Downloads folder", ("files", "execute_command", "task", "unknown"))]:
    t = time.time(); resp = rpc("system.classify_intent", {"query": q}); r = resp.get("result") or {}
    detail = f" error: {resp['error'].get('message', '')[:120]}" if "error" in resp else ""
    ok(r.get("intent") in want, f"{q!r} -> {r.get('intent')} in {time.time()-t:.1f}s via {r.get('source')}{detail}")
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
PY
echo "== $pass shell checks passed, $fail failed (Python checks listed above)"
REMOTE
