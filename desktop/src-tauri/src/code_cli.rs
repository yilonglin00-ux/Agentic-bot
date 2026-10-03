//! `noki code` inside a Code Space terminal (or any terminal) while the Noki
//! app runs: a normal program. It connects to the app's Code service, gets
//! a session for THIS terminal, reads tasks at its own `›` prompt and prints
//! what really happens (model, files, preview checks, repairs, result).
//! `/exit` or ⌃D ends the program - the terminal is back at its shell.

use serde_json::{json, Value};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

extern "C" {
    fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
}
static SIGINT: AtomicBool = AtomicBool::new(false);
/// The user left Noki Code: the closing connection is expected.
static ENDE: AtomicBool = AtomicBool::new(false);
extern "C" fn bei_sigint(_: i32) {
    SIGINT.store(true, Ordering::SeqCst);
}

const HILFE: &str = "  /modus funktional | /modus kreativ   Modus dieser Sitzung (auch über die Modus-Buttons)\n  /vorschau   /finder   Vorschau bzw. Projektordner öffnen\n  /neu        nächste Aufgabe in einem neuen Projekt\n  /exit       Noki Code beenden (zurück zur Shell) · ⌃C bricht eine laufende Aufgabe ab";

fn modus_text(m: &str) -> &'static str {
    if m == "creative" { "Kreativ" } else { "Funktional" }
}

fn home(p: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if p.starts_with(&h) => format!("~{}", &p[h.len()..]),
        _ => p.to_string(),
    }
}

fn lane(m: &Value) -> String {
    let l = match m["execution_lane"].as_str().unwrap_or("") {
        "LOCAL" => "Local",
        "FREE_CLOUD" => "Free Cloud",
        "PAID_CLOUD" => "Paid Cloud",
        _ => "",
    };
    let name = m["display_name"].as_str().unwrap_or("")
        .replace(" Free (Mistral)", "")
        .replace(" Free", "");
    if l.is_empty() { name } else { format!("{name} · {l}") }
}

fn zeile(_g: &str, farbe: &str, label: &str, text: &str) {
    // Hierarchy by coloured label text - no decorative status symbol.
    let mut it = text.lines();
    println!("\r  \x1b[1;{farbe}m{label:<9}\x1b[0m {}", it.next().unwrap_or(""));
    for l in it.take(12) {
        println!("             \x1b[2m{l}\x1b[0m");
    }
}

/// A real diff (from the files on disk), coloured, capped for the terminal.
fn diff_zeigen(diff: &str) {
    let zeilen: Vec<&str> = diff.lines().collect();
    for l in zeilen.iter().take(40) {
        let (f, t) = match l.chars().next() {
            Some('+') => ("32", *l),
            Some('-') => ("31", *l),
            Some('@') => ("2;36", *l),
            _ => ("2", *l),
        };
        println!("             \x1b[{f}m{t}\x1b[0m");
    }
    if zeilen.len() > 40 {
        println!("             \x1b[2m… {} weitere Diff-Zeilen (vollständig in .noki/projekt.json · Code öffnen)\x1b[0m", zeilen.len() - 40);
    }
}

/// Preview reports are one long sentence chain: one fact per line.
fn saetze(t: &str) -> String {
    t.replace(". ", ".\n").replace(" · ", "\n")
}

fn prompt() {
    print!("\x1b[38;5;180m›\x1b[0m ");
    let _ = io::stdout().flush();
}

struct Zustand {
    beschaeftigt: AtomicBool,
    modell: Mutex<String>,
    raus: Mutex<Option<UnixStream>>,
}

impl Zustand {
    fn senden(&self, v: Value) {
        if let Ok(mut g) = self.raus.lock() {
            if let Some(s) = g.as_mut() {
                let mut z = v.to_string();
                z.push('\n');
                let _ = s.write_all(z.as_bytes());
            }
        }
    }
}

fn ausgeben(z: &Zustand, m: &Value) {
    match m["typ"].as_str().unwrap_or("") {
        "bereit" => {
            let projekt = m["projekt"]["pfad"].as_str().map(home).unwrap_or_else(|| "neu bei der ersten Aufgabe".into());
            println!(
                "\x1b[2m{} · Modus {} · Projekt: {}\x1b[0m",
                m["titel"].as_str().unwrap_or("Terminal"),
                modus_text(m["modus"].as_str().unwrap_or("")),
                projekt
            );
            println!("\x1b[2m{}\x1b[0m", "─".repeat(56));
            println!("Noki Code bereit. Beschreibe eine Coding-Aufgabe (mehrzeilig einfügen geht). /hilfe zeigt Befehle.");
        }
        "angenommen" => {}
        "start" => {
            z.beschaeftigt.store(true, Ordering::SeqCst);
            z.modell.lock().unwrap_or_else(|e| e.into_inner()).clear();
            let p = home(m["pfad"].as_str().unwrap_or(""));
            zeile("◆", "35", "Projekt", &format!("{} {p}", if m["neu"] == true { "erstellt ·" } else { "geöffnet ·" }));
            zeile("◇", "36", "Modus", modus_text(m["modus"].as_str().unwrap_or("")));
        }
        "noki" => zeile("→", "35", "Noki", m["text"].as_str().unwrap_or("")),
        "wechsel" => zeile("!", "33", "Wechsel", m["text"].as_str().unwrap_or("")),
        "modell" => {
            // Once per model (and on every fallback), not before each step.
            let l = lane(m);
            let mut g = z.modell.lock().unwrap_or_else(|e| e.into_inner());
            if *g == l {
                return;
            }
            *g = l.clone();
            zeile("◆", "38;5;180", "Modell", &l);
        }
        "denkt" => {
            print!("\r  \x1b[2mSchritt {} …\x1b[0m\x1b[K", m["schritt"].as_u64().unwrap_or(1));
            let _ = io::stdout().flush();
            return;
        }
        "schritt" => {
            let label = m["label"].as_str().unwrap_or("");
            let ok = m["ok"].as_bool().unwrap_or(false);
            let detail = m["detail"].as_str().unwrap_or("");
            match m["art"].as_str().unwrap_or("") {
                "notiz" => zeile("◇", "36", "Denken", detail),
                "datei" if ok => {
                    let (p, mi) = (m["plus"].as_u64().unwrap_or(0), m["minus"].as_u64().unwrap_or(0));
                    println!("\r  \x1b[1;33m{:<9}\x1b[0m {label}  \x1b[32m+{p}\x1b[0m \x1b[31m-{mi}\x1b[0m", "Datei");
                    diff_zeigen(m["diff"].as_str().unwrap_or(""));
                }
                "audit" => {
                    let farbe = if ok { "32" } else { "33" };
                    println!("\r  \x1b[1;{farbe}m{:<9}\x1b[0m {label}", "Prüfung");
                    for l in detail.lines() {
                        let (f, w) = if l.starts_with('✓') { ("32", "erfüllt  ") } else if l.starts_with('△') { ("33", "teilweise") } else { ("31", "offen    ") };
                        let rest = l.trim_start_matches(['✓', '△', '✕']).trim_start();
                        println!("             \x1b[{f}m{w} {rest}\x1b[0m");
                    }
                }
                "test" => {
                    let text = format!("{label}\n{}", saetze(detail));
                    if ok { zeile("", "32", "Test ok", &text) } else { zeile("", "31", "Test Fehler", &text) }
                }
                _ if !ok => zeile("×", "31", "Fehler", &format!("{label}\n{detail}")),
                _ => zeile("◇", "36", "Lesen", label),
            }
        }
        "ende" => {
            z.beschaeftigt.store(false, Ordering::SeqCst);
            let status = m["status"].as_str().unwrap_or("");
            let farbe = if status == "fertig" { "32" } else { "33" };
            println!();
            println!("\r  \x1b[1;{farbe}mErgebnis · {status}\x1b[0m  \x1b[2m{} s · {} Dateischritte · {} Prüfrunden\x1b[0m",
                m["sekunden"].as_u64().unwrap_or(0), m["schreibschritte"].as_u64().unwrap_or(0), m["pruefrunden"].as_u64().unwrap_or(0));
            let pruef = m["pruefung"].as_str().unwrap_or("");
            if !pruef.is_empty() {
                let (w, f) = if pruef.contains("FEHLER") { ("Fehler", "31") } else { ("bestanden", "32") };
                println!("    \x1b[1;{f}mLetzte Vorschau-Prüfung · {w}\x1b[0m");
                for l in saetze(pruef).lines() {
                    println!("      \x1b[2m{l}\x1b[0m");
                }
            }
            if let Some(a) = m["audit"].as_array().filter(|a| !a.is_empty()) {
                println!("    Anforderungen");
                for x in a {
                    let s = x["status"].as_str().unwrap_or("");
                    let (w, f) = match s { "PASS" => ("erfüllt  ", "32"), "PARTIAL" => ("teilweise", "33"), _ => ("offen    ", "31") };
                    println!("      \x1b[{f}m{w}\x1b[0m {} \x1b[2m{}\x1b[0m", x["anforderung"].as_str().unwrap_or(""), x["befund"].as_str().unwrap_or(""));
                }
            }
            if let Some(st) = m["stats"].as_array().filter(|a| !a.is_empty()) {
                println!("    Dateien");
                for x in st {
                    let neu = x["neu"] == true;
                    println!("      {:<28} \x1b[32m+{}\x1b[0m {}{}",
                        x["datei"].as_str().unwrap_or(""), x["plus"].as_u64().unwrap_or(0),
                        if neu { String::new() } else { format!("\x1b[31m−{}\x1b[0m", x["minus"].as_u64().unwrap_or(0)) },
                        if neu { " \x1b[2m(neu)\x1b[0m" } else { "" });
                }
            }
            if m["modell"].is_object() {
                println!("    Modell  {}", lane(&m["modell"]));
            }
            println!("\n  \x1b[2m/vorschau öffnet das Ergebnis · /finder den Projektordner\x1b[0m");
            prompt();
            return;
        }
        "modus" => zeile("◇", "36", "Modus", &format!("{} – gilt für die nächste Aufgabe", modus_text(m["modus"].as_str().unwrap_or("")))),
        "info" => zeile("·", "37", "Info", m["text"].as_str().unwrap_or("")),
        "fehler" => {
            zeile("×", "31", "Fehler", m["text"].as_str().unwrap_or(""));
            prompt();
            return;
        }
        _ => return,
    }
    let _ = io::stdout().flush();
}

fn stty(arg: &str) {
    let _ = std::process::Command::new("/bin/stty")
        .arg(arg)
        .stdin(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Ok(false): the Noki app's Code service is not reachable (caller falls
/// back to the stand-alone session).
pub fn run(cwd: &Path) -> Result<bool, String> {
    let Ok(stream) = UnixStream::connect(crate::intelligence::code_socket_pfad()) else {
        return Ok(false);
    };
    let lesen = stream.try_clone().map_err(|e| e.to_string())?;
    let z = Arc::new(Zustand { beschaeftigt: AtomicBool::new(false), modell: Mutex::new(String::new()), raus: Mutex::new(Some(stream)) });
    z.senden(json!({
        "t": "hallo",
        "terminal": std::env::var("NOKI_TERMINAL_ID").unwrap_or_default(),
        "cwd": cwd.to_string_lossy(),
    }));
    println!("\x1b[38;5;180m{}\x1b[0m", crate::code_terminal::BANNER);
    println!("\x1b[1mNOKI CODE\x1b[0m  \x1b[2mläuft in diesem Terminal · Modell wählt der Noki-Router je Aufgabe\x1b[0m");
    // Clean echo of pasted multi-line prompts; restored on exit.
    stty("-echoctl");
    print!("\x1b[?2004h");
    let _ = io::stdout().flush();
    {
        let z = z.clone();
        std::thread::spawn(move || {
            for l in BufReader::new(lesen).lines() {
                let Ok(l) = l else { break };
                if let Ok(m) = serde_json::from_str::<Value>(&l) {
                    let erster = m["typ"] == "bereit";
                    ausgeben(&z, &m);
                    if erster {
                        prompt();
                    }
                }
            }
            if ENDE.load(Ordering::SeqCst) {
                return;
            }
            println!("\r\n  \x1b[33mVerbindung zur Noki-App beendet.\x1b[0m");
            print!("\x1b[?2004l");
            stty("echoctl");
            std::process::exit(0);
        });
    }
    unsafe {
        signal(2, bei_sigint);
    }
    {
        let z = z.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if SIGINT.swap(false, Ordering::SeqCst) {
                if z.beschaeftigt.load(Ordering::SeqCst) {
                    z.senden(json!({ "t": "abbrechen" }));
                    println!("\n  \x1b[33m⌃C Abbruch angefordert\x1b[0m");
                } else {
                    println!("\n  \x1b[2m/exit oder ⌃D beendet Noki Code.\x1b[0m");
                    prompt();
                }
            }
        });
    }
    let stdin = io::stdin();
    let mut eingabe = stdin.lock();
    let mut einfuegen: Option<String> = None;
    loop {
        let mut line = String::new();
        if eingabe.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            break; // ⌃D
        }
        // Bracketed paste: one multi-line prompt = one task.
        if let Some(i) = line.find("\x1b[200~") {
            let rest = line[i + 6..].to_string();
            einfuegen = Some(String::new());
            line = rest;
        }
        if let Some(buf) = einfuegen.as_mut() {
            if let Some(i) = line.find("\x1b[201~") {
                buf.push_str(&line[..i]);
                buf.push_str(&line[i + 6..]);
                line = einfuegen.take().unwrap_or_default();
            } else {
                buf.push_str(&line);
                continue;
            }
        }
        let text = line.trim();
        if text.is_empty() {
            prompt();
            continue;
        }
        match text {
            "/exit" | "/quit" | "exit" => break,
            "/hilfe" | "/help" => {
                println!("{HILFE}");
                prompt();
            }
            "/vorschau" | "/finder" | "/neu" | "/abbrechen" => {
                z.senden(json!({ "t": &text[1..] }));
                std::thread::sleep(std::time::Duration::from_millis(150));
                prompt();
            }
            t if t.starts_with("/modus") => {
                let m = if t.contains("kreativ") || t.contains("creative") { "creative" } else { "functional" };
                z.senden(json!({ "t": "modus", "modus": m }));
                std::thread::sleep(std::time::Duration::from_millis(150));
                prompt();
            }
            t => {
                if z.beschaeftigt.load(Ordering::SeqCst) {
                    println!("  \x1b[33mNoki arbeitet noch an der letzten Aufgabe – ⌃C bricht ab.\x1b[0m");
                    prompt();
                } else {
                    z.senden(json!({ "t": "aufgabe", "text": t }));
                }
            }
        }
    }
    if z.beschaeftigt.load(Ordering::SeqCst) {
        println!("  \x1b[33mLaufende Aufgabe wird abgebrochen.\x1b[0m");
    }
    ENDE.store(true, Ordering::SeqCst);
    z.senden(json!({ "t": "tschuess" }));
    print!("\x1b[?2004l");
    let _ = io::stdout().flush();
    stty("echoctl");
    println!("Noki Code beendet.");
    Ok(true)
}
