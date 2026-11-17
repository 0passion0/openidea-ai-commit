//! Cursor-grade multi-language Fill-In-The-Middle inline completion, PyCharm-style popup completion,
//! companion bridge compatibility, and interactive multi-candidate smart completion.

mod cache;
mod catalog;
mod context;
pub mod fim;
pub mod popup;

#[cfg(test)]
mod tests;

use ai_core::{
    ServerConfig, complete as ai_complete, copy_to_clipboard, cut_utf8, notify, report_error,
};
use openidea_plugin::providers::{CompletionItem, Document, InlineSuggestion};
use openidea_plugin::{
    Error, HttpRequest, Invocation, LogLevel, NotificationAction, NotificationLevel, PickItem,
    Position, Project, Range, host, text,
};
use serde::{Deserialize, Serialize};

use crate::selection::{ApplyOutcome, apply_replacement};

const SMART_PICK_PREFIX: &str = "smart:";

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

fn complete_via_openai(doc: &Document, at: Position) -> Result<Option<InlineSuggestion>, Error> {
    if doc.language.trim().is_empty() {
        return Ok(None);
    }
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
        return Ok(Some(InlineSuggestion {
            insert_text: cached,
            replace: None,
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

    let raw = ai_complete(&server, &prompt, "AI Assistant")?;
    let effective_lines = match mode {
        fim::FimMode::SingleLine => 1,
        fim::FimMode::MultiLine { max_lines: m } => m,
    };
    let cleaned = fim::sanitize_fim_output(&raw, line_before, req.after, effective_lines);
    if cleaned.trim().is_empty() {
        return Ok(None);
    }

    cache::store(&doc.path, at.line, line_before, &suffix_fp, &cleaned);
    Ok(Some(InlineSuggestion {
        insert_text: cleaned,
        replace: None,
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

/// Computes an optional inline ghost-text completion suggestion across languages.
#[must_use]
pub fn inline_completion(doc: &Document, at: Position) -> Option<InlineSuggestion> {
    match complete(doc, at) {
        Ok(suggestion) => suggestion,
        Err(Error::Cancelled | Error::Stale) => None,
        Err(_) => {
            host::log(
                LogLevel::Warn,
                "AI completion unavailable; check Server settings or the local completion service",
            );
            None
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

/// Runs the interactive multi-candidate AI Smart Code Completion command (`smart-complete`).
///
/// # Errors
///
/// Returns host errors only if reporting an error notification itself fails.
pub fn run_smart_complete(ctx: &Invocation, plugin_name: &str) -> Result<Option<String>, Error> {
    let project = ctx.project();
    if let Err(err) = execute_smart_complete(ctx, project.as_ref(), plugin_name) {
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

fn execute_smart_complete(
    ctx: &Invocation,
    project: Option<&Project>,
    plugin_name: &str,
) -> Result<(), Error> {
    let editor = ctx
        .editor()
        .ok_or_else(|| Error::Failed("请先打开并聚焦一个代码编辑器。".to_owned()))?;
    let doc = editor.document()?;
    let path = editor.path();
    let language = editor.language();
    let selections = editor.selections()?;
    let primary_range = selections
        .first()
        .copied()
        .unwrap_or_else(|| Range::new(Position::new(0, 0), Position::new(0, 0)));

    let start_offset = text::offset(&doc.text, primary_range.start)
        .ok_or_else(|| Error::Failed("无法定位当前光标位置。".to_owned()))?;
    let end_offset = text::offset(&doc.text, primary_range.end).unwrap_or(start_offset);

    let prefix = doc.text.get(..start_offset).unwrap_or("");
    let suffix = doc.text.get(end_offset..).unwrap_or("");
    let selected_hint = if start_offset < end_offset {
        doc.text.get(start_offset..end_offset).unwrap_or("")
    } else {
        ""
    };

    let limit = parse_context_limit().unwrap_or(24_000);
    let max_lines = parse_max_lines();
    let mut code_ctx = context::extract_code_context(
        prefix,
        &doc.text,
        &path,
        &language,
        primary_range.start.line,
        limit / 4,
    );
    let line_before = prefix.rsplit('\n').next().unwrap_or(prefix);
    context::append_workspace_symbols(&mut code_ctx, &path, line_before, limit / 4);
    let fim_ctx = fim::FimContext {
        path: &path,
        language: &language,
        before: prefix,
        after: suffix,
        context: &code_ctx,
    };
    let prompt = fim::build_smart_candidates_prompt(
        &fim_ctx,
        selected_hint,
        limit.max(ai_core::MIN_MAX_INPUT_BYTES),
    );
    let server = resolve_completion_server(None)?;

    let task = host::begin_task("AI 正在生成多方案智能补全候选", true);
    let input_kb = prompt.total_bytes().div_ceil(1_000);
    task.report(
        35,
        Some(100),
        Some(&format!("请求 {} ({input_kb} KB)...", server.model)),
    );
    if task.is_cancelled() {
        return Err(Error::Cancelled);
    }

    let raw = ai_complete(&server, &prompt, plugin_name)?;
    if task.is_cancelled() {
        return Err(Error::Cancelled);
    }

    task.report(85, Some(100), Some("整理智能补全候选..."));
    let candidates = fim::parse_smart_candidates(&raw, line_before, suffix, max_lines);
    if candidates.is_empty() {
        return Err(Error::Failed(
            "未能从模型输出中提取有效的代码补全候选。".to_owned(),
        ));
    }

    task.report(100, Some(100), None);
    drop(task);

    pick_and_apply_smart_candidate(
        &editor,
        project,
        doc.version,
        primary_range,
        &candidates,
        plugin_name,
    )
}

fn pick_and_apply_smart_candidate(
    editor: &openidea_plugin::TextEditor,
    project: Option<&Project>,
    doc_version: u64,
    primary_range: Range,
    candidates: &[fim::SmartCandidate],
    plugin_name: &str,
) -> Result<(), Error> {
    let items: Vec<PickItem> = candidates
        .iter()
        .enumerate()
        .map(|(idx, cand)| {
            let preview = single_line_preview(&cand.code);
            PickItem::new(
                &format!("{SMART_PICK_PREFIX}{idx}"),
                &format!("{}  |  {preview}", cand.label),
            )
            .description(&cand.description)
        })
        .collect();

    let title = if primary_range.is_empty() {
        "AI 智能代码补全 — 回车直接插入光标处"
    } else {
        "AI 智能代码补全 — 回车替换选中代码"
    };
    let Some(picked_key) = host::pick(title, &items)? else {
        return Ok(());
    };
    let idx: usize = picked_key
        .strip_prefix(SMART_PICK_PREFIX)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| Error::Failed("无效的补全候选项。".into()))?;
    let chosen = candidates
        .get(idx)
        .ok_or_else(|| Error::Failed("未找到所选补全候选项。".into()))?;

    let _ = host::storage::set(crate::LAST_RESULT_KEY, &chosen.code);
    match apply_replacement(editor, doc_version, &[primary_range], &chosen.code)? {
        ApplyOutcome::Applied(_) | ApplyOutcome::Unchanged => Ok(()),
        ApplyOutcome::Stale => {
            let _ = copy_to_clipboard(&chosen.code, plugin_name);
            notify(
                project,
                NotificationLevel::Warning,
                "文档内容已修改，未插入补全（已复制到剪贴板）",
                &single_line_preview(&chosen.code),
                &[NotificationAction::new("复制补全结果", "copy-last-result")
                    .argument(&chosen.code)],
            )
        }
        ApplyOutcome::Denied => {
            copy_to_clipboard(&chosen.code, plugin_name)?;
            notify(
                project,
                NotificationLevel::Info,
                "当前文件只读或未授权编辑，补全代码已复制到剪贴板",
                &single_line_preview(&chosen.code),
                &[],
            )
        }
    }
}

fn single_line_preview(code: &str) -> String {
    let first = code.lines().next().unwrap_or("").trim();
    let short = cut_utf8(first, 64);
    if code.contains('\n') || first.len() > short.len() {
        format!("{short} …")
    } else {
        short.to_owned()
    }
}
