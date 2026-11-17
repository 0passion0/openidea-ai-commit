//! Fill-In-The-Middle prompt construction, overlap sanitization, and smart multi-candidate parsing.

use ai_core::{PromptPayload, cut_utf8, strip_reasoning_tags};
use serde::Deserialize;

/// Cursor snapshot inputs used to build bounded FIM and candidate prompts.
#[derive(Clone, Copy, Debug)]
pub struct FimContext<'a> {
    /// File path of the active document.
    pub path: &'a str,
    /// Editor language identifier.
    pub language: &'a str,
    /// Source slice preceding the cursor or selection.
    pub before: &'a str,
    /// Source slice following the cursor or selection.
    pub after: &'a str,
    /// Extracted enclosing scopes, imports, definitions, and workspace symbols.
    pub context: &'a str,
}

/// Completion generation mode derived from cursor line context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FimMode {
    /// Cursor precedes meaningful inline tokens on the same line; restrict to one line.
    SingleLine,
    /// Cursor is at end-of-line or before only closing delimiters; allow up to `max_lines`.
    MultiLine {
        /// Maximum permitted output line count.
        max_lines: usize,
    },
}

/// Inspects `after_caret` to decide whether single-line or multi-line FIM completion fits best.
#[must_use]
pub fn detect_fim_mode(after_caret: &str, max_lines: usize) -> FimMode {
    let rest_of_line = after_caret.split('\n').next().unwrap_or("");
    let meaningful = rest_of_line
        .trim()
        .trim_matches(|ch| matches!(ch, ')' | ']' | '}' | ';' | ',' | '"' | '\'' | '`'));
    if meaningful.is_empty() {
        FimMode::MultiLine {
            max_lines: max_lines.max(1),
        }
    } else {
        FimMode::SingleLine
    }
}

/// Builds a strictly bounded Fill-In-The-Middle prompt for OpenAI-compatible chat models.
#[must_use]
pub fn build_fim_prompt(
    ctx: &FimContext<'_>,
    mode: FimMode,
    max_input_bytes: usize,
) -> PromptPayload {
    let mode_rule = match mode {
        FimMode::SingleLine => "Output at most ONE line with NO newline characters to finish the current expression or statement.".to_owned(),
        FimMode::MultiLine { max_lines } => format!(
            "Output the exact next logical statement or block (at most {max_lines} lines), matching the active indentation."
        ),
    };

    let system = format!(
        "You are an ultra-precise code completion engine using Fill-In-The-Middle (FIM) inference.\n\
         Language: {}\n\
         File: {}\n\n\
         Strict rules:\n\
         1. Output ONLY the exact raw code to insert at <|CURSOR|>.\n\
         2. Do NOT wrap output in Markdown code fences (```), <think> tags, quotes, or explanations.\n\
         3. Do NOT repeat code already present immediately before <|CURSOR|> or immediately after <|CURSOR|>.\n\
         4. Follow project types, variable names, and idioms visible in <CONTEXT>, <PREFIX>, and <SUFFIX>.\n\
         5. {mode_rule}",
        ctx.language, ctx.path
    );

    let budget = max_input_bytes.saturating_sub(system.len());
    let mut user = String::with_capacity(budget.min(8_192));

    let context_block = if ctx.context.trim().is_empty() {
        String::new()
    } else {
        format!("<CONTEXT>\n{}</CONTEXT>\n", ctx.context)
    };
    let prefix_open = "<PREFIX>\n";
    let prefix_close = "<|CURSOR|></PREFIX>\n";
    let suffix_open = "<SUFFIX>\n";
    let suffix_close = "\n</SUFFIX>";

    let fixed_overhead =
        prefix_open.len() + prefix_close.len() + suffix_open.len() + suffix_close.len();
    let available_body = budget.saturating_sub(fixed_overhead);

    let context_slice = cut_utf8(&context_block, available_body / 4);
    let after_context = available_body.saturating_sub(context_slice.len());
    let suffix_slice = cut_utf8(ctx.after, after_context / 4);
    let prefix_budget = after_context.saturating_sub(suffix_slice.len());
    let prefix_slice = tail_utf8(ctx.before, prefix_budget);

    user.push_str(context_slice);
    user.push_str(prefix_open);
    user.push_str(prefix_slice);
    user.push_str(prefix_close);
    user.push_str(suffix_open);
    user.push_str(suffix_slice);
    user.push_str(suffix_close);

    PromptPayload { system, user }
}

fn tail_utf8(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut start = text.len().saturating_sub(max_bytes);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    text.get(start..).unwrap_or("")
}

/// Cleans reasoning blocks/fences, removes prefix & suffix overlaps, and enforces `max_lines`.
#[must_use]
pub fn sanitize_fim_output(
    raw: &str,
    line_before_caret: &str,
    after_caret: &str,
    max_lines: usize,
) -> String {
    let no_think = strip_reasoning_tags(raw);
    let mut text = unwrap_fence_preserving_indent(&no_think);
    for marker in [
        "<|CURSOR|>",
        "<|fim_middle|>",
        "<|fim_prefix|>",
        "<|fim_suffix|>",
    ] {
        text = text.replace(marker, "");
    }

    text = strip_prefix_overlap(&text, line_before_caret);
    text = strip_suffix_overlap(&text, after_caret);
    clamp_lines(&text, max_lines.max(1))
}

fn unwrap_fence_preserving_indent(input: &str) -> String {
    let trimmed_start = input.trim_start_matches(['\n', '\r']);
    let Some(first_line_end) = trimmed_start.find('\n') else {
        let single = trimmed_start.trim_end();
        if single.starts_with("```") && single.ends_with("```") && single.len() > 6 {
            return single
                .strip_prefix("```")
                .and_then(|s| s.strip_suffix("```"))
                .unwrap_or("")
                .to_owned();
        }
        return single.to_owned();
    };

    let first_line = trimmed_start.get(..first_line_end).unwrap_or("").trim();
    if !first_line.starts_with("```") {
        return trimmed_start.trim_end_matches(['\r', '\n']).to_owned();
    }

    let body = trimmed_start.get(first_line_end + 1..).unwrap_or("");
    let body_no_trailing_nl = body.trim_end_matches(['\r', '\n']);
    if let Some(last_nl) = body_no_trailing_nl.rfind('\n') {
        let last_line = body_no_trailing_nl.get(last_nl + 1..).unwrap_or("").trim();
        if last_line.starts_with("```") {
            return body_no_trailing_nl.get(..last_nl).unwrap_or("").to_owned();
        }
    } else if body_no_trailing_nl.trim().starts_with("```") {
        return String::new();
    }
    body_no_trailing_nl.to_owned()
}

fn strip_prefix_overlap(text: &str, line_before_caret: &str) -> String {
    if text.is_empty() || line_before_caret.is_empty() {
        return text.to_owned();
    }
    if let Some(rest) = text.strip_prefix(line_before_caret) {
        return rest.to_owned();
    }
    let trimmed_before = line_before_caret.trim_start();
    if !trimmed_before.is_empty()
        && let Some(rest) = text.strip_prefix(trimmed_before)
    {
        return rest.to_owned();
    }

    for (idx, _) in line_before_caret.char_indices() {
        if idx == 0 {
            continue;
        }
        let prev_char = line_before_caret
            .get(..idx)
            .and_then(|s| s.chars().next_back());
        let at_boundary = prev_char.is_some_and(|ch| {
            ch.is_whitespace() || matches!(ch, '(' | '[' | '{' | ',' | ':' | '=' | ';')
        });
        if !at_boundary {
            continue;
        }
        let suffix_part = line_before_caret.get(idx..).unwrap_or("");
        if suffix_part.trim().len() >= 2
            && let Some(rest) = text.strip_prefix(suffix_part)
        {
            return rest.to_owned();
        }
    }

    text.to_owned()
}

fn strip_suffix_overlap(text: &str, after_caret: &str) -> String {
    if text.is_empty() || after_caret.is_empty() {
        return text.to_owned();
    }
    let mut out = text.to_owned();
    let same_line_after = after_caret.split('\n').next().unwrap_or("").trim_end();
    if !same_line_after.is_empty() && out.ends_with(same_line_after) {
        let stripped = out.strip_suffix(same_line_after).unwrap_or(&out);
        let safe_to_strip = matches!(same_line_after, ";" | "," | ":")
            || same_line_after.chars().any(char::is_alphanumeric)
            || has_surplus_closers(&out, same_line_after);
        if safe_to_strip {
            out = stripped.to_owned();
        }
    }

    if let Some(next_nonempty_after) = after_caret
        .lines()
        .map(str::trim_end)
        .find(|line| !line.trim().is_empty())
        && out.contains('\n')
        && let Some((head, last_line)) = out.rsplit_once('\n')
        && last_line.trim_end() == next_nonempty_after
    {
        out = head.to_owned();
    }
    out
}

fn has_surplus_closers(text: &str, suffix: &str) -> bool {
    let balance = |open: char, close: char| -> i32 {
        let opens = text.chars().filter(|&c| c == open).count();
        let closes = text.chars().filter(|&c| c == close).count();
        i32::try_from(closes).unwrap_or(i32::MAX) - i32::try_from(opens).unwrap_or(i32::MAX)
    };
    (suffix.contains(')') && balance('(', ')') > 0)
        || (suffix.contains(']') && balance('[', ']') > 0)
        || (suffix.contains('}') && balance('{', '}') > 0)
}

fn clamp_lines(text: &str, max_lines: usize) -> String {
    if max_lines == 1 {
        return text
            .lines()
            .next()
            .unwrap_or("")
            .trim_end_matches(['\r', '\n'])
            .to_owned();
    }
    let collected: Vec<&str> = text.lines().take(max_lines).collect();
    collected
        .join("\n")
        .trim_end_matches(['\r', '\n'])
        .to_owned()
}

/// Interactive multi-candidate suggestion for the `smart-complete` Quick Pick command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmartCandidate {
    /// Concise summary displayed in Quick Pick (for example `1. Concise expression`).
    pub label: String,
    /// Short explanation of trade-offs or behavior.
    pub description: String,
    /// Code snippet ready to insert at the caret or replace the selection.
    pub code: String,
}

#[derive(Deserialize)]
struct SmartEnvelope {
    #[serde(default)]
    candidates: Vec<RawSmartCandidate>,
}

#[derive(Deserialize)]
struct RawSmartCandidate {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    code: String,
}

/// Builds a bounded prompt requesting 3 distinct smart code completion candidates as JSON.
#[must_use]
pub fn build_smart_candidates_prompt(
    ctx: &FimContext<'_>,
    selected_hint: &str,
    max_input_bytes: usize,
) -> PromptPayload {
    let system = format!(
        "You are an expert software architect providing Cursor-quality code completion candidates.\n\
         Language: {}\n\
         File: {}\n\n\
         Reply ONLY with a single JSON object matching this exact schema:\n\
         {{\"candidates\":[{{\"label\":\"short title\",\"description\":\"concise Chinese explanation\",\"code\":\"exact raw code to insert\"}}]}}\n\n\
         Rules:\n\
         1. Provide up to 3 distinct, high-value candidates ranked from most concise/idiomatic to complete implementation with edge-case handling.\n\
         2. Match project types, indentation, naming, and style seen in <CONTEXT>, <PREFIX>, and <SUFFIX>.\n\
         3. Do NOT repeat existing prefix/suffix code inside `code`.\n\
         4. Output strict JSON only with NO <think> tags or markdown fences outside the JSON strings.",
        ctx.language, ctx.path
    );

    let budget = max_input_bytes.saturating_sub(system.len());
    let mut user = String::with_capacity(budget.min(8_192));

    let hint_block = if selected_hint.trim().is_empty() {
        String::new()
    } else {
        format!("<INTENT>\n{selected_hint}\n</INTENT>\n")
    };
    let context_block = if ctx.context.trim().is_empty() {
        String::new()
    } else {
        format!("<CONTEXT>\n{}</CONTEXT>\n", ctx.context)
    };
    let prefix_open = "<PREFIX>\n";
    let prefix_close = "<|CURSOR|></PREFIX>\n";
    let suffix_open = "<SUFFIX>\n";
    let suffix_close = "\n</SUFFIX>";

    let overhead = hint_block.len()
        + prefix_open.len()
        + prefix_close.len()
        + suffix_open.len()
        + suffix_close.len();
    let available = budget.saturating_sub(overhead);
    let ctx_slice = cut_utf8(&context_block, available / 4);
    let rem = available.saturating_sub(ctx_slice.len());
    let suffix_slice = cut_utf8(ctx.after, rem / 4);
    let prefix_slice = tail_utf8(ctx.before, rem.saturating_sub(suffix_slice.len()));

    user.push_str(cut_utf8(&hint_block, budget));
    user.push_str(ctx_slice);
    user.push_str(prefix_open);
    user.push_str(prefix_slice);
    user.push_str(prefix_close);
    user.push_str(suffix_open);
    user.push_str(suffix_slice);
    user.push_str(suffix_close);

    PromptPayload { system, user }
}

/// Parses structured completion candidates from JSON or falls back to raw single-code sanitization.
#[must_use]
pub fn parse_smart_candidates(
    raw: &str,
    line_before_caret: &str,
    after_caret: &str,
    max_lines: usize,
) -> Vec<SmartCandidate> {
    let cleaned = strip_reasoning_tags(raw);
    let json_text = unwrap_json_body(&cleaned);

    if let Ok(env) = serde_json::from_str::<SmartEnvelope>(&json_text) {
        let mut out = Vec::new();
        for (idx, item) in env.candidates.into_iter().take(5).enumerate() {
            let code = sanitize_fim_output(&item.code, line_before_caret, after_caret, max_lines);
            if code.trim().is_empty() {
                continue;
            }
            let label = item
                .label
                .or(item.title)
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("方案 {}", idx + 1));
            let description = item
                .description
                .map_or_else(|| "AI 推荐补全代码".to_owned(), |s| s.trim().to_owned());
            out.push(SmartCandidate {
                label,
                description,
                code,
            });
        }
        if !out.is_empty() {
            return out;
        }
    }

    let fallback = sanitize_fim_output(raw, line_before_caret, after_caret, max_lines);
    if fallback.trim().is_empty() {
        return Vec::new();
    }
    vec![SmartCandidate {
        label: "推荐补全".to_owned(),
        description: "模型直接输出的代码补全".to_owned(),
        code: fallback,
    }]
}

fn unwrap_json_body(cleaned: &str) -> String {
    let trimmed = cleaned.trim();
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && start < end
    {
        return trimmed.get(start..=end).unwrap_or(trimmed).to_owned();
    }
    unwrap_fence_preserving_indent(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_fences_think_tags_and_prefix_suffix_overlaps() {
        let raw = "<think>analyze</think>\n```python\ndf.dropna(subset=[\"id\"]);\n```";
        let result = sanitize_fim_output(raw, "    df.", ";\nreturn df", 16);
        assert_eq!(result, "dropna(subset=[\"id\"])");

        let balanced = sanitize_fim_output("get_name()", "print(user.", ")\n", 16);
        assert_eq!(balanced, "get_name()");

        let surplus = sanitize_fim_output("get_name())", "print(user.", ")\n", 16);
        assert_eq!(surplus, "get_name()");

        let raw_indent = "```python\n    total = 0\n    return total\n```";
        let keep_indent = sanitize_fim_output(raw_indent, "", "\n", 16);
        assert_eq!(keep_indent, "    total = 0\n    return total");

        let dedup_indent = sanitize_fim_output(raw_indent, "    ", "\n", 16);
        assert_eq!(dedup_indent, "total = 0\n    return total");
    }

    #[test]
    fn detects_single_line_versus_multiline_and_bounds_prompt() {
        assert_eq!(detect_fim_mode(" + tax)\n", 16), FimMode::SingleLine);
        assert_eq!(
            detect_fim_mode(")\n    return x", 16),
            FimMode::MultiLine { max_lines: 16 }
        );

        let huge_before = "x = 1\n".repeat(10_000);
        let huge_after = "y = 2\n".repeat(10_000);
        let ctx = FimContext {
            path: "/repo/a.py",
            language: "python",
            before: &huge_before,
            after: &huge_after,
            context: "import os\n",
        };
        let payload = build_fim_prompt(&ctx, FimMode::MultiLine { max_lines: 16 }, 8_000);
        assert!(payload.total_bytes() <= 8_000);
        assert!(payload.user.contains("<PREFIX>"));
        assert!(payload.user.contains("<SUFFIX>"));
    }

    #[test]
    fn parses_smart_candidates_json_and_fallback() {
        let raw = "<think>x</think>{\"candidates\":[{\"label\":\"简洁写法\",\"description\":\"直接返回求和\",\"code\":\"sum(items)\"}]}";
        let list = parse_smart_candidates(raw, "return ", "\n", 16);
        assert_eq!(list.len(), 1);
        assert_eq!(list.first().map(|c| c.code.as_str()), Some("sum(items)"));
    }
}
