//! Noki long-term memory: local SQLite (system libsqlite3), curated.
//! Never stores chats, screenshots, browser history or credentials.
use serde::Serialize;
use std::{
    ffi::{c_char, c_int, c_void, CStr, CString},
    path::Path,
};

#[link(name = "sqlite3")]
extern "C" {
    fn sqlite3_open_v2(
        f: *const c_char,
        db: *mut *mut c_void,
        flags: c_int,
        vfs: *const c_char,
    ) -> c_int;
    fn sqlite3_close(db: *mut c_void) -> c_int;
    fn sqlite3_exec(
        db: *mut c_void,
        sql: *const c_char,
        cb: *const c_void,
        arg: *mut c_void,
        err: *mut *mut c_char,
    ) -> c_int;
    fn sqlite3_prepare_v2(
        db: *mut c_void,
        sql: *const c_char,
        n: c_int,
        st: *mut *mut c_void,
        tail: *mut *const c_char,
    ) -> c_int;
    fn sqlite3_bind_text(st: *mut c_void, i: c_int, v: *const c_char, n: c_int, d: isize) -> c_int;
    fn sqlite3_bind_int64(st: *mut c_void, i: c_int, v: i64) -> c_int;
    fn sqlite3_step(st: *mut c_void) -> c_int;
    fn sqlite3_column_text(st: *mut c_void, i: c_int) -> *const c_char;
    fn sqlite3_finalize(st: *mut c_void) -> c_int;
    fn sqlite3_errmsg(db: *mut c_void) -> *const c_char;
}
const TRANSIENT: isize = -1;
const ROW: c_int = 100;
const DONE: c_int = 101;

pub enum Val<'a> {
    T(&'a str),
    I(i64),
}
struct Db(*mut c_void);
unsafe impl Send for Db {}
impl Drop for Db {
    fn drop(&mut self) {
        unsafe {
            sqlite3_close(self.0);
        }
    }
}
impl Db {
    fn open(path: &Path) -> Result<Self, String> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let c = CString::new(path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        let mut db = std::ptr::null_mut();
        let rc =
            unsafe { sqlite3_open_v2(c.as_ptr(), &mut db, 0x2 | 0x4 | 0x10000, std::ptr::null()) }; // READWRITE|CREATE|FULLMUTEX
        let db = Db(db);
        if rc != 0 {
            return Err(db.error());
        }
        Ok(db)
    }
    fn error(&self) -> String {
        unsafe {
            CStr::from_ptr(sqlite3_errmsg(self.0))
                .to_string_lossy()
                .into_owned()
        }
    }
    fn exec(&self, sql: &str) -> Result<(), String> {
        let c = CString::new(sql).map_err(|e| e.to_string())?;
        if unsafe {
            sqlite3_exec(
                self.0,
                c.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } == 0
        {
            Ok(())
        } else {
            Err(self.error())
        }
    }
    /// Parameterised statement; returns every row as text columns.
    fn query(&self, sql: &str, args: &[Val], cols: usize) -> Result<Vec<Vec<String>>, String> {
        let c = CString::new(sql).map_err(|e| e.to_string())?;
        let mut st = std::ptr::null_mut();
        if unsafe { sqlite3_prepare_v2(self.0, c.as_ptr(), -1, &mut st, std::ptr::null_mut()) } != 0
        {
            return Err(self.error());
        }
        let texts: Vec<CString> = args
            .iter()
            .map(|a| match a {
                Val::T(t) => CString::new(t.replace('\0', "")).unwrap(),
                Val::I(_) => CString::default(),
            })
            .collect();
        for (i, a) in args.iter().enumerate() {
            unsafe {
                match a {
                    Val::T(_) => {
                        sqlite3_bind_text(st, i as c_int + 1, texts[i].as_ptr(), -1, TRANSIENT)
                    }
                    Val::I(v) => sqlite3_bind_int64(st, i as c_int + 1, *v),
                };
            }
        }
        let mut rows = Vec::new();
        let result = loop {
            match unsafe { sqlite3_step(st) } {
                ROW => rows.push(
                    (0..cols)
                        .map(|i| unsafe {
                            let p = sqlite3_column_text(st, i as c_int);
                            if p.is_null() {
                                String::new()
                            } else {
                                CStr::from_ptr(p).to_string_lossy().into_owned()
                            }
                        })
                        .collect(),
                ),
                DONE => break Ok(rows),
                _ => break Err(self.error()),
            }
        };
        unsafe {
            sqlite3_finalize(st);
        }
        result
    }
}

pub const KINDS: [&str; 5] = [
    "user_preferences",
    "project_context",
    "workflows",
    "interaction_feedback",
    "facts",
];
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: i64,
    pub kind: String,
    pub text: String,
    pub origin: String,
    pub created: i64,
}

pub struct NokiMemory {
    db: Db,
}
impl NokiMemory {
    pub fn open(path: &Path) -> Result<Self, String> {
        let db = Db::open(path)?;
        db.exec("CREATE TABLE IF NOT EXISTS memories(id INTEGER PRIMARY KEY, kind TEXT NOT NULL CHECK(kind IN ('user_preferences','project_context','workflows','interaction_feedback','facts')), text TEXT NOT NULL UNIQUE, origin TEXT NOT NULL, created INTEGER NOT NULL);
                 CREATE TABLE IF NOT EXISTS learn_candidates(key TEXT PRIMARY KEY, count INTEGER NOT NULL);")?;
        Ok(Self { db })
    }
    /// Write policy is enforced here too: sensitive text is never stored.
    pub fn add(&self, kind: &str, text: &str, origin: &str) -> Result<i64, String> {
        let text = text.trim();
        if sensitive(text) {
            return Err("sensibel".into());
        }
        if !KINDS.contains(&kind) || text.is_empty() || text.chars().count() > 300 {
            return Err("ungültig".into());
        }
        self.db.query("INSERT OR IGNORE INTO memories(kind,text,origin,created) VALUES(?,?,?,strftime('%s','now'))", &[Val::T(kind), Val::T(text), Val::T(origin)], 0)?;
        let r = self
            .db
            .query("SELECT id FROM memories WHERE text=?", &[Val::T(text)], 1)?;
        r.first()
            .and_then(|r| r[0].parse().ok())
            .ok_or_else(|| "nicht gespeichert".into())
    }
    pub fn list(&self) -> Vec<Entry> {
        self.db
            .query(
                "SELECT id,kind,text,origin,created FROM memories ORDER BY id DESC",
                &[],
                5,
            )
            .unwrap_or_default()
            .into_iter()
            .map(|r| Entry {
                id: r[0].parse().unwrap_or(0),
                kind: r[1].clone(),
                text: r[2].clone(),
                origin: r[3].clone(),
                created: r[4].parse().unwrap_or(0),
            })
            .collect()
    }
    pub fn count(&self) -> usize {
        self.db
            .query("SELECT count(*) FROM memories", &[], 1)
            .ok()
            .and_then(|r| r.first().and_then(|r| r[0].parse().ok()))
            .unwrap_or(0)
    }
    pub fn delete(&self, id: i64) -> Result<(), String> {
        self.db
            .query("DELETE FROM memories WHERE id=?", &[Val::I(id)], 0)
            .map(|_| ())
    }
    pub fn clear(&self) -> Result<(), String> {
        self.db
            .exec("DELETE FROM memories; DELETE FROM learn_candidates;")
    }
    /// Counts repeated harmless signals for auto-learning; returns the new count.
    pub fn bump(&self, key: &str) -> i64 {
        let _ = self.db.query("INSERT INTO learn_candidates(key,count) VALUES(?,1) ON CONFLICT(key) DO UPDATE SET count=count+1", &[Val::T(key)], 0);
        self.db
            .query(
                "SELECT count FROM learn_candidates WHERE key=?",
                &[Val::T(key)],
                1,
            )
            .ok()
            .and_then(|r| r.first().and_then(|r| r[0].parse().ok()))
            .unwrap_or(0)
    }
    /// Keyword retrieval: only the few entries sharing content words with the question.
    pub fn retrieve(&self, query: &str, limit: usize) -> Vec<Entry> {
        let q = stems(query);
        if q.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(usize, Entry)> = self
            .list()
            .into_iter()
            .map(|e| {
                let s = stems(&e.text);
                (q.iter().filter(|w| s.contains(w)).count(), e)
            })
            .filter(|x| x.0 > 0)
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.id.cmp(&a.1.id)));
        scored.into_iter().take(limit).map(|x| x.1).collect()
    }
}

const STOP: &[&str] = &[
    "ich", "mich", "mir", "mein", "meine", "meinen", "du", "dir", "dein", "was", "wie", "wer",
    "womit", "welche", "welcher", "welches", "wo", "wann", "warum", "ist", "sind", "bin", "der",
    "die", "das", "den", "dem", "des", "ein", "eine", "einen", "und", "oder", "aber", "mit", "für",
    "von", "vom", "zu", "zum", "zur", "im", "in", "am", "an", "auf", "noki", "bitte", "dass",
    "gerne", "liebsten", "immer", "auch", "nicht", "the", "and", "what",
];
/// Lowercase content-word stems (first 5 chars) for tolerant German matching.
pub fn stems(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3 && !STOP.contains(w))
        .map(|w| {
            w.chars()
                .take(5)
                .collect::<String>()
                .trim_end_matches('s')
                .to_owned()
        })
        .collect()
}
/// Credentials, payment and identity data never enter memory — not even on request.
pub fn sensitive(text: &str) -> bool {
    // `tan(x)` / `Math.tan(` is the tangent, not a bank TAN.
    let t = text.to_lowercase().replace("tan(", "tangens(").replace("tan (", "tangens (");
    let w: Vec<&str> = t
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    w.iter().any(|w| {
        [
            "pin",
            "tan",
            "cvv",
            "iban",
            "bic",
            "token",
            "passwort",
            "password",
            "kennwort",
            "passwörter",
            "apikey",
            "secret",
            "login",
            "geheim",
            "zugangsdaten",
            "keychain",
            "schlüsselbund",
            "kreditkarte",
            "sozialversicherungsnummer",
        ]
        .contains(w)
            || w.starts_with("passw")
    }) || ["api key", "api-key", "credit card"]
        .iter()
        .any(|p| t.contains(p))
        || w.iter().any(|w| {
            // Long digit runs (account/card numbers) - but not a plain date
            // like 20261001 in a project name ("…-20261001-1535").
            let ziffern = w.chars().filter(|c| c.is_ascii_digit()).count();
            let datum = w.len() == 8 && ziffern == 8 && {
                let (j, m, t) = (&w[0..4], &w[4..6], &w[6..8]);
                j.starts_with("20") && ("01"..="12").contains(&m) && ("01"..="31").contains(&t)
            };
            ziffern >= 8 && !datum
        })
        // Token-shaped secrets: one contiguous ASCII run, digit-heavy or
        // mixed case with digits. Not prose ("不要用100个小部件…" is one
        // whitespace chunk) and not names like "gt3-functional-final".
        || text
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
            .any(|x| {
                let ziffern = x.chars().filter(|c| c.is_ascii_digit()).count();
                let gross = x.chars().filter(|c| c.is_ascii_uppercase()).count();
                let klein = x.chars().filter(|c| c.is_ascii_lowercase()).count();
                // A name slug of words and date/time numbers
                // ("baue-kleines-snake-funktional-20261001-1535") is no secret.
                let teile: Vec<&str> = x.split(['-', '_']).filter(|t| !t.is_empty()).collect();
                let slug = teile.len() >= 3
                    && teile.iter().all(|t| t.chars().all(|c| c.is_ascii_alphabetic()) || (t.chars().all(|c| c.is_ascii_digit()) && t.len() <= 8));
                !slug && x.len() >= 20 && (ziffern >= 6 || (ziffern >= 3 && gross >= 3 && klein >= 3))
            })
        || text.contains('@')
}
pub fn classify(text: &str) -> &'static str {
    let t = text.to_lowercase();
    if [
        "bevorzug",
        "lieber",
        "am liebsten",
        "öffne",
        "immer mit",
        "sprache",
        "kurz",
        "ausführlich",
    ]
    .iter()
    .any(|k| t.contains(k))
    {
        "user_preferences"
    } else if ["→", "->", "dann", "workflow", "ablauf", "danach"]
        .iter()
        .any(|k| t.contains(k))
    {
        "workflows"
    } else if [
        "projekt",
        "uni",
        "studium",
        "kurs",
        "vorlesung",
        "arbeite an",
    ]
    .iter()
    .any(|k| t.contains(k))
    {
        "project_context"
    } else {
        "facts"
    }
}
/// "Merk dir, dass ich PDFs mit GoodNotes öffne." → "Ich PDFs mit GoodNotes öffne"
pub fn explicit_write(q: &str) -> Option<String> {
    let l = q.to_lowercase();
    let at = [
        "merk dir",
        "merke dir",
        "merk:",
        "speichere dir",
        "speicher dir",
        "behalte im kopf",
    ]
    .iter()
    .filter_map(|p| l.find(p).map(|i| i + p.len()))
    .min()?;
    let rest = q
        .get(at..)?
        .trim_start_matches(|c: char| c == ',' || c == ':' || c.is_whitespace());
    let clause = rest
        .strip_prefix("dass ")
        .or_else(|| rest.strip_prefix("Dass "));
    let rest = clause
        .unwrap_or(rest)
        .trim()
        .trim_end_matches(|c: char| c == '.' || c == '!');
    // "dass ich PDFs mit GoodNotes öffne" → main clause "Ich öffne PDFs mit GoodNotes".
    let w: Vec<&str> = rest.split_whitespace().collect();
    let rest = if clause.is_some() && w.len() >= 3 && w[0].eq_ignore_ascii_case("ich") {
        format!(
            "{} {} {}",
            w[0],
            w[w.len() - 1],
            w[1..w.len() - 1].join(" ")
        )
    } else {
        rest.to_owned()
    };
    let mut c = rest.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .filter(|s| s.chars().count() >= 3)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tmp(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("noki-mem-{name}-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }
    #[test]
    fn explicit_write_persist_and_retrieve_across_sessions() {
        let p = tmp("persist");
        let text =
            explicit_write("Merk dir, dass ich PDFs bevorzugt mit GoodNotes öffne.").unwrap();
        assert_eq!(text, "Ich öffne PDFs bevorzugt mit GoodNotes");
        {
            let m = NokiMemory::open(&p).unwrap();
            m.add(classify(&text), &text, "explicit").unwrap();
            m.add("project_context", "Ich arbeite an Noki", "explicit")
                .unwrap();
        }
        let m = NokiMemory::open(&p).unwrap(); // new session
        assert_eq!(m.count(), 2);
        let hit = m.retrieve("Womit öffne ich PDFs am liebsten?", 3);
        assert_eq!(hit.len(), 1);
        assert!(hit[0].text.contains("GoodNotes"));
        assert_eq!(hit[0].kind, "user_preferences");
        assert!(
            m.retrieve("Was ist RAM?", 3).is_empty(),
            "irrelevant memory stays out of the prompt"
        );
        m.delete(hit[0].id).unwrap();
        assert_eq!(m.count(), 1);
        m.clear().unwrap();
        assert_eq!(m.count(), 0);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn sensitive_data_is_never_stored() {
        let m = NokiMemory::open(&tmp("sens")).unwrap();
        for s in [
            "Mein Passwort ist hunter2",
            "API key sk-abcdef1234567890abcd",
            "Meine IBAN DE89370400440532013000",
            "PIN 4711",
            "mail an a@b.de",
        ] {
            assert!(sensitive(s), "{s}");
            assert!(m.add("facts", s, "explicit").is_err());
        }
        assert!(!sensitive("Ich öffne PDFs mit GoodNotes"));
        // A coding brief with the tangent and long mixed-script sentences.
        assert!(!sensitive("yawRate = speed / wheelbase * tan(steeringAngle)"));
        assert!(!sensitive("不要用100个小部件掩盖错误的车身比例。 gt3-functional-final"));
        assert!(!sensitive(include_str!("../tests/fixtures/gt3_rs_funktional_auftrag.txt")));
        assert!(sensitive("meine TAN ist 123456"));
        assert!(!sensitive("Projekt baue-kleines-snake-funktional-20261001-1535"));
        assert!(sensitive("Konto 12345678901"));
        assert!(sensitive("ghp_a8F3kL9mQ2xZ7vB1nC4dE6"));
        assert_eq!(m.count(), 0);
    }
    #[test]
    fn learn_candidates_count() {
        let m = NokiMemory::open(&tmp("learn")).unwrap();
        assert_eq!(m.bump("tool:timer"), 1);
        assert_eq!(m.bump("tool:timer"), 2);
    }
}
