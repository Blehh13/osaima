#!/bin/bash
# Start the on-device LLM (llama.cpp llama-server, OpenAI-compatible API).
# Picks a model tier from available RAM (PDR §6.3) unless OSAIMA_LLM_GGUF is set.
#
#   OSAIMA_MODELS_DIR   where *.gguf files live  (default ~/.local/share/osaima/models,
#                                                 the live ISO's medium, or /usr/share/osaima/models)
#   OSAIMA_LLAMA_SERVER path to llama-server     (default: on PATH, else ~/.local/share/osaima/llama)
#   OSAIMA_LLM_PORT     listen port              (default 8080, matches the daemon default)
#   OSAIMA_LLM_DOWNLOAD 0 = never download a model (default: when there is none, fetch the
#                       free default model once, into ~/.local/share/osaima/models)
set -euo pipefail

# The model the full ISO ships: free to download, no account. The lite ISO leaves it out
# and fetches it here on first start.
DEFAULT_MODEL_URL="${OSAIMA_LLM_MODEL_URL:-https://huggingface.co/bartowski/microsoft_Phi-4-mini-instruct-GGUF/resolve/main/microsoft_Phi-4-mini-instruct-Q4_K_M.gguf}"
DEFAULT_MODEL_FILE="phi-4-mini-Q4_K_M.gguf"

# Download the default model; retry until it arrives (at boot the network may not be up
# yet). The file only gets its real name once it is complete and looks like a model.
download_default_model() {
	local dir="$HOME/.local/share/osaima/models" part
	mkdir -p "$dir"
	part="$dir/$DEFAULT_MODEL_FILE.part"
	until [ -f "$dir/$DEFAULT_MODEL_FILE" ]; do
		echo "OSAIMA LLM: no model on this computer; downloading $DEFAULT_MODEL_FILE (about 2.4 GB, once)" >&2
		local fetched=0
		if command -v curl >/dev/null; then
			curl -fL --retry 3 -C - -o "$part" "$DEFAULT_MODEL_URL" && fetched=1
		else
			wget -c -O "$part" "$DEFAULT_MODEL_URL" && fetched=1
		fi
		# a broken-off download resumes next time (-C - / -c); only a finished one is used
		if [ "$fetched" = 1 ] && [ "$(head -c 4 "$part")" = GGUF ] && [ "$(stat -c %s "$part")" -gt 100000000 ]; then
			mv "$part" "$dir/$DEFAULT_MODEL_FILE"
		else
			echo "OSAIMA LLM: download did not finish; trying again in 30 s" >&2
			sleep 30
		fi
	done
	echo "$dir/$DEFAULT_MODEL_FILE"
}

MODELS_DIR="${OSAIMA_MODELS_DIR:-}"
if [ -z "$MODELS_DIR" ]; then
	# the live ISO keeps the weights uncompressed on the boot medium
	for d in "$HOME/.local/share/osaima/models" /run/initramfs/live/osaima/models /usr/share/osaima/models; do
		[ -d "$d" ] && MODELS_DIR="$d" && break
	done
fi
PORT="${OSAIMA_LLM_PORT:-8080}"

SERVER="${OSAIMA_LLAMA_SERVER:-$(command -v llama-server || true)}"
[ -n "$SERVER" ] || SERVER="$(find "$HOME/.local/share/osaima/llama" -name llama-server -type f 2>/dev/null | head -1)"
[ -x "$SERVER" ] || { echo "llama-server not found; set OSAIMA_LLAMA_SERVER" >&2; exit 1; }

# Tier by available memory; every list ends with the others as fallbacks.
avail_gb=$(awk '/MemAvailable/ {print int($2/1024/1024)}' /proc/meminfo)
# A model chosen in Settings (downloaded to the models folder) is recorded in llm.env.
if [ -z "${OSAIMA_LOCAL_MODEL:-}" ]; then
	ENVF="${XDG_CONFIG_HOME:-$HOME/.config}/osaima/llm.env"
	[ -f "$ENVF" ] && OSAIMA_LOCAL_MODEL="$(grep -m1 '^OSAIMA_LOCAL_MODEL=' "$ENVF" | cut -d= -f2- | tr -d '"' || true)"
fi
if [ -n "${OSAIMA_LLM_GGUF:-}" ]; then
	MODEL="$OSAIMA_LLM_GGUF"
elif [ -n "${OSAIMA_LOCAL_MODEL:-}" ] && [ -f "$OSAIMA_LOCAL_MODEL" ]; then
	MODEL="$OSAIMA_LOCAL_MODEL"
else
	if   [ "$avail_gb" -ge 10 ]; then tiers="llama-3.1-8b phi-4-mini gemma"
	elif [ "$avail_gb" -ge 4 ];  then tiers="phi-4-mini llama-3.2-3b gemma"
	else                              tiers="gemma llama-3.2-3b phi-4-mini"
	fi
	MODEL=""
	for t in $tiers; do
		MODEL="$(ls "$MODELS_DIR"/"$t"*.gguf 2>/dev/null | head -1 || true)"
		[ -n "$MODEL" ] && break
	done
	[ -n "$MODEL" ] || MODEL="$(ls "$MODELS_DIR"/*.gguf 2>/dev/null | head -1 || true)"
	if [ -z "$MODEL" ] && [ "${OSAIMA_LLM_DOWNLOAD:-1}" != 0 ]; then
		MODEL="$(download_default_model)"
	fi
fi
[ -f "$MODEL" ] || { echo "No .gguf model found in ${MODELS_DIR:-<none>}" >&2; exit 1; }

# All cores but one: the desktop needs a core too, and llama.cpp slows down
# sharply when any of its threads is preempted (measured: n-1 is fastest).
cores=$(nproc)
THREADS=$(( cores > 2 ? cores - 1 : cores ))

echo "OSAIMA LLM: $(basename "$MODEL") (${avail_gb} GB free) on 127.0.0.1:$PORT"
# --jinja uses the model's own chat template, which tool calling needs.
# One slot (-np 1): requests queue instead of splitting the context window,
# and 4096 tokens fits the ~2k-token intent prompt while keeping the KV cache
# small enough for 4-6 GB machines. After OSAIMA_LLM_IDLE seconds without a
# request the server unloads the model (~4 GB -> ~90 MB) and reloads it on the
# next one (a few seconds), since the intent router answers most commands
# without the LLM. Bound to localhost only.
# GPU offload only when this llama-server build actually lists a GPU device.
NGL=()
if [ -z "${OSAIMA_LLM_NO_GPU:-}" ] && "$SERVER" --list-devices 2>&1 | grep -qiE '^[[:space:]]*(cuda|vulkan|rocm|hip|sycl|metal)[0-9]*:'; then
	NGL=(-ngl 99)
	echo "OSAIMA LLM: GPU offload on"
fi
exec "$SERVER" ${NGL[@]+"${NGL[@]}"} -m "$MODEL" --host 127.0.0.1 --port "$PORT" \
	--jinja -np 1 -c "${OSAIMA_LLM_CTX:-8192}" -t "${OSAIMA_LLM_THREADS:-$THREADS}" --alias osaima-local \
	--sleep-idle-seconds "${OSAIMA_LLM_IDLE:-900}"
