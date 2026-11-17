//! UTF-16 safe editor selection extraction, cursor word expansion, and stale-safe edits.

use ai_core::cut_utf8;
use openidea_plugin::text::{offset, position};
use openidea_plugin::{Error, Position, Range, TextEdit, TextEditor};

/// Controls how an empty selection (cursor only) is expanded into target text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmptySelectionFallback {
    /// Expand to the word/identifier under cursor; if empty, fall back to the trimmed current line.
    WordOrLine,
    /// Expand strictly to the identifier or contiguous phrase under cursor.
    WordOnly,
}

/// Result of extracting selected text and local surrounding context from an editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionTarget {
    /// Editor file path.
    pub path: String,
    /// Editor language identifier (for example `"rust"`, `"java"`, `"plaintext"`).
    pub language: String,
    /// Document version snapshot captured when reading text.
    pub doc_version: u64,
    /// Target text extracted from the primary selection (bounded to budget).
    pub primary_text: String,
    /// All target ranges in the document to replace upon confirmation.
    pub ranges: Vec<Range>,
    /// Context lines immediately preceding the primary selection.
    pub context_before: String,
    /// Context lines immediately following the primary selection.
    pub context_after: String,
    /// Whether `primary_text` was truncated to fit the byte limit.
    pub truncated: bool,
}

/// Outcome of attempting to apply an edit back to the focused editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Edits were committed in one undoable change; carries new edit count.
    Applied(usize),
    /// Selected text already matched the desired replacement.
    Unchanged,
    /// Document changed while waiting for the model; nothing was overwritten.
    Stale,
    /// Editor is read-only or lacks `editor.write` capability.
    Denied,
}

/// Extracts the target selection (or word/line under cursor) plus bounded context lines.
///
/// # Errors
///
/// Returns [`Error::Denied`] if `editor.read` is missing, or [`Error::Failed`]
/// when no non-empty text can be extracted from the selection or cursor.
pub fn extract_target(
    editor: &TextEditor,
    fallback: EmptySelectionFallback,
    context_lines: usize,
    max_selection_bytes: usize,
) -> Result<SelectionTarget, Error> {
    let doc = editor.document()?;
    let raw_selections = editor.selections()?;

    let mut ranges: Vec<Range> = raw_selections
        .iter()
        .copied()
        .filter(|r| !r.is_empty())
        .filter(|r| doc.text_in(*r).is_some_and(|s| !s.trim().is_empty()))
        .collect();

    if ranges.is_empty()
        && let Some(cursor) = raw_selections.first()
        && let Some(expanded) = expand_empty_selection(&doc.text, cursor.start, fallback)
    {
        ranges.push(expanded);
    }

    let Some(primary_range) = ranges.first().copied() else {
        return Err(Error::Failed(match fallback {
            EmptySelectionFallback::WordOrLine => {
                "请先在编辑器中选中需要翻译的文本，或将光标置于非空词句行内。".to_owned()
            }
            EmptySelectionFallback::WordOnly => {
                "请先在编辑器中选中需要优化命名的中文描述或变量名，或将光标置于目标词句内。"
                    .to_owned()
            }
        }));
    };

    let raw_primary = doc.text_in(primary_range).unwrap_or("");
    let (primary_text, truncated) = if raw_primary.len() > max_selection_bytes {
        (cut_utf8(raw_primary, max_selection_bytes).to_owned(), true)
    } else {
        (raw_primary.to_owned(), false)
    };

    let (context_before, context_after) =
        extract_surrounding_lines(&doc.text, primary_range, context_lines);

    Ok(SelectionTarget {
        path: editor.path(),
        language: editor.language(),
        doc_version: doc.version,
        primary_text,
        ranges,
        context_before,
        context_after,
        truncated,
    })
}

/// Applies `replacement` across all `ranges` at `expected_version` without clobbering concurrent edits.
///
/// # Errors
///
/// Propagates unexpected editor host errors other than [`Error::Stale`] and [`Error::Denied`].
pub fn apply_replacement(
    editor: &TextEditor,
    expected_version: u64,
    ranges: &[Range],
    replacement: &str,
) -> Result<ApplyOutcome, Error> {
    if ranges.is_empty() {
        return Ok(ApplyOutcome::Unchanged);
    }

    let doc = editor.document()?;
    if doc.version != expected_version {
        return Ok(ApplyOutcome::Stale);
    }

    let mut edits = Vec::with_capacity(ranges.len());
    for &range in ranges {
        let Some(current) = doc.text_in(range) else {
            continue;
        };
        if current != replacement {
            edits.push(TextEdit {
                range,
                text: replacement.to_owned(),
            });
        }
    }

    if edits.is_empty() {
        return Ok(ApplyOutcome::Unchanged);
    }

    edits.sort_by_key(|edit| edit.range);
    edits.dedup_by(|later, earlier| later.range.start < earlier.range.end);
    let applied_count = edits.len();

    match editor.apply_edits(expected_version, &edits) {
        Ok(_) => Ok(ApplyOutcome::Applied(applied_count)),
        Err(Error::Stale) => Ok(ApplyOutcome::Stale),
        Err(Error::Denied(_)) => Ok(ApplyOutcome::Denied),
        Err(other) => Err(other),
    }
}

fn expand_empty_selection(
    text: &str,
    cursor: Position,
    fallback: EmptySelectionFallback,
) -> Option<Range> {
    let byte_at = offset(text, cursor)?;
    if let Some(word) = expand_word_around(text, byte_at) {
        return Some(word);
    }
    if fallback == EmptySelectionFallback::WordOrLine {
        return expand_nonempty_line(text, cursor.line);
    }
    None
}

fn is_identifier_or_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '-'
}

fn expand_word_around(text: &str, at: usize) -> Option<Range> {
    let before = text.get(..at)?;
    let after = text.get(at..)?;

    let probe_at = if after.chars().next().is_some_and(is_identifier_or_word_char) {
        at
    } else if let Some((prev_idx, prev_ch)) = before.char_indices().next_back()
        && is_identifier_or_word_char(prev_ch)
    {
        prev_idx
    } else {
        return None;
    };

    let start = text
        .get(..probe_at)?
        .char_indices()
        .rev()
        .take_while(|&(_, ch)| is_identifier_or_word_char(ch))
        .last()
        .map_or(probe_at, |(idx, _)| idx);

    let end = text
        .get(probe_at..)?
        .char_indices()
        .find(|&(_, ch)| !is_identifier_or_word_char(ch))
        .map_or(text.len(), |(idx, _)| probe_at + idx);

    (start < end).then(|| Range::new(position(text, start), position(text, end)))
}

fn expand_nonempty_line(text: &str, line_idx: u32) -> Option<Range> {
    let line = text.split('\n').nth(line_idx as usize)?;
    let line_no_cr = line.strip_suffix('\r').unwrap_or(line);
    let trimmed_start = line_no_cr.len() - line_no_cr.trim_start().len();
    let trimmed_end = line_no_cr.trim_end().len();
    if trimmed_start >= trimmed_end {
        return None;
    }

    let start_utf16 = line_no_cr
        .get(..trimmed_start)?
        .chars()
        .map(|c| u32::try_from(c.len_utf16()).unwrap_or(2))
        .sum();
    let end_utf16 = line_no_cr
        .get(..trimmed_end)?
        .chars()
        .map(|c| u32::try_from(c.len_utf16()).unwrap_or(2))
        .sum();

    Some(Range::new(
        Position::new(line_idx, start_utf16),
        Position::new(line_idx, end_utf16),
    ))
}

fn extract_surrounding_lines(text: &str, range: Range, context_lines: usize) -> (String, String) {
    if context_lines == 0 {
        return (String::new(), String::new());
    }

    let lines: Vec<&str> = text.lines().collect();
    let start_line = range.start.line as usize;
    let end_line = range.end.line as usize;

    let before_start = start_line.saturating_sub(context_lines);
    let before = lines
        .get(before_start..start_line)
        .map(|slice| slice.join("\n"))
        .unwrap_or_default();

    let after_start = end_line.saturating_add(1).min(lines.len());
    let after_end = after_start.saturating_add(context_lines).min(lines.len());
    let after = lines
        .get(after_start..after_end)
        .map(|slice| slice.join("\n"))
        .unwrap_or_default();

    (before, after)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openidea_plugin::mock::MockHost;

    #[test]
    fn expands_cursor_on_chinese_and_utf16_emoji_lines() {
        let host = MockHost::install().with_capabilities(&["editor.read", "editor.write"]);
        // "😀 " is 3 UTF-16 units before "用户订单".
        host.open_editor(
            "/p/a.rs",
            "rust",
            "let prefix = 1;\n// 😀 用户订单金额 = 0;\nlet next = 2;",
            &[(1, 8, 1, 8)],
        );
        let editor = host.invocation().editor().unwrap();
        let target = extract_target(&editor, EmptySelectionFallback::WordOnly, 1, 10_000).unwrap();
        assert_eq!(target.primary_text, "用户订单金额");
        assert_eq!(target.context_before, "let prefix = 1;");
        assert_eq!(target.context_after, "let next = 2;");

        let outcome = apply_replacement(
            &editor,
            target.doc_version,
            &target.ranges,
            "user_order_amount",
        )
        .unwrap();
        assert_eq!(outcome, ApplyOutcome::Applied(1));
        assert_eq!(
            host.editor_text(),
            "let prefix = 1;\n// 😀 user_order_amount = 0;\nlet next = 2;"
        );
    }

    #[test]
    fn protects_against_stale_edits_when_document_changes_concurrently() {
        let host = MockHost::install().with_capabilities(&["editor.read", "editor.write"]);
        host.open_editor("/p/a.rs", "rust", "let x = 用户名称;", &[(0, 8, 0, 12)]);
        let editor = host.invocation().editor().unwrap();
        let target = extract_target(&editor, EmptySelectionFallback::WordOnly, 0, 10_000).unwrap();

        host.type_in_editor("\n// typed while waiting");
        let outcome =
            apply_replacement(&editor, target.doc_version, &target.ranges, "user_name").unwrap();
        assert_eq!(outcome, ApplyOutcome::Stale);
        assert!(host.editor_text().contains("用户名称"));
    }

    #[test]
    fn word_or_line_falls_back_to_comment_line_when_cursor_is_on_whitespace() {
        let host = MockHost::install().with_capabilities(&["editor.read"]);
        host.open_editor(
            "/p/a.rs",
            "rust",
            "  // Calculate total order amount  ",
            &[(0, 4, 0, 4)],
        );
        let editor = host.invocation().editor().unwrap();
        let target =
            extract_target(&editor, EmptySelectionFallback::WordOrLine, 0, 10_000).unwrap();
        assert_eq!(target.primary_text, "// Calculate total order amount");
    }
}
