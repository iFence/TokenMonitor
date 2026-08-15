//! Model-name normalization: local model names → OpenRouter canonical IDs.
//! Mirrors tokei's `_normalize` and `_FAMILY`.

/// Family keyword → representative canonical id, used when exact matching fails.
/// Mirrors tokei's `_FAMILY`.
pub(crate) const FAMILY: &[(&str, &str)] = &[
    ("opus", "anthropic/claude-opus-4.8"),
    ("sonnet", "anthropic/claude-sonnet-4.6"),
    ("haiku", "anthropic/claude-haiku-4.5"),
    ("gpt-5", "openai/gpt-5.5"),
    ("qwen", "qwen/qwen3.7-max"),
    ("deepseek", "deepseek/deepseek-v4-pro"),
    ("glm", "z-ai/glm-5.2"),
    ("mimo", "xiaomi/mimo-v2.5-pro"),
    ("hy3", "tencent/hy3"),
];

/// Normalize a local model name to an OpenRouter canonical id, following
/// tokei's `_normalize`. Returns `None` for empty / `<synthetic>` models.
pub fn normalize(model: &str) -> Option<String> {
    let m = model.trim().to_lowercase();
    if m.is_empty() || m == "<synthetic>" {
        return None;
    }
    let mut m = m.split_whitespace().collect::<Vec<_>>().join("-");
    // Free-tier suffix is priced at the base rate.
    if let Some(stripped) = m.strip_suffix(":free").or_else(|| m.strip_suffix("-free")) {
        m = stripped.to_string();
    }
    if m.contains('/') {
        return Some(m); // already OpenRouter format
    }
    if m.starts_with("claude") {
        m = fix_claude_version(&m);
        return Some(format!("anthropic/{m}"));
    }
    if is_openai(&m) {
        return Some(format!("openai/{m}"));
    }
    if m.starts_with("gemini") {
        return Some(format!("google/{m}"));
    }
    if m.starts_with("grok") {
        return Some(format!("x-ai/{m}"));
    }
    if m.starts_with("qwen") {
        return Some(format!("qwen/{m}"));
    }
    if m.starts_with("deepseek") {
        return Some(format!("deepseek/{m}"));
    }
    if m.starts_with("glm") {
        return Some(format!("z-ai/{m}"));
    }
    if m.starts_with("mimo") {
        return Some(format!("xiaomi/{m}"));
    }
    if m == "hy3" {
        return Some("tencent/hy3".to_string());
    }
    if m == "hy3-preview" || m == "hy3 preview" {
        return Some("tencent/hy3-preview".to_string());
    }
    Some(m)
}

/// OpenAI models are `gpt-*`, `chatgpt`, or `o<digit>` (reasoning series).
fn is_openai(m: &str) -> bool {
    m.starts_with("gpt")
        || m.starts_with("chatgpt")
        || (m.starts_with('o') && m.as_bytes().get(1).is_some_and(|b| b.is_ascii_digit()))
}

/// `claude-opus-4-8` → `claude-opus-4.8`: collapse a trailing `-<digits>-<digits>`
/// into `-<digits>.<digits>`. Models already dotted (e.g. `claude-opus-4.5`) are
/// left untouched because the last dash segment isn't all digits.
fn fix_claude_version(m: &str) -> String {
    let last_dash = match m.rfind('-') {
        Some(i) => i,
        None => return m.to_string(),
    };
    let after_last = &m[last_dash + 1..];
    if after_last.is_empty() || !after_last.chars().all(|c| c.is_ascii_digit()) {
        return m.to_string();
    }
    let before_last = &m[..last_dash];
    let prev_dash = match before_last.rfind('-') {
        Some(i) => i,
        None => return m.to_string(),
    };
    let digits = &before_last[prev_dash + 1..];
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return m.to_string();
    }
    format!("{}-{}.{}", &m[..prev_dash], digits, after_last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_claude_with_hyphenated_version() {
        assert_eq!(
            normalize("claude-opus-4-8").as_deref(),
            Some("anthropic/claude-opus-4.8")
        );
        assert_eq!(
            normalize("claude-sonnet-4-6").as_deref(),
            Some("anthropic/claude-sonnet-4.6")
        );
        assert_eq!(
            normalize("claude-haiku-4-5").as_deref(),
            Some("anthropic/claude-haiku-4.5")
        );
        // Already-dotted version is unchanged.
        assert_eq!(
            normalize("claude-opus-4.5").as_deref(),
            Some("anthropic/claude-opus-4.5")
        );
    }

    #[test]
    fn normalizes_other_families() {
        assert_eq!(normalize("gpt-5.5").as_deref(), Some("openai/gpt-5.5"));
        assert_eq!(normalize("o4-mini").as_deref(), Some("openai/o4-mini"));
        assert_eq!(normalize("chatgpt-4").as_deref(), Some("openai/chatgpt-4"));
        assert_eq!(
            normalize("gemini-3.5-flash").as_deref(),
            Some("google/gemini-3.5-flash")
        );
        assert_eq!(
            normalize("qwen3.7-plus").as_deref(),
            Some("qwen/qwen3.7-plus")
        );
        assert_eq!(normalize("grok-4.5").as_deref(), Some("x-ai/grok-4.5"));
        assert_eq!(
            normalize("deepseek-v4-pro").as_deref(),
            Some("deepseek/deepseek-v4-pro")
        );
        assert_eq!(normalize("glm-5.2").as_deref(), Some("z-ai/glm-5.2"));
        assert_eq!(normalize("hy3").as_deref(), Some("tencent/hy3"));
        assert_eq!(
            normalize("hy3 preview").as_deref(),
            Some("tencent/hy3-preview")
        );
    }

    #[test]
    fn strips_free_suffix_and_lowercases() {
        assert_eq!(normalize("GPT-4O:free").as_deref(), Some("openai/gpt-4o"));
        assert_eq!(
            normalize("Claude-Opus-4-8-free").as_deref(),
            Some("anthropic/claude-opus-4.8")
        );
        assert_eq!(normalize("gpt-5.5").as_deref(), Some("openai/gpt-5.5"));
    }

    #[test]
    fn passes_through_already_canonical_and_unknown() {
        assert_eq!(
            normalize("anthropic/claude-opus-4.8").as_deref(),
            Some("anthropic/claude-opus-4.8")
        );
        // Unknown names are returned as-is (lowercased), no prefix added.
        assert_eq!(normalize("mystery-model").as_deref(), Some("mystery-model"));
    }

    #[test]
    fn empty_and_synthetic_are_none() {
        assert_eq!(normalize(""), None);
        assert_eq!(normalize("   "), None);
        assert_eq!(normalize("<synthetic>"), None);
    }
}
