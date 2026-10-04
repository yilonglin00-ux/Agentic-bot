# NOKI lokaler Modell-Benchmark

Wählt die lokalen Modelle für NOKIs Rollen aus – nach Messung, nicht nach Namen:

| Rolle | Wo | Standard ohne Benchmark |
|---|---|---|
| Chat · Allgemein | Noki Chat, normales Gespräch | `qwen3.5-9b` |
| Chat · Reasoning | Noki Chat, Analyse/Planung/Mathematik (DEEP) | `qwen3.5-9b` |
| Chat · Werkzeuge | Noki Chat, mehrere Werkzeugschritte / Aktionen | `qwen3.5-9b` |
| Code · Funktional | Noki Code, Modus „Funktional“ | `jackod-9b` |
| Code · Kreativ | Noki Code, Modus „Kreativ“ | `jackod-9b` |

Noki Chat hat **keine** sichtbaren Funktional/Kreativ-Modi; die Chat-Rolle ergibt sich automatisch aus der vorhandenen Einstufung (`reasoning::classify` + Werkzeugplan). Funktional/Kreativ gibt es nur in Noki Code.

## Ausführen (auf dem Mac)

```sh
cd ~/NOKI/desktop/models/noki-bench
python3 bench.py inventar          # was ist installiert (models.ini)
python3 bench.py pruefen           # Repos + Q4_K_M-GGUF verifizieren, nichts herunterladen
python3 bench.py alles             # messen (lädt Kandidaten einzeln), Bericht: ~/NOKI/.local/llama-models/bench/ergebnis.md
python3 bench.py uebernehmen       # Plan anzeigen
python3 bench.py uebernehmen --ja  # Gewinner eintragen, temporäre Verlierer löschen
```

Danach Noki neu starten (der llama.cpp-Router liest `models.ini` beim Start). Rollen lassen sich jederzeit in Einstellungen › Intelligence von Hand ändern.

## Regeln

- **Ein Modell im Speicher:** NOKIs Router (:8080) wird vor jeder Messung entladen; je Kandidat läuft ein eigener `llama-server` (:8091), der danach beendet wird.
- **Nur 4 Bit:** Q4_K_M; Q8/F16 werden nie geladen.
- **Nur eindeutige Quellen:** feste Repos aus `kandidaten.json`; ohne Repo nur bei genau einem exakten Namenstreffer. GGUF aus dem Original-Repo oder von einem bekannten Quantisierer, dessen `base_model` auf genau dieses Repo zeigt.
- **SSD schonen:** Ein Kandidat, der nach seiner Messung keine Rolle anführt, wird sofort gelöscht. Gelöscht wird nur, was in `bench/downloads.json` steht (selbst heruntergeladen); jede Löschung steht vorher in `bench/aufraeumen.log`. Installierte Modelle werden nie gelöscht.
- **Phase 2** (Opus-Distill bzw. Qwopus/Sushi) nur, wenn eine Gruppe keinen klaren Gewinner hat (< 3 Punkte).
- Vorhandene Modelle behalten ihre Rolle, solange ein Kandidat nicht mindestens 3 Punkte besser ist (Chat Allgemein: 5). Werkzeuge bzw. Kreativ werden mit dem Reasoning- bzw. Funktional-Gewinner zusammengelegt, wenn der Unterschied knapp ist – weniger Modelle auf der SSD.

## Aufgaben (gleich für jedes Modell, NOKIs echte Prompts)

- **General (5)**, **Creative (4)**: Gespräch, Instruction Following, Zusammenfassung, JSON, Umformulieren; Ideen, Varianten, Slogan mit Vorgaben, UI-Ideen.
- **Reasoning (6)**: Logik, zwei Rechenaufgaben, Planung mit Abhängigkeiten, Terminkonflikt, Fangfrage. Mit Denken; gemessen werden Qualität **und** Denk-Tokens (Effizienz = Qualität / (1 + Denk-Tokens/1000)).
- **Tools (6)**: richtige Auswahl, Argumente, keine erfundenen Werkzeuge, mehrstufig, Stopp nach Erfolg (OpenAI-Tool-Calls, simulierte Ergebnisse).
- **Code Funktional (7)**: Bugfixes in Python/JS/Rust/TS (ausgeführt, minimal), Regression vermeiden, Refactoring und ein echter Agent-Loop mit NOKIs JSON-Aktionsprotokoll (`fs.read`/`fs.patch`/`shell.test`), Tests dürfen nicht angefasst werden.
- **Code Kreativ (6)**: UI-Karte, drei Varianten, Animation, Prototyp, Designvorgabe → CSS, Agent-Loop (Knopf neu gestalten, nur CSS).

Gemessen je Modell: Ladezeit, RAM nach dem Laden und Spitze (RSS des `llama-server`), TTFT, Tokens/s, Denk-Tokens, Tool-Erfolgsrate, Format-Erfolgsrate, Punkte je Kategorie.

`python3 test_bench.py` prüft den kompletten Ablauf mit simulierten Modellen (ohne Download).
