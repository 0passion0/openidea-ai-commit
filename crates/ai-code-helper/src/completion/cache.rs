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
}
