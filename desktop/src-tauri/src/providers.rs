//! Provider-Registry für Noki Intelligence.
//!
//! WOZU. Bisher stand "lokal" überall implizit im Code: model_manager kennt
//! llama.cpp, die Oberfläche zeigt einen fest getexteten Namen. Damit später
//! ein Cloud-Anbieter danebentreten kann, ohne Work-/UI-Architektur
//! umzubauen, braucht es EINE Stelle, die sagt: welche Anbieter gibt es,
//! welcher ist aktiv, welche Modelle bringt er mit, was kann er.
//!
//! WAS DIESES MODUL NICHT TUT. Es spricht mit niemandem. Kein Netz, kein
//! Prozess, kein Schlüssel. Es beschreibt nur — die Ausführung bleibt
//! vollständig beim vorhandenen model_manager (llama.cpp, Ollama-Rollback).
//! Ein Cloud-Anbieter ist hier deshalb höchstens VORHANDEN, nie verbunden.
//!
//! SCHLÜSSEL. `auth_reference` ist ein NAME, kein Geheimnis: die Kennung
//! eines Eintrags im Schlüsselbund, den eine spätere Anbindung nachschlägt.
//! Ein echter Schlüssel wird hier nie entgegengenommen, nie gespeichert und
//! nie protokolliert — genau darum gibt es das Feld überhaupt.

use serde::{Deserialize, Serialize};

/// Woher die Antwort kommt. Mehr Fälle braucht die Oberfläche nicht: sie
/// entscheidet daran nur, was sie anzeigt.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// Läuft auf diesem Rechner (llama.cpp/Metal, Ollama-Rollback).
    Local,
    /// Läuft bei einem Dienst. Noch keiner angebunden.
    Cloud,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Local => "local",
            ProviderKind::Cloud => "cloud",
        }
    }
}

/// Was ein Modell kann. Nur Felder, die wirklich bekannt sind — geraten
/// wird hier nichts, sonst steht in der Oberfläche eine Behauptung.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    /// Kontextfenster in Token, falls konfiguriert.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub supports_streaming: bool,
}

impl ModelInfo {
    pub fn neu(id: &str, display_name: &str) -> Self {
        ModelInfo {
            id: id.to_owned(),
            display_name: display_name.to_owned(),
            ..Default::default()
        }
    }
    pub fn mit_kontext(mut self, n: u32) -> Self {
        self.context_window = Some(n);
        self
    }
    pub fn mit_werkzeugen(mut self, an: bool) -> Self {
        self.supports_tools = an;
        self
    }
    pub fn mit_strom(mut self, an: bool) -> Self {
        self.supports_streaming = an;
        self
    }
}

/// Compatibility adapter for the existing UI/provider contract. Presentation
/// labels may stay UI-specific, while identity, context and capabilities come
/// exclusively from the canonical model registry.
pub fn model_info_from_registry(
    model: &crate::model_registry::CanonicalModelDefinition,
    display_name: &str,
) -> ModelInfo {
    ModelInfo {
        id: model.exact_model_version.to_string(),
        display_name: display_name.to_string(),
        context_window: Some(model.capabilities.context_tokens),
        supports_tools: model.capabilities.tools,
        supports_vision: model.capabilities.vision,
        supports_streaming: model.capabilities.streaming,
    }
}

/// Ein Anbieter, wie ihn die Oberfläche sieht.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct IntelligenceProvider {
    pub id: String,
    pub kind: ProviderKind,
    pub display_name: String,
    /// Bei lokal die Laufzeit-Adresse, bei Cloud die Dienstadresse.
    /// Leer heißt: noch nicht eingerichtet.
    pub endpoint: String,
    pub models: Vec<ModelInfo>,
    /// Freie Merkmale des Anbieters (z. B. "metal", "tools").
    pub capabilities: Vec<String>,
    /// NAME eines Schlüsselbund-Eintrags, niemals der Schlüssel selbst.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_reference: Option<String>,
    /// Ist er gerade wirklich ansprechbar?
    pub connected: bool,
    /// Darf er benutzt werden?
    pub enabled: bool,
}

/// Eine Anfrage an einen Anbieter. Absichtlich anbieterneutral: kein Feld
/// hier kennt llama.cpp, Ollama oder einen Dienstnamen.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct ProviderRequest {
    pub model: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub stream: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct ProviderResponse {
    pub model: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u32>,
}

/// Warum es nicht ging. Die Oberfläche unterscheidet daran, ob sie zum
/// Einrichten führt oder einen Fehler zeigt.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "art", content = "text", rename_all = "snake_case")]
pub enum ProviderError {
    /// Kein Anbieter dieser Art eingerichtet.
    NichtKonfiguriert(String),
    /// Eingerichtet, aber gerade nicht erreichbar.
    NichtVerbunden(String),
    /// Der Anbieter kennt das Modell nicht.
    ModellUnbekannt(String),
    /// Alles andere.
    Fehler(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let t = match self {
            ProviderError::NichtKonfiguriert(t)
            | ProviderError::NichtVerbunden(t)
            | ProviderError::ModellUnbekannt(t)
            | ProviderError::Fehler(t) => t,
        };
        f.write_str(t)
    }
}

/// Der momentane Laufzeitzustand — das, was die Oberfläche anzeigt.
/// EINE Wahrheit: die Oberfläche denkt sich nichts dazu.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RuntimeState {
    pub active_provider_kind: ProviderKind,
    pub active_provider: String,
    pub active_model: String,
    pub active_model_label: String,
    /// Die tatsächliche Laufzeit, z. B. "llama.cpp" oder "Ollama".
    pub runtime: String,
    /// Der Rechenweg, z. B. "Metal". Leer, wenn unbekannt.
    pub accelerator: String,
    /// Kurzzeile für die Oberfläche: "Local · llama.cpp · Metal".
    pub label: String,
    /// Nur bei Cloud und nur, solange nichts eingerichtet ist.
    pub cloud_configured: bool,
}

/// Die eine Registry. Lokal ist immer dabei; Cloud-Anbieter werden später
/// hier eingehängt (und sind bis dahin schlicht nicht vorhanden).
#[derive(Clone, Debug, Default)]
pub struct Registry {
    provider: Vec<IntelligenceProvider>,
}

/// Der lokale Anbieter. Endpunkt und Laufzeit kommen von der Seite, die sie
/// wirklich kennt (model_manager), damit hier nichts doppelt behauptet wird.
pub fn lokaler_provider(
    runtime: &str,
    endpoint: &str,
    accelerator: &str,
    models: Vec<ModelInfo>,
) -> IntelligenceProvider {
    let mut capabilities = vec![runtime.to_owned()];
    if !accelerator.is_empty() {
        capabilities.push(accelerator.to_owned());
    }
    IntelligenceProvider {
        id: "local".into(),
        kind: ProviderKind::Local,
        display_name: "Local".into(),
        endpoint: endpoint.to_owned(),
        models,
        capabilities,
        auth_reference: None,
        connected: true,
        enabled: true,
    }
}

impl Registry {
    pub fn neu() -> Self {
        Registry {
            provider: Vec::new(),
        }
    }

    /// Anbieter aufnehmen oder ersetzen (gleiche id = derselbe Anbieter).
    /// Ein Schlüssel kann hier nicht hineingeraten: das Feld heißt
    /// auth_reference und trägt einen Schlüsselbund-NAMEN.
    pub fn setzen(&mut self, p: IntelligenceProvider) {
        match self.provider.iter_mut().find(|x| x.id == p.id) {
            Some(alt) => *alt = p,
            None => self.provider.push(p),
        }
    }

    pub fn entfernen(&mut self, id: &str) -> bool {
        let vorher = self.provider.len();
        self.provider.retain(|p| p.id != id);
        self.provider.len() != vorher
    }

    pub fn alle(&self) -> &[IntelligenceProvider] {
        &self.provider
    }

    pub fn finden(&self, id: &str) -> Option<&IntelligenceProvider> {
        self.provider.iter().find(|p| p.id == id)
    }

    pub fn erster(&self, kind: ProviderKind) -> Option<&IntelligenceProvider> {
        self.provider
            .iter()
            .find(|p| p.kind == kind && p.enabled && p.connected)
    }

    pub fn hat(&self, kind: ProviderKind) -> bool {
        self.provider.iter().any(|p| p.kind == kind && p.enabled)
    }

    /// Was die Oberfläche anzeigt. Bei Cloud ohne eingerichteten Anbieter
    /// steht hier ausdrücklich "Nicht konfiguriert" — und der Aufrufer weiß
    /// über `cloud_configured`, dass er nichts losschicken darf.
    pub fn runtime_state(
        &self,
        gewuenscht: ProviderKind,
        model: &str,
        model_label: &str,
    ) -> RuntimeState {
        if gewuenscht == ProviderKind::Cloud {
            if let Some(p) = self.erster(ProviderKind::Cloud) {
                let label = format!("Cloud · {}", p.display_name);
                return RuntimeState {
                    active_provider_kind: ProviderKind::Cloud,
                    active_provider: p.id.clone(),
                    active_model: model.to_owned(),
                    active_model_label: model_label.to_owned(),
                    runtime: p.display_name.clone(),
                    accelerator: String::new(),
                    label,
                    cloud_configured: true,
                };
            }
            return RuntimeState {
                active_provider_kind: ProviderKind::Cloud,
                active_provider: String::new(),
                active_model: String::new(),
                active_model_label: String::new(),
                runtime: String::new(),
                accelerator: String::new(),
                label: "Cloud · Nicht konfiguriert".into(),
                cloud_configured: false,
            };
        }
        let p = self.finden("local");
        let runtime = p
            .and_then(|p| p.capabilities.first().cloned())
            .unwrap_or_default();
        let accelerator = p
            .and_then(|p| p.capabilities.get(1).cloned())
            .unwrap_or_default();
        let mut label = String::from("Local");
        if !runtime.is_empty() {
            label.push_str(" · ");
            label.push_str(&runtime);
        }
        if !accelerator.is_empty() {
            label.push_str(" · ");
            label.push_str(&accelerator);
        }
        RuntimeState {
            active_provider_kind: ProviderKind::Local,
            active_provider: "local".into(),
            active_model: model.to_owned(),
            active_model_label: model_label.to_owned(),
            runtime,
            accelerator,
            label,
            cloud_configured: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lokal() -> IntelligenceProvider {
        lokaler_provider(
            "llama.cpp",
            "http://127.0.0.1:8080",
            "Metal",
            vec![ModelInfo::neu("qwen3.5-4b", "Qwen3.5 4B")
                .mit_kontext(32768)
                .mit_werkzeugen(true)
                .mit_strom(true)],
        )
    }

    #[test]
    fn lokal_ist_die_grundstellung() {
        let mut r = Registry::neu();
        r.setzen(lokal());
        let s = r.runtime_state(ProviderKind::Local, "qwen3.5-4b", "Qwen3.5 4B");
        assert_eq!(s.label, "Local · llama.cpp · Metal");
        assert_eq!(s.active_provider, "local");
        assert_eq!(s.active_model_label, "Qwen3.5 4B");
        assert_eq!(s.runtime, "llama.cpp");
        assert_eq!(s.accelerator, "Metal");
    }

    #[test]
    fn cloud_ohne_anbieter_ist_nicht_konfiguriert() {
        let mut r = Registry::neu();
        r.setzen(lokal());
        let s = r.runtime_state(ProviderKind::Cloud, "egal", "Egal");
        assert!(!s.cloud_configured);
        assert_eq!(s.label, "Cloud · Nicht konfiguriert");
        // Wichtig: kein Modell, kein Anbieter — es gibt nichts anzusprechen.
        assert!(s.active_provider.is_empty());
        assert!(s.active_model.is_empty());
        assert!(!r.hat(ProviderKind::Cloud));
    }

    #[test]
    fn ein_fremder_anbieter_braucht_keine_kernaenderung() {
        // Genau der Fall aus der Vorgabe: ein Testanbieter wird registriert,
        // erscheint mit seinen Modell-Metadaten und wird danach wieder
        // entfernt. Kein Zweig im Kern kennt seinen Namen.
        let mut r = Registry::neu();
        r.setzen(lokal());
        r.setzen(IntelligenceProvider {
            id: "fixture".into(),
            kind: ProviderKind::Cloud,
            display_name: "Fixture".into(),
            endpoint: "https://example.invalid/v1".into(),
            models: vec![ModelInfo::neu("fixture-m", "Fixture M")
                .mit_kontext(8192)
                .mit_strom(true)],
            capabilities: vec!["tools".into()],
            auth_reference: Some("noki.provider.fixture".into()),
            connected: true,
            enabled: true,
        });
        let s = r.runtime_state(ProviderKind::Cloud, "fixture-m", "Fixture M");
        assert!(s.cloud_configured);
        assert_eq!(s.active_provider, "fixture");
        assert_eq!(s.label, "Cloud · Fixture");
        let p = r.finden("fixture").unwrap();
        assert_eq!(p.models[0].context_window, Some(8192));
        assert!(p.models[0].supports_streaming);
        assert!(!p.models[0].supports_vision);
        // Der Schluessel selbst steht nirgends — nur sein Name.
        assert_eq!(p.auth_reference.as_deref(), Some("noki.provider.fixture"));
        assert!(r.entfernen("fixture"));
        assert!(!r.hat(ProviderKind::Cloud));
        // Und lokal ist unveraendert da.
        assert_eq!(
            r.runtime_state(ProviderKind::Local, "qwen3.5-4b", "Qwen3.5 4B")
                .label,
            "Local · llama.cpp · Metal"
        );
    }

    #[test]
    fn auth_reference_wird_serialisiert_ohne_geheimnis() {
        let p = lokal();
        let j = serde_json::to_string(&p).unwrap();
        assert!(!j.contains("auth_reference"), "lokal hat keinen Verweis");
        assert!(j.contains("\"kind\":\"local\""));
        assert!(j.contains("Metal"));
    }
}
