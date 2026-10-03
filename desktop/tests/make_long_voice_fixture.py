"""Create a local, natural 60–90 s German speech fixture with real pauses."""
from pathlib import Path
import subprocess
import wave

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / ".local" / "qa"
OUT.mkdir(parents=True, exist_ok=True)
SAMPLE_RATE = 16000
PHRASES = [
    "Hallo Noki, ich habe eine Frage und muss erst einmal ein bisschen ausholen.",
    "Es geht um Red Bull Purple. Ich habe diese Sorte früher öfter gesehen, weißt du.",
    "Jetzt schaue ich bei Kaufland in das Sortiment und finde sie irgendwie nicht mehr.",
    "Das ist aus der Hölle, sage ich mal, aber das ist nur so eine Redewendung.",
    "Meine eigentliche Frage ist: Warum ist Red Bull Purple dort gerade nicht verfügbar?",
    "Kannst du mir erklären, was dazu tatsächlich bekannt ist, und was du nicht belegen kannst?",
]
PAUSES = [5, 10, 20, 5, 5]
audio = bytearray()
for i, phrase in enumerate(PHRASES):
    aiff = OUT / f"voice_part_{i}.aiff"
    raw = OUT / f"voice_part_{i}.pcm"
    subprocess.run(["say", "-v", "Anna", "-r", "145", "-o", str(aiff), phrase], check=True)
    subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", str(aiff), "-ar", str(SAMPLE_RATE),
                    "-ac", "1", "-f", "s16le", str(raw)], check=True)
    audio.extend(raw.read_bytes())
    if i < len(PAUSES):
        audio.extend(b"\0" * (PAUSES[i] * SAMPLE_RATE * 2))
with wave.open(str(OUT / "voice_90s.wav"), "wb") as wav:
    wav.setnchannels(1)
    wav.setsampwidth(2)
    wav.setframerate(SAMPLE_RATE)
    wav.writeframes(audio)
print(f"{len(audio) / (SAMPLE_RATE * 2):.1f}s {OUT / 'voice_90s.wav'}")
