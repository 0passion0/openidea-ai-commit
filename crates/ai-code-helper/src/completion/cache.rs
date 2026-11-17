//! Thread-local bounded LRU cache with zero-latency prefix-continuation matching.

use std::cell::RefCell;
use std::collections::VecDeque;

const MAX_CACHE_ENTRIES: usize = 64;
const SUFFIX_FINGERPRINT_BYTES: usize = 64;

#[derive(Clone, Debug)]
struct CacheEntry {
    path: String,
    line: u32,
    line_prefix: String,
    suffix_head: String,
    insert_text: String,
}

thread_local! {
    static COMPLETION_CACHE: RefCell<VecDeque<CacheEntry>> = const { RefCell::new(VecDeque::new()) };
}

/// Computes a bounded suffix fingerprint for cache key comparison.
#[must_use]
pub fn suffix_fingerprint(after: &str) -> String {
    ai_core::cut_utf8(after, SUFFIX_FINGERPRINT_BYTES).to_owned()
}

/// Looks up an exact hit or a typed-prefix continuation from recent completions.
///
/// When a previously cached suggestion was `foobar(x)` at `line_prefix = "call("`
/// and the user has typed `"foo"` so `line_prefix = "call(foo"`, this returns
/// `Some("bar(x)")` in 0 ms without issuing a network request.
#[must_use]
pub fn lookup(path: &str, line: u32, line_prefix: &str, suffix_head: &str) -> Option<String> {
    COMPLETION_CACHE.with(|cell| {
        let mut entries = cell.borrow_mut();

        if let Some(pos) = entries.iter().position(|e| {
            e.path == path
                && e.line == line
                && e.line_prefix == line_prefix
                && e.suffix_head == suffix_head
        }) {
            let entry = entries.remove(pos)?;
            let hit = entry.insert_text.clone();
            entries.push_front(entry);
            return Some(hit);
        }

        let mut best: Option<(usize, String)> = None;
        for (idx, entry) in entries.iter().enumerate() {
            if entry.path != path || entry.line != line || entry.suffix_head != suffix_head {
                continue;
            }
            if let Some(typed_delta) = line_prefix.strip_prefix(&entry.line_prefix)
                && !typed_delta.is_empty()
                && let Some(remaining) = entry.insert_text.strip_prefix(typed_delta)
                && !remaining.trim().is_empty()
            {
                let should_replace = best
                    .as_ref()
                    .is_none_or(|(_, prev): &(usize, String)| remaining.len() > prev.len());
                if should_replace {
                    best = Some((idx, remaining.to_owned()));
                }
            }
        }

        if let Some((pos, hit)) = best {
            if let Some(entry) = entries.remove(pos) {
                entries.push_front(entry);
            }
            return Some(hit);
        }

        None
    })
}

/// Returns the most relevant cached AI prediction applicable at `(path, line, line_prefix)`.
#[must_use]
pub fn latest_for_line(path: &str, line: u32, line_prefix: &str) -> Option<String> {
    COMPLETION_CACHE.with(|cell| {
        let entries = cell.borrow();
        for entry in entries.iter() {
            if entry.path != path || entry.line != line {
                continue;
            }
            if entry.line_prefix == line_prefix {
                return Some(entry.insert_text.clone());
            }
            if let Some(typed_delta) = line_prefix.strip_prefix(&entry.line_prefix)
                && let Some(remaining) = entry.insert_text.strip_prefix(typed_delta)
                && !remaining.trim().is_empty()
            {
                return Some(remaining.to_owned());
            }
        }
        None
    })
}

/// Stores a newly produced inline completion suggestion in the thread-local LRU cache.
pub fn store(path: &str, line: u32, line_prefix: &str, suffix_head: &str, insert_text: &str) {
    if insert_text.trim().is_empty() {
        return;
    }
    COMPLETION_CACHE.with(|cell| {
        let mut entries = cell.borrow_mut();
        entries.retain(|e| {
            !(e.path == path
                && e.line == line
                && e.line_prefix == line_prefix
                && e.suffix_head == suffix_head)
        });
        entries.push_front(CacheEntry {
            path: path.to_owned(),
            line,
            line_prefix: line_prefix.to_owned(),
            suffix_head: suffix_head.to_owned(),
            insert_text: insert_text.to_owned(),
        });
        while entries.len() > MAX_CACHE_ENTRIES {
            entries.pop_back();
        }
    });
}

/// Stores an inline suggestion and pre-warms `line + 1` cache entries when `insert_text`
/// starts with a newline (`\n`), enabling 0 ms lookup immediately after pressing `Enter`.
pub fn store_with_newline_continuation(
    path: &str,
    line: u32,
    line_prefix: &str,
    after_caret: &str,
    insert_text: &str,
) {
    let fp_same = suffix_fingerprint(after_caret);
    store(path, line, line_prefix, &fp_same, insert_text);

    let Some(after_nl) = insert_text
        .strip_prefix("\r\n")
        .or_else(|| insert_text.strip_prefix('\n'))
    else {
        return;
    };
    if after_nl.trim().is_empty() {
        return;
    }

    let next_line = line.saturating_add(1);
    let first_nl_line = after_nl.lines().next().unwrap_or(after_nl);
    let indent_len = first_nl_line
        .len()
        .saturating_sub(first_nl_line.trim_start().len());
    let next_indent = first_nl_line.get(..indent_len).unwrap_or("");
    let body_after_indent = after_nl.strip_prefix(next_indent).unwrap_or(after_nl);

    store(path, next_line, "", &fp_same, after_nl);
    if !next_indent.is_empty() && !body_after_indent.trim().is_empty() {
        store(path, next_line, next_indent, &fp_same, body_after_indent);
    }

    let after_next_line = after_caret
        .strip_prefix("\r\n")
        .or_else(|| after_caret.strip_prefix('\n'))
        .unwrap_or(after_caret);
    let fp_next = suffix_fingerprint(after_next_line);
    if fp_next != fp_same {
        store(path, next_line, "", &fp_next, after_nl);
        if !next_indent.is_empty() && !body_after_indent.trim().is_empty() {
            store(path, next_line, next_indent, &fp_next, body_after_indent);
        }
    }
}

/// Clears the completion cache (intended for unit tests and plugin deactivation).
#[cfg(test)]
pub fn clear() {
    COMPLETION_CACHE.with(|cell| cell.borrow_mut().clear());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_exact_hit_and_prefix_continuation() {
        clear();
        let fp = suffix_fingerprint("\nreturn x\n");
        store(
            "/repo/main.rs",
            10,
            "    let total = ",
            &fp,
            "calculate_order_total(items)",
        );

        assert_eq!(
            lookup("/repo/main.rs", 10, "    let total = ", &fp).as_deref(),
            Some("calculate_order_total(items)")
        );
        assert_eq!(
            lookup("/repo/main.rs", 10, "    let total = calc", &fp).as_deref(),
            Some("ulate_order_total(items)")
        );
        assert_eq!(
            latest_for_line("/repo/main.rs", 10, "    let total = calculate_").as_deref(),
            Some("order_total(items)")
        );
        assert!(
            lookup(
                "/repo/main.rs",
                10,
                "    let total = calculate_order_total(items)",
                &fp
            )
            .is_none()
        );
        clear();
    }

    #[test]
    fn prewarms_next_line_entries_for_newline_continuations() {
        clear();
        let after = "\n    return 0\n";
        let fp = suffix_fingerprint(after);
        store_with_newline_continuation(
            "/repo/app.py",
            2,
            "def validate(user):",
            after,
            "\n    if not user:\n        return None",
        );

        assert_eq!(
            lookup("/repo/app.py", 2, "def validate(user):", &fp).as_deref(),
            Some("\n    if not user:\n        return None")
        );
        assert_eq!(
            lookup("/repo/app.py", 3, "    ", &fp).as_deref(),
            Some("if not user:\n        return None")
        );
        assert_eq!(
            lookup("/repo/app.py", 3, "    if ", &fp).as_deref(),
            Some("not user:\n        return None")
        );
        assert_eq!(
            lookup("/repo/app.py", 3, "", &fp).as_deref(),
            Some("    if not user:\n        return None")
        );
        clear();
    }
}
