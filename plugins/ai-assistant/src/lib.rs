//! Unified OpenIDEA AI Assistant plugin (`local.ai-commit`).
//!
//! Combines:
//! - One-shot Commit Message Generation (`generate`, `insert`, `copy`,
//!   and `conventional` commit check) via [`ai_commit`].
//! - Editor Selection Translation (`translate`, `replace-last-translation`,
//!   `copy-last-result`) and Variable Naming Optimization (`optimize-naming`),
//!   plus optional Python Inline Completion (`inline_completion`) via
//!   [`ai_code_helper`].
//! - Shared OpenAI-compatible client and connection probe (`test-connection`)
//!   via [`ai_core`].

use ai_core::run_test_connection;
use openidea_plugin::commit::{CommitChecks, CommitProblem};
use openidea_plugin::providers::{CompletionItem, Document, InlineSuggestion, Providers};
use openidea_plugin::{CommitDraft, Error, Invocation, Plugin, Position, register_plugin};

const PLUGIN_DISPLAY_NAME: &str = "AI Assistant";

/// Unified OpenIDEA plugin entrypoint implementing commands, commit checks, and editor providers.
#[derive(Default)]
pub struct AiAssistant;

impl Plugin for AiAssistant {
    fn run_command(
        &mut self,
        id: &str,
        ctx: &Invocation,
        arg: Option<&str>,
    ) -> Result<Option<String>, Error> {
        match id {
            "generate" => ai_commit::run_generate(ctx, PLUGIN_DISPLAY_NAME),
            "insert" => ai_commit::run_insert(arg),
            "copy" => ai_commit::run_copy(arg, PLUGIN_DISPLAY_NAME),
            "translate" => ai_code_helper::run_translate(ctx, PLUGIN_DISPLAY_NAME),
            "optimize-naming" => ai_code_helper::run_optimize_naming(ctx, PLUGIN_DISPLAY_NAME),
            "replace-last-translation" => {
                ai_code_helper::run_replace_last_translation(ctx, arg, PLUGIN_DISPLAY_NAME)
            }
            "copy-last-result" => ai_code_helper::run_copy_last_result(arg, PLUGIN_DISPLAY_NAME),
            "smart-complete" => ai_code_helper::run_smart_complete(ctx, PLUGIN_DISPLAY_NAME),
            "test-connection" => {
                run_test_connection(ctx.project().as_ref(), PLUGIN_DISPLAY_NAME)?;
                Ok(None)
            }
            _ => Err(Error::NotFound(format!("unknown command {id}"))),
        }
    }
}

impl CommitChecks for AiAssistant {
    fn check(
        &mut self,
        check: &str,
        draft: &CommitDraft,
        _ctx: &Invocation,
    ) -> Result<Vec<CommitProblem>, Error> {
        Ok(ai_commit::check_conventional(check, draft))
    }
}

impl Providers for AiAssistant {
    fn completion(
        &mut self,
        doc: &Document,
        at: Position,
        trigger: Option<&str>,
    ) -> Vec<CompletionItem> {
        ai_code_helper::completion::popup_completion(doc, at, trigger)
    }

    fn inline_completion(&mut self, doc: &Document, at: Position) -> Option<InlineSuggestion> {
        ai_code_helper::completion::inline_completion(doc, at)
    }
}

register_plugin!(AiAssistant, commit_checks, providers);

#[cfg(test)]
mod tests {
    use openidea_plugin::mock::MockHost;
    use openidea_plugin::{CommitChange, HttpResponse};
    use serde_json::Value;

    use super::*;

    const ALL_CAPS: &[&str] = &[
        "vcs.read",
        "vcs.write",
        "editor.read",
        "editor.write",
        "clipboard",
        "storage",
        "net:127.0.0.1",
        "net:ask",
        "secrets",
    ];

    #[test]
    fn dispatches_commit_translate_naming_and_test_connection_through_one_plugin() {
        let host = MockHost::install().with_capabilities(ALL_CAPS);
        host.set_project_root(Some("/repo"));
        host.set_commit_draft(Some(CommitDraft {
            revision: 3,
            message: String::new(),
            amend: false,
            changes: vec![CommitChange {
                repository: "/repo".to_owned(),
                path: "/repo/src/lib.rs".to_owned(),
                old_path: None,
                status: "modified".to_owned(),
                diff: Some("+pub fn hello() {}\n".to_owned()),
                truncated: false,
            }],
        }));
        host.open_editor(
            "/repo/src/lib.rs",
            "rust",
            "let 用户编号 = 1;",
            &[(0, 4, 0, 8)],
        );

        host.on_fetch(|req| {
            let body: Value = serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();
            let user_msg = body["messages"][1]["content"].as_str().unwrap_or("");
            let reply = if user_msg.contains("变更文件清单") {
                "feat(lib): 新增 hello 接口"
            } else if user_msg.contains("candidates") || user_msg.contains("待优化命名的输入文本")
            {
                r#"{"candidates":[{"tokens":["user","id"],"meaning":"用户编号"}]}"#
            } else {
                "OK"
            };
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: serde_json::json!({
                    "choices": [{"message": {"content": reply}}]
                })
                .to_string()
                .into_bytes(),
            })
        });

        // 1. Commit message generation
        AiAssistant
            .run_command("generate", &host.invocation(), None)
            .unwrap();
        assert_eq!(
            host.commit_draft().unwrap().message,
            "feat(lib): 新增 hello 接口"
        );

        // 2. Variable naming optimization
        host.answer_next_prompt(Some(&["pick:user_id"]));
        AiAssistant
            .run_command("optimize-naming", &host.invocation(), None)
            .unwrap();
        assert_eq!(host.editor_text(), "let user_id = 1;");

        // 3. Shared server connection test
        AiAssistant
            .run_command("test-connection", &host.invocation(), None)
            .unwrap();
        assert!(
            host.project_notifications()
                .last()
                .unwrap()
                .1
                .notification
                .body
                .contains("OK")
        );

        // 4. Conventional check delegation
        let bad_draft = CommitDraft {
            revision: 4,
            message: "bad message".to_owned(),
            amend: false,
            changes: vec![],
        };
        assert_eq!(
            AiAssistant
                .check("conventional", &bad_draft, &host.invocation())
                .unwrap()
                .len(),
            1
        );

        // 5. Unknown command returns NotFound
        assert!(matches!(
            AiAssistant.run_command("missing", &host.invocation(), None),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn provider_inline_completion_runs_when_enabled() {
        let host = MockHost::install().with_capabilities(ALL_CAPS);
        host.set_setting("completionEnabled", "true");
        host.on_fetch(|_| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: br#"{"insertText":"return 42"}"#.to_vec(),
            })
        });
        let doc = Document {
            path: "/repo/app.py".into(),
            language: "python".into(),
            version: 1,
            text: "def answer():\n    ".into(),
        };
        let suggestion = AiAssistant
            .inline_completion(&doc, Position::new(1, 4))
            .unwrap();
        assert_eq!(suggestion.insert_text, "return 42");
    }

    #[test]
    fn provider_popup_completion_and_smart_complete_work_across_languages() {
        let host = MockHost::install().with_capabilities(ALL_CAPS);
        let rs_doc = Document {
            path: "/repo/src/lib.rs".into(),
            language: "rust".into(),
            version: 1,
            text: "fn f(items: Vec<i32>) {\n    items.for\n}".into(),
        };
        let popup_items = AiAssistant.completion(&rs_doc, Position::new(1, 13), None);
        assert!(
            popup_items
                .iter()
                .any(|item| item.filter_text.as_deref() == Some("for"))
        );

        host.open_editor(
            "/repo/src/lib.rs",
            "rust",
            "fn add(a: i32) -> i32 {\n    \n}",
            &[(1, 4, 1, 4)],
        );
        host.on_fetch(|_| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: serde_json::json!({
                    "choices": [{"message": {"content": "{\"candidates\":[{\"label\":\"Return incremented\",\"description\":\"返回加一\",\"code\":\"a + 1\"}]}"}}]
                })
                .to_string()
                .into_bytes(),
            })
        });
        host.answer_next_prompt(Some(&["smart:0"]));
        AiAssistant
            .run_command("smart-complete", &host.invocation(), None)
            .unwrap();
        assert_eq!(host.editor_text(), "fn add(a: i32) -> i32 {\n    a + 1\n}");
    }
}
