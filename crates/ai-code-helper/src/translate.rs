//! Language direction detection and model translation cleanup.

use ai_core::{strip_reasoning_tags, unwrap_code_fence, unwrap_outer_quotes};

use crate::config::TranslationDirection;

const TRANSLATION_LABELS: &[&str] = &[
    "Translation:",
    "translation:",
    "Translated text:",
    "译文：",
    "译文:",
    "翻译结果：",
    "翻译结果:",
    "翻译：",
    "翻译:",
];

/// Resolves [`TranslationDirection::Auto`] into a concrete target direction.
#[must_use]
pub fn resolve_direction(text: &str, configured: TranslationDirection) -> TranslationDirection {
    match configured {
        TranslationDirection::Auto => detect_direction(text),
        explicit => explicit,
    }
}

/// Detects whether text is primarily Chinese/mixed-Chinese intent (`ZhToEn`) or English (`EnToZh`).
#[must_use]
pub fn detect_direction(text: &str) -> TranslationDirection {
    let mut cjk_count = 0usize;
    let mut latin_count = 0usize;

    for ch in text.chars() {
        if is_cjk_han(ch) {
            cjk_count += 1;
        } else if ch.is_ascii_alphabetic() {
            latin_count += 1;
        }
    }

    if cjk_count > 0 && (cjk_count * 4 >= latin_count || latin_count < 12) {
        TranslationDirection::ZhToEn
    } else {
        TranslationDirection::EnToZh
    }
}

fn is_cjk_han(ch: char) -> bool {
    matches!(
        ch,
        '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{20000}'..='\u{2A6DF}'
    )
}

/// Cleans reasoning tags, accidental outer fences/quotes, and preamble labels from model translation.
#[must_use]
pub fn clean_translation(raw_answer: &str, original_input: &str) -> String {
    let no_reasoning = strip_reasoning_tags(raw_answer);
    let mut text = strip_translation_label(no_reasoning.trim());

    // Unwrap accidental outer markdown fence unless the user actually selected a fenced block.
    if !original_input.trim_start().starts_with("```") {
        text = unwrap_code_fence(&text);
    }
    text = strip_translation_label(&text);
    if !starts_and_ends_with_same_quote(original_input.trim()) {
        text = unwrap_outer_quotes(&text);
    }

    text.trim().to_owned()
}

fn strip_translation_label(input: &str) -> String {
    let trimmed = input.trim();
    for label in TRANSLATION_LABELS {
        if let Some(rest) = trimmed.strip_prefix(label) {
            return rest.trim().to_owned();
        }
    }
    trimmed.to_owned()
}

fn starts_and_ends_with_same_quote(text: &str) -> bool {
    ['"', '\'', '`']
        .into_iter()
        .any(|q| text.starts_with(q) && text.ends_with(q) && text.len() >= 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_chinese_english_and_mixed_inputs_accurately() {
        assert_eq!(
            detect_direction("用户订单总金额"),
            TranslationDirection::ZhToEn
        );
        assert_eq!(
            detect_direction("获取 user_id 并校验 JWT token"),
            TranslationDirection::ZhToEn
        );
        assert_eq!(
            detect_direction("Validates incoming OAuth2 tokens and caches user metadata."),
            TranslationDirection::EnToZh
        );
        assert_eq!(
            detect_direction(
                "Validates incoming OAuth2 tokens and returns error when expired. // 注"
            ),
            TranslationDirection::EnToZh
        );
    }

    #[test]
    fn cleans_translation_output_while_preserving_intentionally_quoted_input() {
        let raw = "<think>ok</think>\n译文：\n```\n校验传入的 OAuth2 令牌\n```";
        assert_eq!(
            clean_translation(raw, "Validates incoming OAuth2 tokens"),
            "校验传入的 OAuth2 令牌"
        );
        assert_eq!(
            clean_translation("\"用户订单\"", "\"User order\""),
            "\"用户订单\""
        );
    }
}
