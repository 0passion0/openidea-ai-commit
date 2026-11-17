//! Sub-5 ms `PyCharm`-style popup completions: cached AI hits, postfix templates, auto-imports, and proximity symbols.

use std::collections::{HashMap, HashSet};

use openidea_plugin::providers::{CompletionItem, CompletionKind, Document};
use openidea_plugin::{Position, Range, TextEdit, host, text};

use super::cache;
use super::catalog;

/// Generates fast local popup completion items combining cached AI hits, postfix templates,
/// stdlib/ecosystem auto-imports, and proximity identifiers from the document.
#[must_use]
pub fn provide_popup_completions(
    doc: &Document,
    at: Position,
    _trigger: Option<&str>,
) -> Vec<CompletionItem> {
    if !host::setting_bool("completionPopupEnabled").unwrap_or(true) {
        return Vec::new();
    }
    let Some(offset) = text::offset(&doc.text, at) else {
        return Vec::new();
    };
    let prefix = doc.text.get(..offset).unwrap_or("");
    let line_before = prefix.rsplit('\n').next().unwrap_or(prefix);
    let indent = leading_whitespace(line_before);
    let word_prefix = trailing_word(line_before);
    let lang = normalize_language(&doc.language);

    let mut items = Vec::new();
    let mut emitted_labels = HashSet::new();

    append_cached_ai_item(
        doc,
        at,
        line_before,
        word_prefix,
        &mut items,
        &mut emitted_labels,
    );

    let is_member_postfix = host::setting_bool("completionPostfixEnabled").unwrap_or(true)
        && append_postfix_items(lang, line_before, indent, at, &mut items);

    if !is_member_postfix {
        if host::setting_bool("completionAutoImportEnabled").unwrap_or(true)
            && word_prefix.len() >= 2
        {
            append_auto_import_items(
                lang,
                &doc.text,
                word_prefix,
                &mut items,
                &mut emitted_labels,
            );
        }
        if !word_prefix.is_empty() {
            append_snippet_items(lang, word_prefix, indent, &mut items, &mut emitted_labels);
        }
        append_proximity_identifiers(&doc.text, at, word_prefix, &mut items, &mut emitted_labels);
    }

    items
}

fn append_cached_ai_item(
    doc: &Document,
    at: Position,
    line_before: &str,
    word_prefix: &str,
    out: &mut Vec<CompletionItem>,
    emitted: &mut HashSet<String>,
) {
    let Some(cached) = cache::latest_for_line(&doc.path, at.line, line_before) else {
        return;
    };
    let rest = cached.trim_end_matches(['\r', '\n']);
    if rest.trim().is_empty() {
        return;
    }
    let insert_text = if word_prefix.is_empty() {
        rest.to_owned()
    } else {
        format!("{word_prefix}{rest}")
    };
    let first_line = insert_text.lines().next().unwrap_or(&insert_text);
    let display = if insert_text.contains('\n') {
        format!("{} …", ai_core::cut_utf8(first_line, 60))
    } else {
        ai_core::cut_utf8(first_line, 72).to_owned()
    };
    if !emitted.insert(display.clone()) {
        return;
    }
    let mut item = CompletionItem::new(&display, CompletionKind::Snippet);
    item.detail = Some("AI Inline Prediction".to_owned());
    item.insert_text.clone_from(&insert_text);
    item.sort_text = Some("00_ai_0".to_owned());
    item.documentation = Some(format!("```{}\n{insert_text}\n```", doc.language));
    out.push(item);
}

fn append_postfix_items(
    lang: &str,
    line_before: &str,
    indent: &str,
    at: Position,
    out: &mut Vec<CompletionItem>,
) -> bool {
    let Some((expr_start_byte, expr, key_prefix)) = parse_postfix_target(line_before) else {
        return false;
    };
    let prefix_before_expr = line_before.get(..expr_start_byte).unwrap_or("");
    let expr_start_utf16 = u32::try_from(prefix_before_expr.encode_utf16().count()).unwrap_or(0);
    let replace_range = Range::new(Position::new(at.line, expr_start_utf16), at);
    let escaped_expr = expr.replace('$', "$$");
    let key_lower = key_prefix.to_ascii_lowercase();

    for (idx, (key, title, docs)) in catalog::POSTFIX_SPECS.iter().enumerate() {
        if !key_lower.is_empty() && !key.starts_with(&key_lower) {
            continue;
        }
        let Some(template) = catalog::expand_postfix_template(lang, key, &escaped_expr, indent)
        else {
            continue;
        };
        let preview = template
            .lines()
            .next()
            .unwrap_or(&template)
            .replace("${1:", "")
            .replace('}', "")
            .replace("$0", "")
            .replace("$$", "$");
        let mut item = CompletionItem::new(
            &format!(".{key} -> {}", ai_core::cut_utf8(&preview, 44)),
            CompletionKind::Snippet,
        );
        item.detail = Some((*title).to_owned());
        item.filter_text = Some((*key).to_owned());
        item.insert_text = template;
        item.snippet = true;
        item.replace = Some(replace_range);
        item.sort_text = Some(format!("01_postfix_{idx:02}"));
        item.documentation = Some(format!("{docs}\n\n```{lang}\n{preview}\n```"));
        out.push(item);
    }

    !out.is_empty()
}

fn parse_postfix_target(line_before: &str) -> Option<(usize, &str, &str)> {
    let after_key_idx = line_before
        .char_indices()
        .rev()
        .find(|&(_, ch)| !ch.is_ascii_alphanumeric() && ch != '_')
        .map_or(0, |(idx, ch)| idx + ch.len_utf8());
    let key_prefix = line_before.get(after_key_idx..).unwrap_or("");
    let before_dot = line_before.get(..after_key_idx)?.strip_suffix('.')?;
    if before_dot.is_empty()
        || before_dot.ends_with('.')
        || before_dot.ends_with(|ch: char| ch.is_whitespace())
    {
        return None;
    }

    let expr_start = find_expr_start(before_dot);
    let expr = before_dot.get(expr_start..)?.trim();
    if expr.is_empty() || expr.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((expr_start, expr, key_prefix))
}

fn find_expr_start(before_dot: &str) -> usize {
    let mut paren_depth: usize = 0;
    let mut bracket_depth: usize = 0;
    let mut last_boundary = 0;
    let mut iter = before_dot.char_indices().rev();

    while let Some((idx, ch)) = iter.next() {
        match ch {
            ')' => paren_depth += 1,
            ']' => bracket_depth += 1,
            '(' => {
                if paren_depth == 0 {
                    last_boundary = idx + ch.len_utf8();
                    break;
                }
                paren_depth -= 1;
            }
            '[' => {
                if bracket_depth == 0 {
                    last_boundary = idx + ch.len_utf8();
                    break;
                }
                bracket_depth -= 1;
            }
            ch if paren_depth == 0
                && bracket_depth == 0
                && (ch.is_whitespace()
                    || matches!(
                        ch,
                        ',' | ';'
                            | '{'
                            | '}'
                            | '='
                            | '+'
                            | '-'
                            | '*'
                            | '/'
                            | '&'
                            | '|'
                            | '!'
                            | '<'
                            | '>'
                            | '?'
                            | ':'
                    )) =>
            {
                if ch == ':' && before_dot.get(..idx).is_some_and(|s| s.ends_with(':')) {
                    let _ = iter.next();
                    continue;
                }
                last_boundary = idx + ch.len_utf8();
                break;
            }
            _ => {}
        }
    }
    last_boundary
}

fn append_auto_import_items(
    lang: &str,
    source: &str,
    word_prefix: &str,
    out: &mut Vec<CompletionItem>,
    emitted: &mut HashSet<String>,
) {
    let entries = catalog::auto_import_table(lang);
    if entries.is_empty() {
        return;
    }
    let lower_query = word_prefix.to_ascii_lowercase();
    let insert_pos = import_insert_position(source, lang);

    for (idx, &(symbol, import_line, detail, kind)) in entries.iter().enumerate() {
        if !symbol.to_ascii_lowercase().starts_with(&lower_query) {
            continue;
        }
        if already_imported(source, symbol, import_line) || !emitted.insert(symbol.to_owned()) {
            continue;
        }
        let mut item = CompletionItem::new(symbol, kind);
        item.detail = Some(format!("auto-import ({detail})"));
        item.sort_text = Some(format!("03_import_{idx:02}"));
        item.documentation = Some(format!("Auto-import:\n\n```{lang}\n{import_line}\n```"));
        item.additional_edits = vec![TextEdit {
            range: Range::new(insert_pos, insert_pos),
            text: format!("{import_line}\n"),
        }];
        out.push(item);
    }
}

fn already_imported(source: &str, symbol: &str, import_line: &str) -> bool {
    if source.lines().any(|line| line.trim() == import_line) {
        return true;
    }
    source
        .lines()
        .take(80)
        .filter(|line| super::context::is_import_line(line.trim_start()))
        .any(|line| {
            line.split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|w| w == symbol)
        })
}

fn import_insert_position(source: &str, lang: &str) -> Position {
    let mut insert_line: u32 = 0;
    for (idx, raw_line) in source.lines().take(100).enumerate() {
        let trimmed = raw_line.trim_start();
        let line_no = u32::try_from(idx).unwrap_or(u32::MAX);
        if super::context::is_import_line(trimmed) {
            insert_line = line_no.saturating_add(1);
            continue;
        }
        if trimmed.starts_with("#!")
            || (lang == "rust" && trimmed.starts_with("//!"))
            || (lang == "java" && trimmed.starts_with("package "))
            || (lang == "typescript" && trimmed.starts_with("\"use "))
        {
            insert_line = line_no.saturating_add(1);
        } else if !trimmed.is_empty()
            && (insert_line > 0 || (!trimmed.starts_with("//") && !trimmed.starts_with('#')))
        {
            break;
        }
    }
    Position::new(insert_line, 0)
}

fn append_snippet_items(
    lang: &str,
    word_prefix: &str,
    indent: &str,
    out: &mut Vec<CompletionItem>,
    emitted: &mut HashSet<String>,
) {
    let sub = format!("{indent}    ");
    let query = word_prefix.to_ascii_lowercase();
    let templates = catalog::language_snippets(lang, indent, &sub);

    for (idx, (trigger, desc, body)) in templates.into_iter().enumerate() {
        if !trigger.starts_with(&query) || !emitted.insert(trigger.to_owned()) {
            continue;
        }
        let mut item = CompletionItem::new(trigger, CompletionKind::Snippet);
        item.detail = Some(desc.to_owned());
        item.insert_text = body;
        item.snippet = true;
        item.sort_text = Some(format!("04_snip_{idx:02}"));
        out.push(item);
    }
}

fn append_proximity_identifiers(
    source: &str,
    at: Position,
    word_prefix: &str,
    out: &mut Vec<CompletionItem>,
    emitted: &mut HashSet<String>,
) {
    let lower_prefix = word_prefix.to_ascii_lowercase();
    let target_line = at.line as usize;
    let mut best_distance: HashMap<&str, usize> = HashMap::new();

    for (line_idx, line) in source.lines().enumerate() {
        let dist = line_idx.abs_diff(target_line);
        for token in line.split(|ch: char| !ch.is_alphanumeric() && ch != '_') {
            if token.len() < 3
                || token == word_prefix
                || token.bytes().next().is_some_and(|b| b.is_ascii_digit())
            {
                continue;
            }
            if !lower_prefix.is_empty() && !token.to_ascii_lowercase().starts_with(&lower_prefix) {
                continue;
            }
            best_distance
                .entry(token)
                .and_modify(|prev| {
                    if dist < *prev {
                        *prev = dist;
                    }
                })
                .or_insert(dist);
        }
    }

    let mut ranked: Vec<(&str, usize)> = best_distance.into_iter().collect();
    ranked.sort_by(|(a_tok, a_dist), (b_tok, b_dist)| {
        a_dist.cmp(b_dist).then_with(|| a_tok.cmp(b_tok))
    });

    for (rank, (ident, _)) in ranked.into_iter().take(25).enumerate() {
        if !emitted.insert(ident.to_owned()) {
            continue;
        }
        let mut item = CompletionItem::new(ident, CompletionKind::Variable);
        item.detail = Some("local symbol".to_owned());
        item.sort_text = Some(format!("05_local_{rank:03}"));
        out.push(item);
    }
}

fn normalize_language(language: &str) -> &str {
    match language.to_ascii_lowercase().as_str() {
        "python" | "py" => "python",
        "rust" | "rs" => "rust",
        "typescript" | "tsx" | "ts" => "typescript",
        "javascript" | "jsx" | "js" => "javascript",
        "go" | "golang" => "go",
        "java" => "java",
        "kotlin" | "kt" => "kotlin",
        "csharp" | "cs" => "csharp",
        _ => "generic",
    }
}

fn leading_whitespace(line: &str) -> &str {
    let trimmed = line.trim_start();
    line.get(..line.len() - trimmed.len()).unwrap_or("")
}

fn trailing_word(line: &str) -> &str {
    let start = line
        .char_indices()
        .rev()
        .find(|&(_, ch)| !ch.is_alphanumeric() && ch != '_')
        .map_or(0, |(idx, ch)| idx + ch.len_utf8());
    line.get(start..).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use openidea_plugin::mock::MockHost;

    #[test]
    fn generates_postfix_templates_with_full_expression_replace_range() {
        let _host = MockHost::install();
        let doc = Document {
            path: "/repo/app.py".into(),
            language: "python".into(),
            version: 1,
            text: "def f(user_items):\n    user_items.for".into(),
        };
        let items = provide_popup_completions(&doc, Position::new(1, 18), None);
        let for_item = items
            .iter()
            .find(|item| item.filter_text.as_deref() == Some("for"))
            .expect("postfix .for item");
        assert!(for_item.snippet);
        assert_eq!(
            for_item.insert_text,
            "for ${1:item} in user_items:\n        $0"
        );
        assert_eq!(
            for_item.replace,
            Some(Range::new(Position::new(1, 4), Position::new(1, 18)))
        );
    }

    #[test]
    fn generates_auto_import_items_with_additional_edits_and_skips_existing() {
        let _host = MockHost::install();
        let doc = Document {
            path: "/repo/main.rs".into(),
            language: "rust".into(),
            version: 1,
            text: "use std::sync::Arc;\n\nfn run() {\n    let m = Hash\n}".into(),
        };
        let items = provide_popup_completions(&doc, Position::new(3, 16), None);
        let hashmap = items
            .iter()
            .find(|item| item.label == "HashMap")
            .expect("HashMap auto-import");
        assert_eq!(hashmap.additional_edits.len(), 1);
        assert_eq!(
            hashmap.additional_edits.first().map(|e| e.text.as_str()),
            Some("use std::collections::HashMap;\n")
        );
        assert_eq!(
            hashmap.additional_edits.first().map(|e| e.range),
            Some(Range::new(Position::new(1, 0), Position::new(1, 0)))
        );

        let arc_doc = Document {
            path: "/repo/main.rs".into(),
            language: "rust".into(),
            version: 1,
            text: "use std::sync::Arc;\n\nfn run() {\n    let a = Ar\n}".into(),
        };
        let arc_items = provide_popup_completions(&arc_doc, Position::new(3, 14), None);
        assert!(
            !arc_items
                .iter()
                .any(|item| item.label == "Arc" && !item.additional_edits.is_empty())
        );
    }

    #[test]
    fn surfaces_cached_ai_prediction_and_proximity_symbols() {
        let _host = MockHost::install();
        cache::clear();
        cache::store(
            "/repo/app.ts",
            1,
            "    const total = ",
            "\n",
            "computeSubtotal(cartItems)",
        );
        let doc = Document {
            path: "/repo/app.ts".into(),
            language: "typescript".into(),
            version: 2,
            text: "const customerInvoice = 1;\n    const total = comp\n".into(),
        };
        let items = provide_popup_completions(&doc, Position::new(1, 22), None);
        assert_eq!(
            items.first().map(|i| i.insert_text.as_str()),
            Some("computeSubtotal(cartItems)")
        );
        cache::clear();
    }
}
