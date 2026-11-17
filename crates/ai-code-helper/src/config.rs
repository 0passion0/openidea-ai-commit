//! Strongly-typed configuration for the AI Code Helper plugin.

use openidea_plugin::host;

pub use ai_core::{DEFAULT_MAX_INPUT_BYTES, parse_max_input_bytes, setting_bool, setting_string};

/// Default number of distinct semantic naming groups requested from the model.
pub const DEFAULT_CANDIDATE_COUNT: usize = 3;

/// Default number of context lines before and after the editor selection.
pub const DEFAULT_CONTEXT_LINES: usize = 3;

/// Translation direction policy for selected text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TranslationDirection {
    /// Detect automatically: Chinese/mixed phrases -> English; English -> Simplified Chinese.
    #[default]
    Auto,
    /// Always translate input into Simplified Chinese.
    EnToZh,
    /// Always translate input into natural English.
    ZhToEn,
}

impl TranslationDirection {
    /// Parses a persisted setting string into [`TranslationDirection`].
    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "en-to-zh" => Self::EnToZh,
            "zh-to-en" => Self::ZhToEn,
            _ => Self::Auto,
        }
    }
}

/// How completed translations are presented to the user.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TranslationPresentation {
    /// Show a multiline preview dialog; OK replaces selection, Cancel preserves original and offers actions.
    #[default]
    DialogPreview,
    /// Show a non-blocking notification toast with Replace and Copy action buttons.
    NotifyActions,
    /// Immediately replace the selected text in the editor (single undoable edit).
    ReplaceDirectly,
}

impl TranslationPresentation {
    /// Parses a persisted setting string into [`TranslationPresentation`].
    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "notify-actions" => Self::NotifyActions,
            "replace-directly" => Self::ReplaceDirectly,
            _ => Self::DialogPreview,
        }
    }
}

/// Whether dismissing the translation preview dialog also copies the translation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CancelCopyPolicy {
    /// Leave the clipboard untouched unless the user clicks Copy.
    #[default]
    KeepClipboard,
    /// Automatically copy the translation to clipboard when Cancel/Esc is pressed.
    CopyTranslation,
}

impl CancelCopyPolicy {
    /// Builds a [`CancelCopyPolicy`] from the boolean setting flag.
    #[must_use]
    pub const fn from_flag(enabled: bool) -> Self {
        if enabled {
            Self::CopyTranslation
        } else {
            Self::KeepClipboard
        }
    }
}

/// Clipboard behavior after selecting an identifier from the Quick Pick naming list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PickCopyPolicy {
    /// Replace the editor selection without touching the clipboard.
    #[default]
    ReplaceOnly,
    /// Replace the editor selection and copy the picked identifier to the clipboard.
    ReplaceAndCopy,
}

impl PickCopyPolicy {
    /// Builds a [`PickCopyPolicy`] from the boolean setting flag.
    #[must_use]
    pub const fn from_flag(enabled: bool) -> Self {
        if enabled {
            Self::ReplaceAndCopy
        } else {
            Self::ReplaceOnly
        }
    }
}

/// Supported identifier casing styles rendered in the structured naming list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NamingStyle {
    /// `camelCase` (小驼峰命名).
    Camel,
    /// `PascalCase` (大驼峰命名).
    Pascal,
    /// `snake_case` (下划线命名).
    Snake,
    /// `SCREAMING_SNAKE_CASE` (全大写常量命名).
    ScreamingSnake,
    /// `kebab-case` (短横线命名).
    Kebab,
}

impl NamingStyle {
    /// Canonical display order of all supported identifier styles.
    pub const ALL: [Self; 5] = [
        Self::Camel,
        Self::Pascal,
        Self::Snake,
        Self::ScreamingSnake,
        Self::Kebab,
    ];

    /// Machine-readable style name (for example `"camelCase"`).
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Camel => "camelCase",
            Self::Pascal => "PascalCase",
            Self::Snake => "snake_case",
            Self::ScreamingSnake => "SCREAMING_SNAKE_CASE",
            Self::Kebab => "kebab-case",
        }
    }

    /// Concise Chinese style label shown in structured Quick Pick rows.
    #[must_use]
    pub const fn zh_label(self) -> &'static str {
        match self {
            Self::Camel => "小驼峰",
            Self::Pascal => "大驼峰",
            Self::Snake => "下划线",
            Self::ScreamingSnake => "全大写常量",
            Self::Kebab => "短横线",
        }
    }

    /// Deterministically formats normalized lowercase word tokens into this style.
    #[must_use]
    pub fn format_tokens(self, tokens: &[String]) -> String {
        if tokens.is_empty() {
            return String::new();
        }
        match self {
            Self::Camel => {
                let mut out = String::new();
                for (idx, word) in tokens.iter().enumerate() {
                    if idx == 0 {
                        out.push_str(&word.to_ascii_lowercase());
                    } else {
                        out.push_str(&capitalize_ascii(word));
                    }
                }
                out
            }
            Self::Pascal => tokens.iter().map(|w| capitalize_ascii(w)).collect(),
            Self::Snake => tokens
                .iter()
                .map(|w| w.to_ascii_lowercase())
                .collect::<Vec<_>>()
                .join("_"),
            Self::ScreamingSnake => tokens
                .iter()
                .map(|w| w.to_ascii_uppercase())
                .collect::<Vec<_>>()
                .join("_"),
            Self::Kebab => tokens
                .iter()
                .map(|w| w.to_ascii_lowercase())
                .collect::<Vec<_>>()
                .join("-"),
        }
    }
}

fn capitalize_ascii(word: &str) -> String {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out = String::with_capacity(word.len());
    out.push(first.to_ascii_uppercase());
    for ch in chars {
        out.push(ch.to_ascii_lowercase());
    }
    out
}

/// Effective plugin configuration loaded from OpenIDEA settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelperConfig {
    /// Configured translation direction.
    pub direction: TranslationDirection,
    /// Configured translation output UI flow.
    pub presentation: TranslationPresentation,
    /// Clipboard behavior when cancelling the translation dialog.
    pub cancel_copy: CancelCopyPolicy,
    /// Ordered list of identifier styles included in the Quick Pick output.
    pub naming_styles: Vec<NamingStyle>,
    /// Number of semantic candidate schemes requested (`1..=5`).
    pub naming_candidate_count: usize,
    /// Clipboard behavior after picking an identifier.
    pub pick_copy: PickCopyPolicy,
    /// Context lines captured before/after the selection (`0..=15`).
    pub context_lines: usize,
    /// Total prompt byte cap (`system + user <= max_input_bytes`).
    pub max_input_bytes: usize,
}

impl Default for HelperConfig {
    fn default() -> Self {
        Self {
            direction: TranslationDirection::Auto,
            presentation: TranslationPresentation::DialogPreview,
            cancel_copy: CancelCopyPolicy::KeepClipboard,
            naming_styles: NamingStyle::ALL.to_vec(),
            naming_candidate_count: DEFAULT_CANDIDATE_COUNT,
            pick_copy: PickCopyPolicy::ReplaceOnly,
            context_lines: DEFAULT_CONTEXT_LINES,
            max_input_bytes: DEFAULT_MAX_INPUT_BYTES,
        }
    }
}

impl HelperConfig {
    /// Reads effective settings from OpenIDEA host settings storage.
    #[must_use]
    pub fn from_host() -> Self {
        let direction =
            TranslationDirection::from_setting(&setting_string("translationDirection", "auto"));
        let presentation = TranslationPresentation::from_setting(&setting_string(
            "translationPresentation",
            "dialog-preview",
        ));
        let cancel_copy =
            CancelCopyPolicy::from_flag(setting_bool("autoCopyTranslationOnCancel", false));
        let pick_copy = PickCopyPolicy::from_flag(setting_bool("copyOnPickNaming", false));

        let style_flags = [
            (NamingStyle::Camel, "includeCamelCase"),
            (NamingStyle::Pascal, "includePascalCase"),
            (NamingStyle::Snake, "includeSnakeCase"),
            (NamingStyle::ScreamingSnake, "includeScreamingSnakeCase"),
            (NamingStyle::Kebab, "includeKebabCase"),
        ];
        let mut naming_styles: Vec<NamingStyle> = style_flags
            .into_iter()
            .filter(|(_, key)| setting_bool(key, true))
            .map(|(style, _)| style)
            .collect();
        if naming_styles.is_empty() {
            naming_styles = NamingStyle::ALL.to_vec();
        }

        let naming_candidate_count =
            host::setting_number("namingCandidateCount").map_or(DEFAULT_CANDIDATE_COUNT, |n| {
                #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let count = n.clamp(1.0, 5.0) as usize;
                count
            });

        let context_lines =
            host::setting_number("contextLines").map_or(DEFAULT_CONTEXT_LINES, |n| {
                #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let lines = n.clamp(0.0, 15.0) as usize;
                lines
            });

        let max_input_bytes = parse_max_input_bytes(DEFAULT_MAX_INPUT_BYTES);

        Self {
            direction,
            presentation,
            cancel_copy,
            naming_styles,
            naming_candidate_count,
            pick_copy,
            context_lines,
            max_input_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openidea_plugin::mock::MockHost;

    #[test]
    fn formats_all_five_naming_styles_accurately() {
        let tokens = vec![
            "user".to_owned(),
            "order".to_owned(),
            "total".to_owned(),
            "amount".to_owned(),
        ];
        assert_eq!(
            NamingStyle::Camel.format_tokens(&tokens),
            "userOrderTotalAmount"
        );
        assert_eq!(
            NamingStyle::Pascal.format_tokens(&tokens),
            "UserOrderTotalAmount"
        );
        assert_eq!(
            NamingStyle::Snake.format_tokens(&tokens),
            "user_order_total_amount"
        );
        assert_eq!(
            NamingStyle::ScreamingSnake.format_tokens(&tokens),
            "USER_ORDER_TOTAL_AMOUNT"
        );
        assert_eq!(
            NamingStyle::Kebab.format_tokens(&tokens),
            "user-order-total-amount"
        );
    }

    #[test]
    fn restores_all_styles_if_user_disables_every_toggle() {
        let host = MockHost::install();
        for key in [
            "includeCamelCase",
            "includePascalCase",
            "includeSnakeCase",
            "includeScreamingSnakeCase",
            "includeKebabCase",
        ] {
            host.set_setting(key, "false");
        }
        let cfg = HelperConfig::from_host();
        assert_eq!(cfg.naming_styles, NamingStyle::ALL);
    }
}
