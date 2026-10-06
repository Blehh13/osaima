#!/bin/sh
# Tests for gentoo/overlay/gui-apps/osaima-shell/files/osaima-session using fake services.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
SESSION=$HERE/../../gentoo/overlay/gui-apps/osaima-shell/files/osaima-session
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

FAILED=0
pass() { printf 'ok   - %s\n' "$1"; }
fail() { printf 'FAIL - %s\n' "$1"; FAILED=1; }
check() { # check DESCRIPTION COMMAND...
	desc=$1
	shift
	if "$@" >/dev/null 2>&1; then pass "$desc"; else fail "$desc"; fi
}

# Wait up to ~15s for a command to succeed.
wait_for() {
	i=0
	while [ "$i" -lt 150 ]; do
		if "$@" >/dev/null 2>&1; then return 0; fi
		sleep 0.1
		i=$((i + 1))
	done
	return 1
}

mkdir -p "$TMP/bin" "$TMP/run"

# A long-running fake service. The odd sleep length lets us find leftovers.
cat >"$TMP/bin/mcp" <<'EOF'
#!/bin/sh
echo "ai-core started" >>"$LOGFILE"
exec sleep 3187
EOF

# The assistant crashes on its first two starts, then stays up.
cat >"$TMP/bin/agent" <<'EOF'
#!/bin/sh
n=$(cat "$COUNTFILE" 2>/dev/null || echo 0)
n=$((n + 1))
echo "$n" >"$COUNTFILE"
echo "assistant start $n args=$*" >>"$LOGFILE"
if [ "$n" -lt 3 ]; then exit 1; fi
exec sleep 3187
EOF

cat >"$TMP/bin/ollama" <<'EOF'
#!/bin/sh
echo "ollama started args=$*" >>"$LOGFILE"
exec sleep 3187
EOF

# Exits with status 7 once the assistant is up on its third start.
cat >"$TMP/bin/compositor" <<'EOF'
#!/bin/sh
i=0
while [ "$i" -lt 200 ]; do
	grep -q "assistant start 3" "$LOGFILE" && break
	sleep 0.1
	i=$((i + 1))
done
exit 7
EOF

# Stays up until killed.
cat >"$TMP/bin/forever" <<'EOF'
#!/bin/sh
exec sleep 3187
EOF
chmod +x "$TMP"/bin/*

run_session() { # extra environment is passed as arguments: VAR=value ...
	env XDG_RUNTIME_DIR="$TMP/run" DBUS_SESSION_BUS_ADDRESS=unix:path=/dev/null \
		LOGFILE="$TMP/log" COUNTFILE="$TMP/count" \
		OSAIMA_MCP_BIN="$TMP/bin/mcp" OSAIMA_AGENT_BIN="$TMP/bin/agent" \
		OSAIMA_OLLAMA_BIN="$TMP/bin/ollama" OSAIMA_RESTART_MAX=4 \
		"$@" "$SESSION"
}

leftovers() { pgrep -f "sleep 3187" >/dev/null 2>&1; }
no_leftovers() { ! leftovers; }

# ── 1. services start, crashes are restarted with backoff, session ends with the compositor
: >"$TMP/log"
rm -f "$TMP/count"
status=0
run_session OSAIMA_COMPOSITOR="$TMP/bin/compositor" 2>"$TMP/err" || status=$?

[ "$status" -eq 7 ] && pass "session exits with the compositor's status" || fail "exit status was $status, expected 7"
check "AI Core was started" grep -q "ai-core started" "$TMP/log"
check "assistant is started with 'serve'" grep -q "assistant start 1 args=serve" "$TMP/log"
check "crashed assistant was restarted twice" grep -q "assistant start 3" "$TMP/log"
check "ollama was started" grep -q "ollama started args=serve" "$TMP/log"
check "first restart waits 1s" grep -q "assistant exited with status 1; restarting in 1s" "$TMP/err"
check "second restart waits 2s" grep -q "assistant exited with status 1; restarting in 2s" "$TMP/err"
check "no services are left running" wait_for no_leftovers

# ── 2. Ollama can be switched off
: >"$TMP/log"
rm -f "$TMP/count"
echo 2 >"$TMP/count" # so the assistant stays up immediately
run_session OSAIMA_COMPOSITOR="$TMP/bin/compositor" OSAIMA_START_OLLAMA=0 2>/dev/null || true
check "OSAIMA_START_OLLAMA=0 skips ollama" sh -c "! grep -q 'ollama started' '$TMP/log'"
check "no services are left running (ollama off)" wait_for no_leftovers

# ── 3. SIGTERM (logout) stops the compositor and every service
: >"$TMP/log"
rm -f "$TMP/count"
echo 2 >"$TMP/count"
run_session OSAIMA_COMPOSITOR="$TMP/bin/forever" 2>/dev/null &
SESSION_PID=$!
check "services come up under a long-running compositor" wait_for grep -q "ollama started" "$TMP/log"
kill -TERM "$SESSION_PID"
status=0
wait "$SESSION_PID" || status=$?
[ "$status" -eq 143 ] && pass "SIGTERM ends the session with status 143" || fail "SIGTERM status was $status, expected 143"
check "no services are left running after SIGTERM" wait_for no_leftovers

# ── 4. a session without a login manager is refused
status=0
env -u XDG_RUNTIME_DIR DBUS_SESSION_BUS_ADDRESS=unix:path=/dev/null "$SESSION" 2>"$TMP/err" || status=$?
[ "$status" -eq 1 ] && grep -q "XDG_RUNTIME_DIR is not set" "$TMP/err" \
	&& pass "refuses to run without XDG_RUNTIME_DIR" || fail "missing XDG_RUNTIME_DIR was not refused"

exit "$FAILED"
