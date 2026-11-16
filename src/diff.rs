//! Git diff normalization, noise classification, compaction, and fair budgeting.

use std::fmt::Write as _;

use openidea_plugin::CommitChange;

const MAX_COMPACT_LINE_BYTES: usize = 420;
const LARGE_DELETED_LINE_THRESHOLD: usize = 24;
const MIN_DIFF_SLICE_BYTES: usize = 48;
const TRUNCATED_MARKER: &str = "\n... [truncated]";

/// Semantic priority used when a large diff must be compacted or truncated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileTier {
    /// Primary source files, manifests, schemas, and config files.
    Source = 0,
    /// Documentation and test suites.
    DocsAndTests = 1,
    /// Deleted files whose paths already communicate most of the intent.
    Deleted = 2,
    /// Lockfiles, minified bundles, generated artifacts, and binary files.
    LowSignal = 3,
}

impl FileTier {
    /// Ordered priority tiers from highest to lowest semantic signal.
    pub const ORDER: [Self; 4] = [
        Self::Source,
        Self::DocsAndTests,
        Self::Deleted,
        Self::LowSignal,
    ];
}

/// Line-level diff statistics for a single file change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileStats {
    /// Count of added (`+`) lines.
    pub added: usize,
    /// Count of deleted (`-`) lines.
    pub deleted: usize,
    /// Count of unified diff hunks (`@@`).
    pub hunks: usize,
}

/// Rendered diff payload plus truncation telemetry for the prompt footer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OptimizedDiff {
    /// Formatted diff text guaranteed to be `<= max_bytes`.
    pub text: String,
    /// Number of files whose diffs were partially cut.
    pub truncated_files: usize,
    /// Number of files whose diffs were omitted to preserve higher-priority files.
    pub omitted_files: usize,
    /// Whether the IDE host already truncated raw diffs at its 1 MiB cap.
    pub host_truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContentKind {
    Text,
    Binary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostTruncation {
    Complete,
    Truncated,
}

#[derive(Clone, Debug)]
struct PreparedChange {
    index: usize,
    rel_path: String,
    header: String,
    tier: FileTier,
    stats: FileStats,
    tier1_body: String,
    content_kind: ContentKind,
    host_truncation: HostTruncation,
}

#[derive(Clone, Debug)]
enum RenderedItem {
    Full(String),
    Truncated { header: String, body: String },
    Omitted(String),
}

/// Converts a repository-scoped path to a clean forward-slash relative path.
#[must_use]
pub fn relative_path(repo: &str, path: &str) -> String {
    let repo_trimmed = repo.trim_end_matches(['/', '\\']);
    let rel = path
        .strip_prefix(repo_trimmed)
        .map_or(path, |rest| rest.trim_start_matches(['/', '\\']));
    rel.replace('\\', "/")
}

/// Maps an OpenIDEA VCS status string to its single-letter git status code.
#[must_use]
pub fn status_code(status: &str) -> char {
    match status {
        "added" => 'A',
        "modified" => 'M',
        "deleted" => 'D',
        "renamed" => 'R',
        "copied" => 'C',
        "type-changed" => 'T',
        _ => '?',
    }
}

/// Classifies a changed file into its prioritization tier.
#[must_use]
pub fn classify_tier(rel_path: &str, status: &str, is_binary: bool) -> FileTier {
    if is_binary || is_low_signal_path(rel_path) {
        return FileTier::LowSignal;
    }
    if status == "deleted" {
        return FileTier::Deleted;
    }
    if is_docs_or_tests_path(rel_path) {
        return FileTier::DocsAndTests;
    }
    FileTier::Source
}

fn is_low_signal_path(rel_path: &str) -> bool {
    let lower = rel_path.to_ascii_lowercase();
    let file_name = lower.rsplit('/').next().unwrap_or(lower.as_str());

    if matches!(
        file_name,
        "cargo.lock"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "bun.lock"
            | "bun.lockb"
            | "poetry.lock"
            | "pipfile.lock"
            | "uv.lock"
            | "gemfile.lock"
            | "composer.lock"
            | "go.sum"
            | "flake.lock"
            | "mix.lock"
            | "pubspec.lock"
            | "package.resolved"
    ) {
        return true;
    }

    [
        ".min.js",
        ".min.css",
        ".bundle.js",
        ".map",
        ".snap",
        ".pb.go",
        ".pb.rs",
        ".g.dart",
        ".lockb",
    ]
    .iter()
    .any(|ext| file_name.ends_with(ext))
        || lower.starts_with("dist/")
        || lower.contains("/dist/")
        || lower.starts_with("vendor/")
        || lower.contains("/vendor/")
        || lower.starts_with("node_modules/")
}

fn is_docs_or_tests_path(rel_path: &str) -> bool {
    let lower = rel_path.to_ascii_lowercase();
    let file_name = lower.rsplit('/').next().unwrap_or(lower.as_str());

    let has_doc_ext = std::path::Path::new(file_name)
        .extension()
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("md")
                || ext.eq_ignore_ascii_case("mdx")
                || ext.eq_ignore_ascii_case("rst")
                || ext.eq_ignore_ascii_case("txt")
        });

    lower.starts_with("docs/")
        || lower.contains("/docs/")
        || lower.starts_with("tests/")
        || lower.contains("/tests/")
        || lower.contains("/__tests__/")
        || has_doc_ext
        || file_name.contains("_test.")
        || file_name.contains(".test.")
        || file_name.contains(".spec.")
}

/// Truncates `text` to at most `max_bytes` on a UTF-8 character boundary.
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

/// Truncates `text` on a line boundary (falling back to a UTF-8 boundary) within `max_bytes`.
fn cut_at_line_boundary(text: &str, max_bytes: usize) -> &str {
    let prefix = cut_utf8(text, max_bytes);
    if prefix.len() == text.len() {
        return prefix;
    }
    match prefix.rfind('\n') {
        Some(pos) if pos > 0 => prefix.get(..pos).unwrap_or(prefix),
        _ => prefix,
    }
}

/// Computes added, deleted, and hunk counts from a raw unified git diff without allocating.
#[must_use]
pub fn summarize_stats(raw_diff: &str) -> FileStats {
    scan_raw_diff(raw_diff, None).0
}

/// Strips redundant `git diff` metadata headers and collects change line stats.
#[must_use]
pub fn strip_git_boilerplate(raw_diff: &str) -> (String, FileStats) {
    let mut out = String::with_capacity(raw_diff.len());
    let (stats, saw_binary) = scan_raw_diff(raw_diff, Some(&mut out));
    if saw_binary {
        return ("[binary diff omitted]\n".to_owned(), stats);
    }
    (out, stats)
}

fn scan_raw_diff(raw_diff: &str, mut out: Option<&mut String>) -> (FileStats, bool) {
    let mut stats = FileStats::default();
    let mut saw_binary = false;

    for raw_line in raw_diff.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if is_redundant_git_header(line) {
            continue;
        }
        if line == "GIT binary patch"
            || (line.starts_with("Binary files ") && line.ends_with(" differ"))
        {
            saw_binary = true;
            break;
        }
        if line.starts_with("@@ ") {
            stats.hunks += 1;
        } else if line.starts_with('+') {
            stats.added += 1;
        } else if line.starts_with('-') {
            stats.deleted += 1;
        }
        if let Some(buf) = out.as_deref_mut() {
            buf.push_str(line);
            buf.push('\n');
        }
    }

    (stats, saw_binary)
}

fn is_redundant_git_header(line: &str) -> bool {
    [
        "diff --git ",
        "index ",
        "--- ",
        "+++ ",
        "old mode ",
        "new mode ",
        "new file mode ",
        "deleted file mode ",
        "similarity index ",
        "dissimilarity index ",
        "rename from ",
        "rename to ",
        "copy from ",
        "copy to ",
    ]
    .iter()
    .any(|prefix| line.starts_with(prefix))
        || line == "\\ No newline at end of file"
}

/// Compacts a header-stripped diff by trimming unchanged context runs and capping giant lines.
#[must_use]
pub fn compact_diff(stripped: &str, max_line_bytes: usize) -> String {
    let lines: Vec<&str> = stripped.lines().collect();
    if lines.is_empty() {
        return String::new();
    }

    let is_changed = |idx: usize| -> bool {
        lines
            .get(idx)
            .is_some_and(|l| l.starts_with('+') || l.starts_with('-'))
    };

    let mut out = String::with_capacity(stripped.len() / 2 + 64);
    let mut skipped_context = false;

    for (idx, &line) in lines.iter().enumerate() {
        if line.starts_with("@@ ") {
            skipped_context = false;
            append_capped_line(&mut out, line, max_line_bytes);
            continue;
        }

        if is_changed(idx) {
            if skipped_context {
                out.push_str(" ...\n");
                skipped_context = false;
            }
            append_capped_line(&mut out, line, max_line_bytes);
            continue;
        }

        let near_prev = idx > 0 && is_changed(idx - 1);
        let near_next = idx + 1 < lines.len() && is_changed(idx + 1);
        if near_prev || near_next {
            if skipped_context {
                out.push_str(" ...\n");
                skipped_context = false;
            }
            append_capped_line(&mut out, line, max_line_bytes);
        } else {
            skipped_context = true;
        }
    }

    out
}

fn append_capped_line(out: &mut String, line: &str, max_line_bytes: usize) {
    if line.len() <= max_line_bytes {
        out.push_str(line);
        out.push('\n');
        return;
    }
    let kept = cut_utf8(line, max_line_bytes.saturating_sub(24));
    let omitted = line.len().saturating_sub(kept.len());
    let _ = writeln!(out, "{kept}...[+{omitted}B]");
}

fn prepare_changes(changes: &[CommitChange]) -> Vec<PreparedChange> {
    changes
        .iter()
        .enumerate()
        .map(|(index, change)| {
            let rel_path = relative_path(&change.repository, &change.path);
            let code = status_code(&change.status);
            let content_kind = if change.diff.is_none() {
                ContentKind::Binary
            } else {
                ContentKind::Text
            };
            let tier = classify_tier(
                &rel_path,
                &change.status,
                content_kind == ContentKind::Binary,
            );

            let (tier1_body, stats) = match change.diff.as_deref() {
                Some(raw) => strip_git_boilerplate(raw),
                None => ("[binary file]\n".to_owned(), FileStats::default()),
            };

            let mut header = String::with_capacity(rel_path.len() + 36);
            match change.old_path.as_deref() {
                Some(old) => {
                    let old_rel = relative_path(&change.repository, old);
                    let _ = write!(header, "=== {code} {old_rel} -> {rel_path}");
                }
                None => {
                    let _ = write!(header, "=== {code} {rel_path}");
                }
            }
            if stats.added > 0 || stats.deleted > 0 {
                let _ = write!(header, " (+{}/-{})", stats.added, stats.deleted);
            }
            header.push_str(" ===\n");

            PreparedChange {
                index,
                rel_path,
                header,
                tier,
                stats,
                tier1_body,
                content_kind,
                host_truncation: if change.truncated {
                    HostTruncation::Truncated
                } else {
                    HostTruncation::Complete
                },
            }
        })
        .collect()
}

fn tier2_body_for(item: &PreparedChange) -> String {
    if item.content_kind == ContentKind::Binary {
        return "[binary file]\n".to_owned();
    }
    if item.tier == FileTier::LowSignal {
        return format!(
            "[low-signal diff omitted: +{}/-{} lines]\n",
            item.stats.added, item.stats.deleted
        );
    }
    if item.tier == FileTier::Deleted && item.stats.deleted > LARGE_DELETED_LINE_THRESHOLD {
        return format!(
            "[deleted file diff omitted: -{} lines]\n",
            item.stats.deleted
        );
    }
    let compacted = compact_diff(&item.tier1_body, MAX_COMPACT_LINE_BYTES);
    if compacted.is_empty() && !item.tier1_body.is_empty() {
        item.tier1_body.clone()
    } else {
        compacted
    }
}

/// Renders an optimized, prioritized, and strictly byte-bounded diff block.
#[must_use]
pub fn render_optimized(changes: &[CommitChange], max_bytes: usize) -> OptimizedDiff {
    let prepared = prepare_changes(changes);
    let host_truncated = prepared
        .iter()
        .any(|c| c.host_truncation == HostTruncation::Truncated);
    if max_bytes == 0 || prepared.is_empty() {
        return OptimizedDiff {
            text: String::new(),
            truncated_files: 0,
            omitted_files: usize::from(max_bytes == 0) * prepared.len(),
            host_truncated,
        };
    }

    let tier1_total: usize = prepared
        .iter()
        .map(|c| c.header.len() + c.tier1_body.len())
        .sum();
    if tier1_total <= max_bytes {
        let mut text = String::with_capacity(tier1_total);
        for item in &prepared {
            text.push_str(&item.header);
            text.push_str(&item.tier1_body);
        }
        return OptimizedDiff {
            text,
            truncated_files: 0,
            omitted_files: 0,
            host_truncated,
        };
    }

    let tier2_bodies: Vec<String> = prepared.iter().map(tier2_body_for).collect();
    let tier2_total: usize = prepared
        .iter()
        .zip(&tier2_bodies)
        .map(|(item, body)| item.header.len() + body.len())
        .sum();

    if tier2_total <= max_bytes {
        let mut text = String::with_capacity(tier2_total);
        let mut truncated_files = 0;
        for (item, body) in prepared.iter().zip(&tier2_bodies) {
            if body.len() < item.tier1_body.len() {
                truncated_files += 1;
            }
            text.push_str(&item.header);
            text.push_str(body);
        }
        return OptimizedDiff {
            text,
            truncated_files,
            omitted_files: 0,
            host_truncated,
        };
    }

    allocate_within_budget(&prepared, &tier2_bodies, max_bytes, host_truncated)
}

fn allocate_within_budget(
    prepared: &[PreparedChange],
    bodies: &[String],
    max_bytes: usize,
    host_truncated: bool,
) -> OptimizedDiff {
    let mut allocations = vec![0usize; prepared.len()];
    let mut remaining = max_bytes;

    for tier in FileTier::ORDER {
        if remaining == 0 {
            break;
        }
        let mut tier_indices: Vec<usize> = prepared
            .iter()
            .filter(|c| c.tier == tier)
            .map(|c| c.index)
            .collect();
        if tier_indices.is_empty() {
            continue;
        }

        // Allocate smallest files first inside the tier so small edits stay intact
        // and leftover bytes flow fairly to larger files via water-filling.
        tier_indices.sort_by_key(|&idx| {
            let header_len = prepared.get(idx).map_or(0, |c| c.header.len());
            let body_len = bodies.get(idx).map_or(0, String::len);
            (header_len + body_len, idx)
        });

        let count = tier_indices.len();
        for (pos, &idx) in tier_indices.iter().enumerate() {
            let left_in_tier = count - pos;
            let fair_share = remaining / left_in_tier;
            let header_len = prepared.get(idx).map_or(0, |c| c.header.len());
            let body_len = bodies.get(idx).map_or(0, String::len);
            let needed = header_len + body_len;

            let grant = needed.min(fair_share);
            if let Some(slot) = allocations.get_mut(idx) {
                *slot = grant;
            }
            remaining = remaining.saturating_sub(grant);
        }

        // Distribute integer-division remainder bytes across files in this tier that
        // still need more space.
        if remaining > 0 {
            for &idx in &tier_indices {
                if remaining == 0 {
                    break;
                }
                let header_len = prepared.get(idx).map_or(0, |c| c.header.len());
                let body_len = bodies.get(idx).map_or(0, String::len);
                let needed = header_len + body_len;
                let current = allocations.get(idx).copied().unwrap_or(0);
                if current < needed {
                    let extra = (needed - current).min(remaining);
                    if let Some(slot) = allocations.get_mut(idx) {
                        *slot += extra;
                    }
                    remaining = remaining.saturating_sub(extra);
                }
            }
        }
    }

    render_allocated(prepared, bodies, &allocations, max_bytes, host_truncated)
}

fn render_allocated(
    prepared: &[PreparedChange],
    bodies: &[String],
    allocations: &[usize],
    max_bytes: usize,
    host_truncated: bool,
) -> OptimizedDiff {
    let mut rendered: Vec<RenderedItem> = Vec::with_capacity(prepared.len());
    let mut truncated_files = 0usize;
    let mut omitted_files = 0usize;

    for (idx, item) in prepared.iter().enumerate() {
        let budget = allocations.get(idx).copied().unwrap_or(0);
        let body = bodies.get(idx).map_or("", String::as_str);
        let needed = item.header.len() + body.len();

        if budget >= needed {
            let mut full = String::with_capacity(needed);
            full.push_str(&item.header);
            full.push_str(body);
            if body.len() < item.tier1_body.len() {
                truncated_files += 1;
            }
            rendered.push(RenderedItem::Full(full));
            continue;
        }

        let min_viable = item.header.len() + MIN_DIFF_SLICE_BYTES + TRUNCATED_MARKER.len();
        if budget >= min_viable {
            let body_budget = budget - item.header.len() - TRUNCATED_MARKER.len();
            let cut = cut_at_line_boundary(body, body_budget);
            if !cut.trim().is_empty() {
                let mut slice = String::with_capacity(cut.len() + TRUNCATED_MARKER.len() + 1);
                slice.push_str(cut.trim_end_matches('\n'));
                slice.push_str(TRUNCATED_MARKER);
                slice.push('\n');
                truncated_files += 1;
                rendered.push(RenderedItem::Truncated {
                    header: item.header.clone(),
                    body: slice,
                });
                continue;
            }
        }

        omitted_files += 1;
        rendered.push(RenderedItem::Omitted(format!(
            "# {} {} (+{}/-{} omitted)\n",
            status_code_from_header(&item.header),
            item.rel_path,
            item.stats.added,
            item.stats.deleted,
        )));
    }

    let mut text = String::with_capacity(max_bytes);
    for entry in rendered {
        let chunk = match &entry {
            RenderedItem::Full(s) | RenderedItem::Omitted(s) => s.as_str(),
            RenderedItem::Truncated { header, body } => {
                if text.len() + header.len() + body.len() > max_bytes {
                    continue;
                }
                text.push_str(header);
                body.as_str()
            }
        };
        if text.len() + chunk.len() <= max_bytes {
            text.push_str(chunk);
        }
    }

    OptimizedDiff {
        text,
        truncated_files,
        omitted_files,
        host_truncated,
    }
}

fn status_code_from_header(header: &str) -> char {
    header
        .strip_prefix("=== ")
        .and_then(|rest| rest.chars().next())
        .unwrap_or('M')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_change(path: &str, status: &str, diff: Option<&str>) -> CommitChange {
        CommitChange {
            repository: "/repo".to_owned(),
            path: format!("/repo/{path}"),
            old_path: None,
            status: status.to_owned(),
            diff: diff.map(str::to_owned),
            truncated: false,
        }
    }

    #[test]
    fn strips_git_headers_and_counts_stats() {
        let raw = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1234567..89abcdef 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@ fn demo() {
 let a = 1;
-let b = 2;
+let b = 3;
+let c = 4;
\\ No newline at end of file
";
        let (stripped, stats) = strip_git_boilerplate(raw);
        assert!(!stripped.contains("diff --git"));
        assert!(!stripped.contains("index 1234567"));
        assert!(!stripped.contains("--- a/"));
        assert!(!stripped.contains("+++ b/"));
        assert!(!stripped.contains("No newline"));
        assert!(stripped.contains("@@ -1,3 +1,4 @@ fn demo() {"));
        assert_eq!(
            stats,
            FileStats {
                added: 2,
                deleted: 1,
                hunks: 1
            }
        );
    }

    #[test]
    fn compacts_context_and_long_lines() {
        let mut stripped = String::from("@@ -1,10 +1,10 @@ fn f()\n");
        for i in 0..8 {
            let _ = writeln!(stripped, " ctx_{i}");
        }
        stripped.push_str("-old_val\n+new_val\n");
        for i in 0..8 {
            let _ = writeln!(stripped, " tail_{i}");
        }

        let compact = compact_diff(&stripped, 420);
        assert!(compact.contains(" ctx_7\n"));
        assert!(!compact.contains(" ctx_0\n"));
        assert!(compact.contains(" tail_0\n"));
        assert!(!compact.contains(" tail_7\n"));
        assert!(compact.contains(" ...\n"));

        let huge_line = format!("+{}\n", "x".repeat(1000));
        let capped = compact_diff(&huge_line, 120);
        assert!(capped.len() < 160);
        assert!(capped.contains("...[+"));
    }

    #[test]
    fn prioritizes_source_files_over_lockfiles_and_stays_under_budget() {
        let mut lock_diff = String::new();
        for i in 0..600 {
            let _ = writeln!(lock_diff, "+version = \"0.1.{i}\"");
        }
        let src_diff = "@@ -10,2 +10,3 @@ fn auth() {\n-    deny()\n+    allow()\n+}\n";

        let changes = vec![
            make_change("Cargo.lock", "modified", Some(&lock_diff)),
            make_change("src/auth.rs", "modified", Some(src_diff)),
        ];

        let rendered = render_optimized(&changes, 4_000);
        assert!(rendered.text.len() <= 4_000);
        assert!(rendered.text.contains("src/auth.rs"));
        assert!(rendered.text.contains("+    allow()"));
        assert!(rendered.text.contains("low-signal diff omitted"));
    }

    #[test]
    fn distributes_budget_fairly_without_starving_later_files() {
        let mut big_a = String::new();
        let mut big_b = String::new();
        for i in 0..250 {
            let _ = writeln!(big_a, "+let line_a_{i} = {i};");
            let _ = writeln!(big_b, "+let line_b_{i} = {i};");
        }

        let changes = vec![
            make_change("src/a.rs", "modified", Some(&big_a)),
            make_change("src/b.rs", "modified", Some(&big_b)),
        ];

        let rendered = render_optimized(&changes, 3_000);
        assert!(rendered.text.len() <= 3_000);
        assert!(rendered.text.contains("src/a.rs"));
        assert!(rendered.text.contains("src/b.rs"));
        assert!(rendered.text.contains("+let line_a_0"));
        assert!(rendered.text.contains("+let line_b_0"));
        assert_eq!(rendered.truncated_files, 2);
    }
}
