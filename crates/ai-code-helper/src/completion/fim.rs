//! Fill-In-The-Middle prompt construction, overlap sanitization, and direct-stream prompt building.

use ai_core::{PromptPayload, cut_utf8, strip_reasoning_tags};

use super::align::cursor_state_rule;
pub use super::align::{CursorLineState, classify_cursor_line, sanitize_and_align_fim};

/// Cursor snapshot inputs used to build bounded FIM and streaming prompts.
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
    let line_before = ctx.before.rsplit('\n').next().unwrap_or(ctx.before);
    let state_rule = cursor_state_rule(line_before, ctx.after, ctx.language);

    let system = format!(
        "You are an ultra-precise code completion engine using Fill-In-The-Middle (FIM) inference.\n\
         Language: {}\n\
         File: {}\n\n\
         Strict rules:\n\
         1. Output ONLY the exact raw code to insert at <|CURSOR|>.\n\
         2. Do NOT wrap output in Markdown code fences (```), <think> tags, quotes, or explanations.\n\
         3. Do NOT repeat code already present immediately before <|CURSOR|> or immediately after <|CURSOR|>.\n\
         4. Follow project types, variable names, and idioms visible in <CONTEXT>, <PREFIX>, and <SUFFIX>.\n\
         5. {mode_rule}{state_rule}",
        ctx.language, ctx.path
    );

    assemble_fim_payload(system, ctx, "", max_input_bytes)
}

/// Builds a bounded prompt requesting a single high-quality completion ready for streaming into the file.
#[must_use]
pub fn build_direct_stream_prompt(
    ctx: &FimContext<'_>,
    selected_hint: &str,
    max_lines: usize,
    max_input_bytes: usize,
) -> PromptPayload {
    let line_before = ctx.before.rsplit('\n').next().unwrap_or(ctx.before);
    let state_rule = cursor_state_rule(line_before, ctx.after, ctx.language);
    let intent_rule = if selected_hint.trim().is_empty() {
        String::new()
    } else {
        "\n7. <TARGET> shows the selected intent/stub; output the complete clean replacement code only."
            .to_owned()
    };

    let system = format!(
        "You are an ultra-precise software engineer generating production-ready code at <|CURSOR|>.\n\
         Language: {}\n\
         File: {}\n\n\
         Strict rules:\n\
         1. Output ONLY the single best raw code snippet to write directly into the file at <|CURSOR|> (no multiple options).\n\
         2. Do NOT wrap output in Markdown code fences (```), <think> tags, quotes, or explanations.\n\
         3. Do NOT repeat existing code immediately before or after <|CURSOR|>.\n\
         4. Follow types, imports, style, and naming from <CONTEXT>, <PREFIX>, and <SUFFIX>.\n\
         5. Keep output concise, complete, and well-indented (at most {} lines).{state_rule}{intent_rule}",
        ctx.language,
        ctx.path,
        max_lines.max(1)
    );

    let hint_block = if selected_hint.trim().is_empty() {
        String::new()
    } else {
        format!("<TARGET>\n{selected_hint}\n</TARGET>\n")
    };
    assemble_fim_payload(system, ctx, &hint_block, max_input_bytes)
}

fn assemble_fim_payload(
    system: String,
    ctx: &FimContext<'_>,
    hint_block: &str,
    max_input_bytes: usize,
) -> PromptPayload {
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

    let overhead = prefix_open.len() + prefix_close.len() + suffix_open.len() + suffix_close.len();
    let hint_slice = cut_utf8(hint_block, budget / 6);
    let available = budget
        .saturating_sub(overhead)
        .saturating_sub(hint_slice.len());

    let context_slice = cut_utf8(&context_block, available / 4);
    let after_context = available.saturating_sub(context_slice.len());
    let suffix_slice = cut_utf8(ctx.after, after_context / 4);
    let prefix_slice = tail_utf8(ctx.before, after_context.saturating_sub(suffix_slice.len()));

    user.push_str(hint_slice);
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
    let had_leading_nl = input.starts_with('\n') || input.starts_with("\r\n");
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
        return if had_leading_nl && !single.is_empty() {
            format!("\n{single}")
        } else {
            single.to_owned()
        };
    };

    let first_line = trimmed_start.get(..first_line_end).unwrap_or("").trim();
    if !first_line.starts_with("```") {
        let body = trimmed_start.trim_end_matches(['\r', '\n']);
        return if had_leading_nl && !body.is_empty() {
            format!("\n{body}")
        } else {
            body.to_owned()
        };
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
            .trim_start_matches(['\r', '\n'])
            .lines()
            .next()
            .unwrap_or("")
            .trim_end_matches(['\r', '\n'])
            .to_owned();
    }
    if let Some(rest) = text.strip_prefix('\n') {
        let collected: Vec<&str> = rest.lines().take(max_lines).collect();
        return format!("\n{}", collected.join("\n"))
            .trim_end_matches(['\r', '\n'])
            .to_owned();
    }
    let collected: Vec<&str> = text.lines().take(max_lines).collect();
    collected
        .join("\n")
        .trim_end_matches(['\r', '\n'])
        .to_owned()
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
        let direct = build_direct_stream_prompt(&ctx, "return 0", 16, 8_000);
        assert!(direct.total_bytes() <= 8_000);
        assert!(direct.user.contains("<TARGET>"));
    }
}
