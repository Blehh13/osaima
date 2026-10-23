#!/bin/bash
# Start the distilled intent model (ai-core/intent-model) on its own llama-server,
# next to the general model of start-llm.sh. The daemon asks it first for every
# request the intent router can't settle (OSAIMA_INTENT_URL, default port 8081).
#
#   OSAIMA_INTENT_GGUF   the model file (default: osaima-intent*.gguf in the models folder)
#   OSAIMA_MODELS_DIR    where *.gguf files live (same search as start-llm.sh)
#   OSAIMA_LLAMA_SERVER  path to llama-server
#   OSAIMA_INTENT_PORT   listen port (default 8081)
set -euo pipefail

MODELS_DIR="${OSAIMA_MODELS_DIR:-}"
if [ -z "$MODELS_DIR" ]; then
	for d in "$HOME/.local/share/osaima/models" /run/initramfs/live/osaima/models /usr/share/osaima/models; do
		[ -d "$d" ] && MODELS_DIR="$d" && break
	done
fi
PORT="${OSAIMA_INTENT_PORT:-8081}"
SERVER="${OSAIMA_LLAMA_SERVER:-$(command -v llama-server || true)}"
[ -n "$SERVER" ] || SERVER="$(find "$HOME/.local/share/osaima/llama" -name llama-server -type f 2>/dev/null | head -1)"
[ -x "$SERVER" ] || { echo "llama-server not found; set OSAIMA_LLAMA_SERVER" >&2; exit 1; }
MODEL="${OSAIMA_INTENT_GGUF:-$(ls "$MODELS_DIR"/osaima-intent*.gguf 2>/dev/null | head -1 || true)}"
[ -f "$MODEL" ] || { echo "No osaima-intent*.gguf in ${MODELS_DIR:-<none>}" >&2; exit 1; }

cores=$(nproc)
THREADS=$(( cores > 2 ? cores - 1 : cores ))
echo "OSAIMA intent model: $(basename "$MODEL") on 127.0.0.1:$PORT"
# A 0.5B model in Q4_K_M is ~400 MB; the short prompt plus answer fits in 2048
# tokens, and it stays loaded (no idle unload) because it answers in about a second.
# The daemon sends a JSON schema with each request, which llama-server turns into
# a grammar, so the output is always valid JSON with a known intent.
exec "$SERVER" -m "$MODEL" --host 127.0.0.1 --port "$PORT" --jinja -np 1 \
	-c "${OSAIMA_INTENT_CTX:-2048}" -t "${OSAIMA_INTENT_THREADS:-$THREADS}" --alias osaima-intent
