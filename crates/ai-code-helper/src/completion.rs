//! Cursor-grade multi-language Fill-In-The-Middle inline completion, newline continuation,
//! companion bridge compatibility, and direct progressive in-file streaming completion.

pub mod align;
mod cache;
mod catalog;
mod context;
pub mod fim;
pub mod popup;
pub mod stream;

#[cfg(test)]
mod tests;

use ai_core::{ServerConfig, complete as ai_complete, cut_utf8, report_error};
use openidea_plugin::providers::{CompletionItem, Document, InlineSuggestion};
use openidea_plugin::{Error, HttpRequest, Invocation, LogLevel, Position, Range, host, text};
use serde::{Deserialize, Serialize};

pub use stream::{run_accept_completion, run_reject_completion};

#[derive(Serialize)]
struct CompletionRequest<'a> {
    path: &'a str,
    language: &'a str,
    version: u64,
    position: Cursor,
    before: &'a str,
    after: &'a str,
    context: String,
}

#[derive(Serialize)]
struct Cursor {
    line: u32,
    character: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompanionResponse {
    insert_text: String,
}

fn build_request(doc: &Document, at: Position, limit: usize) -> Option<CompletionRequest<'_>> {
    let offset = text::offset(&doc.text, at)?;
    let (prefix, suffix) = doc.text.split_at(offset);
    let context = context::extract_code_context(
        prefix,
        &doc.text,
        &doc.path,
        &doc.language,
        at.line,
        limit / 4,
    );
    let after = cut_utf8(suffix, limit / 4);
    let before_limit = limit
        .saturating_sub(context.len())
        .saturating_sub(after.len());
    let mut start = prefix.len().saturating_sub(before_limit);
    while start < prefix.len() && !prefix.is_char_boundary(start) {
        start += 1;
    }
    Some(CompletionRequest {
        path: &doc.path,
        language: &doc.language,
        version: doc.version,
        position: Cursor {
            line: at.line,
            character: at.character,
        },
        before: prefix.get(start..).unwrap_or(prefix),
        after,
        context,
    })
}

fn read_string_setting(key: &str) -> Option<String> {
    host::setting_string(key).or_else(|| host::setting(key))
}

fn parse_context_limit() -> Result<usize, Error> {
    let limit = host::setting("completionContextBytes").map_or(Ok(24_000), |value| {
        value
            .parse::<usize>()
            .map_err(|_| Error::Invalid("Invalid completion context limit".into()))
    })?;
    if !(1024..=128_000).contains(&limit) {
        return Err(Error::Invalid(
            "Completion context limit must be 1024–128000 bytes".into(),
        ));
    }
    Ok(limit)
}

fn parse_timeout_ms() -> Result<u32, Error> {
    let timeout = host::setting("completionRequestTimeoutMs").map_or(Ok(12_000), |value| {
        value
            .parse::<u32>()
            .map_err(|_| Error::Invalid("Invalid completion request timeout".into()))
    })?;
    if !(100..=180_000).contains(&timeout) {
        return Err(Error::Invalid(
            "Completion timeout must be 100–180000 milliseconds".into(),
        ));
    }
    Ok(timeout)
}

fn parse_max_lines() -> usize {
    host::setting_number("completionMaxLines").map_or(16, |n| {
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let clamped = n.clamp(1.0, 64.0) as usize;
        clamped
    })
}

fn complete(doc: &Document, at: Position) -> Result<Option<InlineSuggestion>, Error> {
    if !host::setting_bool("completionEnabled").unwrap_or(false) {
        return Ok(None);
    }

    let backend =
        read_string_setting("completionBackend").unwrap_or_else(|| "companion".to_owned());
    match backend.trim().to_ascii_lowercase().as_str() {
        "openai" => complete_via_openai(doc, at),
        _ => complete_via_companion(doc, at),
    }
}

fn complete_via_companion(doc: &Document, at: Position) -> Result<Option<InlineSuggestion>, Error> {
    if doc.language != "python" {
        return Ok(None);
    }
    let port = host::setting("completionPort").map_or(Ok(8765), |value| {
        value
            .parse::<u16>()
            .map_err(|_| Error::Invalid("Invalid completion port".into()))
    })?;
    if port == 0 {
        return Err(Error::Invalid("Completion port must be positive".into()));
    }
    let limit = parse_context_limit()?;
    let timeout = parse_timeout_ms()?;
    let Some(payload) = build_request(doc, at, limit) else {
        return Ok(None);
    };
    let body = serde_json::to_string(&payload)
        .map_err(|_| Error::Failed("Unable to serialize completion request".into()))?;
    let mut request = HttpRequest::post(&format!("http://127.0.0.1:{port}/v1/completion"), body)
        .header("content-type", "application/json");
    request.timeout_ms = Some(timeout);
    let response = host::fetch(&request)?;
    if response.status != 200 {
        return Err(Error::Failed(format!(
            "Completion service returned HTTP {}",
            response.status
        )));
    }
    let result: CompanionResponse = serde_json::from_slice(&response.body)
        .map_err(|_| Error::Failed("Invalid completion service response".into()))?;
    if result.insert_text.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(InlineSuggestion {
        insert_text: result.insert_text,
        replace: None,
    }))
}

pub(crate) fn compute_replace_range(
    at: Position,
    after_caret: &str,
    insert_text: &str,
) -> Option<Range> {
    let same_line_after = after_caret.split('\n').next().unwrap_or("").trim_end();
    let is_closing_only = !same_line_after.is_empty()
        && same_line_after
            .chars()
            .all(|ch| matches!(ch, ')' | ']' | '}' | ';' | ',' | '"' | '\'' | '`'));
    if !is_closing_only || !insert_text.ends_with(same_line_after) {
        return None;
    }

    let balance = |open: char, close: char| -> bool {
        insert_text.chars().filter(|&c| c == close).count()
            > insert_text.chars().filter(|&c| c == open).count()
    };
    let odd_quote = |q: char| -> bool { insert_text.chars().filter(|&c| c == q).count() % 2 == 1 };
    let safe_to_replace = (same_line_after.contains(')') && balance('(', ')'))
        || (same_line_after.contains(']') && balance('[', ']'))
        || (same_line_after.contains('}') && balance('{', '}'))
        || (same_line_after.contains('"') && odd_quote('"'))
        || (same_line_after.contains('\'') && odd_quote('\''))
        || (same_line_after.contains('`') && odd_quote('`'))
        || same_line_after.chars().all(|c| matches!(c, ';' | ','));
    if !safe_to_replace {
        return None;
    }

    let utf16_len = align::utf16_len(same_line_after);
    Some(Range::new(
        at,
        Position::new(at.line, at.character.saturating_add(utf16_len)),
    ))
}

fn complete_via_openai(doc: &Document, at: Position) -> Result<Option<InlineSuggestion>, Error> {
    if doc.language.trim().is_empty() {
        return Ok(None);
    }
    stream::clear_stale_confirmation_on_edit(&doc.path, doc.version);

    let limit = parse_context_limit()?;
    let timeout = parse_timeout_ms()?;
    let max_lines = parse_max_lines();

    let Some(offset) = text::offset(&doc.text, at) else {
        return Ok(None);
    };
    let (full_prefix, full_suffix) = doc.text.split_at(offset);
    let line_before = full_prefix.rsplit('\n').next().unwrap_or(full_prefix);
    let suffix_fp = cache::suffix_fingerprint(full_suffix);

    if let Some(cached) = cache::lookup(&doc.path, at.line, line_before, &suffix_fp) {
        stream::update_inline_status(true);
        let replace = compute_replace_range(at, full_suffix, &cached);
        return Ok(Some(InlineSuggestion {
            insert_text: cached,
            replace,
        }));
    }

    let Some(mut req) = build_request(doc, at, limit) else {
        return Ok(None);
    };
    context::append_workspace_symbols(&mut req.context, &doc.path, line_before, limit / 4);
    let mode = fim::detect_fim_mode(req.after, max_lines);
    let ctx = fim::FimContext {
        path: req.path,
        language: req.language,
        before: req.before,
        after: req.after,
        context: &req.context,
    };
    let prompt = fim::build_fim_prompt(&ctx, mode, limit.max(ai_core::MIN_MAX_INPUT_BYTES));
    let server = resolve_completion_server(Some(timeout))?;

    let _task = host::begin_task("✨ AI 智能补全推理中...", false);
    let raw = ai_complete(&server, &prompt, "AI Assistant")?;
    let effective_lines = match mode {
        fim::FimMode::SingleLine => 1,
        fim::FimMode::MultiLine { max_lines: m } => m,
    };
    let cleaned =
        fim::sanitize_and_align_fim(&raw, line_before, req.after, &doc.language, effective_lines);
    if cleaned.trim().is_empty() {
        return Ok(None);
    }

    cache::store_with_newline_continuation(&doc.path, at.line, line_before, full_suffix, &cleaned);
    stream::update_inline_status(true);
    let replace = compute_replace_range(at, req.after, &cleaned);
    Ok(Some(InlineSuggestion {
        insert_text: cleaned,
        replace,
    }))
}

fn resolve_completion_server(timeout_override: Option<u32>) -> Result<ServerConfig, Error> {
    let mut server = ServerConfig::from_host()?;
    if let Some(timeout) = timeout_override {
        server.timeout_ms = timeout.min(58_000);
    }
    if let Some(override_model) = read_string_setting("completionModel") {
        let trimmed = override_model.trim();
        if !trimmed.is_empty() {
            trimmed.clone_into(&mut server.model);
        }
    }
    Ok(server)
}

#[cfg(target_arch = "wasm32")]
fn empty_completion_sentinel() -> Option<InlineSuggestion> {
    if host::setting_bool("completionEnabled").unwrap_or(false) {
        Some(InlineSuggestion {
            insert_text: String::new(),
            replace: None,
        })
    } else {
        None
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn empty_completion_sentinel() -> Option<InlineSuggestion> {
    None
}

/// Computes an optional inline ghost-text completion suggestion across languages.
#[must_use]
pub fn inline_completion(doc: &Document, at: Position) -> Option<InlineSuggestion> {
    match complete(doc, at) {
        Ok(Some(suggestion)) => Some(suggestion),
        Ok(None) => {
            if host::setting_bool("completionEnabled").unwrap_or(false) {
                stream::update_inline_status(false);
            }
            empty_completion_sentinel()
        }
        Err(Error::Cancelled | Error::Stale) => empty_completion_sentinel(),
        Err(_) => {
            host::log(
                LogLevel::Warn,
                "AI completion unavailable; check Server settings or the local completion service",
            );
            if host::setting_bool("completionEnabled").unwrap_or(false) {
                stream::update_inline_status(false);
            }
            empty_completion_sentinel()
        }
    }
}

/// Generates fast local popup completion items (cached AI predictions, postfix templates,
/// auto-imports, snippets, and in-file proximity symbols).
#[must_use]
pub fn popup_completion(
    doc: &Document,
    at: Position,
    trigger: Option<&str>,
) -> Vec<CompletionItem> {
    popup::provide_popup_completions(doc, at, trigger)
}

/// Streams the single best AI completion directly into the editor file for in-place confirmation.
///
/// # Errors
///
/// Returns host errors only if reporting an error notification itself fails.
pub fn run_smart_complete(ctx: &Invocation, plugin_name: &str) -> Result<Option<String>, Error> {
    let project = ctx.project();
    if let Err(err) = stream::execute_smart_complete(ctx, project.as_ref(), plugin_name) {
        report_error(
            project.as_ref(),
            "智能代码补全失败",
            "smart-complete",
            err,
            "重试",
            "测试连接",
        )?;
    }
    Ok(None)
}
