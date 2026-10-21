#!/bin/bash
set -e

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$PROJECT_ROOT"

# Clean up old instances
echo "Cleaning up old instances..."
pkill -f "ui/osaima-shell/server.py" 2>/dev/null || true
pkill -f mcp-daemon 2>/dev/null || true

export PORT="${PORT:-8777}"

# On-device LLM (no API keys). Cloud fallback only if the user exports
# GROQ_API_KEY / GEMINI_API_KEY themselves.
# Intent router (NLU fast path); the daemon falls back to the LLM without it.
if [ ! -S "${XDG_RUNTIME_DIR:-/tmp}/osaima/router.sock" ]; then
    echo "Starting intent router..."
    python3 "$PROJECT_ROOT/ai-core/intent-router/router_service.py" > /tmp/osaima-router.log 2>&1 &
fi
if ! curl -sf "http://127.0.0.1:${OSAIMA_LLM_PORT:-8080}/v1/models" >/dev/null 2>&1; then
    echo "Starting local LLM..."
    "$PROJECT_ROOT/ai-core/llm/start-llm.sh" > /tmp/osaima-llm.log 2>&1 &
fi

echo "Starting OSAIMA Shell server on port $PORT..."
python3 "$PROJECT_ROOT/ui/osaima-shell/server.py" &
SERVER_PID=$!

sleep 1

URL="http://localhost:$PORT"
echo "Opening OSAIMA Desktop Shell in browser: $URL"
if command -v xdg-open >/dev/null 2>&1; then
    xdg-open "$URL" >/dev/null 2>&1 &
elif command -v firefox >/dev/null 2>&1; then
    firefox "$URL" >/dev/null 2>&1 &
fi

echo "============================================================"
echo "  ✦ OSAIMA - Interstellar OS Desktop Shell is running!"
echo "  ✦ URL: $URL"
echo "  ✦ Press Ctrl+C to stop."
echo "============================================================"

trap "echo 'Stopping OSAIMA...'; kill $SERVER_PID 2>/dev/null; exit 0" SIGINT SIGTERM
wait $SERVER_PID
