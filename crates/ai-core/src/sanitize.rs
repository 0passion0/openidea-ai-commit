//! Shared UTF-8 truncation and LLM response text sanitization helpers.

/// Truncates `text` to at most `max_bytes` on a valid UTF-8 character boundary.
#[must_use]
pub fn cut_utf8(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or("")
}

/// Removes `<think>...</think>` and `<thought>...</thought>` reasoning blocks
/// and normalizes CRLF newlines to LF.
#[must_use]
pub fn strip_reasoning_tags(input: &str) -> String {
    let normalized = input.replace("\r\n", "\n");
    let no_think = strip_tag_block(&normalized, "<think>", "</think>");
    strip_tag_block(&no_think, "<thought>", "</thought>")
}

fn strip_tag_block(input: &str, open_tag: &str, close_tag: &str) -> String {
    let mut remaining = input;
    let mut out = String::with_capacity(input.len());

    while let Some(start) = remaining.find(open_tag) {
        let (before, after_start) = remaining.split_at(start);
        out.push_str(before);
        if let Some(end) = after_start.find(close_tag) {
            let after_end = end + close_tag.len();
            remaining = after_start.get(after_end..).unwrap_or("");
        } else {
            return out;
        }
    }
    out.push_str(remaining);
    out
}

/// Unwraps a single outer Markdown code fence (` ```...``` `) when the entire
/// text is enclosed inside it.
#[must_use]
pub fn unwrap_code_fence(input: &str) -> String {
    let trimmed = input.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_owned();
    }
    let mut lines: Vec<&str> = trimmed.lines().collect();
    if !lines.is_empty() {
        lines.remove(0);
    }
    if lines
        .last()
        .is_some_and(|line| line.trim_start().starts_with("```"))
    {
        lines.pop();
    }
    lines.join("\n").trim().to_owned()
}

/// Strips matching outer quotes (`"`, `'`, `` ` ``) when the inner text does
/// not contain the same quote character.
#[must_use]
pub fn unwrap_outer_quotes(input: &str) -> String {
    let mut text = input.trim().to_owned();
    for quote in ['"', '\'', '`'] {
        if let Some(inner) = text.strip_prefix(quote).and_then(|s| s.strip_suffix(quote))
            && !inner.contains(quote)
        {
            text = inner.trim().to_owned();
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_multibyte_utf8_without_splitting_characters() {
        let sample = "中文😀abc";
        assert_eq!(cut_utf8(sample, 100), sample);
        assert_eq!(cut_utf8(sample, 4), "中");
        assert_eq!(cut_utf8(sample, 6), "中文");
        assert_eq!(cut_utf8(sample, 8), "中文");
        assert_eq!(cut_utf8(sample, 10), "中文😀");
    }

    #[test]
    fn strips_reasoning_blocks_and_code_fences() {
        let raw = "<think>internal plan</think>\r\n```json\r\n{\"a\": 1}\r\n```\r\n";
        let stripped = strip_reasoning_tags(raw);
        assert_eq!(unwrap_code_fence(&stripped), "{\"a\": 1}");
        assert_eq!(unwrap_outer_quotes("`hello`"), "hello");
    }
}
