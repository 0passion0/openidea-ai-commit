//! Cursor-line state classification and indentation alignment for FIM & streamed completions.

/// Semantic classification of the caret's current line for newline and indentation alignment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CursorLineState {
    /// Caret sits at the end of a completed statement, comment, or block opener.
    EndOfComplete {
        /// Expected leading indentation on the following line.
        next_indent: String,
        /// `true` when the line ends with an indented block opener (`:`, `{`, `=>`, `then`, `do`).
        is_block_opener: bool,
    },
    /// Caret sits on an empty or whitespace-only line that already has leading indentation.
    IndentedBlank {
        /// Existing indentation on the line before the caret.
        indent: String,
    },
    /// Caret sits mid-line or precedes non-whitespace same-line tokens.
    MidLine {
        /// Leading indentation of the current line.
        base_indent: String,
        /// Auto-paired closing delimiters immediately following the caret on the same line.
        closing_tail: String,
    },
}

/// Classifies the caret's current line context across languages.
#[must_use]
pub fn classify_cursor_line(
    line_before_caret: &str,
    after_caret: &str,
    language: &str,
) -> CursorLineState {
    let base_indent = leading_ws(line_before_caret).to_owned();
    let same_line_after = after_caret.split('\n').next().unwrap_or("");
    let trimmed_after = same_line_after.trim();
    let is_closing = !trimmed_after.is_empty()
        && trimmed_after
            .chars()
            .all(|ch| matches!(ch, ')' | ']' | '}' | ';' | ',' | '"' | '\'' | '`'));
    let closing_tail = if is_closing {
        trimmed_after.to_owned()
    } else {
        String::new()
    };

    if line_before_caret.trim().is_empty() && trimmed_after.is_empty() {
        return CursorLineState::IndentedBlank {
            indent: base_indent,
        };
    }

    if trimmed_after.is_empty() {
        let trimmed_before = line_before_caret.trim_end();
        let code_before = trimmed_before.trim_start();
        let lang = language.to_ascii_lowercase();
        let is_opener = is_block_opener_line(code_before, &lang);
        let is_complete = is_opener
            || trimmed_before.ends_with(';')
            || is_comment_only_line(code_before)
            || code_before.starts_with('@')
            || super::context::is_import_line(code_before);
        if is_complete {
            let next_indent = if is_opener {
                format!("{base_indent}{}", indent_step(&lang, &base_indent))
            } else {
                base_indent
            };
            return CursorLineState::EndOfComplete {
                next_indent,
                is_block_opener: is_opener,
            };
        }
    }

    CursorLineState::MidLine {
        base_indent,
        closing_tail,
    }
}

pub(super) fn cursor_state_rule(line_before: &str, after: &str, language: &str) -> String {
    match classify_cursor_line(line_before, after, language) {
        CursorLineState::EndOfComplete {
            next_indent,
            is_block_opener: true,
        } => format!("\n6. Caret ends a block opener; begin your output with '\\n{next_indent}'."),
        CursorLineState::EndOfComplete {
            next_indent,
            is_block_opener: false,
        } => format!(
            "\n6. Caret is at the end of a completed statement/comment; if continuing on a new line, begin with '\\n{next_indent}'."
        ),
        CursorLineState::IndentedBlank { indent } => format!(
            "\n6. Caret is already indented by {} whitespace chars; do NOT repeat that leading indent on line 1, and indent lines 2+ from \"{indent}\".",
            indent.len()
        ),
        CursorLineState::MidLine { closing_tail, .. } if !closing_tail.is_empty() => format!(
            "\n6. Auto-closed `{closing_tail}` follows the caret; avoid unbalanced closing delimiters."
        ),
        CursorLineState::MidLine { .. } => String::new(),
    }
}

/// Sanitizes model output and aligns leading newlines and indentation to [`classify_cursor_line`].
#[must_use]
pub fn sanitize_and_align_fim(
    raw: &str,
    line_before_caret: &str,
    after_caret: &str,
    language: &str,
    max_lines: usize,
) -> String {
    let cleaned = super::fim::sanitize_fim_output(raw, line_before_caret, after_caret, max_lines);
    if cleaned.trim().is_empty() || max_lines <= 1 {
        return cleaned;
    }
    let same_line_after = after_caret.split('\n').next().unwrap_or("").trim();
    match classify_cursor_line(line_before_caret, after_caret, language) {
        CursorLineState::EndOfComplete {
            next_indent,
            is_block_opener,
        } if same_line_after.is_empty()
            && (is_block_opener || is_comment_only_line(line_before_caret.trim())) =>
        {
            if let Some(after_nl) = cleaned.strip_prefix('\n') {
                format!("\n{}", indent_block_lines(after_nl, &next_indent, false))
            } else if starts_with_continuation_token(&cleaned) {
                cleaned
            } else {
                format!("\n{}", indent_block_lines(&cleaned, &next_indent, true))
            }
        }
        CursorLineState::EndOfComplete { next_indent, .. } if same_line_after.is_empty() => {
            if let Some(after_nl) = cleaned.strip_prefix('\n') {
                format!("\n{}", indent_block_lines(after_nl, &next_indent, false))
            } else {
                cleaned
            }
        }
        CursorLineState::IndentedBlank { indent } => {
            let trimmed = cleaned.trim_start_matches(['\r', '\n']);
            let line1_dedup = trimmed.strip_prefix(&indent).unwrap_or(trimmed);
            indent_tail_lines(line1_dedup, &indent)
        }
        CursorLineState::MidLine { base_indent, .. } => {
            let Some(after_nl) = cleaned.strip_prefix('\n') else {
                return cleaned;
            };
            let trimmed_before = line_before_caret.trim_end();
            let allows_nl = trimmed_before.ends_with('(')
                || trimmed_before.ends_with('[')
                || trimmed_before.ends_with(',');
            let next_indent_len = leading_ws(after_nl.lines().next().unwrap_or("")).len();
            if allows_nl && next_indent_len > base_indent.len() {
                cleaned
            } else {
                after_nl.to_owned()
            }
        }
        CursorLineState::EndOfComplete { .. } => cleaned,
    }
}

fn leading_ws(line: &str) -> &str {
    let trimmed = line.trim_start();
    line.get(..line.len().saturating_sub(trimmed.len()))
        .unwrap_or("")
}

fn indent_step(lang: &str, base_indent: &str) -> &'static str {
    if base_indent.contains('\t') {
        "\t"
    } else if matches!(lang, "yaml" | "yml")
        || (!base_indent.is_empty() && base_indent.len() % 4 == 2)
    {
        "  "
    } else {
        "    "
    }
}

fn is_block_opener_line(code_before: &str, lang: &str) -> bool {
    if code_before.is_empty() || is_comment_only_line(code_before) {
        return false;
    }
    if matches!(lang, "python" | "py" | "yaml" | "yml") {
        return code_before.ends_with(':') && delimiters_balanced(code_before);
    }
    code_before.ends_with('{')
        || (code_before.ends_with("=>") && delimiters_balanced(code_before))
        || code_before == "then"
        || code_before.ends_with(" then")
        || code_before == "do"
        || code_before.ends_with(" do")
}

fn delimiters_balanced(text: &str) -> bool {
    let mut parens: i32 = 0;
    let mut brackets: i32 = 0;
    let mut braces: i32 = 0;
    for ch in text.chars() {
        match ch {
            '(' => parens += 1,
            ')' => parens = parens.saturating_sub(1),
            '[' => brackets += 1,
            ']' => brackets = brackets.saturating_sub(1),
            '{' => braces += 1,
            '}' => braces = braces.saturating_sub(1),
            _ => {}
        }
    }
    parens == 0 && brackets == 0 && braces == 0
}

fn is_comment_only_line(code_before: &str) -> bool {
    code_before.starts_with("//")
        || code_before.starts_with('#')
        || code_before.ends_with("*/")
        || code_before == "\"\"\""
        || (code_before.len() >= 6
            && code_before.starts_with("\"\"\"")
            && code_before.ends_with("\"\"\""))
}

fn starts_with_continuation_token(text: &str) -> bool {
    let trimmed = text.trim_start();
    [".", ",", ";", ")", "]", "}", "->", "where", "else", "{"]
        .iter()
        .any(|tok| trimmed.starts_with(tok))
}

fn indent_block_lines(body: &str, indent: &str, force_first: bool) -> String {
    if indent.is_empty() {
        return body.to_owned();
    }
    let min_indent = body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| leading_ws(l).len())
        .min()
        .unwrap_or(usize::MAX);
    if min_indent >= indent.len() && !force_first {
        return body.to_owned();
    }
    let shift_all = min_indent < indent.len();
    body.lines()
        .enumerate()
        .map(|(idx, l)| {
            if l.trim().is_empty() {
                String::new()
            } else if shift_all || (idx == 0 && leading_ws(l).is_empty()) {
                format!("{indent}{l}")
            } else {
                l.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn indent_tail_lines(body: &str, indent: &str) -> String {
    if indent.is_empty() || !body.contains('\n') {
        return body.to_owned();
    }
    let mut lines = body.lines();
    let Some(first) = lines.next() else {
        return String::new();
    };
    let tail: Vec<&str> = lines.collect();
    let min_tail_indent = tail
        .iter()
        .copied()
        .filter(|l| !l.trim().is_empty())
        .map(|l| leading_ws(l).len())
        .min()
        .unwrap_or(usize::MAX);
    let shift_tail = min_tail_indent < indent.len();
    let mut out = vec![first.to_owned()];
    for line in tail {
        if !line.trim().is_empty() && shift_tail {
            out.push(format!("{indent}{line}"));
        } else {
            out.push(line.to_owned());
        }
    }
    out.join("\n")
}

pub(super) fn utf16_len(s: &str) -> u32 {
    u32::try_from(s.encode_utf16().count()).unwrap_or(u32::MAX)
}

pub(super) fn end_position_after_insert(
    start: openidea_plugin::Position,
    inserted: &str,
) -> openidea_plugin::Position {
    let mut parts = inserted.split('\n');
    let mut last_seg = parts.next().unwrap_or("");
    let mut extra_lines: u32 = 0;
    for seg in parts {
        extra_lines = extra_lines.saturating_add(1);
        last_seg = seg;
    }
    let col = utf16_len(last_seg.trim_end_matches('\r'));
    if extra_lines == 0 {
        openidea_plugin::Position::new(start.line, start.character.saturating_add(col))
    } else {
        openidea_plugin::Position::new(start.line.saturating_add(extra_lines), col)
    }
}

pub(super) fn split_into_progressive_chunks(code: &str, lines_per_chunk: usize) -> Vec<String> {
    if code.is_empty() {
        return Vec::new();
    }
    let mut ends: Vec<usize> = code.match_indices('\n').map(|(idx, _)| idx).collect();
    if code.starts_with('\n') && ends.first() == Some(&0) {
        ends.remove(0);
    }
    if ends.last().copied() != Some(code.len()) {
        ends.push(code.len());
    }

    let step = lines_per_chunk.max(1).max(ends.len().max(1).div_ceil(10));
    let mut chunks = Vec::new();
    let mut idx = step.saturating_sub(1);
    while let Some(&end) = ends.get(idx) {
        if end > 0
            && let Some(slice) = code.get(..end)
            && !slice.trim().is_empty()
        {
            chunks.push(slice.to_owned());
        }
        idx = idx.saturating_add(step);
    }
    if chunks.last().map(String::as_str) != Some(code) {
        chunks.push(code.to_owned());
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_newline_and_aligns_indentation_across_cursor_states() {
        let eol = sanitize_and_align_fim("return 42", "def answer():", "\n", "python", 16);
        assert_eq!(eol, "\n    return 42");

        let eol_raw_nl = sanitize_and_align_fim(
            "\n    if True:\n        return 1",
            "def f():",
            "",
            "python",
            16,
        );
        assert_eq!(eol_raw_nl, "\n    if True:\n        return 1");

        let comment = sanitize_and_align_fim("total = sum(items)", "    # sum", "\n", "python", 16);
        assert_eq!(comment, "\n    total = sum(items)");

        let blank = sanitize_and_align_fim("    x = 1\nreturn x", "    ", "\n", "python", 16);
        assert_eq!(blank, "x = 1\n    return x");

        let mid = sanitize_and_align_fim(
            "<think>ok</think>\ncalculate(items)",
            "    let total = ",
            "\n}\n",
            "rust",
            16,
        );
        assert_eq!(mid, "calculate(items)");
    }
}
