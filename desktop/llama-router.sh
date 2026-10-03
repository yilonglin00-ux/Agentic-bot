#!/bin/zsh
set -euo pipefail
ROOT="${0:A:h:h}"
BIN="${LLAMA_SERVER_BIN:-}"
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  if [ -f "$ROOT/.local/llama-src/build/bin/llama-server" ] && [ ! -L "$ROOT/.local/llama-src/build/bin/llama-server" ] && [ -x "$ROOT/.local/llama-src/build/bin/llama-server" ]; then
    BIN="$ROOT/.local/llama-src/build/bin/llama-server"
  elif [ -x "/opt/homebrew/opt/llama.cpp/bin/llama-server" ]; then
    BIN="/opt/homebrew/opt/llama.cpp/bin/llama-server"
  elif command -v brew >/dev/null 2>&1 && [ -x "$(brew --prefix llama.cpp 2>/dev/null)/bin/llama-server" ]; then
    BIN="$(brew --prefix llama.cpp)/bin/llama-server"
  elif command -v llama-server >/dev/null 2>&1; then
    BIN="$(command -v llama-server)"
  elif [ -x "$ROOT/.local/llama-src/build/bin/llama-server" ]; then
    BIN="$ROOT/.local/llama-src/build/bin/llama-server"
  else
    echo "Fehler: Kein lauffähiger llama-server gefunden." >&2
    exit 1
  fi
fi
MODELS="$ROOT/.local/llama-models"
exec "$BIN" --models-preset "$MODELS/models.ini" --models-dir "$MODELS" --models-max 1 --parallel 1 --gpu-layers all --ctx-size 8192 --load-mode mmap --cache-prompt --flash-attn auto --host 127.0.0.1 --port 8080
