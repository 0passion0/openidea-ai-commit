//! Deterministic naming engine, LLM JSON candidate parser, and Quick Pick builder.

use std::collections::HashSet;
use std::fmt::Write as _;

use ai_core::{strip_reasoning_tags, unwrap_code_fence};
use openidea_plugin::PickItem;
use serde::Deserialize;

use crate::config::NamingStyle;

/// Key returned when the user selects the bottom "Copy all structured list" utility row.
pub const COPY_ALL_KEY: &str = "__copy_all__";
const PICK_KEY_PREFIX: &str = "pick:";

/// A single semantic naming candidate extracted from the model response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamingCandidate {
    /// Sanitized lowercase ASCII identifier words (for example `["user", "order", "amount"]`).
    pub tokens: Vec<String>,
    /// Concise Chinese semantic description explaining why this candidate fits.
    pub meaning_zh: String,
    /// Semantic kind hint (`"variable"`, `"type"`, `"constant"`, `"function"`, etc.).
    pub kind: String,
}

/// One formatted identifier row in the Quick Pick structured list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructuredNamingItem {
    /// Formatted identifier text ready for insertion.
    pub identifier: String,
    /// Case style applied to produce [`Self::identifier`].
    pub style: NamingStyle,
    /// 1-based semantic group index.
    pub group_index: usize,
    /// Chinese semantic explanation.
    pub meaning_zh: String,
}

/// Full structured naming output ready for Quick Pick interaction and Markdown clipboard export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructuredNamingPlan {
    /// Structured identifier variants in display order (deduplicated by identifier).
    pub items: Vec<StructuredNamingItem>,
    /// Formatted Markdown table containing all semantic groups and naming styles.
    pub markdown_summary: String,
}

impl StructuredNamingPlan {
    /// Converts items into OpenIDEA [`PickItem`] entries plus a trailing "Copy All" action.
    #[must_use]
    pub fn to_pick_items(&self) -> Vec<PickItem> {
        let mut out = Vec::with_capacity(self.items.len() + 1);
        for item in &self.items {
            let desc = if item.meaning_zh.is_empty() {
                format!(
                    "[{} {}] 方案{}",
                    item.style.zh_label(),
                    item.style.code(),
                    item.group_index
                )
            } else {
                format!(
                    "[{} {}] 方案{} · {}",
                    item.style.zh_label(),
                    item.style.code(),
                    item.group_index,
                    item.meaning_zh
                )
            };
            out.push(
                PickItem::new(
                    &format!("{PICK_KEY_PREFIX}{}", item.identifier),
                    &item.identifier,
                )
                .description(&desc),
            );
        }
        out.push(
            PickItem::new(COPY_ALL_KEY, "复制全部结构化命名列表 (Markdown)")
                .description("将所有命名风格对照表与说明复制到剪贴板，不修改当前代码"),
        );
        out
    }

    /// Extracts the chosen identifier string from a `host::pick` return key.
    #[must_use]
    pub fn identifier_for_key(key: &str) -> Option<&str> {
        key.strip_prefix(PICK_KEY_PREFIX)
    }
}

#[derive(Deserialize)]
struct ModelCandidatesEnvelope {
    #[serde(default)]
    candidates: Vec<RawModelCandidate>,
}

#[derive(Deserialize)]
struct RawModelCandidate {
    #[serde(default)]
    tokens: Vec<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    identifier: Option<String>,
    #[serde(default)]
    meaning: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    kind: Option<String>,
}

/// Parses model output into semantic [`NamingCandidate`] groups, falling back
/// to token extraction from raw lines if the response is not strict JSON.
#[must_use]
pub fn parse_model_candidates(raw_answer: &str, max_groups: usize) -> Vec<NamingCandidate> {
    let cleaned = unwrap_code_fence(&strip_reasoning_tags(raw_answer));

    let json_candidates = parse_json_candidates(&cleaned, max_groups);
    if !json_candidates.is_empty() {
        return json_candidates;
    }

    parse_fallback_lines(&cleaned, max_groups)
}

fn parse_json_candidates(cleaned: &str, max_groups: usize) -> Vec<NamingCandidate> {
    let Some(start) = cleaned.find('{') else {
        return Vec::new();
    };
    let Some(end) = cleaned.rfind('}') else {
        return Vec::new();
    };
    if end <= start {
        return Vec::new();
    }
    let Some(json_slice) = cleaned.get(start..=end) else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<ModelCandidatesEnvelope>(json_slice) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for raw in parsed.candidates.into_iter().take(max_groups) {
        let mut tokens = sanitize_tokens(&raw.tokens);
        if tokens.is_empty() {
            let fallback_name = raw
                .name
                .as_deref()
                .or(raw.identifier.as_deref())
                .unwrap_or("");
            tokens = tokenize_identifier(fallback_name);
        }
        if tokens.is_empty() {
            continue;
        }
        let meaning_zh = raw
            .meaning
            .or(raw.description)
            .map(|s| s.trim().to_owned())
            .unwrap_or_default();
        let kind = raw
            .kind
            .map_or_else(|| "variable".to_owned(), |k| k.trim().to_ascii_lowercase());
        out.push(NamingCandidate {
            tokens,
            meaning_zh,
            kind,
        });
    }
    out
}

fn parse_fallback_lines(cleaned: &str, max_groups: usize) -> Vec<NamingCandidate> {
    let mut out = Vec::new();
    for line in cleaned.lines() {
        if out.len() >= max_groups {
            break;
        }
        let trimmed = line
            .trim()
            .trim_start_matches([
                '-', '*', '•', '·', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '.', ')',
            ])
            .trim();
        if trimmed.is_empty() {
            continue;
        }
        let (candidate_part, meaning_part) = trimmed
            .split_once(" - ")
            .or_else(|| trimmed.split_once(": "))
            .or_else(|| trimmed.split_once('：'))
            .unwrap_or((trimmed, ""));
        let tokens = tokenize_identifier(candidate_part);
        if tokens.is_empty() {
            continue;
        }
        out.push(NamingCandidate {
            tokens,
            meaning_zh: meaning_part.trim().to_owned(),
            kind: "variable".to_owned(),
        });
    }
    out
}

fn sanitize_tokens(raw_tokens: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for raw in raw_tokens {
        for piece in tokenize_identifier(raw) {
            out.push(piece);
        }
    }
    ensure_valid_leading_token(&mut out);
    out
}

/// Splits any mixed-style identifier or phrase into lowercase ASCII identifier tokens.
#[must_use]
pub fn tokenize_identifier(input: &str) -> Vec<String> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut current = String::new();

    for (idx, &ch) in chars.iter().enumerate() {
        if !ch.is_ascii_alphanumeric() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current).to_ascii_lowercase());
            }
            continue;
        }

        let prev = idx.checked_sub(1).and_then(|i| chars.get(i).copied());
        let next = chars.get(idx + 1).copied();
        let at_boundary = ch.is_ascii_uppercase()
            && prev.is_some_and(|p| {
                p.is_ascii_lowercase()
                    || p.is_ascii_digit()
                    || (p.is_ascii_uppercase() && next.is_some_and(|n| n.is_ascii_lowercase()))
            });

        if at_boundary && !current.is_empty() {
            tokens.push(std::mem::take(&mut current).to_ascii_lowercase());
        }
        current.push(ch);
    }

    if !current.is_empty() {
        tokens.push(current.to_ascii_lowercase());
    }
    ensure_valid_leading_token(&mut tokens);
    tokens
}

fn ensure_valid_leading_token(tokens: &mut [String]) {
    let starts_with_digit =
        |token: &str| token.chars().next().is_some_and(|ch| ch.is_ascii_digit());

    if let Some(first_alpha) = tokens.iter().position(|t| !starts_with_digit(t)) {
        if first_alpha > 0 {
            tokens.rotate_left(first_alpha);
        }
        return;
    }

    if let Some(first) = tokens.first_mut() {
        first.insert(0, 'v');
    }
}

/// Expands semantic candidates across all enabled [`NamingStyle`]s, deduplicating
/// identical identifiers and building a Markdown reference table.
#[must_use]
pub fn build_structured_plan(
    candidates: &[NamingCandidate],
    styles: &[NamingStyle],
    original_input: &str,
) -> Option<StructuredNamingPlan> {
    let effective_styles = if styles.is_empty() {
        NamingStyle::ALL.as_slice()
    } else {
        styles
    };

    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let mut summary = format!("### 变量命名优化结果：`{}`\n\n", original_input.trim());
    summary.push_str("| 方案 | 命名风格 | 推荐标识符 | 语义说明 |\n");
    summary.push_str("| --- | --- | --- | --- |\n");

    for (idx, cand) in candidates.iter().enumerate() {
        let group_index = idx + 1;
        for &style in effective_styles {
            let identifier = style.format_tokens(&cand.tokens);
            if identifier.is_empty() || !seen.insert(identifier.clone()) {
                continue;
            }
            let _ = writeln!(
                summary,
                "| 方案{group_index} | {} ({}) | `{identifier}` | {} |",
                style.zh_label(),
                style.code(),
                if cand.meaning_zh.is_empty() {
                    "-"
                } else {
                    cand.meaning_zh.as_str()
                }
            );
            items.push(StructuredNamingItem {
                identifier,
                style,
                group_index,
                meaning_zh: cand.meaning_zh.clone(),
            });
        }
    }

    if items.is_empty() {
        return None;
    }

    Some(StructuredNamingPlan {
        items,
        markdown_summary: summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_wrapped_in_think_and_fence_and_builds_structured_list() {
        let raw = r#"<think>thinking...</think>
```json
{
  "candidates": [
    {
      "tokens": ["user", "order", "total", "amount"],
      "meaning": "用户订单总金额（语义完整）",
      "kind": "variable"
    },
    {
      "tokens": ["order", "total"],
      "meaning": "订单总额（简洁形式）",
      "kind": "variable"
    }
  ]
}
```"#;
        let candidates = parse_model_candidates(raw, 3);
        assert_eq!(candidates.len(), 2);

        let plan = build_structured_plan(&candidates, &NamingStyle::ALL, "用户订单总金额").unwrap();
        let ids: Vec<&str> = plan.items.iter().map(|i| i.identifier.as_str()).collect();
        assert_eq!(
            ids,
            [
                "userOrderTotalAmount",
                "UserOrderTotalAmount",
                "user_order_total_amount",
                "USER_ORDER_TOTAL_AMOUNT",
                "user-order-total-amount",
                "orderTotal",
                "OrderTotal",
                "order_total",
                "ORDER_TOTAL",
                "order-total",
            ]
        );

        let pick_items = plan.to_pick_items();
        assert_eq!(pick_items.len(), 11);
        assert_eq!(
            pick_items[0].description.as_deref().unwrap(),
            "[小驼峰 camelCase] 方案1 · 用户订单总金额（语义完整）"
        );
        assert_eq!(pick_items[10].key, COPY_ALL_KEY);
        assert!(
            plan.markdown_summary
                .contains("| 方案1 | 小驼峰 (camelCase) | `userOrderTotalAmount` |")
        );
    }

    #[test]
    fn tokenizes_acronyms_camel_snake_and_leading_digits() {
        assert_eq!(
            tokenize_identifier("parseHTTPResponse2Fast"),
            ["parse", "http", "response2", "fast"]
        );
        assert_eq!(
            tokenize_identifier("USER_ORDER_ID"),
            ["user", "order", "id"]
        );
        assert_eq!(tokenize_identifier("2fa_code"), ["code", "2fa"]);
        assert_eq!(
            NamingStyle::Camel.format_tokens(&tokenize_identifier("2fa_code")),
            "code2fa"
        );
        assert_eq!(
            NamingStyle::Snake.format_tokens(&tokenize_identifier("2fa_code")),
            "code_2fa"
        );
        assert_eq!(tokenize_identifier("123"), ["v123"]);
    }

    #[test]
    fn falls_back_gracefully_when_model_replies_plain_lines() {
        let raw = "1. user_account_balance: 用户账户余额\n2. accountBalance: 账户余额";
        let candidates = parse_model_candidates(raw, 3);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].tokens, ["user", "account", "balance"]);
        assert_eq!(candidates[0].meaning_zh, "用户账户余额");
    }
}
