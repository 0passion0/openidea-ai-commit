//! Editor code assistant domain library for OpenIDEA.
//!
//! Provides bilingual translation (`ZH <-> EN`), structured variable naming
//! optimization (`camelCase`, `PascalCase`, `snake_case`,
//! `SCREAMING_SNAKE_CASE`, `kebab-case`), Cursor-grade inline completion
//! ([`completion::inline_completion`]), PyCharm-style popup completion
//! ([`completion::popup_completion`]), and interactive smart completion
//! ([`run_smart_complete`]).

pub mod completion;
pub mod config;
pub mod naming;
pub mod prompt;
pub mod selection;
pub mod translate;

pub use completion::run_smart_complete;

use ai_core::{ServerConfig, complete, copy_to_clipboard, cut_utf8, notify, report_error};
use openidea_plugin::{
    Error, Invocation, NotificationAction, NotificationLevel, Project, TextEditor, host,
};

use config::{CancelCopyPolicy, HelperConfig, PickCopyPolicy, TranslationPresentation};
use naming::COPY_ALL_KEY;
use selection::{ApplyOutcome, EmptySelectionFallback, SelectionTarget};

const LAST_TRANSLATION_KEY: &str = "last-translation";
const LAST_RESULT_KEY: &str = "last-result";

struct EditorDestination<'a> {
    project: Option<&'a Project>,
    editor: &'a TextEditor,
    target: &'a SelectionTarget,
    plugin_name: &'a str,
}

fn preview_snippet(text: &str) -> String {
    let first_line = text.lines().next().unwrap_or_default().trim();
    let short: String = first_line.chars().take(100).collect();
    if first_line.chars().count() > 100 || text.contains('\n') {
        format!("{short}…")
    } else {
        short
    }
}

fn apply_or_offer_copy(
    dest: &EditorDestination<'_>,
    replacement: &str,
    kind_label: &str,
) -> Result<(), Error> {
    match selection::apply_replacement(
        dest.editor,
        dest.target.doc_version,
        &dest.target.ranges,
        replacement,
    )? {
        ApplyOutcome::Applied(_) | ApplyOutcome::Unchanged => Ok(()),
        ApplyOutcome::Stale => {
            let _ = copy_to_clipboard(replacement, dest.plugin_name);
            notify(
                dest.project,
                NotificationLevel::Warning,
                &format!("文档内容已发生变化，未覆盖原文（{kind_label}已复制到剪贴板）"),
                &format!("预览：{}", preview_snippet(replacement)),
                &[NotificationAction::new("复制结果", "copy-last-result").argument(replacement)],
            )
        }
        ApplyOutcome::Denied => {
            copy_to_clipboard(replacement, dest.plugin_name)?;
            notify(
                dest.project,
                NotificationLevel::Info,
                &format!("当前文件只读或未授权编辑，{kind_label}已复制到剪贴板"),
                &preview_snippet(replacement),
                &[],
            )
        }
    }
}

fn deliver_translation(
    dest: &EditorDestination<'_>,
    translated: &str,
    direction_tag: &str,
    cfg: &HelperConfig,
) -> Result<(), Error> {
    let _ = host::storage::set(LAST_TRANSLATION_KEY, translated);
    let _ = host::storage::set(LAST_RESULT_KEY, translated);

    match cfg.presentation {
        TranslationPresentation::ReplaceDirectly => apply_or_offer_copy(dest, translated, "译文"),
        TranslationPresentation::NotifyActions => {
            let title = if dest.target.truncated {
                format!("翻译完成（{direction_tag} · 原文已按预算截断）")
            } else {
                format!("翻译完成（{direction_tag}）")
            };
            notify(
                dest.project,
                NotificationLevel::Info,
                &title,
                &preview_snippet(translated),
                &[
                    NotificationAction::new("替换为译文", "replace-last-translation")
                        .argument(translated),
                    NotificationAction::new("复制译文", "copy-last-result").argument(translated),
                ],
            )
        }
        TranslationPresentation::DialogPreview => {
            let prompt_title = if dest.target.truncated {
                format!("翻译结果（{direction_tag} · 已按上限截断）— OK 替换原文，取消则保留")
            } else {
                format!("翻译结果（{direction_tag}）— 点 OK/Ctrl+Enter 替换原文，取消则保留")
            };
            if let Some(chosen) = host::input_multiline(&prompt_title, Some(translated))? {
                let final_text = if chosen.trim().is_empty() {
                    translated.to_owned()
                } else {
                    chosen
                };
                let _ = host::storage::set(LAST_TRANSLATION_KEY, &final_text);
                let _ = host::storage::set(LAST_RESULT_KEY, &final_text);
                apply_or_offer_copy(dest, &final_text, "译文")
            } else {
                if cfg.cancel_copy == CancelCopyPolicy::CopyTranslation {
                    let _ = copy_to_clipboard(translated, dest.plugin_name);
                }
                let copy_hint = if cfg.cancel_copy == CancelCopyPolicy::CopyTranslation {
                    "译文预览（原文未修改，已自动复制）"
                } else {
                    "译文预览（原文未修改）"
                };
                notify(
                    dest.project,
                    NotificationLevel::Info,
                    copy_hint,
                    &preview_snippet(translated),
                    &[
                        NotificationAction::new("复制译文", "copy-last-result")
                            .argument(translated),
                        NotificationAction::new("替换为译文", "replace-last-translation")
                            .argument(translated),
                    ],
                )
            }
        }
    }
}

/// Translates the focused editor's selected text (or word/line under cursor).
///
/// # Errors
///
/// Returns host errors only if reporting a failure notification itself fails.
pub fn run_translate(ctx: &Invocation, plugin_name: &str) -> Result<Option<String>, Error> {
    let project = ctx.project();
    if let Err(err) = execute_translate(ctx, project.as_ref(), plugin_name) {
        report_error(
            project.as_ref(),
            "翻译失败",
            "translate",
            err,
            "重试",
            "测试连接",
        )?;
    }
    Ok(None)
}

fn execute_translate(
    ctx: &Invocation,
    project: Option<&Project>,
    plugin_name: &str,
) -> Result<(), Error> {
    let editor = ctx
        .editor()
        .ok_or_else(|| Error::Failed("请先打开并聚焦一个代码或文本编辑器。".to_owned()))?;
    let cfg = HelperConfig::from_host();
    let server = ServerConfig::from_host()?;

    let target = selection::extract_target(
        &editor,
        EmptySelectionFallback::WordOrLine,
        cfg.context_lines,
        cfg.max_input_bytes,
    )?;
    let direction = translate::resolve_direction(&target.primary_text, cfg.direction);
    let direction_tag = match direction {
        config::TranslationDirection::ZhToEn => "中 → 英",
        config::TranslationDirection::EnToZh | config::TranslationDirection::Auto => "英 → 中",
    };

    let task = host::begin_task(&format!("AI 正在翻译（{direction_tag}）"), true);
    task.report(20, Some(100), Some("构造翻译请求..."));
    if task.is_cancelled() {
        return Err(Error::Cancelled);
    }

    let payload = prompt::build_translation_prompt(&target, direction, cfg.max_input_bytes);
    let input_kb = payload.total_bytes().div_ceil(1_000);
    task.report(
        45,
        Some(100),
        Some(&format!("请求 {} ({input_kb} KB)...", server.model)),
    );
    if task.is_cancelled() {
        return Err(Error::Cancelled);
    }

    let raw_answer = complete(&server, &payload, plugin_name)?;
    if task.is_cancelled() {
        return Err(Error::Cancelled);
    }

    task.report(90, Some(100), Some("整理译文..."));
    let cleaned = translate::clean_translation(&raw_answer, &target.primary_text);
    if cleaned.is_empty() {
        return Err(Error::Failed("模型返回了空的翻译结果。".to_owned()));
    }

    task.report(100, Some(100), None);
    drop(task);

    let dest = EditorDestination {
        project,
        editor: &editor,
        target: &target,
        plugin_name,
    };
    deliver_translation(&dest, &cleaned, direction_tag, &cfg)
}

/// Optimizes a selected Chinese phrase, imperfect English identifier, or Pinyin
/// into a structured Quick Pick list of standard English variable names.
///
/// # Errors
///
/// Returns host errors only if reporting a failure notification itself fails.
pub fn run_optimize_naming(ctx: &Invocation, plugin_name: &str) -> Result<Option<String>, Error> {
    let project = ctx.project();
    if let Err(err) = execute_optimize_naming(ctx, project.as_ref(), plugin_name) {
        report_error(
            project.as_ref(),
            "变量命名优化失败",
            "optimize-naming",
            err,
            "重试",
            "测试连接",
        )?;
    }
    Ok(None)
}

fn execute_optimize_naming(
    ctx: &Invocation,
    project: Option<&Project>,
    plugin_name: &str,
) -> Result<(), Error> {
    let editor = ctx
        .editor()
        .ok_or_else(|| Error::Failed("请先打开并聚焦一个代码编辑器。".to_owned()))?;
    let cfg = HelperConfig::from_host();
    let server = ServerConfig::from_host()?;

    let target = selection::extract_target(
        &editor,
        EmptySelectionFallback::WordOnly,
        cfg.context_lines,
        cfg.max_input_bytes,
    )?;

    let task = host::begin_task("AI 正在生成结构化变量命名", true);
    task.report(20, Some(100), Some("分析选中词句与代码上下文..."));
    if task.is_cancelled() {
        return Err(Error::Cancelled);
    }

    let payload = prompt::build_naming_prompt(
        &target,
        cfg.naming_candidate_count,
        &cfg.naming_styles,
        cfg.max_input_bytes,
    );
    let input_kb = payload.total_bytes().div_ceil(1_000);
    task.report(
        45,
        Some(100),
        Some(&format!("请求 {} ({input_kb} KB)...", server.model)),
    );
    if task.is_cancelled() {
        return Err(Error::Cancelled);
    }

    let raw_answer = complete(&server, &payload, plugin_name)?;
    if task.is_cancelled() {
        return Err(Error::Cancelled);
    }

    task.report(85, Some(100), Some("生成标准命名列表..."));
    let candidates = naming::parse_model_candidates(&raw_answer, cfg.naming_candidate_count);
    let plan = naming::build_structured_plan(&candidates, &cfg.naming_styles, &target.primary_text)
        .ok_or_else(|| Error::Failed("未能从模型输出中解析出有效的英文命名方案。".to_owned()))?;

    let _ = host::storage::set(LAST_RESULT_KEY, &plan.markdown_summary);

    task.report(100, Some(100), None);
    drop(task);

    let pick_items = plan.to_pick_items();
    let header_input: String = cut_utf8(target.primary_text.trim(), 60).to_owned();
    let title = format!("命名优化「{header_input}」— 回车直接替换选中文本");

    let Some(chosen_key) = host::pick(&title, &pick_items)? else {
        return Ok(());
    };

    if chosen_key == COPY_ALL_KEY {
        copy_to_clipboard(&plan.markdown_summary, plugin_name)?;
        notify(
            project,
            NotificationLevel::Info,
            "已复制全部结构化命名列表",
            &format!("共包含 {} 个标准命名候选", plan.items.len()),
            &[],
        )?;
        return Ok(());
    }

    let identifier = naming::StructuredNamingPlan::identifier_for_key(&chosen_key)
        .ok_or_else(|| Error::Failed("无效的命名候选项。".to_owned()))?;
    let _ = host::storage::set(LAST_RESULT_KEY, identifier);

    if cfg.pick_copy == PickCopyPolicy::ReplaceAndCopy {
        let _ = copy_to_clipboard(identifier, plugin_name);
    }
    let dest = EditorDestination {
        project,
        editor: &editor,
        target: &target,
        plugin_name,
    };
    apply_or_offer_copy(&dest, identifier, "推荐命名")
}

/// Replaces the current editor selection with `arg` or the most recent stored translation.
///
/// # Errors
///
/// Returns [`Error::Failed`] when no focused editor or stored translation is available.
pub fn run_replace_last_translation(
    ctx: &Invocation,
    arg: Option<&str>,
    plugin_name: &str,
) -> Result<Option<String>, Error> {
    let editor = ctx
        .editor()
        .ok_or_else(|| Error::Failed("请先打开并聚焦一个代码或文本编辑器。".to_owned()))?;
    let text = match arg {
        Some(v) => v.to_owned(),
        None => host::storage::get(LAST_TRANSLATION_KEY)?
            .ok_or_else(|| Error::Failed("尚无可用的最近翻译结果。".to_owned()))?,
    };
    let target = selection::extract_target(
        &editor,
        EmptySelectionFallback::WordOrLine,
        0,
        ai_core::DEFAULT_MAX_INPUT_BYTES,
    )?;
    let project = ctx.project();
    let dest = EditorDestination {
        project: project.as_ref(),
        editor: &editor,
        target: &target,
        plugin_name,
    };
    apply_or_offer_copy(&dest, &text, "译文")?;
    Ok(None)
}

/// Copies `arg` or the last stored translation / naming summary to the clipboard.
///
/// # Errors
///
/// Returns [`Error::Failed`] when no stored result exists or clipboard access is denied.
pub fn run_copy_last_result(arg: Option<&str>, plugin_name: &str) -> Result<Option<String>, Error> {
    let text = match arg {
        Some(v) => v.to_owned(),
        None => host::storage::get(LAST_RESULT_KEY)?
            .ok_or_else(|| Error::Failed("尚无可用的最近 AI 结果。".to_owned()))?,
    };
    copy_to_clipboard(&text, plugin_name)?;
    Ok(None)
}

#[cfg(test)]
mod tests {
    use openidea_plugin::HttpResponse;
    use openidea_plugin::mock::MockHost;
    use serde_json::Value;

    use super::*;

    const TEST_CAPS: &[&str] = &[
        "editor.read",
        "editor.write",
        "clipboard",
        "storage",
        "net:127.0.0.1",
        "net:ask",
        "secrets",
    ];

    fn install_host_with_editor(
        language: &str,
        text: &str,
        selections: &[(u32, u32, u32, u32)],
    ) -> MockHost {
        let host = MockHost::install().with_capabilities(TEST_CAPS);
        host.set_project_root(Some("/repo"));
        host.open_editor("/repo/src/main.rs", language, text, selections);
        host
    }

    #[test]
    fn translates_selection_and_replaces_when_dialog_confirmed() {
        let host = install_host_with_editor(
            "rust",
            "// Calculate total order amount\nlet x = 1;",
            &[(0, 0, 0, 31)],
        );
        host.on_fetch(|req| {
            assert_eq!(req.url, "http://127.0.0.1:8045/v1/chat/completions");
            let body: Value = serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();
            assert_eq!(body["model"], "gemini-3.8-flash-low");
            assert!(
                body["messages"][1]["content"]
                    .as_str()
                    .unwrap()
                    .contains("Calculate total order amount")
            );
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body:
                    r#"{"choices":[{"message":{"content":"<think>t</think>\n// 计算订单总金额"}}]}"#
                        .as_bytes()
                        .to_vec(),
            })
        });
        host.answer_next_prompt(Some(&["// 计算订单总金额"]));

        run_translate(&host.invocation(), "AI Assistant").unwrap();

        assert_eq!(host.editor_text(), "// 计算订单总金额\nlet x = 1;");
        assert_eq!(
            host.storage().get(LAST_TRANSLATION_KEY).unwrap(),
            "// 计算订单总金额"
        );
    }

    #[test]
    fn cancelling_translation_dialog_preserves_source_and_offers_copy_and_replace_actions() {
        let host =
            install_host_with_editor("rust", "// Validate request signature", &[(0, 3, 0, 29)]);
        host.on_fetch(|_| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: r#"{"choices":[{"message":{"content":"校验请求签名"}}]}"#
                    .as_bytes()
                    .to_vec(),
            })
        });
        host.answer_next_prompt(None);

        run_translate(&host.invocation(), "AI Assistant").unwrap();

        assert_eq!(host.editor_text(), "// Validate request signature");
        let notices = host.project_notifications();
        assert_eq!(notices.len(), 1);
        let actions: Vec<&str> = notices[0]
            .1
            .actions
            .iter()
            .map(|a| a.command.as_str())
            .collect();
        assert_eq!(actions, ["copy-last-result", "replace-last-translation"]);

        run_copy_last_result(Some("校验请求签名"), "AI Assistant").unwrap();
        assert_eq!(host.clipboard().as_deref(), Some("校验请求签名"));
    }

    #[test]
    fn optimizes_chinese_or_imperfect_english_and_replaces_all_matching_cursors() {
        let host = install_host_with_editor(
            "rust",
            "let 用户订单金额 = 0;\nprintln!(\"{}\", 用户订单金额);",
            &[(0, 4, 0, 10), (1, 15, 1, 21)],
        );
        host.on_fetch(|_| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: r#"{"choices":[{"message":{"content":"{\"candidates\":[{\"tokens\":[\"user\",\"order\",\"amount\"],\"meaning\":\"用户订单金额\",\"kind\":\"variable\"}]}"}}]}"#
                    .as_bytes()
                    .to_vec(),
            })
        });
        host.answer_next_prompt(Some(&["pick:user_order_amount"]));

        run_optimize_naming(&host.invocation(), "AI Assistant").unwrap();

        assert_eq!(
            host.editor_text(),
            "let user_order_amount = 0;\nprintln!(\"{}\", user_order_amount);"
        );
    }

    #[test]
    fn supports_copy_all_structured_table_option_without_modifying_editor() {
        let host = install_host_with_editor("typescript", "const usrInf = {};", &[(0, 6, 0, 12)]);
        host.on_fetch(|_| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: r#"{"choices":[{"message":{"content":"{\"candidates\":[{\"tokens\":[\"user\",\"info\"],\"meaning\":\"用户信息\"}]}"}}]}"#
                    .as_bytes()
                    .to_vec(),
            })
        });
        host.answer_next_prompt(Some(&[COPY_ALL_KEY]));

        run_optimize_naming(&host.invocation(), "AI Assistant").unwrap();

        assert_eq!(host.editor_text(), "const usrInf = {};");
        let copied = host.clipboard().expect("clipboard should contain table");
        assert!(copied.contains("userInfo"));
        assert!(copied.contains("UserInfo"));
        assert!(copied.contains("user_info"));
        assert!(copied.contains("USER_INFO"));
        assert!(copied.contains("user-info"));
    }

    #[test]
    fn falls_back_to_clipboard_when_editor_write_is_denied() {
        let host = MockHost::install().with_capabilities(&[
            "editor.read",
            "clipboard",
            "storage",
            "net:127.0.0.1",
        ]);
        host.set_project_root(Some("/repo"));
        host.open_editor("/repo/ro.rs", "rust", "let 订单编号 = 1;", &[(0, 4, 0, 8)]);
        host.on_fetch(|_| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: r#"{"choices":[{"message":{"content":"{\"candidates\":[{\"tokens\":[\"order\",\"id\"],\"meaning\":\"订单编号\"}]}"}}]}"#
                    .as_bytes()
                    .to_vec(),
            })
        });
        host.answer_next_prompt(Some(&["pick:orderId"]));

        run_optimize_naming(&host.invocation(), "AI Assistant").unwrap();

        assert_eq!(host.editor_text(), "let 订单编号 = 1;");
        assert_eq!(host.clipboard().as_deref(), Some("orderId"));
        assert_eq!(host.project_notifications().len(), 1);
    }

    #[test]
    fn cancelled_task_aborts_silently() {
        let host = install_host_with_editor("rust", "let a = 1;", &[(0, 4, 0, 5)]);
        host.on_fetch(|_| Err(Error::Cancelled));

        run_translate(&host.invocation(), "AI Assistant").unwrap();

        assert_eq!(host.editor_text(), "let a = 1;");
        assert_eq!(host.project_notifications().len(), 0);
    }
}
