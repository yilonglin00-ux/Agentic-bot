#!/usr/bin/env python3
"""End-to-end-Test des Benchmarks ohne echte Modelle.

Simuliert: Hugging Face (API + Downloads), NOKIs llama.cpp-Router (:8080)
und llama-server (OpenAI-kompatibel, SSE). Die Antwortqualitaet haengt vom
Inhalt der "GGUF"-Datei ab ("gut", "mittel", "schwach", "gutcode"). Geprueft
wird der ganze Weg: pruefen -> download -> messen (ein Modell zur Zeit) ->
bewerten (echte Ausfuehrung von Python/Node/Rust) -> Gewinner -> uebernehmen
-> nur eigene Downloads geloescht.
"""
import http.server
import json
import os
import socket
import stat
import sys
import tempfile
import threading
import unittest
from pathlib import Path

HIER = Path(__file__).resolve().parent


def frei():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


# ---------------------------------------------------------------- Mock: Hugging Face + Router

HF_REPOS = {
    "Jackrong/Qwen3.5-9B-Neo": {"siblings": [{"rfilename": "model.safetensors"}]},
    "mradermacher/Qwen3.5-9B-Neo-GGUF": {"cardData": {"base_model": "Jackrong/Qwen3.5-9B-Neo"},
                                         "siblings": [{"rfilename": "Qwen3.5-9B-Neo.Q8_0.gguf"}, {"rfilename": "Qwen3.5-9B-Neo.Q4_K_M.gguf"}]},
    "fremd/Qwen3.5-9B-Neo-GGUF": {"cardData": {"base_model": "irgendwas/anderes"}, "siblings": [{"rfilename": "x.Q4_K_M.gguf"}]},
    "empero-ai/Qwen3.5-9B-Claude-Code": {"siblings": [{"rfilename": "Qwen3.5-9B-Claude-Code-Q4_K_M.gguf"}]},
    "labA/Qwen3.5-9B-R7-Research": {"siblings": [{"rfilename": "Qwen3.5-9B-R7-Research-Q4_K_M.gguf"}]},
    "swe/Qwen3.5-9B-MTP-SWE-Agent": {"siblings": [{"rfilename": "MTP-SWE-Q4_K_M.gguf"}]},
    "empero-ai/Qwen3.5-9B-Claude-Opus-4.6-Distill": {"siblings": [{"rfilename": "opus-Q4_K_M.gguf"}]},
}
SUCHE = {
    "Qwen3.5-9B-Neo": ["mradermacher/Qwen3.5-9B-Neo-GGUF", "fremd/Qwen3.5-9B-Neo-GGUF"],
    "Qwen3.5-9B-R7-Research": ["labA/Qwen3.5-9B-R7-Research", "bartowski/Qwen3.5-9B-R7-Research"],
    "Qwen3.5-9B-Harmonic": ["a/Qwen3.5-9B-Harmonic", "b/Qwen3.5-9B-Harmonic"],          # mehrdeutig
    "Qwen3.5-9B-MTP-SWE-Agent": ["swe/Qwen3.5-9B-MTP-SWE-Agent"],
    "Qwopus3.5-9B-Coder": [], "Qwen3.5-9B-Sushi-Coder-RL": [],
}
INHALT = {"Qwen3.5-9B-Neo.Q4_K_M.gguf": "gut", "Qwen3.5-9B-Claude-Code-Q4_K_M.gguf": "schwach",
          "Qwen3.5-9B-R7-Research-Q4_K_M.gguf": "schwach", "MTP-SWE-Q4_K_M.gguf": "gutcode", "opus-Q4_K_M.gguf": "schwach"}
ROUTER_ENTLADEN = []


class HF(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def antwort(self, code, daten, typ="application/json"):
        b = daten if isinstance(daten, bytes) else json.dumps(daten).encode()
        self.send_response(code)
        self.send_header("Content-Type", typ)
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def do_GET(self):
        from urllib.parse import urlparse, parse_qs
        u = urlparse(self.path)
        if u.path == "/models":  # NOKIs Router
            return self.antwort(200, {"data": [{"id": "qwen3.5-9b", "status": {"value": "loaded"}}]})
        if u.path == "/api/models":
            q = parse_qs(u.query).get("search", [""])[0]
            return self.antwort(200, [{"id": i} for i in SUCHE.get(q, [])])
        if u.path.startswith("/api/models/"):
            repo = u.path[len("/api/models/"):]
            return self.antwort(200, HF_REPOS[repo]) if repo in HF_REPOS else self.antwort(404, {"error": "nope"})
        if "/resolve/main/" in u.path:
            name = u.path.rsplit("/", 1)[1]
            return self.antwort(200, INHALT.get(name, "schwach").encode(), "application/octet-stream")
        self.antwort(404, {})

    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(n) or b"{}")
        if self.path == "/models/unload":
            ROUTER_ENTLADEN.append(body.get("model"))
            return self.antwort(200, {"ok": True})
        self.antwort(404, {})


# ---------------------------------------------------------------- Mock: llama-server

LLAMA = r'''#!/usr/bin/env python3
import http.server, json, sys, time, os
args = sys.argv[1:]
modell = args[args.index("-m") + 1]; port = int(args[args.index("--port") + 1])
art = open(modell).read().strip()
open(os.environ["MOCK_LAUF_LOG"], "a").write(art + "\n")
GUT = {
 "wie geht's": "Mir geht es gut, danke der Nachfrage. Wie geht es dir heute?",
 "Vorteile von SSDs": "1. Die Zugriffszeiten sind kürzer und der Start ist schneller.\n2. Sie sind leise und es gibt keine Mechanik.\n3. Sie sind robuster gegen Stöße.",
 "höchstens 40 Wörtern": "Die Bibliothek öffnet ab März werktags von 8 bis 21 Uhr und samstags von 10 bis 18 Uhr und bekommt 40 neue Arbeitsplätze.",
 "Extrahiere": '{"name": "Mira Hoffmann", "stadt": "Leipzig", "alter": 34}',
 "höflicher": "Könntest du mir bitte die Datei schicken, wenn es dir passt?",
 "Wochenende in den Bergen": "1. Die Hütte und ein Brettspiel\n2. Ein Museum im Tal besuchen\n3. In der Therme entspannen\n4. Eine Käserei besichtigen\n5. Kochen mit den Freunden",
 "drei deutlich unterschiedliche Varianten": "1. Der Termin am Montag wird verlegt.\n2. Wir schieben den Termin vom Montag auf später.\n3. Montag findet der Termin nicht statt, ein neuer folgt.",
 "Slogan": "Noki ordnet deinen Tag",
 "UI-Ideen": "1. Ruhiger Ring: Ein Ring zeigt die Zeit.\n2. Ohne Ziffern: Die Zeit erscheint nur als Fläche.\n3. Sanftes Ende: Ein leiser Ton und das Licht wird wärmer.",
 "Wer ist am kleinsten": "Dana > Anna > Ben > Cem.\nErgebnis: Cem",
 "Durchschnittsgeschwindigkeit": "Strecke 210 + 144 = 354 km in 4 h.\nErgebnis: 88,5 km/h",
 "durch 3 oder durch 5": "33 + 20 - 6 = 47\nErgebnis: 47",
 "Reihenfolge der Aufgaben": "Reihenfolge: A, B, C, D, E, F",
 "gemeinsamen einstündigen Termin": "Termin: Donnerstag, 14 Uhr",
 "Schläger und ein Ball": "Ergebnis: 0,05 €",
 "mittelwert": "```python\ndef mittelwert_ohne_extreme(werte):\n    \"\"\"Mittelwert ohne kleinsten und groessten Wert (mind. 3 Werte).\"\"\"\n    s = sorted(werte)\n    innen = s[1:len(s) - 1]\n    return sum(innen) / len(innen)\n```",
 "Postleitzahlen": "```js\nfunction istGueltigePlz(plz) {\n  // Deutsche Postleitzahl: genau 5 Ziffern\n  return /^\\d{5}$/.test(plz);\n}\n```",
 "Lebensdauer": "```rust\npub fn laengstes<'a>(woerter: &'a [String]) -> Option<&'a str> {\n    let mut best: Option<&str> = None;\n    for w in woerter {\n        if best.map_or(true, |b| w.len() > b.len()) {\n            best = Some(w.as_str());\n        }\n    }\n    best\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n    #[test]\n    fn findet() {\n        let v = vec![\"a\".to_string(), \"abc\".to_string(), \"ab\".to_string()];\n        assert_eq!(laengstes(&v), Some(\"abc\"));\n        assert_eq!(laengstes(&[]), None);\n    }\n}\n```",
 "summiert Beträge": "```js\nfunction summeBetraege(posten) {\n  return posten.reduce((s, p) => s + parseFloat(p.betrag.replace(\",\", \".\")), 0);\n}\n```",
 "negative Sekunden": "```python\ndef format_dauer(sekunden):\n    if sekunden < 0:\n        return \"-\" + format_dauer(-sekunden)\n    m, s = divmod(sekunden, 60)\n    h, m = divmod(m, 60)\n    if h:\n        return f\"{h}:{m:02d}:{s:02d}\"\n    return f\"{m}:{s:02d}\"\n```",
 "doppelten Code": "```python\ndef _netto(brutto, satz):\n    return round(brutto - brutto * satz, 2)\n\ndef netto_a(brutto):\n    return _netto(brutto, 0.19)\n\ndef netto_b(brutto):\n    return _netto(brutto, 0.07)\n```",
 "Einstellungskarte": "```html\n<!doctype html><html><head><style>body{background:#1c1c1e;color:#f5f5f7}.karte{padding:16px;border-radius:12px;background:#2c2c2e}</style></head><body><div class=\"karte\"><h2>Benachrichtigungen</h2><p>Hinweise von Noki.</p><label><input type=\"checkbox\" role=\"switch\"> Benachrichtigungen</label></div></body></html>\n```",
 "Lade-Indikatoren": "```css\n.a{animation:dreh 1s linear infinite}@keyframes dreh{to{transform:rotate(360deg)}}\n```\n```css\n.b span{animation:puls .8s ease-in-out infinite alternate}@keyframes puls{from{opacity:.2}to{opacity:1}}\n```\n```css\n.c::after{content:'';display:block;height:2px;background:#888;animation:balken 1.2s ease infinite}@keyframes balken{0%{width:0}100%{width:100%}}\n```",
 "Kreisbahn": "```js\nconst c = document.querySelector('canvas');\nconst ctx = c.getContext('2d');\nconst ruhig = matchMedia('(prefers-reduced-motion: reduce)').matches;\nfunction bild(t) {\n  const w = ruhig ? 0 : t / 1000;\n  ctx.clearRect(0, 0, c.width, c.height);\n  ctx.beginPath();\n  ctx.arc(c.width / 2 + Math.cos(w) * 80, c.height / 2 + Math.sin(w) * 80, 6, 0, Math.PI * 2);\n  ctx.fill();\n  if (!ruhig) requestAnimationFrame(bild);\n}\nrequestAnimationFrame(bild);\n```",
 "Todo-Liste": "```html\n<input id=\"neu\"><button id=\"dazu\">Hinzufügen</button><ul id=\"liste\"></ul>\n```\n```js\nconst liste = document.getElementById('liste');\ndocument.getElementById('dazu').addEventListener('click', () => {\n  const li = document.createElement('li');\n  li.textContent = document.getElementById('neu').value;\n  li.addEventListener('click', () => li.remove());\n  liste.appendChild(li);\n});\n```",
 "Designvorgabe": "```css\n.karte {\n  display: flex;\n  gap: 12px;\n  padding: 16px;\n  font-size: 14px;\n  border-radius: 10px;\n  background: #1c1c1e;\n}\n```",
}
SCHLECHT = {"Ergebnis": "Ergebnis: 50", "```": "```python\npass\n```"}
def antwort_fuer(prompt, gut):
    for k, v in GUT.items():
        if k in prompt:
            if gut: return v
            return "Das weiß ich nicht genau." if "Ergebnis" not in v and "```" not in v else ("Ergebnis: 50" if "Ergebnis" in v else "```\nunveraendert\n```")
    return "Okay."
PATCH_F = "--- a/src/preise.py\n+++ b/src/preise.py\n@@ -1,3 +1,3 @@\n def rabatt(preis, prozent):\n     \"\"\"prozent als Zahl 0-100\"\"\"\n-    return preis * prozent\n+    return preis * prozent / 100\n"
PATCH_K = "--- a/stil.css\n+++ b/stil.css\n@@ -1,4 +1,15 @@\n .knopf {\n-  background: #333;\n-  color: #fff;\n+  background: #2c2c2e;\n+  color: #f5f5f7;\n+  border: 1px solid #3a3a3c;\n+  border-radius: 8px;\n+  padding: 8px 16px;\n+  transition: background 160ms ease;\n }\n+.knopf:hover {\n+  background: #3a3a3c;\n+}\n+.knopf:focus-visible {\n+  outline: 2px solid #0a84ff;\n+}\n"
def agent(transkript, gut):
    if not gut:
        return {"action": "final", "tool": "", "args": {"answer": "Erledigt."}, "reason_summary": "fertig"}
    n = transkript.count("TOOL_RESULT")
    kreativ = "stil.css" in transkript.split("AUFGABE:")[1][:400]
    if kreativ:
        schritte = [("fs.read", {"path": "stil.css"}), ("fs.patch", {"patch": PATCH_K})]
    else:
        schritte = [("fs.read", {"path": "src/preise.py"}), ("fs.patch", {"patch": PATCH_F}), ("shell.test", {"command": ["python3", "tests/test_preise.py"]})]
    if n < len(schritte):
        t, a = schritte[n]
        return {"action": "tool", "tool": t, "args": a, "reason_summary": "Schritt"}
    return {"action": "final", "tool": "", "args": {"answer": "Fertig, Tests grün."}, "reason_summary": "fertig"}
def tools(msgs, gut):
    user = [m for m in msgs if m["role"] == "user"][-1]["content"]
    n = sum(1 for m in msgs if m["role"] == "tool")
    if not gut: return None, "Okay."
    plan = {"Timer": [("timer_starten", {"minuten": 25})], "Notiz mit dem Titel Einkauf": [("notiz_speichern", {"titel": "Einkauf", "text": "Milch, Brot und Kaffee"})],
            "Flug": [], "notizen.txt": [("datei_lesen", {"pfad": "notizen.txt"}), ("notiz_speichern", {"titel": "Wichtig", "text": "Steuererklärung bis 31. Juli abgeben"})],
            "MAX_RETRIES": [("projekt_suchen", {"muster": "MAX_RETRIES"})], "spät": [("uhrzeit", {})]}
    ende = {"Flug": "Das kann ich leider nicht buchen.", "MAX_RETRIES": "In src/config.rs, Wert 5.", "spät": "Es ist 14:37 Uhr."}
    for k, schritte in plan.items():
        if k in user:
            if n < len(schritte): return schritte[n], ""
            return None, ende.get(k, "Erledigt.")
    return None, "Okay."
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        self.send_response(200); self.send_header("Content-Type", "application/json"); self.end_headers(); self.wfile.write(b'{"status":"ok"}')
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        msgs = body["messages"]; prompt = msgs[-1]["content"] or ""
        gut_chat = art in ("gut", "mittel"); gut_code = art in ("gut", "gutcode")
        self.send_response(200); self.send_header("Content-Type", "text/event-stream"); self.end_headers()
        def sende(d): self.wfile.write(("data: " + json.dumps(d) + "\n\n").encode()); self.wfile.flush()
        if body.get("chat_template_kwargs", {}).get("enable_thinking"):
            for _ in range(40 if art == "gut" else 300): sende({"choices": [{"delta": {"reasoning_content": "hm "}}]})
        if body.get("response_format"):
            text = json.dumps(agent(prompt, gut_code))
        elif body.get("tools"):
            call, text = tools(msgs, art == "gut")
            if call:
                sende({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"name": call[0], "arguments": json.dumps(call[1])}}]}}]})
                text = ""
        else:
            ist_code = "```" in prompt or "Block" in prompt or "Datei" in prompt and "HTML" in prompt
            gut = gut_code if ist_code else (art == "gut" if any(k in prompt for k in ("Ergebnis", "Reihenfolge:", "Termin:")) else gut_chat)
            text = antwort_fuer(prompt, gut)
        for i in range(0, len(text), 12): sende({"choices": [{"delta": {"content": text[i:i+12]}}]})
        sende({"choices": [{"delta": {}, "finish_reason": "stop"}], "timings": {"predicted_n": max(1, len(text) // 4), "predicted_per_second": 42.0}})
        self.wfile.write(b"data: [DONE]\n\n")
time.sleep(0.2)
http.server.HTTPServer(("127.0.0.1", port), H).serve_forever()
'''


class Bench(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = Path(tempfile.mkdtemp(prefix="noki-bench-test-"))
        hf_port = frei()
        cls.hf = http.server.ThreadingHTTPServer(("127.0.0.1", hf_port), HF)
        threading.Thread(target=cls.hf.serve_forever, daemon=True).start()
        modelle = cls.tmp / "llama-models"
        modelle.mkdir()
        (modelle / "basis-qwen.gguf").write_text("mittel")
        (modelle / "JackOD-9B-Coder.Q4_K_M.gguf").write_text("schwach")
        (modelle / "models.ini").write_text("[*]\nctx-size = 8192\n\n[qwen3.5-9b]\nmodel = basis-qwen.gguf\n\n[jackod-9b]\nmodel = JackOD-9B-Coder.Q4_K_M.gguf\n")
        llama = cls.tmp / "llama-server"
        llama.write_text(LLAMA)
        llama.chmod(llama.stat().st_mode | stat.S_IEXEC)
        cls.lauf_log = cls.tmp / "laeufe.log"
        os.environ.update({"NOKI_LLAMA_MODELS": str(modelle), "NOKI_BENCH_HF": f"http://127.0.0.1:{hf_port}",
                           "LLAMA_SERVER_BIN": str(llama), "NOKI_BENCH_PORT": str(frei()),
                           "NOKI_ROUTER": f"127.0.0.1:{hf_port}", "MOCK_LAUF_LOG": str(cls.lauf_log)})
        sys.path.insert(0, str(HIER))
        import importlib
        import bench
        cls.b = importlib.reload(bench)
        cls.modelle = modelle

    @classmethod
    def tearDownClass(cls):
        cls.hf.shutdown()

    def test_ganzer_ablauf(self):
        b = self.b
        b.main(["alles"])
        verif = json.loads((b.BENCH / "verifiziert.json").read_text())
        # nur eindeutige Upstreams; GGUF nur von vertrauenswuerdigem Quantisierer mit passendem base_model
        self.assertEqual(verif["neo"]["gguf_repo"], "mradermacher/Qwen3.5-9B-Neo-GGUF")
        self.assertEqual(verif["neo"]["dateien"][0]["datei"], "Qwen3.5-9B-Neo.Q4_K_M.gguf", "Q4_K_M, nie Q8")
        self.assertEqual(verif["r7"]["repo"], "labA/Qwen3.5-9B-R7-Research", "Quantisierer-Kopie zaehlt nicht als Upstream")
        self.assertNotIn("harmonic", verif, "mehrdeutiger Name wird nicht geladen")
        self.assertNotIn("qwopus", verif)
        # ein Modell zur Zeit, NOKIs Router vorher entladen
        self.assertIn("qwen3.5-9b", ROUTER_ENTLADEN)
        e = b.ergebnisse_laden()
        self.assertEqual(set(e), {"qwen3.5-9b", "jackod-9b", "r7", "neo", "claude-code", "mtp-swe"}, "Phase 2 entfaellt bei klaren Gewinnern")
        kz = {k: b.kennzahlen(v) for k, v in e.items()}
        self.assertEqual(kz["neo"]["kategorien"]["reasoning"], 100.0)
        self.assertEqual(kz["neo"]["tool_erfolg"], 100.0)
        self.assertEqual(kz["neo"]["kategorien"]["general"], 100.0)
        self.assertEqual(kz["neo"]["kategorien"]["creative"], 100.0)
        self.assertEqual(kz["mtp-swe"]["kategorien"]["code_funktional"], 100.0, json.dumps(e["mtp-swe"]["aufgaben"], ensure_ascii=False)[:3000])
        self.assertEqual(kz["mtp-swe"]["kategorien"]["code_kreativ"], 100.0, json.dumps(e["mtp-swe"]["aufgaben"], ensure_ascii=False)[:3000])
        self.assertLess(kz["r7"]["kategorien"]["reasoning"], 50)
        self.assertGreater(kz["r7"]["reasoning_denk_tokens"], kz["neo"]["reasoning_denk_tokens"], "Denk-Tokens gemessen")
        self.assertTrue(kz["neo"]["ram_peak_mb"] > 0 and kz["neo"]["laden_ms"] > 0)
        gew = json.loads((b.BENCH / "ergebnis.json").read_text())["gewinner"]
        self.assertEqual(gew["chat.general"]["key"], "qwen3.5-9b", "vorhandenes Qwen bleibt Allgemein, wenn es mithaelt")
        self.assertEqual(gew["chat.reasoning"]["key"], "neo")
        self.assertEqual(gew["chat.tools"]["key"], "neo")
        self.assertEqual(gew["code.funktional"]["key"], "mtp-swe")
        self.assertEqual(gew["code.kreativ"]["key"], "mtp-swe", "ein Coding-Modell deckt beide Rollen ab")
        # Verlierer-Downloads sind schon waehrend des Laufs wieder weg
        dl = json.loads((b.BENCH / "downloads.json").read_text())
        self.assertEqual(set(dl), {"neo", "mtp-swe"})
        self.assertFalse((b.BENCH / "Qwen3.5-9B-Claude-Code-Q4_K_M.gguf").exists())
        self.assertIn("Claude-Code", (b.BENCH / "aufraeumen.log").read_text())
        # uebernehmen
        b.main(["uebernehmen", "--ja"])
        ini = (self.modelle / "models.ini").read_text()
        self.assertIn("[noki-neo]\nmodel = Qwen3.5-9B-Neo.Q4_K_M.gguf", ini)
        self.assertIn("[noki-mtp-swe]", ini)
        self.assertTrue((self.modelle / "basis-qwen.gguf").exists() and (self.modelle / "JackOD-9B-Coder.Q4_K_M.gguf").exists(), "vorhandene Modelle bleiben")
        rollen = json.loads((self.modelle / "noki-rollen.json").read_text())
        self.assertEqual(rollen["chat"], {"general": "qwen3.5-9b", "reasoning": "noki-neo", "tools": "noki-neo"})
        self.assertEqual(rollen["code"], {"funktional": "noki-mtp-swe", "kreativ": "noki-mtp-swe"})
        self.assertEqual(rollen["modelle"]["noki-neo"]["repo"], "Jackrong/Qwen3.5-9B-Neo")
        self.assertEqual(json.loads((b.BENCH / "downloads.json").read_text()), {})
        self.assertTrue(list(self.modelle.glob("models.ini.vor-bench-*")), "Sicherung von models.ini")
        bericht = (b.BENCH / "ergebnis.md").read_text()
        self.assertIn("chat.reasoning** → neo", bericht)


if __name__ == "__main__":
    unittest.main(verbosity=2)
