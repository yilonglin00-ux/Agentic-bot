# Lokales Modell für Noki Intelligence

Die aktuellen Chat-/Code-Modi nutzen den lokalen **llama.cpp**-Router. Ollama bleibt als Rollback installiert; die bisherigen Qwen2.5-GGUF-Dateien bleiben unverändert erhalten und werden nicht ins App-Bundle kopiert.

Defaults: Schnell `qwen2.5:3b-instruct-q5_K_M`, Normal `qwen3.5:9b` ohne Thinking, Intensiv dasselbe `qwen3.5:9b` mit Thinking, Code `mannix/JackOD-9B-Coder:Q4_K_M`. `NOKI_CHAT_FAST_MODEL`, `NOKI_CHAT_MODEL`, `NOKI_CODE_MODEL` und `NOKI_MODEL_KEEP_ALIVE` erlauben lokale Overrides. Der ModelManager entlädt vor einem Gewichtswechsel das alte Modell und prüft den jeweiligen Runtime-Status; Normal ↔ Intensiv wechselt nur die Thinking-Einstellung.
Der llama.cpp-Router wird mit `desktop/llama-router.sh` gestartet und hört auf `127.0.0.1:8080`; `models.ini` erzwingt 8192 Kontext, mmap, Prompt-Cache und maximal ein geladenes Modell. Ollama bleibt für Rollback über `NOKI_LLM_RUNTIME=ollama` verfügbar.
JackOD verwendet seine veröffentlichten Ollama-Samplerwerte; Noki setzt für Code nur Kontext- und Ausgabelimit. Die folgenden GGUF-Angaben beschreiben die aufbewahrten Legacy-Gewichte.

## Legacy-GGUF (nicht die Ask-Noki-Defaults)

Offizielle Quelle: https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF
Revision: `9217f5db79a29953eb74d5343926648285ec7e67`
Datei: `qwen2.5-0.5b-instruct-q4_k_m.gguf`
SHA-256: `74a4da8c9fdbcd15bd1f6d01d621410d31c6fc00986f5eb687824e7b93d7a9db`
Lizenz: https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/blob/main/LICENSE
Runtime: https://github.com/utilityai/llama-cpp-rs

## Einrichtung

Die GGUF-Datei einmalig aus der offiziellen Quelle in dieses Verzeichnis legen.
`bauen.sh` übernimmt sie in `Noki.app/Contents/Resources/models/`.
Die App lädt niemals selbst Modellgewichte herunter. Gewichte, App-Paket und lokale Build-Caches sind gitignoriert.
RAM unter 4 GB verhindert das Laden. Die Auswahl verwendet ausschließlich bereits vorhandene Gewichte. Keine automatischen Downloads. Das 0,5B-Modell ist insbesondere bei deutschen Antworten deutlich schwächer.

Explizite Einrichtung: `python3 desktop/models/download.py --model 1.5b` (oder `0.5b`). Das Skript prüft Größe und offiziellen SHA-256 und speichert die Revision als JSON neben dem Modell. Es wird weder von Noki noch vom Build aufgerufen.

1,5B-Quelle: https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF
Revision: `91cad51170dc346986eccefdc2dd33a9da36ead9`
Prüfsumme: siehe `qwen2.5-1.5b-instruct-q4_k_m.gguf.json`.

## Legacy-Verhalten und Grenzen

- Normaler Klick: Ask Noki; Arbeit und ^/° → 1–9 bleiben bestehen.
- Inferenz auf Rust-Arbeitsthreads, maximal eine Anfrage, 2 CPU-Threads, 2048 Kontexttokens, maximal 128 Antworttokens; Abbruch/60-s-Limit zwischen Decode-Schritten.
- KV-Kontext wird nach jeder Anfrage freigegeben, Modell nach 3 Minuten Inaktivität entladen.
- Kontext: freigegebene App/Fenstertitel, beobachtete Dauer, Noki-Timer/Fokus/Werkzeuge und bis zu 5 Ablage-Dateinamen. Keine Dateiinhalte oder vollständigen Pfade.
- App-/Fensterkontext wird manuell beim Öffnen und bei aktivierter Proaktivität höchstens alle 15 Sekunden erhoben. Die Dauer ist seit Beobachtungsbeginn, keine rückwirkende Arbeitszeiterfassung.
- Alle sechs Freigaben starten ausgeschaltet. Screen/OCR sind sichtbar deaktiviert und in Phase 1 nicht implementiert. Keine Screenshots durch Intelligence.
- Einstellungen und aggregiertes Hinweis-Feedback: `~/NOKI/.local/intelligence/`. Kein Chat-Log, keine Desktop-Historie, keine Gewichtsanpassung. Dialogkontext wird beim Schließen verworfen.
- SILENT ist Standard; einzige proaktive Regel: Timer zwischen vier und fünf verbleibenden Minuten. Je Timer höchstens ein Hinweis, mit Cooldowns (60/30/15 min) und Stundenlimits (1/2/4).
- Toolentscheidungen sind feste, explizite Nutzerabsichten. Modelltext wird nie ausgeführt. Erst ein Klick auf den Vorschlag öffnet die vorhandene Aktion/Auswahl; bestehende Bestätigungen bleiben erhalten.
- Das kleine Modell kann ungenaue Antworten geben. Bildschirmverständnis, Build-Fehler-Erkennung und autonomes Handeln gehören nicht zu Phase 1.

## Prüfen

CMake ist eine Buildvoraussetzung. `desktop/bauen.sh` findet die hier eingerichteten projektlokalen Werkzeuge automatisch. Für die Tests in dieser Arbeitskopie:

```sh
export CARGO_HOME="$PWD/.local/cargo"
export CMAKE="$PWD/.local/cmake/cmake/data/bin/cmake"
export TMPDIR="$PWD/.local/tmp"
cargo test --manifest-path desktop/src-tauri/Cargo.toml -j 2
cargo test --manifest-path desktop/src-tauri/Cargo.toml intelligence::tests::local_inference -- --ignored --nocapture
PLAYWRIGHT_MODULE=/pfad/zu/playwright node desktop/tests/intelligence-ui.cjs
```

Der UI-Test blockiert externe Netzverbindungen und simuliert die native Bridge. Der gesonderte Rust-Smoke-Test nutzt echte lokale Modellinferenz.

## Phase-1-Verifikation (2026-09-15)

31 Rust-Tests bestanden; gesonderter echter Modelltest (Wissensfrage und Desktop-Kontext) bestanden. UI-Test mit realer lokaler Inferenz: normaler Noki-Klick, Enter, Esc/Außenklick, Abbruch, bestätigte Tools, persistierte Einstellungen und alle 9 Shortcuts bestanden. Browser-Renderzeiten mit und ohne Inferenz: Median und P95 jeweils 16,7 ms (ca. 60 FPS); kein nativer WKWebView-FPS-Benchmark. Kleine Modelle bleiben fehleranfällig, besonders auf Deutsch.
