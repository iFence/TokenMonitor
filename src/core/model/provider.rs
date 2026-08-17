use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// AI coding tool whose local usage data rToken can track.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    Claude,
    Codex,
    Gemini,
    Codebuddy,
    OpenCode,
    Qoder,
    OpenClaw,
    DeepSeek,
    Pi,
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

impl Provider {
    pub const ALL: [Provider; 9] = [
        Provider::Claude,
        Provider::Codex,
        Provider::Gemini,
        Provider::Codebuddy,
        Provider::OpenCode,
        Provider::Qoder,
        Provider::OpenClaw,
        Provider::DeepSeek,
        Provider::Pi,
    ];

    /// Stable lowercase identifier, used as the DB provider column and serde tag.
    pub fn id(self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
            Provider::Gemini => "gemini",
            Provider::Codebuddy => "codebuddy",
            Provider::OpenCode => "opencode",
            Provider::Qoder => "qoder",
            Provider::OpenClaw => "openclaw",
            Provider::DeepSeek => "deepseek",
            Provider::Pi => "pi",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Provider::Claude => "Claude Code",
            Provider::Codex => "Codex CLI",
            Provider::Gemini => "Gemini CLI",
            Provider::Codebuddy => "CodeBuddy",
            Provider::OpenCode => "OpenCode",
            Provider::Qoder => "Qoder",
            Provider::OpenClaw => "OpenClaw",
            Provider::DeepSeek => "DeepSeek Harness",
            Provider::Pi => "Pi",
        }
    }

    pub fn from_id(s: &str) -> Option<Self> {
        Provider::ALL.iter().copied().find(|p| p.id() == s)
    }
}

/// One app row in the user's selection: whether it is tracked and where it
/// sits in the display/scan order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderEntry {
    pub provider: Provider,
    pub enabled: bool,
}

/// The user's app selection: every known provider in display/scan order, each
/// with an `enabled` flag. Persisted as JSON in the settings table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSelection {
    pub entries: Vec<ProviderEntry>,
}

impl Default for ProviderSelection {
    fn default() -> Self {
        ProviderSelection {
            entries: Provider::ALL
                .iter()
                .copied()
                .map(|provider| ProviderEntry {
                    provider,
                    enabled: true,
                })
                .collect(),
        }
    }
}

impl ProviderSelection {
    /// Enabled providers, in display/scan order.
    pub fn enabled(&self) -> Vec<Provider> {
        self.entries
            .iter()
            .filter(|e| e.enabled)
            .map(|e| e.provider)
            .collect()
    }

    pub fn is_enabled(&self, provider: Provider) -> bool {
        self.entries
            .iter()
            .any(|e| e.provider == provider && e.enabled)
    }

    /// Set `provider`'s enabled flag (appending it if somehow absent).
    pub fn set_enabled(&mut self, provider: Provider, enabled: bool) {
        for e in &mut self.entries {
            if e.provider == provider {
                e.enabled = enabled;
                return;
            }
        }
        self.entries.push(ProviderEntry { provider, enabled });
    }

    /// Move `provider` up (`dir < 0`) or down (`dir > 0`). No-op at the bounds.
    pub fn move_entry(&mut self, provider: Provider, dir: isize) {
        let Some(idx) = self.entries.iter().position(|e| e.provider == provider) else {
            return;
        };
        let target = if dir < 0 {
            idx.checked_sub(1)
        } else {
            idx.checked_add(1)
        };
        let Some(target) = target else { return };
        if target >= self.entries.len() {
            return;
        }
        self.entries.swap(idx, target);
    }

    /// Reconcile `entries` against the known provider set: drop unknown or
    /// duplicated providers, and append any missing provider as enabled. This
    /// keeps persisted data valid as providers are added over time.
    pub fn normalize(&mut self) {
        let mut seen: HashSet<Provider> = HashSet::new();
        let mut entries = Vec::with_capacity(self.entries.len());
        for e in self.entries.drain(..) {
            if seen.insert(e.provider) {
                entries.push(e);
            }
        }
        for provider in Provider::ALL {
            if !seen.contains(&provider) {
                entries.push(ProviderEntry {
                    provider,
                    enabled: true,
                });
            }
        }
        self.entries = entries;
    }
}
