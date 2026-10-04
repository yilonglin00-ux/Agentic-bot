//! Noki Kamera - reine Logik der Galerie (ohne Tauri/macOS, testbar):
//! welche Dateien dazugehoeren, Pfadpruefung (nur IM Ordner "Noki Kamera"),
//! freie Namen fuer Kopien/Exporte, Liste (neueste zuerst), Cache-Schluessel
//! und HTTP-Range fuer die Wiedergabe.
use std::path::{Path, PathBuf};

pub const FOTO: [&str; 4] = ["png", "jpg", "jpeg", "heic"];
pub const VIDEO: [&str; 3] = ["mov", "mp4", "m4v"];

pub fn endung(name: &str) -> String {
    Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}
/// "foto" | "video" | None (gehoert nicht zur Galerie).
pub fn art(name: &str) -> Option<&'static str> {
    let e = endung(name);
    if FOTO.contains(&e.as_str()) { Some("foto") } else if VIDEO.contains(&e.as_str()) { Some("video") } else { None }
}
/// Nur ein nackter Dateiname: kein Pfad, nichts Verstecktes, nur Medien.
pub fn name_ok(name: &str) -> bool {
    !name.is_empty() && name.len() <= 255 && !name.starts_with('.')
        && !name.contains(['/', '\\', '\0', ':']) && art(name).is_some()
}
/// Der echte Pfad einer Datei, die WIRKLICH direkt im Ordner liegt
/// (keine Symlinks, kein "..", keine Unterordner) - sonst None.
pub fn pfad_im_ordner(ordner: &Path, name: &str) -> Option<PathBuf> {
    if !name_ok(name) { return None; }
    let p = ordner.join(name);
    let m = std::fs::symlink_metadata(&p).ok()?;
    if !m.file_type().is_file() { return None; }
    let echt = p.canonicalize().ok()?;
    let o = ordner.canonicalize().ok()?;
    (echt.parent()? == o.as_path()).then_some(echt)
}
/// "<stamm>-<zusatz>.<endung>", bei Bedarf "-2", "-3" … - nie ueberschreiben.
pub fn freier_name(ordner: &Path, original: &str, zusatz: &str, endung: &str) -> String {
    let stamm = Path::new(original).file_stem().and_then(|s| s.to_str()).unwrap_or("Noki");
    // Kopie einer Kopie: "-bearbeitet-bearbeitet" vermeiden.
    let basis = stamm.trim_end_matches(|c: char| c.is_ascii_digit() || c == '-');
    let stamm = if basis.ends_with(&format!("-{zusatz}")) { basis.trim_end_matches(&format!("-{zusatz}")) } else { stamm };
    let mut n = 1;
    loop {
        let kandidat = if n == 1 { format!("{stamm}-{zusatz}.{endung}") } else { format!("{stamm}-{zusatz}-{n}.{endung}") };
        if std::fs::symlink_metadata(ordner.join(&kandidat)).is_err() { return kandidat; }
        n += 1;
    }
}

#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct Medium {
    pub name: String,
    pub art: &'static str,
    pub groesse: u64,
    /// Aenderungszeit in ms (sortiert, versioniert die Vorschau-URL).
    pub zeit: u64,
    pub w: Option<u32>,
    pub h: Option<u32>,
    pub dauer: Option<f64>,
}
/// Alle Medien des Ordners, neueste zuerst (ohne Metadaten).
pub fn liste(ordner: &Path) -> Vec<Medium> {
    let mut v: Vec<Medium> = std::fs::read_dir(ordner).map(|it| it.filter_map(|e| {
        let e = e.ok()?;
        let name = e.file_name().to_str()?.to_string();
        if !name_ok(&name) { return None; }
        let m = e.metadata().ok()?;
        if !m.is_file() { return None; }
        let zeit = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
        Some(Medium { art: art(&name)?, name, groesse: m.len(), zeit, w: None, h: None, dauer: None })
    }).collect()).unwrap_or_default();
    v.sort_by(|a, b| b.zeit.cmp(&a.zeit).then_with(|| b.name.cmp(&a.name)));
    v
}
/// Cache-Schluessel: aendert sich mit Inhalt (Groesse) und Zeit.
pub fn cache_schluessel(name: &str, groesse: u64, zeit: u64) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in name.bytes().chain(groesse.to_le_bytes()).chain(zeit.to_le_bytes()) { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); }
    format!("{h:016x}")
}
/// Prozent-Dekodierung eines URL-Pfadsegments (UTF-8).
pub fn url_dekodieren(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else { out.push(b[i]); i += 1; }
    }
    String::from_utf8(out).ok()
}
/// "bytes=a-b" / "bytes=a-" / "bytes=-n" -> (start, ende inklusiv); offene
/// Bereiche hoechstens `max` Bytes (Video nie komplett in den Speicher).
pub fn range(kopf: &str, gesamt: u64, max: u64) -> Option<(u64, u64)> {
    if gesamt == 0 { return None; }
    let r = kopf.trim().strip_prefix("bytes=")?.split(',').next()?.trim();
    let (a, b) = r.split_once('-')?;
    let (start, ende) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        if n == 0 { return None; }
        (gesamt.saturating_sub(n), gesamt - 1)
    } else {
        let s: u64 = a.parse().ok()?;
        let e = if b.is_empty() { s.saturating_add(max - 1) } else { b.parse().ok()? };
        (s, e.min(gesamt - 1))
    };
    (start <= ende && start < gesamt).then_some((start, ende))
}
pub fn mime(name: &str) -> &'static str {
    match endung(name).as_str() {
        "png" => "image/png", "jpg" | "jpeg" => "image/jpeg", "heic" => "image/heic",
        "mov" => "video/quicktime", "mp4" | "m4v" => "video/mp4", _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tmp(n: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("noki-kamera-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    #[test]
    fn nur_medien_mit_nacktem_namen() {
        assert_eq!(art("Noki-Screenshot-1.png"), Some("foto"));
        assert_eq!(art("a.MOV"), Some("video"));
        assert_eq!(art("notiz.txt"), None);
        for n in ["../x.png", "a/b.png", ".versteckt.png", "", "x.png\0", "C:x.png"] { assert!(!name_ok(n), "{n:?}"); }
        assert!(name_ok("Noki-Recording-2026-10-04-101010.mov"));
    }
    #[test]
    fn pfad_nur_im_ordner() {
        let d = tmp("pfad");
        let o = d.join("Noki Kamera");
        std::fs::create_dir_all(&o).unwrap();
        std::fs::write(o.join("a.png"), b"x").unwrap();
        std::fs::write(d.join("draussen.png"), b"x").unwrap();
        assert!(pfad_im_ordner(&o, "a.png").is_some());
        assert!(pfad_im_ordner(&o, "fehlt.png").is_none());
        assert!(pfad_im_ordner(&o, "../draussen.png").is_none());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.join("draussen.png"), o.join("link.png")).unwrap();
            assert!(pfad_im_ordner(&o, "link.png").is_none(), "Symlink nach draussen");
        }
        std::fs::create_dir_all(o.join("ordner.png")).unwrap();
        assert!(pfad_im_ordner(&o, "ordner.png").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn freie_namen_ueberschreiben_nie() {
        let d = tmp("namen");
        assert_eq!(freier_name(&d, "Noki-Screenshot-1.png", "bearbeitet", "png"), "Noki-Screenshot-1-bearbeitet.png");
        std::fs::write(d.join("Noki-Screenshot-1-bearbeitet.png"), b"x").unwrap();
        assert_eq!(freier_name(&d, "Noki-Screenshot-1.png", "bearbeitet", "png"), "Noki-Screenshot-1-bearbeitet-2.png");
        // Kopie einer Kopie bleibt kurz.
        assert_eq!(freier_name(&d, "Noki-Screenshot-1-bearbeitet.png", "bearbeitet", "png"), "Noki-Screenshot-1-bearbeitet-2.png");
        assert_eq!(freier_name(&d, "Noki-Recording-9.mov", "geschnitten", "mov"), "Noki-Recording-9-geschnitten.mov");
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn liste_neueste_zuerst() {
        let d = tmp("liste");
        for (n, alt) in [("alt.png", 3), ("mitte.mov", 2), ("neu.png", 1)] {
            let p = d.join(n);
            std::fs::write(&p, b"xy").unwrap();
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs(alt * 100);
            std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
        }
        std::fs::write(d.join("notiz.txt"), b"x").unwrap();
        std::fs::write(d.join(".noki-schreibtest"), b"").unwrap();
        let l = liste(&d);
        assert_eq!(l.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["neu.png", "mitte.mov", "alt.png"]);
        assert_eq!(l[1].art, "video");
        assert_eq!(l[0].groesse, 2);
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn range_und_url() {
        assert_eq!(range("bytes=0-1", 100, 10), Some((0, 1)));
        assert_eq!(range("bytes=10-", 100, 8), Some((10, 17)));
        assert_eq!(range("bytes=90-", 100, 50), Some((90, 99)));
        assert_eq!(range("bytes=-5", 100, 50), Some((95, 99)));
        assert_eq!(range("bytes=100-", 100, 50), None);
        assert_eq!(range("items=0-1", 100, 50), None);
        assert_eq!(url_dekodieren("Noki%20Kamera%C3%A4.png").as_deref(), Some("Noki Kameraä.png"));
        assert_eq!(url_dekodieren("%zz"), None);
        assert_ne!(cache_schluessel("a.png", 1, 2), cache_schluessel("a.png", 1, 3));
    }
}
