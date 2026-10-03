//! Least-privilege core for Work.
//!
//! Four things live here, because they are one decision made in one place:
//!
//!  1. `Mode`          - a step is READ or ACT, never both.
//!  2. `CapabilityLease` - one capability, one scope, one task, one deadline.
//!  3. `AppAdapter`    - what Noki may do WITH a given app, declared per app.
//!  4. `ActionAudit`   - what was actually attempted, and how it ended.
//!
//! The rule the rest of the code relies on: a lease is issued from USER INTENT
//! only. Nothing that Noki reads - a document, a web page, a mail, an MCP
//! answer - can create, widen or renew one. Read content is data, never policy.

// Re-exported: `capability` and `mcp_policy` both carry `RiskLevel` in their
// public API, so it has to be reachable from outside the crate to be usable.
pub use crate::permissions::RiskLevel;
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};

/// A phase reads OR acts. Mixing the two is what turns a summariser into a
/// shell: the same step that reads an instruction would be allowed to follow it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Mode {
    Read,
    Act,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Read => "READ",
            Mode::Act => "ACT",
        }
    }
}

/// Which phase a capability belongs to. Unknown capabilities are ACT on
/// purpose: an unclassified tool must not slip through as a harmless read.
pub fn mode_for(capability: &str) -> Mode {
    match capability {
        "fs.read" | "fs.search" | "shell.readonly" | "web.reference" | "document.extract"
        | "file.read" | "app.read" | "mail.search" | "mail.read" | "visual.inspect"
        | "browser.read" | "clipboard.read"
        // MCP reads are classified here, in the same table as everything else,
        // so a server integration cannot define its own idea of what a read is.
        | "mcp.read" | "mcp.list" | "mcp.search" | "data.analyze" => Mode::Read,
        _ => Mode::Act,
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// One capability, for one scope, for one task, for a short time.
///
/// `app.open` on `Spotify` is exactly that - it is not "Noki may drive apps".
/// A second target, a second capability or a later moment each need their own
/// lease, which is why `scope` and `expires_at` are not optional.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapabilityLease {
    pub id: u64,
    pub capability: String,
    pub scope: String,
    pub mode: Mode,
    pub task_id: u64,
    pub issued_at: u64,
    pub expires_at: u64,
    pub risk: RiskLevel,
    /// ACT leases are single use. A spent lease can never be replayed.
    pub consumed: bool,
}

impl CapabilityLease {
    pub fn expired(&self, at: u64) -> bool {
        at > self.expires_at
    }
}

/// Default lifetimes. Short enough that a lease cannot outlive the request that
/// justified it; long enough that a user confirmation dialog still fits inside.
pub const ACT_TTL_MS: u64 = 120_000;
pub const READ_TTL_MS: u64 = 300_000;

#[derive(Default)]
pub struct LeaseStore {
    leases: Mutex<Vec<CapabilityLease>>,
    seq: AtomicU64,
}

impl LeaseStore {
    pub fn issue(
        &self,
        capability: &str,
        scope: &str,
        task_id: u64,
        risk: RiskLevel,
    ) -> CapabilityLease {
        let mode = mode_for(capability);
        let at = now_ms();
        let lease = CapabilityLease {
            id: self.seq.fetch_add(1, Ordering::Relaxed) + 1,
            capability: capability.to_owned(),
            scope: scope.to_owned(),
            mode,
            task_id,
            issued_at: at,
            expires_at: at
                + match mode {
                    Mode::Act => ACT_TTL_MS,
                    Mode::Read => READ_TTL_MS,
                },
            risk,
            consumed: false,
        };
        if let Ok(mut g) = self.leases.lock() {
            g.retain(|l| !l.expired(at) && !l.consumed);
            if g.len() > 64 {
                g.remove(0);
            }
            g.push(lease.clone());
        }
        lease
    }

    /// Validates that THIS capability, for THIS scope, in THIS phase is still
    /// leased - and spends an ACT lease so it cannot be used twice.
    pub fn redeem(
        &self,
        id: u64,
        capability: &str,
        scope: &str,
    ) -> Result<CapabilityLease, String> {
        let at = now_ms();
        let mut g = self.leases.lock().map_err(|e| e.to_string())?;
        let Some(pos) = g.iter().position(|l| l.id == id) else {
            return Err("Keine gültige Berechtigung für diese Aktion.".into());
        };
        let lease = g[pos].clone();
        if lease.consumed {
            return Err("Diese Berechtigung wurde bereits verbraucht.".into());
        }
        if lease.expired(at) {
            g.remove(pos);
            return Err("Die Berechtigung ist abgelaufen. Bitte neu fragen.".into());
        }
        if lease.capability != capability {
            return Err("Die Berechtigung gilt für eine andere Fähigkeit.".into());
        }
        if lease.scope != scope {
            return Err("Die Berechtigung gilt für ein anderes Ziel.".into());
        }
        if lease.mode == Mode::Act {
            g[pos].consumed = true;
        }
        Ok(lease)
    }

    pub fn active(&self) -> Vec<CapabilityLease> {
        let at = now_ms();
        self.leases
            .lock()
            .map(|g| {
                g.iter()
                    .filter(|l| !l.expired(at) && !l.consumed)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Ends a phase: everything still open for this task is dropped. This is
    /// what makes "READ finished -> now ACT" a real boundary and not a wish.
    pub fn end_phase(&self, task_id: u64, mode: Mode) {
        if let Ok(mut g) = self.leases.lock() {
            g.retain(|l| !(l.task_id == task_id && l.mode == mode));
        }
    }
}

// ---------------------------------------------------------------------------
//  App adapter registry - desktop first
// ---------------------------------------------------------------------------

/// How a target can be reached, best first. The order is the product decision:
/// a real app beats a web page, but only when the app can actually take the
/// request - otherwise the user gets a window that ignored what they asked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum OpenPlan {
    /// The installed app takes the whole request via its own URL scheme.
    NativeDeepLink {
        app_path: String,
        uri: String,
        display: String,
    },
    /// The installed app opens; the request carries no further payload.
    NativeOpen { app_path: String, display: String },
    /// The app is installed and opens, but it provably cannot take the search.
    /// Kept apart from `NativeOpen` so Noki can say so instead of opening a
    /// window that ignored the request and then reporting success.
    NativeOpenNoSearch { app_path: String, display: String },
    /// No app can take it: a browser does, with a normal https address.
    /// `app_path` names the browser the user asked for, when they named one.
    Browser {
        url: String,
        display: String,
        app_path: Option<String>,
    },
}

impl OpenPlan {
    pub fn capability(&self) -> &'static str {
        match self {
            OpenPlan::NativeDeepLink { .. } => "app.open_url",
            OpenPlan::NativeOpen { .. } | OpenPlan::NativeOpenNoSearch { .. } => "app.open",
            OpenPlan::Browser { .. } => "browser.open",
        }
    }
    pub fn scope(&self) -> String {
        match self {
            OpenPlan::NativeDeepLink { display, .. }
            | OpenPlan::NativeOpen { display, .. }
            | OpenPlan::NativeOpenNoSearch { display, .. } => display.clone(),
            OpenPlan::Browser { url, .. } => url.clone(),
        }
    }
    pub fn target(&self) -> String {
        match self {
            OpenPlan::NativeDeepLink { uri, .. } => uri.clone(),
            OpenPlan::NativeOpen { app_path, .. }
            | OpenPlan::NativeOpenNoSearch { app_path, .. } => app_path.clone(),
            OpenPlan::Browser { url, .. } => url.clone(),
        }
    }
}

/// What Noki may do with one app - declared, not inferred.
///
/// `search_uri` is the honest part of this table: it is filled in only where a
/// real URL scheme was verified to carry the search. A Chrome/Safari web app
/// (YouTube.app here) launches but drops the address, so it stays `None` and
/// the resolver falls back to the browser instead of opening a window that
/// silently ignores the request.
pub struct AppAdapter {
    pub key: &'static str,
    pub display: &'static str,
    /// Extra names the user might say for the same app.
    pub aliases: &'static [&'static str],
    /// Capabilities this app is allowed to expose. Nothing outside this list.
    pub caps: &'static [&'static str],
    /// Native deep link that carries a search term, `{q}` = percent-encoded.
    pub search_uri: Option<&'static str>,
    /// Browser fallback for a search, `{q}` = percent-encoded.
    pub web_search: Option<&'static str>,
    /// Host of `web_search`, for the opener's allowlist.
    pub web_host: Option<&'static str>,
}

pub const APP_ADAPTERS: &[AppAdapter] = &[
    AppAdapter {
        key: "spotify",
        display: "Spotify",
        aliases: &["spotify"],
        caps: &["app.open", "app.open_url", "app.search", "browser.open"],
        // Verified on macOS: opens the native app straight on the results.
        search_uri: Some("spotify:search:{q}"),
        web_search: Some("https://open.spotify.com/search/{q}"),
        web_host: Some("open.spotify.com"),
    },
    AppAdapter {
        key: "youtube",
        display: "YouTube",
        aliases: &["youtube", "yt"],
        caps: &["app.open", "browser.open", "browser.search"],
        // The installed YouTube.app is a Chrome web app: `open -a` launches it
        // but discards the URL, so a "native" search would silently show Home.
        search_uri: None,
        web_search: Some("https://www.youtube.com/results?search_query={q}"),
        web_host: Some("www.youtube.com"),
    },
    AppAdapter {
        key: "music",
        display: "Musik",
        aliases: &["apple music", "musik", "music"],
        caps: &["app.open", "app.open_url", "app.search"],
        search_uri: Some("music://search?term={q}"),
        web_search: None,
        web_host: None,
    },
    AppAdapter {
        key: "maps",
        display: "Karten",
        aliases: &["karten", "maps", "apple maps"],
        caps: &["app.open", "app.open_url", "app.search", "browser.open"],
        search_uri: Some("maps://?q={q}"),
        web_search: Some("https://www.google.com/maps/search/{q}"),
        web_host: Some("www.google.com"),
    },
    AppAdapter {
        key: "appstore",
        display: "App Store",
        aliases: &["app store", "appstore"],
        caps: &["app.open", "app.open_url", "app.search"],
        search_uri: Some("macappstore://search?term={q}"),
        web_search: None,
        web_host: None,
    },
    AppAdapter {
        key: "dhl",
        display: "DHL",
        aliases: &["dhl", "sendungsverfolgung"],
        caps: &["browser.open"],
        search_uri: None,
        web_search: Some(
            "https://www.dhl.de/de/privatkunden/dhl-sendungsverfolgung.html?piececode={q}",
        ),
        web_host: Some("www.dhl.de"),
    },
    AppAdapter {
        key: "google",
        display: "Google",
        aliases: &["google"],
        caps: &["browser.open", "browser.search"],
        search_uri: None,
        web_search: Some("https://www.google.com/search?q={q}"),
        web_host: Some("www.google.com"),
    },
    AppAdapter {
        key: "duckduckgo",
        display: "DuckDuckGo",
        aliases: &["duckduckgo", "ddg"],
        caps: &["browser.open", "browser.search"],
        search_uri: None,
        web_search: Some("https://duckduckgo.com/?q={q}"),
        web_host: Some("duckduckgo.com"),
    },
    AppAdapter {
        key: "wikipedia",
        display: "Wikipedia",
        aliases: &["wikipedia", "wiki"],
        caps: &["browser.open", "browser.search"],
        search_uri: None,
        web_search: Some("https://de.wikipedia.org/w/index.php?search={q}"),
        web_host: Some("de.wikipedia.org"),
    },
    AppAdapter {
        key: "github",
        display: "GitHub",
        aliases: &["github"],
        caps: &["app.open", "browser.open", "browser.search"],
        search_uri: None,
        web_search: Some("https://github.com/search?q={q}"),
        web_host: Some("github.com"),
    },
    // --- Browsers ------------------------------------------------------
    // A named browser is a search *route*, not a search *target*: the query
    // rides in the URL, which is faster and far more robust than typing into
    // the window. `app_path` on the resulting plan aims it at this browser.
    AppAdapter {
        key: "chrome",
        display: "Google Chrome",
        aliases: &["chrome", "google chrome", "googlechrome"],
        caps: &["app.open", "browser.open", "browser.search"],
        search_uri: None,
        web_search: Some("https://www.google.com/search?q={q}"),
        web_host: Some("www.google.com"),
    },
    AppAdapter {
        key: "safari",
        display: "Safari",
        aliases: &["safari"],
        caps: &["app.open", "browser.open", "browser.search"],
        search_uri: None,
        web_search: Some("https://duckduckgo.com/?q={q}"),
        web_host: Some("duckduckgo.com"),
    },
    // --- Native apps: open only ----------------------------------------
    // No verified URL scheme carries a search into these, so `search_uri`
    // stays None and a search request is answered honestly instead of being
    // downgraded to a bare window. Deeper control belongs to a real adapter,
    // not to a guessed deep link.
    AppAdapter {
        key: "claude",
        display: "Claude",
        aliases: &["claude"],
        caps: &["app.open"],
        search_uri: None,
        web_search: None,
        web_host: None,
    },
    AppAdapter {
        key: "chatgpt",
        display: "ChatGPT",
        aliases: &["chatgpt", "chat gpt", "gpt"],
        caps: &["app.open"],
        search_uri: None,
        web_search: None,
        web_host: None,
    },
    AppAdapter {
        key: "gemini",
        display: "Gemini",
        aliases: &["gemini"],
        caps: &["app.open"],
        search_uri: None,
        web_search: None,
        web_host: None,
    },
    AppAdapter {
        key: "mail",
        display: "Mail",
        aliases: &["mail", "apple mail", "mail app"],
        caps: &["app.open"],
        search_uri: None,
        web_search: None,
        web_host: None,
    },
    AppAdapter {
        key: "photos",
        display: "Fotos",
        aliases: &["fotos", "photos", "bilder"],
        caps: &["app.open"],
        search_uri: None,
        web_search: None,
        web_host: None,
    },
    AppAdapter {
        key: "whatsapp",
        display: "WhatsApp",
        aliases: &["whatsapp", "whats app"],
        caps: &["app.open"],
        search_uri: None,
        web_search: None,
        web_host: None,
    },
    AppAdapter {
        key: "figma",
        display: "Figma",
        aliases: &["figma"],
        caps: &["app.open"],
        search_uri: None,
        web_search: None,
        web_host: None,
    },
    AppAdapter {
        key: "goodnotes",
        display: "Goodnotes",
        aliases: &["goodnotes", "good notes"],
        caps: &["app.open"],
        search_uri: None,
        web_search: None,
        web_host: None,
    },
    AppAdapter {
        key: "antigravity",
        display: "Antigravity",
        aliases: &["antigravity", "anti gravity"],
        caps: &["app.open"],
        search_uri: None,
        web_search: None,
        web_host: None,
    },
    AppAdapter {
        key: "atoll",
        display: "Atoll",
        aliases: &["atoll"],
        caps: &["app.open"],
        search_uri: None,
        web_search: None,
        web_host: None,
    },
    AppAdapter {
        key: "amongus",
        display: "Among Us",
        aliases: &["among us", "amongus"],
        caps: &["app.open"],
        search_uri: None,
        web_search: None,
        web_host: None,
    },
    AppAdapter {
        key: "motion",
        display: "Motion",
        aliases: &["motion", "motion by mosaic"],
        caps: &["app.open"],
        search_uri: None,
        web_search: None,
        web_host: None,
    },
    // --- Web wrappers ---------------------------------------------------
    // Installed as Chrome/Safari web apps: `open -a` launches them but drops
    // the address, exactly like YouTube.app above. They stay search-less so
    // the resolver reaches for the browser instead of a window that ignored
    // the request. Posting, messaging and uploads are deliberately absent.
    AppAdapter {
        key: "teams",
        display: "Microsoft Teams",
        aliases: &["teams", "microsoft teams", "ms teams"],
        caps: &["app.open"],
        search_uri: None,
        web_search: None,
        web_host: None,
    },
    AppAdapter {
        key: "word",
        display: "Microsoft Word",
        aliases: &["word", "microsoft word", "ms word"],
        caps: &["app.open"],
        search_uri: None,
        web_search: None,
        web_host: None,
    },
    AppAdapter {
        key: "netflix",
        display: "Netflix",
        aliases: &["netflix"],
        caps: &["app.open", "browser.open", "browser.search"],
        search_uri: None,
        web_search: Some("https://www.netflix.com/search?q={q}"),
        web_host: Some("www.netflix.com"),
    },
    AppAdapter {
        key: "instagram",
        display: "Instagram",
        aliases: &["instagram", "insta", "ig"],
        caps: &["app.open"],
        search_uri: None,
        web_search: None,
        web_host: None,
    },
    AppAdapter {
        key: "tiktok",
        display: "TikTok",
        aliases: &["tiktok", "tik tok"],
        caps: &["app.open"],
        search_uri: None,
        web_search: None,
        web_host: None,
    },
    AppAdapter {
        key: "amazon",
        display: "Amazon",
        aliases: &["amazon"],
        caps: &["browser.open", "browser.search"],
        search_uri: None,
        web_search: Some("https://www.amazon.de/s?k={q}"),
        web_host: Some("www.amazon.de"),
    },
];

pub fn adapter_for(target: &str) -> Option<&'static AppAdapter> {
    let t = target.to_lowercase().replace([' ', '-', '_'], "");
    APP_ADAPTERS.iter().find(|a| {
        let key = a.key.replace([' ', '-', '_'], "");
        t == key
            || a.aliases
                .iter()
                .any(|al| t == al.replace([' ', '-', '_'], ""))
    })
}

/// Every host the URL opener may be handed. Derived from the registry, so a
/// new service is one table row and never a second allowlist to keep in sync.
pub fn is_known_web_host(host: &str) -> bool {
    APP_ADAPTERS
        .iter()
        .filter_map(|a| a.web_host)
        .any(|h| h == host)
}

/// Only schemes the registry itself declares may be handed to `open`.
/// The URI is built by Noki from a template plus the user's term - this second
/// check makes sure nothing else can ever reach the system opener.
pub fn is_known_app_uri(uri: &str) -> bool {
    let Some((scheme, _)) = uri.split_once(':') else {
        return false;
    };
    if scheme.is_empty() || uri.contains(char::is_control) {
        return false;
    }
    APP_ADAPTERS.iter().any(|a| {
        a.search_uri
            .and_then(|t| t.split_once(':'))
            .map(|(s, _)| s.eq_ignore_ascii_case(scheme))
            .unwrap_or(false)
    })
}

pub fn percent_encode(q: &str) -> String {
    q.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "%20".into(),
            x => format!("%{x:02X}"),
        })
        .collect()
}

/// Desktop first: native deep link > native open > browser.
///
/// `installed` is the live inventory (name, path); nothing is assumed to exist.
/// True for adapters that are a *route* to the web rather than a destination:
/// naming one aims the search URL at that browser.
pub fn is_browser_key(key: &str) -> bool {
    matches!(key, "chrome" | "safari")
}

/// Looks the adapter's app up in the live inventory. Nothing is assumed to
/// exist - an app missing here is simply not installed.
fn installed_path(adapter: &AppAdapter, installed: &[(String, String)]) -> Option<String> {
    installed
        .iter()
        .find(|(name, _)| {
            let n = name.to_lowercase().replace([' ', '-', '_'], "");
            n == adapter.key
                || adapter
                    .aliases
                    .iter()
                    .any(|al| n == al.replace([' ', '-', '_'], ""))
                || n == adapter.display.to_lowercase().replace([' ', '-', '_'], "")
        })
        .map(|(_, path)| path.clone())
}

pub fn plan_open_search(
    target: &str,
    query: &str,
    installed: &[(String, String)],
) -> Option<OpenPlan> {
    let adapter = adapter_for(target)?;
    let app = installed_path(adapter, installed);
    // An empty term is a plain "open it", not a search.
    if query.trim().is_empty() {
        return plan_open(target, installed);
    }
    let q = percent_encode(query);
    // 1. installed app that can really take the search
    if let (Some(path), Some(tpl)) = (app.as_ref(), adapter.search_uri) {
        if adapter.caps.contains(&"app.open_url") {
            return Some(OpenPlan::NativeDeepLink {
                app_path: path.clone(),
                uri: tpl.replace("{q}", &q),
                display: adapter.display.to_owned(),
            });
        }
    }
    // 2. browser, when the app cannot carry the request. A named browser gets
    //    the URL pointed at itself; anything else uses the default browser.
    if let Some(tpl) = adapter.web_search {
        if adapter.caps.contains(&"browser.open") || adapter.caps.contains(&"browser.search") {
            return Some(OpenPlan::Browser {
                url: tpl.replace("{q}", &q),
                display: adapter.display.to_owned(),
                app_path: is_browser_key(adapter.key).then(|| app.clone()).flatten(),
            });
        }
    }
    // 3. The app exists but provably cannot take the search. It is opened and
    //    that limit is stated - the old code reported this as a done search.
    if let Some(path) = app {
        if adapter.caps.contains(&"app.open") {
            return Some(OpenPlan::NativeOpenNoSearch {
                app_path: path,
                display: adapter.display.to_owned(),
            });
        }
    }
    None
}

/// "Open <app>" with no further payload.
pub fn plan_open(target: &str, installed: &[(String, String)]) -> Option<OpenPlan> {
    let adapter = adapter_for(target)?;
    if !adapter.caps.contains(&"app.open") {
        return None;
    }
    installed_path(adapter, installed).map(|app_path| OpenPlan::NativeOpen {
        app_path,
        display: adapter.display.to_owned(),
    })
}

// ---------------------------------------------------------------------------
//  Desktop action audit
// ---------------------------------------------------------------------------

/// What was attempted and how it ended. Deliberately no payload: the intent is
/// truncated, and document, mail or page CONTENT never enters this record.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionAudit {
    pub timestamp: u64,
    pub task_id: u64,
    pub intent: String,
    pub phase: &'static str,
    pub capability: String,
    pub scope: String,
    pub lease_id: u64,
    pub risk: RiskLevel,
    pub confirmed: bool,
    pub tool: String,
    pub result: &'static str,
    pub duration_ms: u64,
    pub error: Option<String>,
}

static ACTION_LOG: Mutex<Vec<ActionAudit>> = Mutex::new(Vec::new());

pub fn log_action(entry: ActionAudit) {
    log::info!(
        "noki-action task={} phase={} cap={} scope={:?} lease={} risk={:?} confirm={} tool={} result={} ms={} err={}",
        entry.task_id,
        entry.phase,
        entry.capability,
        entry.scope,
        entry.lease_id,
        entry.risk,
        entry.confirmed,
        entry.tool,
        entry.result,
        entry.duration_ms,
        entry.error.as_deref().unwrap_or("none")
    );
    if let Ok(mut g) = ACTION_LOG.lock() {
        if g.len() > 300 {
            g.remove(0);
        }
        g.push(entry);
    }
}

pub fn action_log() -> Vec<ActionAudit> {
    ACTION_LOG.lock().map(|g| g.clone()).unwrap_or_default()
}

/// Intent text is a log field, not a transcript: one short line, no newlines.
pub fn compact_intent(q: &str) -> String {
    let one: String = q.split_whitespace().collect::<Vec<_>>().join(" ");
    one.chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inventory() -> Vec<(String, String)> {
        vec![
            ("Spotify".into(), "/Applications/Spotify.app".into()),
            ("YouTube".into(), "/Applications/YouTube.app".into()),
        ]
    }

    #[test]
    fn installed_app_with_real_scheme_beats_the_browser() {
        let plan = plan_open_search("youtube", "lucy custom videos", &inventory()).unwrap();
        // YouTube.app is a web app that drops the URL -> browser is correct here.
        assert!(matches!(plan, OpenPlan::Browser { .. }));

        let plan = plan_open_search("spotify", "daft punk", &inventory()).unwrap();
        match plan {
            OpenPlan::NativeDeepLink { uri, display, .. } => {
                assert_eq!(uri, "spotify:search:daft%20punk");
                assert_eq!(display, "Spotify");
            }
            other => panic!("expected the native app, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_app_falls_back_instead_of_failing() {
        let plan = plan_open_search("spotify", "daft punk", &[]).unwrap();
        assert!(matches!(plan, OpenPlan::Browser { .. }));
    }

    #[test]
    fn read_and_act_never_share_a_lease() {
        assert_eq!(mode_for("document.extract"), Mode::Read);
        assert_eq!(mode_for("app.open"), Mode::Act);
        // An unclassified capability must not pass as a harmless read.
        assert_eq!(mode_for("something.new"), Mode::Act);
    }

    #[test]
    fn an_act_lease_is_single_use_and_scope_bound() {
        let store = LeaseStore::default();
        let lease = store.issue("app.open", "Spotify", 7, RiskLevel::R1);
        assert_eq!(lease.mode, Mode::Act);
        // wrong target is refused even with the right id
        assert!(store.redeem(lease.id, "app.open", "Mail").is_err());
        // wrong capability is refused even with the right target
        assert!(store.redeem(lease.id, "app.control", "Spotify").is_err());
        assert!(store.redeem(lease.id, "app.open", "Spotify").is_ok());
        // replay is refused
        assert!(store.redeem(lease.id, "app.open", "Spotify").is_err());
    }

    #[test]
    fn ending_a_phase_drops_its_leases() {
        let store = LeaseStore::default();
        let read = store.issue("document.extract", "/tmp/a.pdf", 3, RiskLevel::R0);
        let act = store.issue("app.open", "Spotify", 3, RiskLevel::R1);
        store.end_phase(3, Mode::Read);
        assert!(store
            .redeem(read.id, "document.extract", "/tmp/a.pdf")
            .is_err());
        assert!(store.redeem(act.id, "app.open", "Spotify").is_ok());
    }

    #[test]
    fn a_declared_web_fallback_is_also_a_declared_capability() {
        for a in APP_ADAPTERS {
            if a.web_search.is_some() {
                assert!(
                    a.caps.contains(&"browser.open") || a.caps.contains(&"browser.search"),
                    "{} offers a web fallback it is not allowed to use",
                    a.key
                );
                assert!(a.web_host.is_some(), "{} has no host to allowlist", a.key);
            }
        }
    }

    #[test]
    fn only_registry_schemes_are_openable() {
        assert!(is_known_app_uri("spotify:search:abc"));
        assert!(is_known_app_uri("maps://?q=abc"));
        assert!(!is_known_app_uri("file:///etc/passwd"));
        assert!(!is_known_app_uri("ssh://host"));
        assert!(!is_known_app_uri("no-scheme"));
    }

    #[test]
    fn only_registry_hosts_are_openable() {
        assert!(is_known_web_host("www.youtube.com"));
        assert!(!is_known_web_host("evil.example"));
    }
}
