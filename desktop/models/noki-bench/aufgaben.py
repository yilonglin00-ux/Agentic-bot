"""NOKI lokaler Modell-Benchmark - Aufgaben und Bewertung.

Jede Aufgabe ist deterministisch bewertbar (0..1, Teilpunkte moeglich).
Code wird wirklich ausgefuehrt (Python immer; Node/rustc, wenn vorhanden -
sonst zaehlt die Aufgabe als "nicht pruefbar" und faellt aus dem Schnitt).
Kategorien:
  general, creative            -> Noki Chat Allgemein
  reasoning                    -> Noki Chat Reasoning (+ Effizienz)
  tools                        -> Noki Chat Werkzeuge (OpenAI-Tool-Calls)
  code_funktional              -> Noki Code Funktional (inkl. echtem Agent-Loop)
  code_kreativ                 -> Noki Code Kreativ (inkl. echtem Agent-Loop)
"""
import difflib
import html.parser
import json
import os
import re
import shutil
import subprocess
import tempfile

NICHT_PRUEFBAR = None  # Bewertung, wenn eine Werkzeugkette fehlt

# ---------------------------------------------------------------- Hilfen

DEUTSCH = {"der", "die", "das", "und", "ist", "ich", "nicht", "mit", "es", "ein", "eine", "zu", "sie", "wir", "du",
           "mir", "gut", "danke", "auch", "dir", "für", "auf", "den", "dem", "noch", "heute", "wie", "bin", "geht"}


def woerter(t):
    return re.findall(r"[A-Za-zÄÖÜäöüß0-9']+", t)


def ist_deutsch(t):
    w = {x.lower() for x in woerter(t)}
    return len(w & DEUTSCH) >= 2


def saetze(t):
    return [s for s in re.split(r"(?<=[.!?])\s+", t.strip()) if re.search(r"\w", s)]


def code_bloecke(t, sprache=None):
    bl = re.findall(r"```([A-Za-z0-9_+-]*)\s*\n(.*?)```", t, re.S)
    if sprache:
        passend = [c for (l, c) in bl if l.lower() in sprache]
        if passend:
            return passend
    return [c for (_, c) in bl]


def erster_code(t, sprache=None):
    b = code_bloecke(t, sprache)
    return b[0] if b else t


def zahl_in(t, wert, tol=0.01):
    for z in re.findall(r"-?\d+(?:[.,]\d+)?", t.replace(" ", "")):
        try:
            if abs(float(z.replace(",", ".")) - wert) <= tol:
                return True
        except ValueError:
            pass
    return False


def ausfuehren(cmd, cwd, timeout=20, eingabe=None):
    try:
        p = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=timeout, input=eingabe)
        return p.returncode, (p.stdout + p.stderr)[-4000:]
    except (subprocess.TimeoutExpired, FileNotFoundError) as e:
        return 1, str(e)


def python_tests(code, tests):
    d = tempfile.mkdtemp(prefix="noki-bench-")
    try:
        open(os.path.join(d, "loesung.py"), "w").write(code)
        open(os.path.join(d, "test_l.py"), "w").write("from loesung import *\n" + tests)
        rc, out = ausfuehren(["python3", "test_l.py"], d)
        return rc == 0, out
    finally:
        shutil.rmtree(d, ignore_errors=True)


def node_ok():
    return shutil.which("node") is not None


def node_tests(code, tests):
    if not node_ok():
        return NICHT_PRUEFBAR, "node fehlt"
    d = tempfile.mkdtemp(prefix="noki-bench-")
    try:
        open(os.path.join(d, "t.js"), "w").write(code + "\n;\n" + tests)
        rc, out = ausfuehren(["node", "t.js"], d)
        return rc == 0, out
    finally:
        shutil.rmtree(d, ignore_errors=True)


def node_syntax(code):
    if not node_ok():
        return NICHT_PRUEFBAR
    d = tempfile.mkdtemp(prefix="noki-bench-")
    try:
        open(os.path.join(d, "s.js"), "w").write(code)
        rc, _ = ausfuehren(["node", "--check", "s.js"], d)
        return rc == 0
    finally:
        shutil.rmtree(d, ignore_errors=True)


def rust_tests(code):
    if not shutil.which("rustc"):
        return NICHT_PRUEFBAR, "rustc fehlt"
    d = tempfile.mkdtemp(prefix="noki-bench-")
    try:
        open(os.path.join(d, "l.rs"), "w").write(code)
        rc, out = ausfuehren(["rustc", "--edition", "2021", "--test", "-o", "t", "l.rs"], d, timeout=90)
        if rc != 0:
            return False, out
        rc, out = ausfuehren(["./t"], d)
        return rc == 0, out
    finally:
        shutil.rmtree(d, ignore_errors=True)


def geaenderte_zeilen(alt, neu):
    return sum(1 for l in difflib.unified_diff(alt.strip().splitlines(), neu.strip().splitlines(), lineterm="", n=0)
               if (l.startswith("+") or l.startswith("-")) and not l.startswith(("+++", "---")))


class _HtmlPruefer(html.parser.HTMLParser):
    def __init__(self):
        super().__init__()
        self.tags = []
        self.fehler = False

    def handle_starttag(self, tag, attrs):
        self.tags.append((tag, dict(attrs)))


def html_tags(t):
    p = _HtmlPruefer()
    try:
        p.feed(t)
    except Exception:
        return None
    return p.tags


def teilpunkte(*bedingungen):
    b = [x for x in bedingungen if x is not None]
    return sum(1 for x in b if x) / max(1, len(b))


# ---------------------------------------------------------------- GENERAL / CREATIVE

def _g_unterhaltung(r):
    t = r["text"]
    return teilpunkte(ist_deutsch(t), len(saetze(t)) == 2, len(t) < 400)


def _g_liste(r):
    zeilen = [z.strip() for z in r["text"].strip().splitlines() if z.strip()]
    nummern = [z for z in zeilen if re.match(r"^\d+[.)]\s", z)]
    return teilpunkte(len(nummern) == 3, len(zeilen) == 3, ist_deutsch(r["text"]))


ZUSAMMENFASSUNG_TEXT = ("Die Stadtbibliothek Lindenau verlängert ab März ihre Öffnungszeiten. Montags bis freitags ist sie "
                        "künftig von 8 bis 21 Uhr geöffnet, samstags von 10 bis 18 Uhr. Grund ist die hohe Nachfrage von "
                        "Studierenden in der Prüfungszeit. Zusätzlich werden 40 neue Arbeitsplätze mit Steckdosen eingerichtet. "
                        "Die Finanzierung übernimmt zur Hälfte die Universität, den Rest trägt die Stadt. Eine Evaluation ist "
                        "nach sechs Monaten geplant.")


def _g_zusammenfassung(r):
    t = r["text"]
    w = len(woerter(t))
    return teilpunkte(w <= 40, w >= 8, "21" in t, re.search(r"arbeitspl", t, re.I) is not None, ist_deutsch(t))


def _g_json(r):
    t = r["text"].strip()
    m = re.search(r"\{.*\}", t, re.S)
    try:
        d = json.loads(m.group(0)) if m else None
    except ValueError:
        d = None
    if not isinstance(d, dict):
        return 0.0
    nur_json = t.startswith("{") or t.startswith("```")
    return teilpunkte(d.get("name") == "Mira Hoffmann", str(d.get("stadt", "")).lower() == "leipzig",
                      d.get("alter") == 34, set(d.keys()) == {"name", "stadt", "alter"}, nur_json)


def _g_hoeflich(r):
    t = r["text"]
    return teilpunkte("bitte" in t.lower(), "sofort" not in t.lower(), ist_deutsch(t), len(saetze(t)) <= 2)


def _c_ideen(r):
    zeilen = [z for z in r["text"].splitlines() if re.match(r"^\s*(\d+[.)]|[-*])\s+\S", z)]
    eindeutig = len({re.sub(r"\W", "", z.lower())[:30] for z in zeilen})
    return teilpunkte(len(zeilen) == 5, eindeutig == len(zeilen), ist_deutsch(r["text"]))


def _c_varianten(r):
    zeilen = [re.sub(r"^\s*(\d+[.)]|[-*])\s*", "", z).strip() for z in r["text"].splitlines() if re.match(r"^\s*(\d+[.)]|[-*])\s+\S", z)]
    if len(zeilen) != 3:
        return teilpunkte(False, ist_deutsch(r["text"]))
    aehnlich = max(difflib.SequenceMatcher(None, a, b).ratio() for i, a in enumerate(zeilen) for b in zeilen[i + 1:])
    return teilpunkte(True, aehnlich < 0.75, all("termin" in z.lower() for z in zeilen))


def _c_slogan(r):
    t = r["text"].strip().strip('"„“').splitlines()[0] if r["text"].strip() else ""
    return teilpunkte(len(woerter(t)) <= 6, "noki" in t.lower(), len(woerter(t)) >= 2, not t.lower().startswith("hier"))


def _c_ui_ideen(r):
    bloecke = re.findall(r"^\s*(?:\d+[.)]|[-*])\s*\**([^:*\n]{2,40})\**\s*[:–-]\s*(.+)$", r["text"], re.M)
    return teilpunkte(len(bloecke) == 3, all(len(saetze(b[1])) <= 2 for b in bloecke), ist_deutsch(r["text"]))


# ---------------------------------------------------------------- REASONING

def _r_reihenfolge(r):
    t = r["text"]
    letzte = t.strip().splitlines()[-1] if t.strip() else ""
    return teilpunkte("cem" in t.lower(), "cem" in letzte.lower() or "ergebnis" in t.lower())


def _r_zug(r):
    m = re.search(r"Ergebnis:\s*([\d.,]+)", r["text"])
    return teilpunkte(m is not None and zahl_in(m.group(1), 88.5), zahl_in(r["text"], 88.5))


def _r_teilbar(r):
    m = re.search(r"Ergebnis:\s*(\d+)", r["text"])
    return teilpunkte(m is not None and m.group(1) == "47", zahl_in(r["text"], 47, 0))


def _r_plan(r):
    m = re.search(r"Reihenfolge:\s*([A-F](?:\s*,\s*[A-F]){5})", r["text"])
    if not m:
        return 0.0
    folge = [x.strip() for x in m.group(1).split(",")]
    if sorted(folge) != list("ABCDEF"):
        return 0.2
    vor = {"C": "A", "D": "B", "E": "C", "F": "D"}
    ok = all(folge.index(vor[k]) < folge.index(k) for k in vor) and folge.index("E") < folge.index("F")
    return 1.0 if ok else 0.3


def _r_termin(r):
    m = re.search(r"Termin:\s*(\w+)\s*,?\s*(\d{1,2})(?::00)?\s*Uhr", r["text"])
    return 1.0 if m and m.group(1).lower().startswith("donnerstag") and m.group(2) == "14" else (0.3 if "donnerstag" in r["text"].lower() else 0.0)


def _r_schlaeger(r):
    m = re.search(r"Ergebnis:\s*([\d.,]+)", r["text"])
    return teilpunkte(m is not None and zahl_in(m.group(1), 0.05, 0.001), not re.search(r"Ergebnis:\s*0[.,]10\b", r["text"]))


# ---------------------------------------------------------------- TOOLS (OpenAI-Tool-Calls)

WERKZEUGE = [
    {"type": "function", "function": {"name": "timer_starten", "description": "Startet Nokis Timer.",
                                      "parameters": {"type": "object", "properties": {"minuten": {"type": "integer"}}, "required": ["minuten"]}}},
    {"type": "function", "function": {"name": "notiz_speichern", "description": "Speichert eine Notiz in Nokis Ablage.",
                                      "parameters": {"type": "object", "properties": {"titel": {"type": "string"}, "text": {"type": "string"}}, "required": ["titel", "text"]}}},
    {"type": "function", "function": {"name": "datei_lesen", "description": "Liest eine Textdatei aus dem Projekt.",
                                      "parameters": {"type": "object", "properties": {"pfad": {"type": "string"}}, "required": ["pfad"]}}},
    {"type": "function", "function": {"name": "projekt_suchen", "description": "Durchsucht das Projekt nach einem Text.",
                                      "parameters": {"type": "object", "properties": {"muster": {"type": "string"}}, "required": ["muster"]}}},
    {"type": "function", "function": {"name": "uhrzeit", "description": "Aktuelle Uhrzeit.", "parameters": {"type": "object", "properties": {}}}},
]
WERKZEUG_NAMEN = {w["function"]["name"] for w in WERKZEUGE}
SIMULATION = {
    "datei_lesen": lambda a: ("Einkauf am Freitag\nWICHTIG: Steuererklärung bis 31. Juli abgeben\nPflanzen gießen"
                              if "notizen" in str(a.get("pfad", "")).lower() else
                              ("pub const MAX_RETRIES: u32 = 5;\npub const TIMEOUT_MS: u64 = 800;" if "config" in str(a.get("pfad", "")) else "Datei nicht gefunden")),
    "projekt_suchen": lambda a: ("src/config.rs:12: pub const MAX_RETRIES: u32 = 5;" if "MAX_RETRIES" in str(a.get("muster", "")) else "Keine Treffer"),
    "uhrzeit": lambda a: "14:37",
    "timer_starten": lambda a: "Timer läuft.",
    "notiz_speichern": lambda a: "Gespeichert.",
}


def tool_bewertung(verlauf, erwartet, ende=None):
    """verlauf: Liste der Tool-Aufrufe [(name, args)], plus finaler Text."""
    aufrufe = verlauf["aufrufe"]
    erfunden = [a for a in aufrufe if a[0] not in WERKZEUG_NAMEN]
    namen = [a[0] for a in aufrufe]
    korrekt_auswahl = namen[:len(erwartet)] == [e[0] for e in erwartet]
    args_ok = all(e[1](a[1]) for e, a in zip(erwartet, aufrufe)) if korrekt_auswahl else False
    gestoppt = len(aufrufe) <= len(erwartet) + 1 and verlauf.get("final_ohne_tool", False)
    p = teilpunkte(not erfunden, korrekt_auswahl, args_ok, gestoppt, ende(verlauf["text"]) if ende else None)
    return p, {"tool_ok": korrekt_auswahl and args_ok and not erfunden, "erfunden": len(erfunden)}


# ---------------------------------------------------------------- CODING FUNKTIONAL

PY_BUG = '''def mittelwert_ohne_extreme(werte):
    """Mittelwert ohne kleinsten und groessten Wert (mind. 3 Werte)."""
    s = sorted(werte)
    innen = s[1:len(s)]
    return sum(innen) / len(innen)
'''
PY_BUG_TESTS = '''assert mittelwert_ohne_extreme([1, 2, 3]) == 2
assert mittelwert_ohne_extreme([10, 1, 4, 7]) == 5.5
assert mittelwert_ohne_extreme([5, 5, 5, 5]) == 5
print("ok")
'''


def _cf_python(r):
    code = erster_code(r["text"], ("python", "py"))
    ok, _ = python_tests(code, PY_BUG_TESTS)
    minimal = geaenderte_zeilen(PY_BUG, code) <= 4
    return teilpunkte(ok, ok and minimal)


JS_BUG = '''function istGueltigePlz(plz) {
  // Deutsche Postleitzahl: genau 5 Ziffern
  return plz.length == 5 || /^\\d+$/.test(plz);
}
'''
JS_BUG_TESTS = '''const a = require("assert");
a.strictEqual(istGueltigePlz("04109"), true);
a.strictEqual(istGueltigePlz("4109"), false);
a.strictEqual(istGueltigePlz("abcde"), false);
a.strictEqual(istGueltigePlz("123456"), false);
'''


def _cf_js(r):
    code = erster_code(r["text"], ("js", "javascript"))
    ok, _ = node_tests(code, JS_BUG_TESTS)
    if ok is NICHT_PRUEFBAR:
        return NICHT_PRUEFBAR
    return teilpunkte(ok, ok and geaenderte_zeilen(JS_BUG, code) <= 3)


RUST_BUG = '''pub fn laengstes<'a>(woerter: &'a [String]) -> Option<&'a str> {
    let mut best: Option<&str> = None;
    for w in woerter {
        let w = w.clone();
        if best.map_or(true, |b| w.len() > b.len()) {
            best = Some(&w);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn findet() {
        let v = vec!["a".to_string(), "abc".to_string(), "ab".to_string()];
        assert_eq!(laengstes(&v), Some("abc"));
        assert_eq!(laengstes(&[]), None);
    }
}
'''


def _cf_rust(r):
    code = erster_code(r["text"], ("rust", "rs"))
    if "#[cfg(test)]" not in code:
        code = code + "\n" + RUST_BUG[RUST_BUG.index("#[cfg(test)]"):]
    ok, _ = rust_tests(code)
    if ok is NICHT_PRUEFBAR:
        return NICHT_PRUEFBAR
    return teilpunkte(ok, ok and "clone()" not in code.split("#[cfg(test)]")[0])


TS_BUG = '''function summeBetraege(posten) {
  // posten: Array<{ betrag: string }>, Beträge mit Komma als Dezimaltrenner ("12,50")
  return posten.reduce((s, p) => s + p.betrag, 0);
}
'''
TS_TESTS = '''const a = require("assert");
a.strictEqual(summeBetraege([{betrag: "12,50"}, {betrag: "0,75"}]), 13.25);
a.strictEqual(summeBetraege([]), 0);
'''


def _cf_ts(r):
    code = erster_code(r["text"], ("js", "javascript", "ts", "typescript"))
    code = re.sub(r":\s*Array<[^>]+>|:\s*\{[^}]*\}\[\]|:\s*number|:\s*string", "", code)  # einfache TS-Typen weg
    ok, _ = node_tests(code, TS_TESTS)
    if ok is NICHT_PRUEFBAR:
        return NICHT_PRUEFBAR
    return teilpunkte(ok)


PY_FEATURE = '''def format_dauer(sekunden):
    """3725 -> '1:02:05', 65 -> '1:05'"""
    m, s = divmod(sekunden, 60)
    h, m = divmod(m, 60)
    if h:
        return f"{h}:{m:02d}:{s:02d}"
    return f"{m}:{s:02d}"
'''
PY_FEATURE_TESTS = '''assert format_dauer(3725) == "1:02:05"
assert format_dauer(65) == "1:05"
assert format_dauer(0) == "0:00"
assert format_dauer(-65) == "-1:05"
assert format_dauer(-3725) == "-1:02:05"
print("ok")
'''


def _cf_regression(r):
    code = erster_code(r["text"], ("python", "py"))
    ok, _ = python_tests(code, PY_FEATURE_TESTS)
    alt_ok, _ = python_tests(code, "\n".join(PY_FEATURE_TESTS.splitlines()[:3]) + "\n")
    return teilpunkte(alt_ok, ok)


PY_DUPLIKAT = '''def netto_a(brutto):
    steuer = brutto * 0.19
    return round(brutto - steuer, 2)

def netto_b(brutto):
    steuer = brutto * 0.07
    return round(brutto - steuer, 2)
'''


def _cf_refactor(r):
    code = erster_code(r["text"], ("python", "py"))
    ok, _ = python_tests(code, "assert netto_a(100) == 81.0\nassert netto_b(100) == 93.0\nprint('ok')\n")
    hilfs = len(re.findall(r"^def ", code, re.M)) >= 3 and code.count("round(") == 1
    return teilpunkte(ok, ok and hilfs)


# ---- Agent-Loop (NOKIs echtes JSON-Aktionsprotokoll, fs.patch als Unified Diff)

AGENT_FUNKTIONAL = {
    "dateien": {
        "src/preise.py": "def rabatt(preis, prozent):\n    \"\"\"prozent als Zahl 0-100\"\"\"\n    return preis * prozent\n\n\ndef endpreis(preis, prozent):\n    return round(preis - rabatt(preis, prozent), 2)\n",
        "src/__init__.py": "",
        "tests/test_preise.py": "import sys, os\nsys.path.insert(0, os.path.join(os.path.dirname(__file__), '..'))\nfrom src.preise import endpreis, rabatt\nassert rabatt(200, 10) == 20\nassert endpreis(200, 10) == 180.0\nassert endpreis(19.99, 0) == 19.99\nprint('ok')\n",
    },
    "aufgabe": "Die Tests in tests/test_preise.py schlagen fehl. Finde die Ursache und repariere sie mit einem minimalen Patch. Ändere die Tests nicht.",
    "test": ["python3", "tests/test_preise.py"],
    "geschuetzt": ["tests/test_preise.py"],
}

AGENT_KREATIV = {
    "dateien": {
        "index.html": "<!doctype html><html><head><link rel=\"stylesheet\" href=\"stil.css\"></head><body><button class=\"knopf\">Speichern</button></body></html>\n",
        "stil.css": ".knopf {\n  background: #333;\n  color: #fff;\n}\n",
    },
    "aufgabe": "Gestalte den Knopf in stil.css neu: ruhig und hochwertig, border-radius 8px, ein Hover-Zustand und ein sichtbarer :focus-visible-Zustand, eine sanfte transition, KEIN box-shadow und keine Farbverläufe. Ändere nur stil.css.",
    "test": None,
    "geschuetzt": ["index.html"],
}


def css_regeln(css):
    return {
        "radius": re.search(r"border-radius\s*:\s*8px", css) is not None,
        "hover": ":hover" in css,
        "focus": ":focus-visible" in css,
        "transition": "transition" in css,
        "kein_schatten": "box-shadow" not in css,
        "kein_verlauf": "gradient" not in css,
    }


def _ck_agent_bewertung(ergebnis):
    css = ergebnis["dateien"].get("stil.css", "")
    r = css_regeln(css)
    return teilpunkte(ergebnis["final"], ergebnis["unveraendert_geschuetzt"], *r.values())


def _cf_agent_bewertung(ergebnis):
    return teilpunkte(ergebnis["tests_ok"], ergebnis["final"], ergebnis["unveraendert_geschuetzt"],
                      ergebnis["schritte"] <= 8, ergebnis["geaenderte_zeilen"] <= 4)


# ---------------------------------------------------------------- CODING KREATIV

def _ck_settings(r):
    code = erster_code(r["text"], ("html",))
    tags = html_tags(code)
    if tags is None:
        return 0.0
    namen = [t for t, _ in tags]
    schalter = any(t == "input" and a.get("type") == "checkbox" for t, a in tags) or any(a.get("role") == "switch" for _, a in tags)
    return teilpunkte("style" in namen, schalter, "gradient" not in code, re.search(r"https?://", code) is None,
                      "benachrichtigung" in code.lower())


def _ck_varianten(r):
    bl = code_bloecke(r["text"], ("css",))
    if len(bl) < 3:
        return teilpunkte(False, len(bl) == 2)
    bl = bl[:3]
    aehnlich = max(difflib.SequenceMatcher(None, a, b).ratio() for i, a in enumerate(bl) for b in bl[i + 1:])
    return teilpunkte(len(code_bloecke(r["text"], ("css",))) == 3, all("@keyframes" in b for b in bl), aehnlich < 0.7)


def _ck_animation(r):
    code = erster_code(r["text"], ("js", "javascript"))
    return teilpunkte("requestAnimationFrame" in code, "prefers-reduced-motion" in r["text"],
                      re.search(r"Math\.(cos|sin)", code) is not None, node_syntax(code))


def _ck_todo(r):
    js = "\n".join(code_bloecke(r["text"], ("js", "javascript")))
    html_teil = "\n".join(code_bloecke(r["text"], ("html",)))
    gesamt = js or r["text"]
    return teilpunkte(bool(js), "addEventListener" in gesamt, re.search(r"remove|splice|filter", gesamt) is not None,
                      re.search(r"import .*(react|vue|svelte)|from ['\"](react|vue)", gesamt, re.I) is None,
                      node_syntax(js) if js else False, bool(html_teil))


def _ck_spec(r):
    css = erster_code(r["text"], ("css",))
    return teilpunkte(re.search(r"padding\s*:\s*16px", css) is not None, re.search(r"font-size\s*:\s*14px", css) is not None,
                      re.search(r"border-radius\s*:\s*10px", css) is not None, re.search(r"gap\s*:\s*12px", css) is not None,
                      "#1c1c1e" in css.lower(), "box-shadow" not in css)


# ---------------------------------------------------------------- Aufgabenliste

def aufgaben():
    A = []

    def chat(id, kat, prompt, bewertung, max_tokens=600, denken=False, format_aufgabe=False):
        A.append({"id": id, "kategorie": kat, "art": "chat", "prompt": prompt, "bewertung": bewertung,
                  "max_tokens": max_tokens, "denken": denken, "format": format_aufgabe})

    chat("g1", "general", "Hey Noki, wie geht's dir heute? Antworte in genau zwei kurzen Sätzen.", _g_unterhaltung, format_aufgabe=True)
    chat("g2", "general", "Nenne drei Vorteile von SSDs gegenüber Festplatten als nummerierte Liste mit genau 3 Punkten. Keine Einleitung, kein Schlusssatz.", _g_liste, format_aufgabe=True)
    chat("g3", "general", "Fasse den folgenden Text in höchstens 40 Wörtern zusammen. Nenne die neuen Öffnungszeiten und die Arbeitsplätze.\n\n" + ZUSAMMENFASSUNG_TEXT, _g_zusammenfassung, format_aufgabe=True)
    chat("g4", "general", "Extrahiere aus dem Satz die Angaben als JSON mit genau den Schlüsseln name, stadt, alter (alter als Zahl). Gib nur das JSON aus.\n\nSatz: Mira Hoffmann ist 34 Jahre alt und wohnt seit 2019 in Leipzig.", _g_json, format_aufgabe=True)
    chat("g5", "general", "Formuliere höflicher, in einem Satz: „Schick mir sofort die Datei.“", _g_hoeflich, format_aufgabe=True)
    chat("c1", "creative", "Gib mir 5 kurze, unterschiedliche Ideen für ein Wochenende in den Bergen bei Regen. Nummerierte Liste, eine Zeile pro Idee.", _c_ideen)
    chat("c2", "creative", "Schreibe drei deutlich unterschiedliche Varianten dieses Satzes als nummerierte Liste: „Der Termin am Montag wird verschoben.“ Jede Variante muss das Wort Termin enthalten.", _c_varianten)
    chat("c3", "creative", "Erfinde einen Slogan für eine Desktop-App namens Noki. Höchstens 6 Wörter, das Wort Noki muss vorkommen. Gib nur den Slogan aus.", _c_slogan, format_aufgabe=True)
    chat("c4", "creative", "Nenne drei UI-Ideen für eine ruhige Timer-App. Format je Zeile: „1. Titel: ein Satz“.", _c_ui_ideen, format_aufgabe=True)

    chat("r1", "reasoning", "Anna ist größer als Ben. Ben ist größer als Cem. Dana ist größer als Anna. Wer ist am kleinsten? Antworte am Ende mit „Ergebnis: <Name>“.", _r_reihenfolge, 2500, True)
    chat("r2", "reasoning", "Ein Zug fährt 2,5 Stunden mit 84 km/h und danach 1,5 Stunden mit 96 km/h. Wie hoch ist die Durchschnittsgeschwindigkeit über die ganze Strecke? Antworte am Ende mit „Ergebnis: <Zahl> km/h“.", _r_zug, 2500, True)
    chat("r3", "reasoning", "Wie viele ganze Zahlen von 1 bis 100 (einschließlich) sind durch 3 oder durch 5 teilbar? Antworte am Ende mit „Ergebnis: <Zahl>“.", _r_teilbar, 2500, True)
    chat("r4", "reasoning", "Plane die Reihenfolge der Aufgaben A–F. Regeln: C erst nach A. D erst nach B. E erst nach C. F erst nach D und nach E. Gib am Ende eine gültige Reihenfolge an: „Reihenfolge: A, B, …“ (alle sechs, durch Komma getrennt).", _r_plan, 2500, True)
    chat("r5", "reasoning", "Finde einen gemeinsamen einstündigen Termin (volle Stunde, 9–17 Uhr, Montag bis Freitag). Lena kann nur Dienstag und Donnerstag. Tom hat donnerstags bis 13 Uhr Vorlesung und dienstags ganztägig keine Zeit. Mia kann nicht vor 14 Uhr und donnerstags nicht nach 15 Uhr. Gib den frühestmöglichen Termin an als „Termin: <Wochentag>, <Stunde> Uhr“.", _r_termin, 2500, True)
    chat("r6", "reasoning", "Ein Schläger und ein Ball kosten zusammen 1,10 €. Der Schläger kostet 1,00 € mehr als der Ball. Wie viel kostet der Ball? Antworte am Ende mit „Ergebnis: <Betrag> €“.", _r_schlaeger, 2500, True)

    def tool(id, prompt, erwartet, ende=None):
        A.append({"id": id, "kategorie": "tools", "art": "tools", "prompt": prompt, "erwartet": erwartet, "ende": ende, "max_tokens": 400})

    tool("t1", "Starte einen Timer für 25 Minuten.", [("timer_starten", lambda a: int(a.get("minuten", 0)) == 25)])
    tool("t2", "Speichere eine Notiz mit dem Titel Einkauf: Milch, Brot und Kaffee.",
         [("notiz_speichern", lambda a: a.get("titel", "").strip().lower() == "einkauf" and all(x in a.get("text", "").lower() for x in ("milch", "brot", "kaffee")))])
    tool("t3", "Buche mir bitte einen Flug nach Rom für morgen früh.", [], ende=lambda t: re.search(r"kann|nicht|leider|keine", t.lower()) is not None)
    tool("t4", "Lies die Datei notizen.txt und speichere die als WICHTIG markierte Zeile als Notiz mit dem Titel Wichtig.",
         [("datei_lesen", lambda a: "notizen" in a.get("pfad", "").lower()),
          ("notiz_speichern", lambda a: a.get("titel", "").strip().lower() == "wichtig" and "steuererkl" in a.get("text", "").lower())])
    tool("t5", "In welcher Datei ist MAX_RETRIES definiert und welchen Wert hat es? Nutze die Projektsuche.",
         [("projekt_suchen", lambda a: "MAX_RETRIES" in a.get("muster", ""))], ende=lambda t: "5" in t and "config" in t.lower())
    tool("t6", "Wie spät ist es gerade?", [("uhrzeit", lambda a: True)], ende=lambda t: "14:37" in t or "14.37" in t)

    def code(id, kat, prompt, bewertung, max_tokens=1200):
        A.append({"id": id, "kategorie": kat, "art": "code", "prompt": prompt, "bewertung": bewertung, "max_tokens": max_tokens})

    code("cf1", "code_funktional", "Diese Python-Funktion liefert falsche Ergebnisse. Korrigiere NUR den Fehler und gib die vollständige Funktion in einem ```python-Block zurück.\n\n```python\n" + PY_BUG + "```", _cf_python)
    code("cf2", "code_funktional", "Diese JavaScript-Funktion prüft deutsche Postleitzahlen falsch. Korrigiere sie minimal und gib die vollständige Funktion in einem ```js-Block zurück.\n\n```js\n" + JS_BUG + "```", _cf_js)
    code("cf3", "code_funktional", "Dieser Rust-Code kompiliert nicht (Lebensdauer). Behebe den Fehler ohne unnötiges Klonen und gib den vollständigen Code inklusive Tests in einem ```rust-Block zurück.\n\n```rust\n" + RUST_BUG + "```", _cf_rust, 1500)
    code("cf4", "code_funktional", "Diese Funktion (TypeScript/JavaScript) summiert Beträge falsch. Die Beträge sind Strings mit Komma als Dezimaltrenner. Korrigiere sie und gib die Funktion in einem ```js-Block zurück (ohne Typannotationen).\n\n```js\n" + TS_BUG + "```", _cf_ts)
    code("cf5", "code_funktional", "Erweitere die Funktion so, dass auch negative Sekunden korrekt formatiert werden (−65 -> '-1:05', −3725 -> '-1:02:05'). Bestehendes Verhalten darf sich nicht ändern. Gib die vollständige Funktion in einem ```python-Block zurück.\n\n```python\n" + PY_FEATURE + "```", _cf_regression)
    code("cf6", "code_funktional", "Refaktoriere den doppelten Code: eine gemeinsame Hilfsfunktion, netto_a und netto_b bleiben mit gleicher Signatur und gleichem Ergebnis erhalten. Gib den Code in einem ```python-Block zurück.\n\n```python\n" + PY_DUPLIKAT + "```", _cf_refactor)
    A.append({"id": "cf7", "kategorie": "code_funktional", "art": "agent", "stil": "funktional", "projekt": AGENT_FUNKTIONAL, "bewertung": _cf_agent_bewertung})

    code("ck1", "code_kreativ", "Entwirf eine einzelne HTML-Datei (inline CSS, kein JavaScript nötig) mit einer Einstellungskarte im ruhigen, dunklen macOS-Stil: Titel, kurzer Beschreibungstext und ein Schalter für „Benachrichtigungen“. Keine externen Ressourcen, keine Farbverläufe. Gib die Datei in einem ```html-Block zurück.", _ck_settings, 1500)
    code("ck2", "code_kreativ", "Entwickle drei klar unterschiedliche Lade-Indikatoren in CSS. Jede Variante in einem eigenen ```css-Block mit eigener @keyframes-Animation.", _ck_varianten, 1500)
    code("ck3", "code_kreativ", "Schreibe eine kleine requestAnimationFrame-Animation in JavaScript, die einen Punkt auf einer Kreisbahn um die Mitte eines Canvas bewegt. Respektiere prefers-reduced-motion. Gib den Code in einem ```js-Block zurück.", _ck_animation, 1200)
    code("ck4", "code_kreativ", "Baue einen kleinen Prototyp einer Todo-Liste ohne Framework: ein ```html-Block mit dem Markup und ein ```js-Block mit der Logik (Hinzufügen und Entfernen von Einträgen).", _ck_todo, 1500)
    code("ck5", "code_kreativ", "Setze diese Designvorgabe als CSS-Klasse .karte um: Innenabstand 16px, Schriftgröße 14px, Eckenradius 10px, Abstand zwischen Kindelementen 12px (Flexbox), Hintergrund #1c1c1e, kein Schatten. Gib den Code in einem ```css-Block zurück.", _ck_spec, 800)
    A.append({"id": "ck6", "kategorie": "code_kreativ", "art": "agent", "stil": "kreativ", "projekt": AGENT_KREATIV, "bewertung": _ck_agent_bewertung})
    return A


KATEGORIEN = ["general", "creative", "reasoning", "tools", "code_funktional", "code_kreativ"]
