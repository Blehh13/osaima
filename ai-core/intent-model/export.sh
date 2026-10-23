#!/bin/bash
# Merged HF model (train.py) -> GGUF f16 -> Q4_K_M for llama-server.
#   ./export.sh [out/qwen0.5b] [name]
# Needs a llama.cpp checkout for convert_hf_to_gguf.py (cloned into ./llama.cpp
# if missing) and llama-quantize (on PATH or under ~/.local/share/osaima/llama).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${1:-$HERE/out/qwen0.5b}"
NAME="${2:-osaima-intent-qwen0.5b}"
PY="${PYTHON:-$HERE/.venv/bin/python}"
[ -x "$PY" ] || PY=python3

[ -d "$HERE/llama.cpp" ] || git clone --depth 1 https://github.com/ggml-org/llama.cpp "$HERE/llama.cpp"
QUANT="${LLAMA_QUANTIZE:-$(command -v llama-quantize || true)}"
[ -n "$QUANT" ] || QUANT="$(find "$HOME/.local/share/osaima/llama" -name llama-quantize -type f 2>/dev/null | head -1)"
[ -x "$QUANT" ] || { echo "llama-quantize not found; set LLAMA_QUANTIZE" >&2; exit 1; }

"$PY" "$HERE/llama.cpp/convert_hf_to_gguf.py" "$OUT/merged" --outtype f16 --outfile "$OUT/$NAME-f16.gguf"
"$QUANT" "$OUT/$NAME-f16.gguf" "$OUT/$NAME-Q4_K_M.gguf" Q4_K_M "${QUANT_THREADS:-2}"
rm -f "$OUT/$NAME-f16.gguf"
ls -l "$OUT/$NAME-Q4_K_M.gguf"
sha256sum "$OUT/$NAME-Q4_K_M.gguf"
echo "install: cp $OUT/$NAME-Q4_K_M.gguf ~/.local/share/osaima/models/"
