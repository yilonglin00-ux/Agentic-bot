//! Noki Research Browser: invisible, private (incognito) WKWebView, read-only.
//! Only searches, opens pages and extracts text. Never submits forms, clicks,
//! downloads or reads cookies/logins. The local model receives plain text only.
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{webview::PageLoadEvent, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

static WINDOW_SEQ: AtomicU32 = AtomicU32::new(1);
const PAGE_TIMEOUT: Duration = Duration::from_secs(9);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(16); // hard research time budget

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Source {
    pub title: String,
    pub url: String,
    pub fetched_at: u64,
    pub excerpt: String,
    /// When the PAGE was published – never silently used as the date of the event it describes.
    #[serde(default)]
    pub publication_date: Option<String>,
    /// Dates found inside the text (candidates for the event date).
    #[serde(default)]
    pub text_dates: Vec<String>,
    /// 0 = primary/official, 1 = agency, 2 = quality media, 3 = specialist, 4 = other.
    #[serde(default)]
    pub authority: u8,
    /// Sources telling the same story (syndication) share a cluster id.
    #[serde(default)]
    pub cluster: usize,
    /// Lightweight publication identity used for evidence ranking.
    #[serde(default)]
    pub category: SourceCategory,
    /// Filled only for claims that survive synthesis/verification.
    #[serde(default)]
    pub claims_supported: Vec<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(rename_all = "snake_case")]
pub enum SourceCategory {
    PrimaryOfficial,
    CreatorCompany,
    QualityPublication,
    Specialist,
    #[default]
    Secondary,
    UserGenerated,
    SeoAggregator,
}

const RESULTS_JS: &str = r#"JSON.stringify(Array.from(document.querySelectorAll('a.result__a')).slice(0,30).map(a=>{let h=a.href,r=a.closest('.result');try{const g=new URL(h).searchParams.get('uddg');if(g)h=g}catch(e){}return {t:(a.textContent||'').trim().slice(0,160),s:((r&&r.querySelector('.result__snippet')||{}).textContent||'').replace(/\s+/g,' ').trim().slice(0,700),u:h,ad:!!a.closest('.result--ad')}}))"#;
const PAGE_JS: &str = r#"(()=>{const r=document.querySelector('main,article,[role=main]')||document.body;const o=[];if(r)r.querySelectorAll('h1,h2,h3,p,li,td,dd,pre').forEach(e=>{if(e.closest('nav,footer,aside,form,header,[aria-hidden=true]'))return;const s=(e.textContent||'').replace(/\s+/g,' ').trim();if(s.length>25)o.push(s.slice(0,400))});
const m=s=>{const e=document.querySelector(s);return e?(e.getAttribute('content')||e.getAttribute('datetime')||e.textContent||'').trim().slice(0,40):''};
const pub=m('meta[property="article:published_time"]')||m('meta[name="date"]')||m('meta[itemprop="datePublished"]')||m('time[datetime]');
const can=(document.querySelector('link[rel=canonical]')||{}).href||location.href;
return JSON.stringify({t:(document.title||'').slice(0,160),x:o.join('\n').slice(0,30000),p:pub,c:can})})()"#;

struct Browser {
    win: WebviewWindow,
    loads: mpsc::Receiver<()>,
}
impl Browser {
    fn open(app: &tauri::AppHandle, url: Url) -> Result<Self, String> {
        let (tx, loads) = mpsc::channel();
        let label = format!(
            "noki-research-{}",
            WINDOW_SEQ.fetch_add(1, Ordering::Relaxed)
        );
        let win = WebviewWindowBuilder::new(app, label, WebviewUrl::External(url))
            .visible(false)
            .focused(false)
            .skip_taskbar(true)
            .incognito(true)
            .inner_size(1100.0, 800.0)
            .on_navigation(|u| matches!(u.scheme(), "https" | "http" | "about"))
            .on_download(|_, _| false)
            .on_page_load(move |_, p| {
                if p.event() == PageLoadEvent::Finished {
                    let _ = tx.send(());
                }
            })
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { win, loads })
    }
    fn wait_load(&self) -> Result<(), String> {
        self.loads
            .recv_timeout(PAGE_TIMEOUT)
            .map_err(|_| "Seite lädt nicht.".to_owned())?;
        std::thread::sleep(Duration::from_millis(700)); // late client-side rendering
        Ok(())
    }
    fn goto(&self, url: &Url) -> Result<(), String> {
        while self.loads.try_recv().is_ok() {}
        self.win.navigate(url.clone()).map_err(|e| e.to_string())?;
        self.wait_load()
    }
    fn eval(&self, js: &str) -> Result<serde_json::Value, String> {
        eval_json(&self.win, js)
    }
}
/// Evaluates read-only JS and returns its (JSON) result; call from a worker thread only.
pub fn eval_json(win: &WebviewWindow, js: &str) -> Result<serde_json::Value, String> {
    let (tx, rx) = mpsc::channel();
    win.eval_with_callback(js, move |s| {
        let _ = tx.send(s);
    })
    .map_err(|e| e.to_string())?;
    let raw = rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "Seite antwortet nicht.".to_owned())?;
    let v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    match v {
        serde_json::Value::String(s) => {
            Ok(serde_json::from_str(&s).unwrap_or(serde_json::Value::String(s)))
        }
        v => Ok(v),
    }
}
impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.win.destroy();
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
pub fn host(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| h.trim_start_matches("www.").to_owned())
        })
        .unwrap_or_default()
}
fn site(h: &str) -> String {
    let p: Vec<_> = h.rsplit('.').take(2).collect();
    p.into_iter().rev().collect::<Vec<_>>().join(".")
}
fn words(q: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "was",
        "wer",
        "wie",
        "welche",
        "welcher",
        "welches",
        "ist",
        "sind",
        "der",
        "die",
        "das",
        "den",
        "dem",
        "ein",
        "eine",
        "und",
        "oder",
        "noki",
        "bitte",
        "aktuell",
        "aktuelle",
        "aktuellen",
        "gerade",
        "recherchiere",
        "suche",
        "internet",
        "nach",
        "für",
        "mit",
        "von",
        "im",
        "zu",
    ];
    q.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3 && !STOP.contains(w))
        .map(str::to_owned)
        .collect()
}

/// Prefer primary/official and reputable sources; never rely on known low-text or shopping/social pages.
pub fn pick(
    results: &[(String, String, bool, String)],
    limit: usize,
) -> Vec<(String, String, String)> {
    const SKIP: &[&str] = &[
        "duckduckgo.com",
        "bing.com",
        "google.com",
        "pinterest.com",
        "facebook.com",
        "instagram.com",
        "tiktok.com",
        "x.com",
        "twitter.com",
        "youtube.com",
        "amazon.de",
        "amazon.com",
        "ebay.de",
        "quora.com",
    ];
    const GOOD: &[&str] = &[
        "wikipedia.org",
        "apple.com",
        "nike.com",
        "adidas.de",
        "spotify.com",
        "microsoft.com",
        "google.com",
        "mozilla.org",
        "zalando.de",
        "otto.de",
        "snipes.com",
        "footlocker.de",
        "mediamarkt.de",
        "saturn.de",
        "europa.eu",
        "bund.de",
        "admin.ch",
        "gv.at",
        "dge.de",
        "bzfe.de",
        "gesundheitsinformation.de",
        "ods.od.nih.gov",
        "nhs.uk",
        "aok.de",
        "heise.de",
        "golem.de",
        "tagesschau.de",
        "spiegel.de",
        "zeit.de",
        "faz.net",
        "sueddeutsche.de",
        "reuters.com",
        "apnews.com",
        "bbc.com",
        "theverge.com",
        "macrumors.com",
        "arstechnica.com",
    ];
    let mut scored: Vec<(i32, String, String, String)> = results
        .iter()
        .enumerate()
        .filter_map(|(i, (t, u, ad, snippet))| {
            let h = host(u);
            if *ad
                || h.is_empty()
                || !(u.starts_with("https://") || u.starts_with("http://"))
                || SKIP
                    .iter()
                    .any(|s| h == *s || h.ends_with(&format!(".{s}")))
            {
                return None;
            }
            let official = GOOD
                .iter()
                .any(|g| h == *g || h.ends_with(&format!(".{g}")))
                || h.ends_with(".gov")
                || h.ends_with(".edu")
                || h.contains(".ac.")
                || ["support.", "developer.", "docs.", "learn."]
                    .iter()
                    .any(|p| h.starts_with(p));
            let priced = snippet.contains('€') || snippet.to_uppercase().contains("EUR");
            Some((
                30 - (i % 30) as i32 + if official { 12 } else { 0 } + if priced { 12 } else { 0 },
                t.clone(),
                u.clone(),
                snippet.clone(),
            ))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    let mut sites: Vec<(String, usize)> = Vec::new();
    let mut out = Vec::new();
    for (_, t, u, snippet) in scored {
        if out.iter().any(|(_, existing, _)| *existing == u) {
            continue;
        }
        let s = site(&host(&u));
        let max_per_site = if authority(&u) == 0 { 3 } else { 1 };
        if let Some((_, n)) = sites.iter_mut().find(|(name, _)| *name == s) {
            if *n >= max_per_site {
                continue;
            }
            *n += 1;
        } else {
            sites.push((s, 1));
        }
        out.push((t, u, snippet));
        if out.len() == limit {
            break;
        }
    }
    out
}
/// Lines with the most query words, kept in page order.
pub fn excerpt(text: &str, q: &str, max: usize) -> String {
    let ws = words(q);
    let mut lines: Vec<(usize, usize, &str)> = text
        .lines()
        .enumerate()
        .map(|(i, l)| {
            let ll = l.to_lowercase();
            (
                ws.iter().filter(|w| ll.contains(w.as_str())).count() * 4
                    + l.chars().any(|c| c.is_ascii_digit()) as usize,
                i,
                l,
            )
        })
        .filter(|l| l.0 > 0)
        .collect();
    lines.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut chosen = Vec::new();
    let mut len = 0;
    for l in lines {
        if len + l.2.len() > max {
            continue;
        }
        len += l.2.len();
        chosen.push(l);
    }
    chosen.sort_by_key(|l| l.1);
    chosen.iter().map(|l| l.2).collect::<Vec<_>>().join(" … ")
}

/// FAST DIRECT FETCH: plain HTTP GET through the system's curl – no rendering, no images,
/// no ads, no trackers, no cookies. Same read-only posture as the research browser, but
/// a fraction of the cost, so many more sources fit into the time budget.
pub fn direct_fetch(url: &str, timeout_s: u64) -> Option<String> {
    if crate::web_gateway::check_ssrf(url).is_err()
        || crate::web_gateway::check_download_safety(url).is_err()
        || crate::web_gateway::check_secret_leak(url).is_err()
    {
        return None;
    }
    let out = std::process::Command::new("/usr/bin/curl")
        .args(["-sL", "--compressed", "--max-time", &timeout_s.to_string(), "--max-filesize", "3000000",
               "--proto", "=https,=http", "--cookie", "", "--no-keepalive",
               "-w", "\n__EFFECTIVE_URL__:%{url_effective}",
               "-H", "Accept: text/html,application/xhtml+xml", "-H", "Accept-Language: de,en;q=0.8",
               "-A", "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17 Safari/605.1.15",
               url])
        .output().ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout).into_owned();
    let marker = "\n__EFFECTIVE_URL__:";
    let (body, effective_url) = if let Some(idx) = raw.rfind(marker) {
        let b = &raw[..idx];
        let eff = raw[idx + marker.len()..].trim();
        (b.to_string(), eff.to_string())
    } else {
        (raw, url.to_string())
    };
    if crate::web_gateway::check_ssrf(&effective_url).is_err()
        || crate::web_gateway::check_download_safety(&effective_url).is_err()
        || crate::web_gateway::check_secret_leak(&effective_url).is_err()
    {
        return None;
    }
    (body.len() >= 400).then_some(body)
}
/// Deterministic HTML → (title, text, publication date, canonical url). No model, no JS.
pub fn extract(html: &str) -> (String, String, Option<String>, Option<String>) {
    // Drop everything that is never content.
    let mut rein = String::with_capacity(html.len());
    let mut rest = html;
    for tag in ["script", "style", "noscript", "svg", "template"] {
        rein.clear();
        let (open, close) = (format!("<{tag}"), format!("</{tag}>"));
        let mut r = rest;
        loop {
            match r.to_lowercase().find(&open) {
                Some(i) => {
                    rein.push_str(&r[..i]);
                    match r[i..].to_lowercase().find(&close) {
                        Some(j) => r = &r[i + j + close.len()..],
                        None => {
                            r = "";
                            break;
                        }
                    }
                }
                None => {
                    rein.push_str(r);
                    break;
                }
            }
        }
        rest = Box::leak(rein.clone().into_boxed_str());
    }
    let body = rest;
    let attr = |tag_start: &str, attr_name: &str| -> Option<String> {
        let low = html.to_lowercase();
        let i = low.find(&tag_start.to_lowercase())?;
        let ende = low[i..].find('>')? + i;
        let stueck = &html[i..ende];
        let a = stueck.to_lowercase().find(&format!("{attr_name}=\""))? + attr_name.len() + 2;
        let v = &stueck[a..];
        let j = v.find('"')?;
        Some(v[..j].trim().to_owned())
    };
    let zwischen = |start: &str, ende: &str| -> Option<String> {
        let low = html.to_lowercase();
        let i = low.find(start)? + start.len();
        let j = low[i..].find(ende)? + i;
        Some(html[i..j].trim().to_owned())
    };
    let title = zwischen("<title", "</title>")
        .and_then(|t| t.split_once('>').map(|(_, r)| r.trim().to_owned()))
        .unwrap_or_default()
        .chars()
        .take(140)
        .collect::<String>();
    let datum = [
        "<meta property=\"article:published_time\"",
        "<meta name=\"date\"",
        "<meta itemprop=\"datePublished\"",
        "<time",
    ]
    .iter()
    .find_map(|t| attr(t, "content").or_else(|| attr(t, "datetime")));
    let canonical = attr("<link rel=\"canonical\"", "href").filter(|u| u.starts_with("http"));
    // Tags → plain text, block elements become line breaks so `excerpt` can score lines.
    let mut text = String::with_capacity(body.len() / 2);
    let (mut drin, mut zeile) = (false, String::new());
    for c in body.chars() {
        match c {
            '<' => {
                drin = true;
                if !zeile.trim().is_empty() {
                    text.push_str(zeile.trim());
                    text.push('\n');
                }
                zeile.clear();
            }
            '>' => drin = false,
            _ if !drin => zeile.push(if c.is_whitespace() { ' ' } else { c }),
            _ => {}
        }
    }
    if !zeile.trim().is_empty() {
        text.push_str(zeile.trim());
    }
    let text = entities(&text);
    let text: String = text
        .lines()
        .map(str::trim)
        .filter(|l| {
            l.chars().count() > 25 && !l.contains("&#") && !l.contains("{") && !l.contains("|")
        })
        .collect::<Vec<_>>()
        .join("\n");
    (
        title,
        text.chars().take(120_000).collect(),
        datum.and_then(|d| dates(&d).into_iter().next()),
        canonical,
    )
}
/// Decodes the HTML entities that actually occur in German pages. Without this, raw markup
/// like "&ouml;" would travel through the evidence into the answer.
pub fn entities(text: &str) -> String {
    const NAMED: &[(&str, &str)] = &[
        ("&nbsp;", " "),
        ("&amp;", "&"),
        ("&quot;", "\""),
        ("&apos;", "'"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&euro;", "€"),
        ("&auml;", "ä"),
        ("&ouml;", "ö"),
        ("&uuml;", "ü"),
        ("&Auml;", "Ä"),
        ("&Ouml;", "Ö"),
        ("&Uuml;", "Ü"),
        ("&szlig;", "ß"),
        ("&ndash;", "–"),
        ("&mdash;", "—"),
        ("&hellip;", "…"),
        ("&bdquo;", "„"),
        ("&ldquo;", "“"),
        ("&rdquo;", "”"),
        ("&laquo;", "«"),
        ("&raquo;", "»"),
        ("&shy;", ""),
        ("&times;", "×"),
        ("&deg;", "°"),
    ];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let stueck = &rest[i..];
        if let Some((pat, ersatz)) = NAMED.iter().find(|(p, _)| stueck.starts_with(p)) {
            out.push_str(ersatz);
            rest = &stueck[pat.len()..];
            continue;
        }
        // Numeric entities &#228; and &#x00e4;
        if let Some(ende) = stueck.get(..12).unwrap_or(stueck).find(';') {
            let inner = &stueck[1..ende];
            if let Some(zahl) = inner.strip_prefix('#') {
                let code = if let Some(hex) = zahl.strip_prefix(['x', 'X']) {
                    u32::from_str_radix(hex, 16).ok()
                } else {
                    zahl.parse::<u32>().ok()
                };
                if let Some(c) = code.and_then(char::from_u32) {
                    out.push(c);
                    rest = &stueck[ende + 1..];
                    continue;
                }
            }
        }
        out.push('&');
        rest = &stueck[1..];
    }
    out.push_str(rest);
    out
}
/// A structured data download offered by an official page.
#[derive(Debug, Clone, PartialEq)]
pub struct Download {
    pub url: String,
    pub format: &'static str,
    pub label: String,
}
fn absolut(basis: &str, href: &str) -> Option<String> {
    if href.starts_with("http") {
        return Some(href.to_owned());
    }
    Url::parse(basis)
        .ok()?
        .join(href)
        .ok()
        .map(|u| u.to_string())
}
/// Detects the format from extension, filename or the link text – never from Content-Type alone,
/// because official servers often ship a CSV as application/octet-stream.
pub fn daten_format(url: &str, label: &str, disposition: &str) -> Option<&'static str> {
    let hay = format!(
        "{} {} {}",
        url.to_lowercase(),
        label.to_lowercase(),
        disposition.to_lowercase()
    );
    let pfad = url.split(['?', '#']).next().unwrap_or(url).to_lowercase();
    for (ext, fmt) in [
        (".csv", "csv"),
        (".tsv", "tsv"),
        (".json", "json"),
        (".xlsx", "xlsx"),
    ] {
        if pfad.ends_with(ext)
            || hay.contains(&format!("{ext}\""))
            || hay.contains(&format!("filename={}", ext))
        {
            return Some(fmt);
        }
    }
    // "Vorläufige Ergebnisse … csv, utf-8" – the label alone is enough evidence.
    if hay.contains("csv") {
        return Some("csv");
    }
    if hay.contains("xlsx") || hay.contains("excel") {
        return Some("xlsx");
    }
    None
}
/// Scans an official page for structured downloads. Links whose text mentions a dataset win.
pub fn downloads_finden(html: &str, basis: &str) -> Vec<Download> {
    let low = html.to_lowercase();
    let mut out: Vec<Download> = Vec::new();
    let mut i = 0usize;
    while let Some(a) = low[i..].find("<a ") {
        let start = i + a;
        let Some(ende) = low[start..].find("</a>").map(|e| start + e) else {
            break;
        };
        let tag = &html[start..ende.min(html.len())];
        i = ende + 4;
        let Some(hpos) = tag.to_lowercase().find("href=\"") else {
            continue;
        };
        let rest = &tag[hpos + 6..];
        let Some(q) = rest.find('"') else { continue };
        let href = &rest[..q];
        let label: String = tag
            .split('>')
            .skip(1)
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .filter(|c| *c != '<')
            .collect::<String>()
            .trim()
            .chars()
            .take(120)
            .collect();
        let Some(fmt) = daten_format(href, &label, "") else {
            continue;
        };
        let Some(url) = absolut(basis, href) else {
            continue;
        };
        if out.iter().any(|d| d.url == url) {
            continue;
        }
        out.push(Download {
            url,
            format: fmt,
            label,
        });
        if out.len() >= 8 {
            break;
        }
    }
    // Datasets that call themselves a result set come first.
    out.sort_by_key(|d| {
        let l = format!("{} {}", d.label.to_lowercase(), d.url.to_lowercase());
        let gut = [
            "ergebnis",
            "result",
            "stimmen",
            "landes",
            "open data",
            "datensatz",
        ]
        .iter()
        .any(|k| l.contains(k));
        (!gut, d.format != "csv")
    });
    out
}
/// Why a discovered link was not followed – so a zero result is explainable.
#[derive(Default, Debug, Clone, Serialize)]
pub struct GraphSpur {
    pub pages_scanned: usize,
    pub links_seen: usize,
    pub official_child_domains: usize,
    pub result_pages: usize,
    pub download_pages: usize,
    pub structured_links: usize,
    pub fetch_attempts: usize,
    pub redirects: usize,
    pub rejected_offsite: usize,
    pub rejected_irrelevant: usize,
    /// URLs amtlicher Ergebnisseiten - der HTML-Fallback, wenn kein Datensatz passt.
    pub result_urls: Vec<String>,
}
/// Role of a page in the authority graph. A download index has almost no prose but is the
/// most valuable page there is – it must never be filtered out for "too little content".
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum Rolle {
    Dataset,
    DownloadIndex,
    Results,
    Navigation,
    Article,
}
pub fn rolle(url: &str, titel: &str, html: &str) -> Rolle {
    let l = format!("{} {}", url.to_lowercase(), titel.to_lowercase());
    if daten_format(url, titel, "").is_some() {
        return Rolle::Dataset;
    }
    let links = html.matches("<a ").count();
    if ["download", "daten", "opendata", "open-data", "datensatz"]
        .iter()
        .any(|k| l.contains(k))
    {
        return Rolle::DownloadIndex;
    }
    if ["ergebnis", "result", "stimmen", "wahlergebnis"]
        .iter()
        .any(|k| l.contains(k))
    {
        return Rolle::Results;
    }
    if links >= 20 && html.len() < 60_000 {
        return Rolle::Navigation;
    }
    Rolle::Article
}
/// Same-site (or officially linked child domain) links that lead towards a result dataset.
fn navigations_links(html: &str, basis: &str, ziel: &str) -> Vec<(String, String, i32)> {
    let gut = [
        "ergebnis",
        "wahlergebnis",
        "landesergebnis",
        "vorläufig",
        "endgültig",
        "download",
        "csv",
        "daten",
        "datensatz",
        "open data",
        "opendata",
        "tabelle",
        "stimmen",
        "wahl",
    ];
    let low = html.to_lowercase();
    let mut out: Vec<(String, String, i32)> = Vec::new();
    let mut i = 0usize;
    while let Some(a) = low[i..].find("<a ") {
        let start = i + a;
        let Some(ende) = low[start..].find("</a>").map(|e| start + e) else {
            break;
        };
        let tag = &html[start..ende.min(html.len())];
        i = ende + 4;
        let tl = tag.to_lowercase();
        let Some(hpos) = tl.find("href=\"") else {
            continue;
        };
        let rest = &tag[hpos + 6..];
        let Some(q) = rest.find('"') else { continue };
        let href = &rest[..q];
        if href.starts_with('#') || href.starts_with("javascript:") || href.starts_with("mailto:") {
            continue;
        }
        // Label, aria-label und title zaehlen gleichermassen.
        let text: String = tag
            .split('>')
            .skip(1)
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .filter(|c| *c != '<')
            .collect();
        let label = format!(
            "{} {} {}",
            text.trim(),
            tl.split("aria-label=\"")
                .nth(1)
                .and_then(|r| r.split('"').next())
                .unwrap_or(""),
            tl.split("title=\"")
                .nth(1)
                .and_then(|r| r.split('"').next())
                .unwrap_or("")
        );
        let hay = format!("{} {}", href.to_lowercase(), label.to_lowercase());
        let mut punkte: i32 = gut.iter().filter(|k| hay.contains(**k)).count() as i32
            + if daten_format(href, &label, "").is_some() {
                10
            } else {
                0
            }
            + if hay.contains("ergebnis") { 3 } else { 0 };
        let zl = ziel.to_lowercase();
        let jahr = zl
            .split(|c: char| !c.is_ascii_digit())
            .find(|t| t.len() == 4 && t.starts_with("20"));
        let code = jahr.map(|j| {
            format!(
                "{}{}",
                if zl.contains("landtagswahl") {
                    "lt"
                } else if zl.contains("bundestagswahl") {
                    "bt"
                } else {
                    ""
                },
                &j[2..]
            )
        });
        if ["landtagswahl", "sachsen-anhalt"]
            .iter()
            .any(|k| zl.contains(k) && hay.contains(k))
            || jahr.is_some_and(|j| hay.contains(j))
            || code
                .as_ref()
                .is_some_and(|c| c.len() > 2 && hay.contains(c))
        {
            punkte += 100;
        }
        if punkte == 0 {
            continue;
        }
        let Some(url) = absolut(basis, href) else {
            continue;
        };
        if out.iter().any(|(u, _, _)| *u == url) {
            continue;
        }
        out.push((url, label.trim().chars().take(100).collect(), punkte));
    }
    out.sort_by_key(|(_, _, p)| std::cmp::Reverse(*p));
    out.truncate(12);
    out
}
/// Reject a structurally plausible export when its filename identifies a different election.
/// This must happen before downloading: Bundestag and Landtag exports can share the same schema.
fn dataset_context_reject(url: &str, filename: &str, ziel: &str) -> Option<&'static str> {
    let hay = format!("{} {}", url, filename).to_lowercase();
    let ziel = ziel.to_lowercase();
    if ziel.contains("landtagswahl") && (hay.contains("bundestagswahl") || hay.contains("btw")) {
        return Some("election_type_mismatch");
    }
    if ziel.contains("bundestagswahl") && (hay.contains("landtagswahl") || hay.contains("ltw")) {
        return Some("election_type_mismatch");
    }
    let ziel_jahr = ziel
        .split(|c: char| !c.is_ascii_digit())
        .find(|t| t.len() == 4 && t.starts_with("20"));
    if let Some(jahr) = ziel_jahr {
        let fremdes_jahr = hay
            .split(|c: char| !c.is_ascii_digit())
            .any(|t| t.len() == 4 && t.starts_with("20") && t != jahr);
        if fremdes_jahr
            && ["wahl", "ergebnis", "btw", "ltw"]
                .iter()
                .any(|k| hay.contains(k))
        {
            return Some("election_year_mismatch");
        }
    }
    None
}
/// Follows an official authority from its result page down to a structured dataset.
/// Bounded: max `budget` pages, depth 3, same site or a child domain it links to itself.
/// Jahr aus einem Text: volle Jahreszahl oder ein Kuerzel wie "LTW21"/"LW21".
fn jahr_tokens(t: &str) -> Vec<u32> {
    let b: Vec<char> = t.chars().collect();
    let mut out = Vec::new();
    for i in 0..b.len() {
        if !b[i].is_ascii_digit() {
            continue;
        }
        if b[i..].iter().take_while(|c| c.is_ascii_digit()).count() == 4
            && (i == 0 || !b[i - 1].is_ascii_digit())
        {
            let j: String = b[i..i + 4].iter().collect();
            if let Ok(n) = j.parse::<u32>() {
                if (1900..2100).contains(&n) {
                    out.push(n);
                }
            }
        }
    }
    // "ltw21" / "lw21" / "btw21" -> 2021
    for kuerzel in ["ltw", "lw", "btw", "kw"] {
        let mut von = 0;
        while let Some(i) = t[von..].find(kuerzel) {
            let a = von + i + kuerzel.len();
            let zwei: String = t[a..].chars().take(2).collect();
            if zwei.len() == 2 && zwei.chars().all(|c| c.is_ascii_digit()) {
                if let Ok(n) = zwei.parse::<u32>() {
                    out.push(if n < 70 { 2000 + n } else { 1900 + n });
                }
            }
            von = a.max(von + 1);
            if von >= t.len() {
                break;
            }
        }
    }
    out
}
/// §Dataset-Scope: ein Datensatz eines ANDEREN Wahljahres kann die Frage nie beantworten.
/// Gemessen: fuer "Mecklenburg-Vorpommern 2026" wurden LTW21-Dateien aus Baden-Wuerttemberg
/// geladen und geparst, nur um am Schema zu scheitern.
fn jahr_konflikt(ziel: &str, hay: &str) -> Option<(u32, u32)> {
    let gefragt = jahr_tokens(&ziel.to_lowercase());
    let gefragt = gefragt.first().copied()?;
    let kandidat = jahr_tokens(hay);
    if kandidat.is_empty() || kandidat.contains(&gefragt) {
        return None;
    }
    Some((gefragt, kandidat[0]))
}
#[cfg(test)]
mod scope_tests {
    use super::*;
    #[test]
    fn jahr_konflikt_weist_fremdes_wahljahr_ab() {
        let ziel = "Landtagswahl Mecklenburg-Vorpommern 2026";
        // genau die drei Kandidaten aus dem echten Lauf
        assert_eq!(
            jahr_konflikt(ziel, "statistik-bw.de/.../ltw21_kreise.csv ltw21"),
            Some((2026, 2021))
        );
        assert_eq!(
            jahr_konflikt(ziel, "ltw21_gemeinden.csv"),
            Some((2026, 2021))
        );
        assert_eq!(jahr_konflikt(ziel, "ergebnis_2026_land.csv"), None);
        assert_eq!(jahr_konflikt(ziel, "ergebnisse_land.csv"), None); // kein Jahr -> nicht ablehnen
        assert_eq!(jahr_tokens("ltw21"), vec![2021]);
    }
}
pub fn authority_graph(
    start: &str,
    ziel: &str,
    budget: usize,
    cancel: &dyn Fn() -> bool,
    pruefen: &mut dyn FnMut(&str, &str, &'static str, i32, &str),
) -> GraphSpur {
    let mut spur = GraphSpur::default();
    let wurzel = site(&host(start));
    let mut offen: Vec<(String, usize, String, i32)> =
        vec![(start.to_owned(), 0, String::new(), 0)];
    let mut gesehen: Vec<String> = Vec::new();
    let mut probiert: Vec<String> = Vec::new();
    while let Some((url, tiefe, link_label, link_score)) = offen.first().cloned() {
        offen.remove(0);
        if cancel() || spur.pages_scanned >= budget || tiefe > 3 {
            break;
        }
        if gesehen.contains(&url) {
            continue;
        }
        gesehen.push(url.clone());
        // A structured navigation target is a candidate too. Probe it, then continue with
        // every other candidate instead of mistaking "downloaded" for "schema matched".
        if let Some(fmt) = daten_format(&url, "", "") {
            if matches!(fmt, "csv" | "tsv") {
                if !probiert.contains(&url) {
                    probiert.push(url.clone());
                    spur.structured_links += 1;
                    spur.fetch_attempts += 1;
                    let filename = url
                        .rsplit('/')
                        .next()
                        .filter(|s| !s.is_empty())
                        .unwrap_or(&link_label);
                    log::info!("noki-dataset-candidate url={:?} filename={:?} score={} format={} selected=true",
                        url, filename, link_score, fmt);
                    if let Some(reason) = dataset_context_reject(&url, filename, ziel) {
                        log::info!("noki-dataset-probe url={:?} filename={:?} score={} probe_result=rejected schema=not_checked reject_reason={}",
                            url, filename, link_score, reason);
                        continue;
                    }
                    match datei_laden(&url, 8_000_000) {
                        Some(text) if text.lines().count() >= 2 => {
                            log::info!("noki-dataset-probe url={:?} filename={:?} score={} probe_result=downloaded schema=pending reject_reason=none lines={}",
                                url, filename, link_score, text.lines().count());
                            pruefen(&url, filename, fmt, link_score, &text);
                        }
                        Some(_) => log::info!("noki-dataset-probe url={:?} filename={:?} score={} probe_result=rejected schema=unknown reject_reason=too_few_lines",
                            url, filename, link_score),
                        None => log::info!("noki-dataset-probe url={:?} filename={:?} score={} probe_result=rejected schema=unknown reject_reason=fetch_failed",
                            url, filename, link_score),
                    }
                }
                continue;
            }
        }
        let Some(html) = direct_fetch(&url, 6) else {
            continue;
        };
        spur.pages_scanned += 1;
        let titel = html
            .to_lowercase()
            .split("<title")
            .nth(1)
            .and_then(|r| r.split_once('>'))
            .map(|(_, t)| t.split('<').next().unwrap_or("").to_owned())
            .unwrap_or_default();
        match rolle(&url, &titel, &html) {
            Rolle::DownloadIndex => spur.download_pages += 1,
            Rolle::Results => {
                spur.result_pages += 1;
                if !spur.result_urls.contains(&url) {
                    spur.result_urls.push(url.clone());
                }
            }
            _ => {}
        }
        // Direct dataset links on this page first.
        let dls = downloads_finden(&html, &url);
        spur.structured_links += dls.len();
        for (rank, dl) in dls.iter().enumerate() {
            let hay = format!("{} {}", dl.url.to_lowercase(), dl.label.to_lowercase());
            let score = 10
                + ["ergebnis", "result", "stimmen", "landes", "datensatz"]
                    .iter()
                    .filter(|k| hay.contains(**k))
                    .count() as i32
                    * 10
                - rank as i32;
            let scope = jahr_konflikt(ziel, &hay);
            log::info!("noki-dataset-candidate rank={} url={:?} filename={:?} score={} format={} selected={} scope={}",
                rank + 1, dl.url, dl.url.rsplit('/').next().unwrap_or(""), score, dl.format,
                matches!(dl.format, "csv" | "tsv") && scope.is_none(),
                scope.map(|(g, k)| format!("year_mismatch_asked{g}_got{k}")).unwrap_or_else(|| "ok".into()));
            if let Some((g, k)) = scope {
                log::info!("noki-dataset-probe url={:?} filename={:?} score={} probe_result=rejected schema=not_checked reject_reason=scope_year_mismatch asked={} got={}",
                    dl.url, dl.url.rsplit('/').next().unwrap_or(""), score, g, k);
                continue;
            }
            if !matches!(dl.format, "csv" | "tsv") {
                log::info!("noki-dataset-probe url={:?} filename={:?} score={} probe_result=rejected schema=not_checked reject_reason=unsupported_format",
                    dl.url, dl.url.rsplit('/').next().unwrap_or(""), score);
            }
        }
        for (rank, dl) in dls
            .iter()
            .enumerate()
            .filter(|(_, d)| matches!(d.format, "csv" | "tsv"))
        {
            if probiert.contains(&dl.url) {
                continue;
            }
            probiert.push(dl.url.clone());
            let hay = format!("{} {}", dl.url.to_lowercase(), dl.label.to_lowercase());
            let score = 10
                + ["ergebnis", "result", "stimmen", "landes", "datensatz"]
                    .iter()
                    .filter(|k| hay.contains(**k))
                    .count() as i32
                    * 10
                - rank as i32;
            let filename = dl
                .url
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or(&dl.label);
            if let Some(reason) = dataset_context_reject(&dl.url, filename, ziel) {
                log::info!("noki-dataset-probe url={:?} filename={:?} score={} probe_result=rejected schema=not_checked reject_reason={}",
                    dl.url, filename, score, reason);
                continue;
            }
            spur.fetch_attempts += 1;
            match datei_laden(&dl.url, 8_000_000) {
                Some(text) if text.lines().count() >= 2 => {
                    log::info!("noki-dataset-probe url={:?} filename={:?} score={} probe_result=downloaded schema=pending reject_reason=none lines={}",
                        dl.url, filename, score, text.lines().count());
                    pruefen(&dl.url, filename, dl.format, score, &text);
                }
                Some(_) => log::info!("noki-dataset-probe url={:?} filename={:?} score={} probe_result=rejected schema=unknown reject_reason=too_few_lines",
                    dl.url, filename, score),
                None => log::info!("noki-dataset-probe url={:?} filename={:?} score={} probe_result=rejected schema=unknown reject_reason=fetch_failed",
                    dl.url, filename, score),
            }
        }
        // Otherwise one level deeper along the most promising navigation links.
        let links = navigations_links(&html, &url, ziel);
        spur.links_seen += links.len();
        for (u, label, score) in links {
            let h = host(&u);
            if site(&h) != wurzel {
                // §5: a child domain the confirmed authority links to itself stays trusted.
                if authority(&u) == 0 {
                    spur.official_child_domains += 1;
                } else {
                    spur.rejected_offsite += 1;
                    continue;
                }
            }
            if !gesehen.contains(&u) {
                offen.push((u, tiefe + 1, label, score));
            }
        }
        offen.sort_by_key(|(u, t, _, score)| {
            (
                std::cmp::Reverse(*score),
                *t,
                daten_format(u, "", "").is_none(),
            )
        });
    }
    spur
}
/// Downloads a structured file. Only known, non-executable data formats, size and time capped.
pub fn datei_laden(url: &str, max_bytes: usize) -> Option<String> {
    let out = std::process::Command::new("/usr/bin/curl")
        .args([
            "-sL",
            "--compressed",
            "--max-time",
            "8",
            "--max-filesize",
            &max_bytes.to_string(),
            "--proto",
            "=https",
            "--cookie",
            "",
            "-A",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) Safari/605.1.15",
            url,
        ])
        .output()
        .ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    // CSV exports are frequently Windows-1252; decode instead of mangling umlauts.
    let text = match String::from_utf8(out.stdout.clone()) {
        Ok(t) => t,
        Err(_) => encoding_rs::WINDOWS_1252.decode(&out.stdout).0.into_owned(),
    };
    Some(text.trim_start_matches('\u{feff}').to_owned())
}
/// Robust CSV/TSV parsing: BOM, ';' or ',' or tab, quoted fields, empty cells.
pub fn csv_parsen(text: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let text = text.trim_start_matches('\u{feff}');
    // The delimiter is the candidate that occurs most consistently in the first lines.
    let probe: Vec<&str> = text
        .lines()
        .take(12)
        .filter(|l| !l.trim().is_empty())
        .collect();
    let trenner = [';', '\t', ',']
        .into_iter()
        .max_by_key(|d| probe.iter().map(|l| l.matches(*d).count()).sum::<usize>())
        .unwrap_or(';');
    let mut zeilen: Vec<Vec<String>> = Vec::new();
    let (mut feld, mut zeile, mut quoted) = (String::new(), Vec::new(), false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                feld.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == trenner && !quoted => {
                zeile.push(feld.trim().to_owned());
                feld.clear();
            }
            '\n' if !quoted => {
                zeile.push(feld.trim().to_owned());
                feld.clear();
                zeilen.push(std::mem::take(&mut zeile));
            }
            '\r' => {}
            c => feld.push(c),
        }
    }
    if !feld.is_empty() || !zeile.is_empty() {
        zeile.push(feld.trim().to_owned());
        zeilen.push(zeile);
    }
    zeilen.retain(|z| z.iter().any(|f| !f.is_empty()));
    if zeilen.is_empty() {
        return (Vec::new(), Vec::new());
    }
    // The header is the first row that is mostly non-numeric.
    let kopf_idx = zeilen
        .iter()
        .position(|z| {
            z.iter().filter(|f| !f.is_empty()).count() >= 2
                && z.iter()
                    .filter(|f| f.chars().any(|c| c.is_alphabetic()))
                    .count()
                    * 2
                    >= z.len()
        })
        .unwrap_or(0);
    let kopf: Vec<String> = zeilen[kopf_idx]
        .iter()
        .map(|h| h.trim().to_lowercase())
        .collect();
    (kopf, zeilen.split_off(kopf_idx + 1))
}
/// German decimal ("43,8") to f64.
pub fn dezimal(s: &str) -> Option<f64> {
    let t: String = s
        .trim()
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == ',' || *c == '.' || *c == '-')
        .collect();
    if t.is_empty() {
        return None;
    }
    // "1.234,5" -> thousands dot, decimal comma.
    let t = if t.contains(',') {
        t.replace('.', "").replace(',', ".")
    } else {
        t
    };
    t.parse().ok()
}
/// Authority rank of a host: 0 primary/official, 1 agency, 2 quality media, 3 specialist, 4 other.
pub fn authority(url: &str) -> u8 {
    let h = host(url).to_lowercase();
    let amtlich = [
        ".gov",
        ".bund.de",
        "bundeswahlleiter",
        "landeswahlleiter",
        "wahlen",
        "landtag",
        "bundestag",
        "bundesregierung",
        "destatis",
        "statistik",
        "europa.eu",
        "ministerium",
        ".admin.ch",
        ".gv.at",
        "who.int",
        "nhs.uk",
        "gesundheitsinformation.de",
        "dge.de",
        "bzfe.de",
        "ecb.europa",
        "bundesbank",
    ];
    let agentur = [
        "dpa",
        "reuters",
        "afp",
        "apnews",
        "tagesschau",
        "zdf",
        "ard.de",
        "deutschlandfunk",
        "bbc.",
    ];
    let qualitaet = [
        "zeit.de",
        "sueddeutsche",
        "faz.net",
        "spiegel",
        "handelsblatt",
        "nzz.ch",
        "guardian",
        "ft.com",
        "welt.de",
        "taz.de",
    ];
    let fach = [
        "wikipedia.org",
        "heise.de",
        "golem.de",
        "arstechnica",
        "macrumors",
        "theverge",
        "cochranelibrary.com",
        "pubmed.ncbi.nlm.nih.gov",
        "mayoclinic.org",
        "msdmanuals.com",
        "kidney.org",
        "aok.de",
        "apotheken-umschau.de",
        ".edu",
        ".ac.",
    ];
    if amtlich.iter().any(|k| h.contains(k)) || h.ends_with(".gov") {
        0
    } else if agentur.iter().any(|k| h.contains(k)) {
        1
    } else if qualitaet.iter().any(|k| h.contains(k)) {
        2
    } else if fach.iter().any(|k| h.contains(k)) {
        3
    } else {
        4
    }
}

pub fn source_category(source: &Source) -> SourceCategory {
    let host = host(&source.url).to_lowercase();
    let identity = format!("{} {}", source.title, source.excerpt).to_lowercase();
    if ["parked domain", "domain for sale", "originalstore", "original-kaufen", "best-", "top10"]
        .iter()
        .any(|signal| host.contains(signal) || identity.contains(signal))
    {
        return SourceCategory::SeoAggregator;
    }
    if ["reddit.com", "facebook.com", "instagram.com", "tiktok.com", "x.com", "quora.com"]
        .iter()
        .any(|site| host == *site || host.ends_with(&format!(".{site}")))
    {
        return SourceCategory::UserGenerated;
    }
    match authority(&source.url) {
        0 => SourceCategory::PrimaryOfficial,
        1 | 2 => SourceCategory::QualityPublication,
        3 => SourceCategory::Specialist,
        _ if ["official website", "official site", "unternehmen", "about us", "impressum"]
            .iter()
            .any(|signal| identity.contains(signal)) => SourceCategory::CreatorCompany,
        _ => SourceCategory::Secondary,
    }
}
/// Deterministic date extraction – ISO and common German forms. No model call.
pub fn dates(text: &str) -> Vec<String> {
    let b = text.as_bytes();
    let monate = [
        "januar",
        "februar",
        "märz",
        "april",
        "mai",
        "juni",
        "juli",
        "august",
        "september",
        "oktober",
        "november",
        "dezember",
    ];
    let mut out: Vec<String> = Vec::new();
    // ISO yyyy-mm-dd
    for i in 0..b.len().saturating_sub(9) {
        if b[i..i + 4].iter().all(|c| c.is_ascii_digit())
            && b[i + 4] == b'-'
            && b[i + 7] == b'-'
            && b[i + 5..i + 7].iter().all(|c| c.is_ascii_digit())
            && b[i + 8..i + 10].iter().all(|c| c.is_ascii_digit())
        {
            if let Ok(d) = std::str::from_utf8(&b[i..i + 10]) {
                if d.starts_with("19") || d.starts_with("20") {
                    out.push(d.to_owned());
                }
            }
        }
    }
    // dd.mm.yyyy and "12. Mai 2026"
    let low = text.to_lowercase();
    let toks: Vec<&str> = low.split(|c: char| c.is_whitespace()).collect();
    for (i, t) in toks.iter().enumerate() {
        let t = t.trim_matches(|c: char| !c.is_alphanumeric() && c != '.');
        let punkte: Vec<&str> = t.split('.').filter(|x| !x.is_empty()).collect();
        if punkte.len() == 3
            && punkte.iter().all(|x| x.chars().all(|c| c.is_ascii_digit()))
            && punkte[2].len() == 4
        {
            out.push(format!("{}-{:0>2}-{:0>2}", punkte[2], punkte[1], punkte[0]));
        }
        if i + 2 < toks.len() {
            if let Some(m) = monate.iter().position(|m| toks[i + 1].starts_with(m)) {
                let tag = t.trim_end_matches('.');
                let jahr = toks[i + 2].trim_matches(|c: char| !c.is_ascii_digit());
                if tag.chars().all(|c| c.is_ascii_digit()) && !tag.is_empty() && jahr.len() == 4 {
                    out.push(format!("{}-{:02}-{:0>2}", jahr, m + 1, tag));
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out.truncate(8);
    out
}
/// Syndication clustering: same canonical URL, same site, or a near-identical title.
fn titel_aehnlich(a: &str, b: &str) -> bool {
    let wa: Vec<String> = words(a);
    let wb: Vec<String> = words(b);
    if wa.len() < 3 || wb.len() < 3 {
        return false;
    }
    let gleich = wa.iter().filter(|w| wb.contains(w)).count();
    gleich * 10 >= wa.len().min(wb.len()) * 8
}
pub fn cluster(sources: &mut [Source]) -> usize {
    let mut naechste = 0usize;
    for i in 0..sources.len() {
        if sources[i].cluster != 0 {
            continue;
        }
        naechste += 1;
        sources[i].cluster = naechste;
        let s_title = sources[i].title.clone();
        for j in i + 1..sources.len() {
            if sources[j].cluster != 0 {
                continue;
            }
            let same_canonical_url = sources[j].url.trim_end_matches('/')
                == sources[i].url.trim_end_matches('/');
            // A publisher can carry several independent articles. Treating an
            // entire host as one syndicated item discarded useful evidence.
            if same_canonical_url || titel_aehnlich(&s_title, &sources[j].title) {
                sources[j].cluster = naechste;
            }
        }
    }
    naechste
}
/// Broad research: many candidates, deduplicated and read in PARALLEL browser tabs, then
/// compressed locally. Only the compact evidence ever reaches the 1.5B model.
/// `enough` is asked after every batch so slow stragglers are never waited for.
/// Why a candidate did not become evidence – so a low yield is explainable, not a mystery.
#[derive(Default, Debug, Clone, Serialize)]
pub struct Funnel {
    pub accepted: usize,
    pub fetch_failed: usize,
    pub empty: usize,
    pub duplicate: usize,
    pub waves: usize,
    pub direct_fetched: usize,
    pub webkit_fallback: usize,
    pub search_ms: u64,
    pub direct_ms: u64,
    pub webkit_ms: u64,
}
pub struct Broad {
    pub sources: Vec<Source>,
    pub candidates: usize,
    pub independent: usize,
    pub clusters: usize,
    pub primary: usize,
    pub funnel: Funnel,
    pub pool: Vec<(String, String, String)>,
}
const PARALLEL: usize = 4; // WebKit fallback tabs
const DIRECT_PARALLEL: usize = 12; // plain HTTP readers
/// Nur die Suchstufe: Trefferliste zu EINER Query holen, ohne die Seiten zu laden.
/// Fuer die Official-Lane, die Download-/Datensatz-Kandidaten sucht — dort zaehlt
/// Titel/URL/Snippet, der Volltext wird erst spaeter beim Proben gebraucht.
pub fn search_candidates(
    app: &tauri::AppHandle,
    query: &str,
    limit: usize,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<(String, String, String)>, String> {
    if cancel() {
        return Err("Abgebrochen.".into());
    }
    let mut u = Url::parse("https://html.duckduckgo.com/html/").unwrap();
    u.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("kl", "de-de");
    let sucher = Browser::open(app, u)?;
    sucher.wait_load()?;
    let roh: Vec<(String, String, bool, String)> = sucher
        .eval(RESULTS_JS)?
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some((
                        x["t"].as_str()?.to_owned(),
                        x["u"].as_str()?.to_owned(),
                        x["ad"].as_bool().unwrap_or(false),
                        x["s"].as_str().unwrap_or_default().to_owned(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(pick(&roh, limit))
}

pub fn research_broad(
    app: &tauri::AppHandle,
    queries: &[String],
    ziel: usize,
    enough: &(dyn Fn(&[Source]) -> bool + Sync),
    cancel: &(dyn Fn() -> bool + Sync),
    progress: &dyn Fn(&str, usize),
) -> Result<Broad, String> {
    let start = Instant::now();
    progress("search", 0);
    let search_url = |query: &str| {
        let mut u = Url::parse("https://html.duckduckgo.com/html/").unwrap();
        u.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("kl", "de-de");
        u
    };
    let first = queries.first().ok_or("Leerer Suchplan.")?;
    let sucher = Browser::open(app, search_url(first))?;
    sucher.wait_load()?;
    let mut roh = Vec::new();
    for (i, query) in queries.iter().take(8).enumerate() {
        if cancel() {
            return Err("Abgebrochen.".into());
        }
        if start.elapsed() > TOTAL_TIMEOUT {
            break;
        }
        if i > 0 && sucher.goto(&search_url(query)).is_err() {
            continue;
        }
        if let Ok(r) = sucher.eval(RESULTS_JS) {
            roh.extend(
                r.as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| {
                                Some((
                                    x["t"].as_str()?.to_owned(),
                                    x["u"].as_str()?.to_owned(),
                                    x["ad"].as_bool().unwrap_or(false),
                                    x["s"].as_str().unwrap_or_default().to_owned(),
                                ))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            );
        }
        // A rich first result page must not suppress independent claim/authority queries.
        if i >= queries.len().min(3).saturating_sub(1) && pick(&roh, ziel * 2).len() >= ziel * 2 {
            break;
        }
    }
    drop(sucher);
    let candidates = roh.len();
    let pool = pick(&roh, ziel * 3);
    if pool.is_empty() {
        return Err("Keine Suchergebnisse gefunden.".into());
    }
    let focus = queries.join(" ");
    let mut funnel = Funnel::default();
    funnel.search_ms = start.elapsed().as_millis() as u64;
    let mut sources: Vec<Source> = Vec::new();
    let baue = |title: &str,
                u: &str,
                snippet: &str,
                seite: Option<(String, String, Option<String>, Option<String>)>,
                focus: &str|
     -> Option<Source> {
        let mut q = Source {
            title: title.to_owned(),
            url: u.to_owned(),
            fetched_at: now(),
            excerpt: excerpt(snippet, focus, 900),
            authority: authority(u),
            ..Default::default()
        };
        if let Some((t, text, datum, canon)) = seite {
            let ex = excerpt(&text, focus, 900);
            if ex.len() >= 80 {
                q.excerpt = ex;
            }
            if !t.trim().is_empty() {
                q.title = t.trim().chars().take(140).collect();
            }
            if let Some(c) = canon {
                q.authority = authority(&c);
                q.url = c;
            }
            q.publication_date = datum;
            q.text_dates = dates(&q.excerpt);
        }
        if q.excerpt.len() < 60 && snippet.chars().count() >= 60 {
            q.excerpt = snippet.chars().take(500).collect();
        }
        q.category = source_category(&q);
        (q.excerpt.len() >= 60).then_some(q)
    };
    // STAGE 1 – fast direct fetch, highly parallel. Primary/official sources come first.
    let mut offen = pool.clone();
    offen.sort_by_key(|(_, u, _)| authority(u));
    let direkt_start = Instant::now();
    let (tx, rx) = mpsc::channel::<(Option<Source>, (String, String, String))>();
    let mut batches: Vec<Vec<(String, String, String)>> =
        (0..DIRECT_PARALLEL).map(|_| Vec::new()).collect();
    for (i, c) in offen.iter().cloned().enumerate() {
        batches[i % DIRECT_PARALLEL].push(c);
    }
    let mut nachladen: Vec<(String, String, String)> = Vec::new();
    std::thread::scope(|scope| {
        for batch in batches {
            if batch.is_empty() {
                continue;
            }
            let tx = tx.clone();
            let focus = focus.clone();
            scope.spawn(move || {
                for (title, u, snippet) in batch {
                    if cancel() || start.elapsed() > TOTAL_TIMEOUT {
                        break;
                    }
                    let seite = direct_fetch(&u, 6).map(|h| extract(&h));
                    let brauchbar = seite
                        .as_ref()
                        .map(|(_, t, _, _)| t.len() >= 400)
                        .unwrap_or(false);
                    let q = if brauchbar {
                        baue(&title, &u, &snippet, seite, &focus)
                    } else {
                        None
                    };
                    let _ = tx.send((q, (title, u, snippet)));
                }
            });
        }
        drop(tx);
        while let Ok((q, kandidat)) = rx.recv() {
            funnel.waves = 1;
            match q {
                Some(s) => {
                    if sources.iter().any(|x| x.url == s.url) {
                        funnel.duplicate += 1;
                    } else {
                        funnel.accepted += 1;
                        funnel.direct_fetched += 1;
                        sources.push(s);
                    }
                }
                None => nachladen.push(kandidat),
            }
            if enough(&sources) || cancel() || start.elapsed() > TOTAL_TIMEOUT {
                break;
            }
        }
    });
    funnel.direct_ms = direkt_start.elapsed().as_millis() as u64;
    // STAGE 2 – WebKit only for pages the plain reader could not handle (JS-only, blocked).
    if !enough(&sources) && !nachladen.is_empty() && !cancel() && start.elapsed() < TOTAL_TIMEOUT {
        let webkit_start = Instant::now();
        funnel.waves += 1;
        nachladen.sort_by_key(|(_, u, _)| authority(u));
        nachladen.truncate(ziel);
        let (tx, rx) = mpsc::channel::<Option<Source>>();
        let mut batches: Vec<Vec<(String, String, String)>> =
            (0..PARALLEL).map(|_| Vec::new()).collect();
        for (i, c) in nachladen.into_iter().enumerate() {
            batches[i % PARALLEL].push(c);
        }
        std::thread::scope(|scope| {
            for batch in batches {
                if batch.is_empty() {
                    continue;
                }
                let tx = tx.clone();
                let focus = focus.clone();
                scope.spawn(move || {
                    let Ok(browser) = Browser::open(app, Url::parse("about:blank").unwrap()) else {
                        return;
                    };
                    for (title, u, snippet) in batch {
                        if cancel() || start.elapsed() > TOTAL_TIMEOUT {
                            break;
                        }
                        let Ok(parsed) = Url::parse(&u) else {
                            let _ = tx.send(None);
                            continue;
                        };
                        let mut seite = None;
                        if browser.goto(&parsed).is_ok() {
                            if let Ok(page) = browser.eval(PAGE_JS) {
                                seite = Some((
                                    page["t"].as_str().unwrap_or_default().to_owned(),
                                    page["x"].as_str().unwrap_or_default().to_owned(),
                                    dates(page["p"].as_str().unwrap_or_default())
                                        .into_iter()
                                        .next(),
                                    page["c"]
                                        .as_str()
                                        .filter(|c| c.starts_with("http"))
                                        .map(str::to_owned),
                                ));
                            }
                        }
                        let _ = tx.send(baue(&title, &u, &snippet, seite, &focus));
                    }
                });
            }
            drop(tx);
            while let Ok(q) = rx.recv() {
                match q {
                    Some(s) => {
                        if sources.iter().any(|x| x.url == s.url) {
                            funnel.duplicate += 1;
                        } else {
                            funnel.accepted += 1;
                            funnel.webkit_fallback += 1;
                            sources.push(s);
                        }
                    }
                    None => funnel.fetch_failed += 1,
                }
                if enough(&sources) || cancel() || start.elapsed() > TOTAL_TIMEOUT {
                    break;
                }
            }
        });
        funnel.webkit_ms = webkit_start.elapsed().as_millis() as u64;
    }
    sources.sort_by_key(|s| (s.authority, -(s.excerpt.len() as i64)));
    let clusters = cluster(&mut sources);
    let primary = sources.iter().filter(|s| s.authority == 0).count();
    log::info!("noki-funnel kandidaten={} akzeptiert={} direct={} webkit={} fetch_failed={} duplikate={} cluster={} search_ms={} direct_ms={} webkit_ms={}",
        candidates, funnel.accepted, funnel.direct_fetched, funnel.webkit_fallback, funnel.fetch_failed,
        funnel.duplicate, clusters, funnel.search_ms, funnel.direct_ms, funnel.webkit_ms);
    progress("summarize", sources.len());
    Ok(Broad {
        sources,
        candidates,
        independent: clusters,
        clusters,
        primary,
        funnel,
        pool,
    })
}
/// Search + read up to `limit` independent sources. `progress(phase, n)` reports UI states.
pub fn research(
    app: &tauri::AppHandle,
    query: &str,
    limit: usize,
    cancel: &dyn Fn() -> bool,
    progress: &dyn Fn(&str, usize),
) -> Result<Vec<Source>, String> {
    research_queries(app, &[query.to_owned()], limit, cancel, progress)
}

/// Execute a small targeted query plan, merge independent candidates, then
/// stop reading as soon as the requested source budget is satisfied.
pub fn research_queries(
    app: &tauri::AppHandle,
    queries: &[String],
    limit: usize,
    cancel: &dyn Fn() -> bool,
    progress: &dyn Fn(&str, usize),
) -> Result<Vec<Source>, String> {
    research_until(app, queries, limit, &|_| false, cancel, progress)
}
/// `enough` is asked after every fetched source: as soon as the primary answer target is
/// covered, research stops. No extra model round between sources.
pub fn research_until(
    app: &tauri::AppHandle,
    queries: &[String],
    limit: usize,
    enough: &dyn Fn(&[Source]) -> bool,
    cancel: &dyn Fn() -> bool,
    progress: &dyn Fn(&str, usize),
) -> Result<Vec<Source>, String> {
    let start = Instant::now();
    progress("search", 0);
    let search_url = |query: &str| {
        let mut u = Url::parse("https://html.duckduckgo.com/html/").unwrap();
        u.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("kl", "de-de");
        u
    };
    let first = queries.first().ok_or("Leerer Suchplan.")?;
    let url = search_url(first);
    let browser = Browser::open(app, url)?;
    browser.wait_load()?;
    let mut results = Vec::new();
    for (i, query) in queries.iter().take(4).enumerate() {
        if cancel() {
            return Err("Abgebrochen.".into());
        }
        // Enough distinct candidates for the source budget → no further search round.
        if i > 0 && pick(&results, limit).len() >= limit {
            break;
        }
        if i > 0 {
            browser.goto(&search_url(query))?;
        }
        let raw = browser.eval(RESULTS_JS)?;
        results.extend(
            raw.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|r| {
                            Some((
                                r["t"].as_str()?.to_owned(),
                                r["u"].as_str()?.to_owned(),
                                r["ad"].as_bool().unwrap_or(false),
                                r["s"].as_str().unwrap_or_default().to_owned(),
                            ))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        );
    }
    let focus = queries.join(" ");
    let price_search = queries.iter().any(|q| {
        let l = q.to_lowercase();
        l.contains("preis") || l.contains(" eur") || l.contains("kost") || l.contains("teuer")
    });
    let mut chosen = pick(&results, limit);
    if price_search {
        let lower = focus.to_lowercase();
        let brand = if lower.contains("macbook") {
            "apple"
        } else if lower.contains("air max") {
            "nike"
        } else if lower.contains("spotify") {
            "spotify"
        } else {
            lower.split_whitespace().next().unwrap_or("")
        };
        if !brand.is_empty() && !chosen.iter().any(|(_, u, _)| host(u).contains(brand)) {
            if let Some((t, u, _, s)) = results
                .iter()
                .find(|(_, u, ad, _)| !*ad && host(u).contains(brand))
            {
                if chosen.len() == limit {
                    chosen.pop();
                }
                chosen.push((t.clone(), u.clone(), s.clone()));
            }
        }
    }
    if chosen.is_empty() {
        return Err("Keine Suchergebnisse gefunden.".into());
    }
    progress("read", chosen.len());
    let mut sources = Vec::new();
    for (title, u, snippet) in chosen {
        if cancel() {
            return Err("Abgebrochen.".into());
        }
        if start.elapsed() > TOTAL_TIMEOUT || enough(&sources) {
            break;
        }
        let snippet_ex = excerpt(&snippet, &focus, 900);
        if price_search
            && snippet_ex.len() >= 70
            && (snippet_ex.contains('€') || snippet_ex.to_uppercase().contains("EUR"))
        {
            sources.push(Source {
                title,
                url: u,
                fetched_at: now(),
                excerpt: snippet_ex,
                ..Default::default()
            });
            continue;
        }
        let Ok(parsed) = Url::parse(&u) else { continue };
        if browser.goto(&parsed).is_err() {
            if snippet_ex.len() >= 80 {
                sources.push(Source {
                    title,
                    url: u,
                    fetched_at: now(),
                    excerpt: snippet_ex,
                    ..Default::default()
                });
            }
            continue;
        }
        let Ok(page) = browser.eval(PAGE_JS) else {
            continue;
        };
        let text = page["x"].as_str().unwrap_or_default();
        let ex = excerpt(text, &focus, 900);
        if ex.len() < 80 {
            if snippet_ex.len() >= 80 {
                sources.push(Source {
                    title,
                    url: u,
                    fetched_at: now(),
                    excerpt: snippet_ex,
                    ..Default::default()
                });
            }
            continue;
        }
        let t = page["t"]
            .as_str()
            .filter(|t| !t.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or(title);
        let final_url = browser.win.url().map(|u| u.to_string()).unwrap_or(u);
        sources.push(Source {
            title: t.trim().chars().take(140).collect(),
            url: final_url,
            fetched_at: now(),
            excerpt: ex,
            ..Default::default()
        });
    }
    progress("summarize", sources.len());
    Ok(sources)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn picks_official_distinct_sources() {
        let r = vec![
            (
                "Spam".into(),
                "https://best-mac-tipps.example/macos".into(),
                false,
                "".into(),
            ),
            (
                "Ad".into(),
                "https://shop.example/x".into(),
                true,
                "".into(),
            ),
            (
                "Apple".into(),
                "https://support.apple.com/de-de/109033".into(),
                false,
                "".into(),
            ),
            (
                "Apple 2".into(),
                "https://www.apple.com/de/macos/".into(),
                false,
                "".into(),
            ),
            (
                "Wiki".into(),
                "https://de.wikipedia.org/wiki/MacOS".into(),
                false,
                "".into(),
            ),
            (
                "Video".into(),
                "https://www.youtube.com/watch?v=1".into(),
                false,
                "".into(),
            ),
        ];
        let p = pick(&r, 3);
        assert_eq!(p.len(), 3);
        assert!(
            p[0].1.contains("apple.com")
                && p.iter().filter(|x| x.1.contains("apple.com")).count() == 1
        );
        assert!(
            p.iter().any(|x| x.1.contains("wikipedia"))
                && !p
                    .iter()
                    .any(|x| x.1.contains("youtube") || x.1.contains("shop."))
        );
    }

    #[test]
    fn clustering_keeps_distinct_articles_from_one_publisher() {
        let mut sources = vec![
            Source {
                title: "Unternehmen stellt Designerin vor".into(),
                url: "https://example.com/interview".into(),
                ..Default::default()
            },
            Source {
                title: "Fabrik erläutert Herstellung".into(),
                url: "https://example.com/manufacturing".into(),
                ..Default::default()
            },
        ];
        assert_eq!(cluster(&mut sources), 2);
        assert_ne!(sources[0].cluster, sources[1].cluster);
    }
    #[test]
    fn excerpt_keeps_relevant_lines() {
        let t = "Cookies akzeptieren und weiter\nmacOS Tahoe 26 ist die aktuelle Version von macOS.\nImpressum";
        assert_eq!(
            excerpt(t, "Welche macOS-Version ist aktuell?", 700),
            "macOS Tahoe 26 ist die aktuelle Version von macOS."
        );
    }
    #[test]
    fn dataset_candidate_must_match_election_type_and_year() {
        let ziel = "Landtagswahl Sachsen-Anhalt 2026";
        assert_eq!(
            dataset_context_reject(
                "https://amt.example/btw25_kerg2.csv",
                "btw25_kerg2.csv",
                ziel
            ),
            Some("election_type_mismatch")
        );
        assert_eq!(
            dataset_context_reject(
                "https://amt.example/landtagswahl-2021/ergebnis.csv",
                "ergebnis.csv",
                ziel
            ),
            Some("election_year_mismatch")
        );
        assert_eq!(
            dataset_context_reject(
                "https://wahlergebnisse.sachsen-anhalt.de/ltw2026/ergebnis.csv",
                "ergebnis.csv",
                ziel
            ),
            None
        );
    }
}
