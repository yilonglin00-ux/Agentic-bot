#!/bin/zsh
# Noki Talk - einmalige, ausdrueckliche Einrichtung der LOKALEN Spracherkennung.
#
#   1. whisper.cpp (offizielle Quelle) mit Metal bauen -> ~/NOKI/.local/whisper-src
#      (dasselbe Muster wie llama.cpp in ~/NOKI/.local/llama-src)
#   2. Modell (offizielle ggml-Gewichte) laden       -> ~/NOKI/.local/whisper-models
#
# Noki selbst laedt nie etwas herunter. Danach laeuft alles offline; Audio
# verlaesst den Mac nie. Erneut ausfuehren ist gefahrlos (vorhandenes bleibt).
#
#   MODELL=large-v3-turbo-q5_0 (Standard, sehr gutes Deutsch, ~550 MB)
#   MODELL=small | medium-q5_0 | base   (kleiner/schneller, schwaecher)
#   WHISPER_REF=<tag/branch>            (sonst der aktuelle Stand)
set -euo pipefail
ROOT="${0:A:h:h:h}"
LOKAL="$ROOT/.local"
SRC="$LOKAL/whisper-src"
MODELLE="$LOKAL/whisper-models"
MODELL="${MODELL:-large-v3-turbo-q5_0}"
mkdir -p "$LOKAL" "$MODELLE"

if [ ! -x "$SRC/build/bin/whisper-server" ]; then
  command -v cmake >/dev/null || { echo "cmake fehlt (z. B. 'brew install cmake')." >&2; exit 1; }
  if [ ! -d "$SRC/.git" ]; then
    git clone --depth 1 ${WHISPER_REF:+--branch "$WHISPER_REF"} https://github.com/ggml-org/whisper.cpp "$SRC"
  fi
  echo "==> whisper.cpp bauen (Metal)"
  cmake -S "$SRC" -B "$SRC/build" -DCMAKE_BUILD_TYPE=Release -DGGML_METAL=ON -DWHISPER_BUILD_EXAMPLES=ON -DWHISPER_BUILD_SERVER=ON
  cmake --build "$SRC/build" --config Release -j --target whisper-server
fi
echo "==> whisper-server: $SRC/build/bin/whisper-server"

ZIEL="$MODELLE/ggml-$MODELL.bin"
if [ ! -s "$ZIEL" ]; then
  echo "==> Modell ggml-$MODELL.bin laden (huggingface.co/ggerganov/whisper.cpp)"
  curl -fL --retry 3 -C - -o "$ZIEL.part" "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-$MODELL.bin"
  mv "$ZIEL.part" "$ZIEL"
fi
groesse=$(stat -f %z "$ZIEL" 2>/dev/null || stat -c %s "$ZIEL")
[ "$groesse" -gt 30000000 ] || { echo "Modell unvollstaendig ($groesse Bytes) - bitte erneut ausfuehren." >&2; rm -f "$ZIEL"; exit 1; }
echo "==> Modell: $ZIEL ($groesse Bytes)"
shasum -a 256 "$ZIEL" | tee "$ZIEL.sha256"
echo "Fertig. Noki Talk: Option zweimal druecken."
