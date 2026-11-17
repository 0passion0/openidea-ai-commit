//! Multi-language structural scope spine, symbol hints, workspace lookup, and recent edit tracking.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};

use openidea_plugin::host;

const MAX_RECENT_EDITS: usize = 6;

#[derive(Clone, Debug)]
struct RecentEdit {
    path: String,
    line: u32,
    snippet: String,
}

thread_local! {
    static RECENT_EDITS: RefCell<VecDeque<RecentEdit>> = const { RefCell::new(VecDeque::new()) };
}

/// Records the active line snapshot in a bounded thread-local recent-edit history.
pub fn record_edit(path: &str, line: u32, line_text: &str) {
    let trimmed = line_text.trim();
    if trimmed.is_empty() {
        return;
    }
    let snippet = ai_core::cut_utf8(trimmed, 160).to_owned();
    RECENT_EDITS.with(|cell| {
        let mut history = cell.borrow_mut();
        if let Some(pos) = history
            .iter()
            .position(|entry| entry.path == path && entry.line == line)
        {
            history.remove(pos);
        }
        history.push_front(RecentEdit {
            path: path.to_owned(),
            line,
            snippet,
        });
        while history.len() > MAX_RECENT_EDITS {
            history.pop_back();
        }
    });
}

/// Clears recent-edit history (intended for unit tests).
#[cfg(test)]
pub fn clear_history() {
    RECENT_EDITS.with(|cell| cell.borrow_mut().clear());
}

/// Returns `true` when `trimmed` opens a type, function, method, module, or block signature.
#[must_use]
pub fn is_scope_header(trimmed: &str, language: &str) -> bool {
    if is_comment_line(trimmed) {
        return false;
    }
    match language {
        "python" | "py" => {
            trimmed.starts_with("def ")
                || trimmed.starts_with("async def ")
                || trimmed.starts_with("class ")
        }
        "rust" => {
            let core = trimmed
                .strip_prefix("pub(crate) ")
                .or_else(|| trimmed.strip_prefix("pub(super) "))
                .or_else(|| trimmed.strip_prefix("pub "))
                .unwrap_or(trimmed);
            core.starts_with("fn ")
                || core.starts_with("async fn ")
                || core.starts_with("unsafe fn ")
                || core.starts_with("const fn ")
                || core.starts_with("struct ")
                || core.starts_with("enum ")
                || core.starts_with("trait ")
                || core.starts_with("impl ")
                || core.starts_with("impl<")
                || core.starts_with("mod ")
        }
        "typescript" | "javascript" | "tsx" | "jsx" => {
            let core = trimmed
                .strip_prefix("export default ")
                .or_else(|| trimmed.strip_prefix("export "))
                .unwrap_or(trimmed);
            core.starts_with("function ")
                || core.starts_with("async function ")
                || core.starts_with("class ")
                || core.starts_with("interface ")
                || core.starts_with("type ")
                || core.starts_with("enum ")
                || is_method_header(core)
        }
        "go" => trimmed.starts_with("func ") || trimmed.starts_with("type "),
        "java" | "kotlin" | "csharp" | "c" | "cpp" | "swift" => {
            !trimmed.ends_with(';')
                && (trimmed.contains("class ")
                    || trimmed.contains("interface ")
                    || trimmed.contains("struct ")
                    || trimmed.contains("enum ")
                    || trimmed.starts_with("fun ")
                    || trimmed.contains(" fun ")
                    || trimmed.starts_with("func ")
                    || trimmed.starts_with("namespace ")
                    || is_method_header(trimmed))
        }
        _ => {
            trimmed.starts_with("def ")
                || trimmed.starts_with("async def ")
                || trimmed.starts_with("fn ")
                || trimmed.starts_with("func ")
                || trimmed.starts_with("function ")
                || trimmed.starts_with("class ")
        }
    }
}

fn is_method_header(trimmed: &str) -> bool {
    !trimmed.ends_with(';')
        && (trimmed.ends_with('{') || trimmed.ends_with(") {") || trimmed.contains(") -> "))
        && trimmed.contains('(')
        && !trimmed.starts_with("if ")
        && !trimmed.starts_with("if(")
        && !trimmed.starts_with("for ")
        && !trimmed.starts_with("for(")
        && !trimmed.starts_with("while ")
        && !trimmed.starts_with("while(")
        && !trimmed.starts_with("switch ")
        && !trimmed.starts_with("match ")
}

fn is_comment_line(trimmed: &str) -> bool {
    trimmed.starts_with('#') || trimmed.starts_with("//") || trimmed.starts_with("/*")
}

/// Returns `true` when `trimmed` represents an import / use / include directive.
#[must_use]
pub fn is_import_line(trimmed: &str) -> bool {
    trimmed.starts_with("import ")
        || trimmed.starts_with("from ")
        || trimmed.starts_with("use ")
        || trimmed.starts_with("pub use ")
        || trimmed.starts_with("#include ")
        || trimmed.starts_with("package ")
        || trimmed.starts_with("using ")
}

/// Extracts bounded structural context hints across languages:
/// enclosing scope spine, referenced definitions, imports, annotations,
/// workspace symbols (when permitted), and recent edits.
#[must_use]
pub fn extract_code_context(
    prefix: &str,
    source: &str,
    path: &str,
    language: &str,
    line_num: u32,
    limit: usize,
) -> String {
    if limit == 0 {
        return String::new();
    }

    let current_line = prefix.rsplit('\n').next().unwrap_or(prefix);
    record_edit(path, line_num, current_line);

    let scopes = collect_enclosing_scopes(prefix, language);
    let recent_window = collect_recent_identifiers(prefix);
    let definitions = collect_referenced_definitions(prefix, &recent_window);
    let imports: Vec<&str> = source
        .lines()
        .filter(|line| is_import_line(line.trim_start()))
        .take(40)
        .collect();
    let annotations = prefix.lines().rev().filter(|line| {
        let code = line.trim_start();
        code.contains(':')
            && !is_comment_line(code)
            && !is_scope_header(code, language)
            && !code.contains('?')
    });
    let signatures = source
        .lines()
        .filter(|line| is_scope_header(line.trim_start(), language))
        .take(40);

    let mut context = String::new();
    let mut seen_lines = HashSet::new();

    for line in scopes
        .into_iter()
        .chain(definitions)
        .chain(imports)
        .chain(annotations)
        .chain(signatures)
    {
        if context.len() >= limit {
            break;
        }
        push_unique_line(&mut context, &mut seen_lines, line, limit);
    }

    if limit.saturating_sub(context.len()) > 48 {
        append_recent_edits(&mut context, path, line_num, limit);
    }

    context
}

fn push_unique_line<'a>(
    out: &mut String,
    seen: &mut HashSet<&'a str>,
    line: &'a str,
    limit: usize,
) {
    if line.is_empty() || line.len() >= limit.saturating_sub(out.len()) {
        return;
    }
    if seen.insert(line) {
        out.push_str(line);
        out.push('\n');
    }
}

fn push_owned_line(out: &mut String, line: &str, limit: usize) {
    if line.is_empty()
        || line.len() >= limit.saturating_sub(out.len())
        || out.lines().any(|existing| existing == line)
    {
        return;
    }
    out.push_str(line);
    out.push('\n');
}

fn uses_braces(language: &str) -> bool {
    !matches!(
        language,
        "python" | "py" | "yaml" | "yml" | "ruby" | "coffeescript"
    )
}

fn collect_enclosing_scopes<'a>(prefix: &'a str, language: &str) -> Vec<&'a str> {
    let brace_scoped = uses_braces(language);
    let mut scopes: Vec<(usize, i32, &'a str)> = Vec::new();
    let mut brace_depth: i32 = 0;

    for line in prefix.lines() {
        let code = line.trim_start();
        if code.is_empty() || is_comment_line(code) {
            continue;
        }
        let indent = line.len() - code.len();
        if brace_scoped {
            while scopes
                .last()
                .is_some_and(|(lvl, open_depth, _)| *lvl >= indent || brace_depth <= *open_depth)
            {
                scopes.pop();
            }
            let opens_block =
                is_scope_header(code, language) && !(code.contains('{') && code.ends_with('}'));
            if opens_block {
                scopes.push((indent, brace_depth, line));
            }
            for ch in code.chars() {
                match ch {
                    '{' => brace_depth = brace_depth.saturating_add(1),
                    '}' => brace_depth = brace_depth.saturating_sub(1),
                    _ => {}
                }
            }
        } else {
            while scopes.last().is_some_and(|(lvl, _, _)| *lvl >= indent) {
                scopes.pop();
            }
            if is_scope_header(code, language) {
                scopes.push((indent, brace_depth, line));
            }
        }
    }

    scopes.into_iter().map(|(_, _, line)| line).collect()
}

fn collect_recent_identifiers(prefix: &str) -> HashSet<&str> {
    let mut names = HashSet::new();
    for line in prefix.rsplit('\n').take(3) {
        for name in line
            .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
            .filter(|name| !name.is_empty())
        {
            names.insert(name);
        }
    }
    names
}

fn collect_referenced_definitions<'a>(prefix: &'a str, names: &HashSet<&'a str>) -> Vec<&'a str> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for line in prefix.lines().rev() {
        let trimmed = line.trim();
        let Some((left, right)) = trimmed.split_once('=') else {
            continue;
        };
        if right.starts_with('=') {
            continue;
        }
        let Some(name) = extract_lhs_identifier(left) else {
            continue;
        };
        if names.contains(name) && seen.insert(name) {
            out.push(line);
        }
    }
    out
}

fn extract_lhs_identifier(left: &str) -> Option<&str> {
    let before_colon = left.split(':').next().unwrap_or(left).trim();
    let candidate = before_colon
        .split_whitespace()
        .last()
        .unwrap_or(before_colon)
        .trim_start_matches(['*', '&']);
    let is_valid = candidate
        .chars()
        .next()
        .is_some_and(|ch| ch.is_alphabetic() || ch == '_')
        && candidate
            .chars()
            .all(|ch| ch.is_alphanumeric() || ch == '_');
    is_valid.then_some(candidate)
}

/// Enriches `context` with matching cross-file workspace symbols via `project.symbols` when allowed.
pub fn append_workspace_symbols(
    context: &mut String,
    path: &str,
    current_line: &str,
    limit: usize,
) {
    if limit.saturating_sub(context.len()) <= 64 {
        return;
    }
    let Some(project) = host::project_of(path) else {
        return;
    };
    let mut query_idents: Vec<&str> = current_line
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .filter(|tok| tok.len() >= 3)
        .collect();
    query_idents.reverse();
    query_idents.truncate(2);

    for ident in query_idents {
        let Ok(symbols) = project.symbols(ident, 4) else {
            break;
        };
        for sym in symbols {
            if sym.location.path == path {
                continue;
            }
            let file_name = sym
                .location
                .path
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&sym.location.path);
            let container = sym
                .container
                .as_deref()
                .map_or(String::new(), |c| format!("{c}::"));
            let hint = format!(
                "// symbol ({file_name}): {} {container}{}",
                sym.kind, sym.name
            );
            push_owned_line(context, &hint, limit);
        }
    }
}

fn append_recent_edits(context: &mut String, current_path: &str, current_line: u32, limit: usize) {
    RECENT_EDITS.with(|cell| {
        let history = cell.borrow();
        for entry in history.iter() {
            if entry.path == current_path && entry.line == current_line {
                continue;
            }
            let file_name = entry.path.rsplit(['/', '\\']).next().unwrap_or(&entry.path);
            let hint = format!(
                "// recent ({file_name}:{}): {}",
                entry.line + 1,
                entry.snippet
            );
            push_owned_line(context, &hint, limit);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_rust_enclosing_scope_and_drops_closed_siblings() {
        clear_history();
        let src = "\
use std::collections::HashMap;

struct OrderService {
    tax_rate: f64,
}

impl OrderService {
    fn helper_done(&self) -> f64 {
        0.1
    }

    fn compute_total(&self, subtotal: f64) -> f64 {
        let discount = 5.0;
        let total = subtotal - discount";
        let scopes = collect_enclosing_scopes(src, "rust");
        assert!(scopes.iter().any(|s| s.contains("impl OrderService")));
        assert!(scopes.iter().any(|s| s.contains("fn compute_total")));
        assert!(!scopes.iter().any(|s| s.contains("fn helper_done")));
        assert!(!scopes.iter().any(|s| s.contains("struct OrderService")));

        let ctx = extract_code_context(src, src, "/repo/src/order.rs", "rust", 13, 2048);
        assert!(ctx.contains("use std::collections::HashMap;"));
        assert!(ctx.contains("impl OrderService {"));
        assert!(ctx.contains("fn compute_total(&self, subtotal: f64) -> f64 {"));
        assert!(ctx.contains("let discount = 5.0;"));
        assert!(ctx.contains("fn helper_done"));
    }

    #[test]
    fn records_recent_edits_across_files_within_budget() {
        clear_history();
        record_edit("/repo/src/models.py", 12, "class InvoiceItem:");
        let ctx = extract_code_context(
            "def pay():\n    item = ",
            "def pay():\n    item = \n",
            "/repo/src/service.py",
            "python",
            1,
            1024,
        );
        assert!(ctx.contains("models.py:13"));
        assert!(ctx.contains("class InvoiceItem:"));
        assert!(ctx.len() <= 1024);
        clear_history();
    }
}
