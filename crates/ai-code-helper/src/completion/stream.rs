//! Progressive in-file AI streaming completion with diff highlighting and accept/reject confirmation.

use std::sync::{Mutex, PoisonError};

use ai_core::{complete as ai_complete, copy_to_clipboard, cut_utf8, notify};
use openidea_plugin::{
    Decoration, Document, Error, Invocation, NotificationAction, NotificationLevel, Position,
    Project, Range, StatusItem, Task, TextEdit, TextEditor, host, text,
};

use super::{
    align::{end_position_after_insert, split_into_progressive_chunks},
    cache, compute_replace_range, context, fim, parse_context_limit, parse_max_lines,
    resolve_completion_server,
};
use crate::LAST_RESULT_KEY;

/// Decoration set identifier owning streamed AI completion highlights.
pub const STREAM_DECORATION_OWNER: &str = "ai-smart-complete";
/// Contributed status bar item identifier declared in `plugin.toml`.
pub const STATUS_ITEM_ID: &str = "ai-status";
/// Context key toggled while streamed AI code awaits user confirmation.
pub const PENDING_CONFIRM_CONTEXT_KEY: &str = "pendingConfirm";

const READY_TEXT: &str = "✨ AI 补全: 就绪";
const READY_TOOLTIP: &str =
    "点击触发 AI 智能流式补全 (Alt+Shift+Space) | 自动行内幽灵补全已开启 (Tab 确认)";

/// Snapshot of an in-file streamed completion awaiting confirmation or rollback.
#[derive(Clone, Debug)]
pub struct PendingConfirmation {
    /// Active editor file path.
    pub path: String,
    /// Document version immediately after the final streamed chunk was applied.
    pub version_after_insert: u64,
    /// Full document range occupied by the streamed code.
    pub inserted_range: Range,
    /// Caret position prior to streaming, restored on rejection.
    pub original_cursor: Position,
    /// Original text replaced by streaming (`""` for pure insertions).
    pub replaced_text: String,
    /// Final normalized code inserted into the file.
    pub inserted_code: String,
}

struct StreamInput<'a> {
    path: &'a str,
    language: &'a str,
    doc_text: &'a str,
    prefix: &'a str,
    suffix: &'a str,
    selected_hint: &'a str,
    line_before: &'a str,
    start_line: u32,
}

static PENDING_CONFIRM: Mutex<Option<PendingConfirmation>> = Mutex::new(None);

fn with_pending<R>(f: impl FnOnce(&mut Option<PendingConfirmation>) -> R) -> R {
    let mut guard = PENDING_CONFIRM
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    f(&mut guard)
}

fn set_status_item_text(text: &str, tooltip: &str, command: Option<&str>) {
    let item = StatusItem {
        text: text.to_owned(),
        tooltip: Some(tooltip.to_owned()),
        command: command.map(str::to_owned),
    };
    host::set_status_item(STATUS_ITEM_ID, &item);
}

fn set_ready_status() {
    set_status_item_text(READY_TEXT, READY_TOOLTIP, Some("smart-complete"));
}

fn set_stream_decoration(path: &str, range: Range, hover: &str, after: &str) {
    let deco = Decoration {
        range,
        style: "added".to_owned(),
        hover: Some(hover.to_owned()),
        after: Some(after.to_owned()),
    };
    let _ = host::set_decorations(path, STREAM_DECORATION_OWNER, &[deco]);
}

fn clear_confirmation_visuals() {
    let _ = host::clear_decorations(STREAM_DECORATION_OWNER);
    let _ = host::set_context(PENDING_CONFIRM_CONTEXT_KEY, false);
}

fn accept_active_confirmation() {
    with_pending(|slot| *slot = None);
    clear_confirmation_visuals();
    set_ready_status();
}

/// Updates the status bar item when automatic inline completion yields or clears a suggestion.
pub fn update_inline_status(has_suggestion: bool) {
    if with_pending(|slot| slot.is_some()) {
        return;
    }
    if has_suggestion {
        set_status_item_text(
            "✨ AI 补全: 行内就绪 (Tab)",
            "按 Tab 接受行内幽灵补全 | 按 Alt+Shift+Space 触发流式多行写入",
            Some("smart-complete"),
        );
    } else {
        set_ready_status();
    }
}

/// Clears any pending confirmation decorations and state once the user manually edits the document.
pub fn clear_stale_confirmation_on_edit(path: &str, doc_version: u64) {
    let cleared = with_pending(|slot| {
        if slot
            .as_ref()
            .is_some_and(|p| p.path == path && p.version_after_insert != doc_version)
        {
            *slot = None;
            true
        } else {
            false
        }
    });
    if cleared {
        clear_confirmation_visuals();
        set_ready_status();
    }
}

fn parse_stream_chunk_lines() -> usize {
    host::setting_number("completionStreamChunkLines").map_or(2, |n| {
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let clamped = n.clamp(1.0, 16.0) as usize;
        clamped
    })
}

fn single_line_preview(code: &str) -> String {
    let first = code
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let short = cut_utf8(first, 64);
    if code.contains('\n') || first.len() > short.len() {
        format!("{short}…")
    } else {
        short.to_owned()
    }
}

fn request_ai_stream_code(
    input: &StreamInput<'_>,
    task: &Task,
    plugin_name: &str,
) -> Result<String, Error> {
    set_status_item_text(
        "⏳ AI 补全: 正在思考...",
        "正在结合上下文推理最优代码",
        Some("smart-complete"),
    );
    let limit = parse_context_limit().unwrap_or(24_000);
    let max_lines = parse_max_lines();
    let mut code_ctx = context::extract_code_context(
        input.prefix,
        input.doc_text,
        input.path,
        input.language,
        input.start_line,
        limit / 4,
    );
    context::append_workspace_symbols(&mut code_ctx, input.path, input.line_before, limit / 4);
    let fim_ctx = fim::FimContext {
        path: input.path,
        language: input.language,
        before: input.prefix,
        after: input.suffix,
        context: &code_ctx,
    };
    let prompt = fim::build_direct_stream_prompt(
        &fim_ctx,
        input.selected_hint,
        max_lines,
        limit.max(ai_core::MIN_MAX_INPUT_BYTES),
    );
    let server = resolve_completion_server(None)?;
    let kb = prompt.total_bytes().div_ceil(1_000);
    task.report(
        30,
        Some(100),
        Some(&format!("请求 {} ({kb} KB)...", server.model)),
    );
    if task.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let raw = ai_complete(&server, &prompt, plugin_name)?;
    if task.is_cancelled() {
        return Err(Error::Cancelled);
    }
    task.report(75, Some(100), Some("对齐缩进并准备流式写入..."));
    Ok(fim::sanitize_and_align_fim(
        &raw,
        input.line_before,
        input.suffix,
        input.language,
        max_lines,
    ))
}

fn prepare_stream_edit(
    editor: &TextEditor,
    doc: &Document,
    primary_range: Range,
    task: &Task,
    plugin_name: &str,
) -> Result<(String, Range, String), Error> {
    let path = editor.path();
    let language = editor.language();
    let start = text::offset(&doc.text, primary_range.start)
        .ok_or_else(|| Error::Failed("无法定位当前光标位置。".to_owned()))?;
    let end = text::offset(&doc.text, primary_range.end).unwrap_or(start);
    let prefix = doc.text.get(..start).unwrap_or("");
    let suffix = doc.text.get(end..).unwrap_or("");
    let selected_hint = if start < end {
        doc.text.get(start..end).unwrap_or("")
    } else {
        ""
    };
    let line_before = prefix.rsplit('\n').next().unwrap_or(prefix);

    let cached = primary_range
        .is_empty()
        .then(|| cache::latest_for_line(&path, primary_range.start.line, line_before))
        .flatten()
        .filter(|hit| !hit.trim().is_empty());

    let raw_code = if let Some(hit) = cached {
        task.report(70, Some(100), Some("命中零延迟预测缓存，正在流式写入..."));
        hit
    } else {
        let input = StreamInput {
            path: &path,
            language: &language,
            doc_text: &doc.text,
            prefix,
            suffix,
            selected_hint,
            line_before,
            start_line: primary_range.start.line,
        };
        request_ai_stream_code(&input, task, plugin_name).inspect_err(|_| set_ready_status())?
    };

    let code = raw_code.replace("\r\n", "\n");
    if code.trim().is_empty() {
        set_ready_status();
        return Err(Error::Failed(
            "未能从模型输出中提取有效的代码补全。".to_owned(),
        ));
    }
    let replace_range = if primary_range.is_empty() {
        compute_replace_range(primary_range.start, suffix, &code).unwrap_or(primary_range)
    } else {
        primary_range
    };
    let replaced_text = doc.text_in(replace_range).unwrap_or("").to_owned();
    let _ = host::storage::set(LAST_RESULT_KEY, &code);
    if primary_range.is_empty() {
        cache::store_with_newline_continuation(
            &path,
            primary_range.start.line,
            line_before,
            suffix,
            &code,
        );
    }
    Ok((code, replace_range, replaced_text))
}

fn stream_chunks_into_editor(
    editor: &TextEditor,
    project: Option<&Project>,
    plugin_name: &str,
    initial_version: u64,
    replace_range: Range,
    code: &str,
) -> Result<Option<(u64, Range)>, Error> {
    let path = editor.path();
    let chunks = split_into_progressive_chunks(code, parse_stream_chunk_lines());
    let mut version = initial_version;
    let mut range = replace_range;

    for (step_idx, partial) in chunks.iter().enumerate() {
        let edit = TextEdit {
            range,
            text: partial.clone(),
        };
        match editor.apply_edits(version, &[edit]) {
            Ok(new_version) => {
                version = new_version;
                let end = end_position_after_insert(replace_range.start, partial);
                range = Range::new(replace_range.start, end);
                set_stream_decoration(
                    &path,
                    range,
                    "✨ AI 智能流式补全预览",
                    "  ⚡ [AI 正在流式写入...]",
                );
            }
            Err(Error::Denied(_) | Error::Stale) if step_idx == 0 => {
                set_ready_status();
                let _ = copy_to_clipboard(code, plugin_name);
                notify(
                    project,
                    NotificationLevel::Info,
                    "当前文件不可直接写入或已改动，补全代码已复制到剪贴板",
                    &single_line_preview(code),
                    &[NotificationAction::new("复制补全结果", "copy-last-result").argument(code)],
                )?;
                return Ok(None);
            }
            Err(err) => {
                clear_confirmation_visuals();
                set_ready_status();
                return Err(err);
            }
        }
    }
    Ok(Some((version, range)))
}

fn finalize_confirmation(
    editor: &TextEditor,
    project: Option<&Project>,
    pending: PendingConfirmation,
) {
    let caret_end = Range::new(pending.inserted_range.end, pending.inserted_range.end);
    let _ = editor.set_selections(&[caret_end]);
    let _ = editor.reveal(pending.inserted_range);

    set_stream_decoration(
        &pending.path,
        pending.inserted_range,
        "AI 智能补全已流式写入。\n• 确认保留：Alt+Shift+Enter\n• 撤销回退：Alt+Shift+Backspace",
        "  ✨ [AI 补全待确认 — Alt+Shift+Enter 确认保留 | Alt+Shift+Backspace 撤销回退]",
    );

    let preview = single_line_preview(&pending.inserted_code);
    with_pending(|slot| *slot = Some(pending));
    let _ = host::set_context(PENDING_CONFIRM_CONTEXT_KEY, true);
    set_status_item_text(
        "✨ AI 补全: 待确认 (点击确认)",
        "按 Alt+Shift+Enter 确认保留 | Alt+Shift+Backspace 撤销回退",
        Some("accept-completion"),
    );

    let _ = notify(
        project,
        NotificationLevel::Info,
        "✨ AI 代码补全已流式写入文件",
        &preview,
        &[
            NotificationAction::new("✅ 确认保留", "accept-completion"),
            NotificationAction::new("↩️ 撤销回退", "reject-completion"),
        ],
    );
}

/// Streams a single best AI completion directly into the active editor file with confirmation UI.
///
/// # Errors
///
/// Returns host/network errors when reading the editor or querying the AI backend fails.
pub fn execute_smart_complete(
    ctx: &Invocation,
    project: Option<&Project>,
    plugin_name: &str,
) -> Result<(), Error> {
    let editor = ctx
        .editor()
        .ok_or_else(|| Error::Failed("请先打开并聚焦一个代码编辑器。".to_owned()))?;
    let doc = editor.document()?;
    let path = editor.path();

    if with_pending(|slot| {
        slot.as_ref()
            .is_some_and(|p| p.path == path && p.version_after_insert == doc.version)
    }) {
        accept_active_confirmation();
        return Ok(());
    }
    if with_pending(Option::take).is_some() {
        clear_confirmation_visuals();
    }

    let selections = editor.selections()?;
    let primary_range = selections
        .first()
        .copied()
        .unwrap_or_else(|| Range::new(Position::new(0, 0), Position::new(0, 0)));
    let task = host::begin_task("⚡ AI 正在流式生成代码补全...", true);
    let (code, replace_range, replaced_text) =
        prepare_stream_edit(&editor, &doc, primary_range, &task, plugin_name)?;

    let Some((version_after_insert, inserted_range)) = stream_chunks_into_editor(
        &editor,
        project,
        plugin_name,
        doc.version,
        replace_range,
        &code,
    )?
    else {
        return Ok(());
    };

    task.report(100, Some(100), None);
    drop(task);

    finalize_confirmation(
        &editor,
        project,
        PendingConfirmation {
            path,
            version_after_insert,
            inserted_range,
            original_cursor: primary_range.start,
            replaced_text,
            inserted_code: code,
        },
    );
    Ok(())
}

/// Accepts and keeps the currently pending streamed completion in the editor file.
///
/// # Errors
///
/// Infallible; keeps signature aligned with OpenIDEA command handlers.
#[expect(clippy::unnecessary_wraps)]
pub fn run_accept_completion(_ctx: &Invocation) -> Result<Option<String>, Error> {
    accept_active_confirmation();
    Ok(None)
}

/// Rolls back the pending streamed completion and restores the original selection/caret.
///
/// # Errors
///
/// Returns host errors only if editing or notifying fails unexpectedly.
pub fn run_reject_completion(
    ctx: &Invocation,
    _plugin_name: &str,
) -> Result<Option<String>, Error> {
    let pending = with_pending(Option::take);
    clear_confirmation_visuals();

    let project = ctx.project();
    if let Some(pending) = pending
        && let Some(editor) = ctx.editor()
        && editor.path() == pending.path
    {
        let revert = TextEdit {
            range: pending.inserted_range,
            text: pending.replaced_text,
        };
        match editor.apply_edits(pending.version_after_insert, &[revert]) {
            Ok(_) => {
                let restored = Range::new(pending.original_cursor, pending.original_cursor);
                let _ = editor.set_selections(&[restored]);
                let _ = editor.reveal(restored);
            }
            Err(Error::Stale) => {
                set_ready_status();
                let title = "文档已改动，请按 Ctrl+Z 撤销 AI 补全";
                let _ = notify(project.as_ref(), NotificationLevel::Warning, title, "", &[]);
                return Ok(None);
            }
            Err(err) => {
                set_ready_status();
                return Err(err);
            }
        }
    }

    set_status_item_text("↩️ AI 补全: 已回退", READY_TOOLTIP, Some("smart-complete"));
    Ok(None)
}
