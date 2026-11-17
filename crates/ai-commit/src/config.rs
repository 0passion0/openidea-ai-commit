//! Commit-message-specific plugin configuration.

use openidea_plugin::host;

pub use ai_core::{DEFAULT_MAX_INPUT_BYTES, parse_max_input_bytes, setting_string};

/// Default commit message output language.
pub const DEFAULT_LANGUAGE: &str = "中文";

/// Commit message format requested from the model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CommitStyle {
    /// `type(scope): subject` following Conventional Commits.
    #[default]
    Conventional,
    /// Plain imperative subject line.
    Plain,
    /// Leading gitmoji followed by an imperative subject line.
    Gitmoji,
}

impl CommitStyle {
    /// Parses the persisted setting value into a [`CommitStyle`].
    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "plain" => Self::Plain,
            "gitmoji" => Self::Gitmoji,
            _ => Self::Conventional,
        }
    }

    /// Returns `true` when Conventional Commits validation applies.
    #[must_use]
    pub const fn is_conventional(self) -> bool {
        matches!(self, Self::Conventional)
    }
}

/// Controls whether a commit body is generated below the subject line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BodyMode {
    /// Add a structured bullet-list body after the subject line.
    #[default]
    Always,
    /// Add a body unless the diff is trivial and self-explanatory.
    Auto,
    /// Subject line only.
    Never,
}

impl BodyMode {
    /// Parses the persisted setting value into a [`BodyMode`].
    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Self::Auto,
            "never" => Self::Never,
            _ => Self::Always,
        }
    }
}

/// Whether existing non-amend draft text in the commit box acts as a hint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DraftUsage {
    /// Treat user-entered draft text as intent notes.
    #[default]
    UseAsHint,
    /// Ignore user-entered draft text unless amending.
    Ignore,
}

impl DraftUsage {
    /// Converts the boolean setting into a strongly-typed enum.
    #[must_use]
    pub const fn from_bool(enabled: bool) -> Self {
        if enabled {
            Self::UseAsHint
        } else {
            Self::Ignore
        }
    }
}

/// Settings controlling prompt formatting and the 150 KB model input budget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptConfig {
    /// Requested commit message style.
    pub style: CommitStyle,
    /// Output language for the generated commit message.
    pub language: String,
    /// Whether to include a body.
    pub body: BodyMode,
    /// Whether existing message text acts as an author hint.
    pub draft_usage: DraftUsage,
    /// Optional user-supplied instructions.
    pub instructions: String,
    /// Strict upper bound (in bytes) for `system + user` prompt text.
    pub max_input_bytes: usize,
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            style: CommitStyle::Conventional,
            language: DEFAULT_LANGUAGE.to_owned(),
            body: BodyMode::Always,
            draft_usage: DraftUsage::UseAsHint,
            instructions: String::new(),
            max_input_bytes: DEFAULT_MAX_INPUT_BYTES,
        }
    }
}

impl PromptConfig {
    /// Loads prompt settings from OpenIDEA host configuration.
    #[must_use]
    pub fn from_host() -> Self {
        let defaults = Self::default();
        let max_input_bytes = parse_max_input_bytes(defaults.max_input_bytes);

        let raw_language = setting_string("language", &defaults.language);
        let language = if raw_language.trim().is_empty() {
            defaults.language
        } else {
            raw_language
        };

        Self {
            style: CommitStyle::from_setting(&setting_string("style", "conventional")),
            language,
            body: BodyMode::from_setting(&setting_string("body", "always")),
            draft_usage: DraftUsage::from_bool(host::setting_bool("useDraft").unwrap_or(true)),
            instructions: setting_string("instructions", ""),
            max_input_bytes,
        }
    }
}

/// Returns `true` when the configured output language targets Chinese.
#[must_use]
pub fn is_chinese_language(language: &str) -> bool {
    let trimmed = language.trim();
    if trimmed.is_empty() || trimmed.contains('中') {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "zh" | "zh-cn" | "zh_cn" | "zh-tw" | "zh-hans" | "zh-hant"
    ) || lower.contains("chinese")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_chinese_detailed_conventional_commits() {
        let cfg = PromptConfig::default();
        assert_eq!(cfg.style, CommitStyle::Conventional);
        assert_eq!(cfg.body, BodyMode::Always);
        assert!(is_chinese_language(&cfg.language));
        assert!(is_chinese_language("Chinese"));
        assert!(is_chinese_language("zh-CN"));
        assert!(!is_chinese_language("English"));
    }
}
