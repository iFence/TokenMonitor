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
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

impl Provider {
    pub const ALL: [Provider; 6] = [
        Provider::Claude,
        Provider::Codex,
        Provider::Gemini,
        Provider::Codebuddy,
        Provider::OpenCode,
        Provider::Qoder,
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
        }
    }

    pub fn from_id(s: &str) -> Option<Self> {
        Provider::ALL.iter().copied().find(|p| p.id() == s)
    }
}
