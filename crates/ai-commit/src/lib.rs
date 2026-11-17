//! Commit message domain library for OpenIDEA.
//!
//! Provides git-diff noise classification and optimization ([`diff`]), strict
//! 150 KB prompt construction and normalization ([`prompt`]), Conventional
//! Commits validation ([`conventional`]), and command workflows called by the
//! unified plugin entrypoint.

pub mod config;
pub mod conventional;
pub mod diff;
pub mod prompt;

use ai_core::{ServerConfig, complete, copy_to_clipboard, notify, report_error};
use openidea_plugin::commit::CommitProblem;
use openidea_plugin::{
    CommitDraft, Error, Invocation, NotificationAction, NotificationLevel, Project, host,
};

use config::{CommitStyle, PromptConfig, setting_string};

const LAST_MESSAGE_KEY: &str = "last-message";

fn pending_draft() -> Result<CommitDraft, Error> {
    host::pending_commit()?
        .ok_or_else(|| Error::Failed("This project has no Git repository.".to_owned()))
}

fn deliver_message(message: &str, revision: u64, project: Option<&Project>) -> Result<(), Error> {
    let _ = host::storage::set(LAST_MESSAGE_KEY, message);
    match host::set_commit_message(revision, message) {
        Ok(()) => Ok(()),
        Err(Error::Stale) => {
            let preview: String = message
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(120)
                .collect();
            notify(
                project,
                NotificationLevel::Info,
                "Commit draft changed while generating",
                &format!("Your draft was kept untouched. Suggestion: {preview}"),
                &[
                    NotificationAction::new("Insert anyway", "insert").argument(message),
                    NotificationAction::new("Copy", "copy").argument(message),
                ],
            )
        }
        Err(err) => Err(err),
    }
}

fn stored_or_arg(arg: Option<&str>) -> Result<String, Error> {
    match arg {
        Some(msg) => Ok(msg.to_owned()),
        None => host::storage::get(LAST_MESSAGE_KEY)?
            .ok_or_else(|| Error::Failed("No commit message has been generated yet.".to_owned())),
    }
}

/// Generates a commit message for the pending VCS changes and inserts it into the commit box.
///
/// # Errors
///
/// Returns host/UI notification errors only if error reporting itself fails.
pub fn run_generate(ctx: &Invocation, plugin_name: &str) -> Result<Option<String>, Error> {
    let project = ctx.project();
    if let Err(err) = write_commit_message(project.as_ref(), plugin_name) {
        report_error(
            project.as_ref(),
            "Could not generate commit message",
            "generate",
            err,
            "Retry",
            "Test connection",
        )?;
    }
    Ok(None)
}

fn write_commit_message(project: Option<&Project>, plugin_name: &str) -> Result<(), Error> {
    let draft = pending_draft()?;
    if draft.changes.is_empty() {
        return Err(Error::Failed(
            "Select at least one changed file before generating a commit message.".to_owned(),
        ));
    }

    let server = ServerConfig::from_host()?;
    let prompt_cfg = PromptConfig::from_host();

    let task = host::begin_task("Analyzing diff & generating commit message", true);
    task.report(15, Some(100), Some("Optimizing git diff..."));
    if task.is_cancelled() {
        return Err(Error::Cancelled);
    }

    let payload = prompt::build(&draft, &prompt_cfg);
    let input_kb = payload.total_bytes().div_ceil(1_000);
    let request_status = format!("Requesting {} ({input_kb} KB)...", server.model);
    task.report(45, Some(100), Some(&request_status));
    if task.is_cancelled() {
        return Err(Error::Cancelled);
    }

    let raw_answer = complete(&server, &payload, plugin_name)?;
    if task.is_cancelled() {
        return Err(Error::Cancelled);
    }

    task.report(90, Some(100), Some("Applying commit message..."));
    let cleaned = prompt::clean(&raw_answer);
    if cleaned.is_empty() {
        return Err(Error::Failed(
            "The model returned an empty commit message.".to_owned(),
        ));
    }

    task.report(100, Some(100), None);
    drop(task);
    deliver_message(&cleaned, draft.revision, project)
}

/// Inserts the last generated (or argument-provided) commit message into the active commit draft.
///
/// # Errors
///
/// Returns [`Error::Stale`] if the commit draft changed, or [`Error::Failed`] if no message exists.
pub fn run_insert(arg: Option<&str>) -> Result<Option<String>, Error> {
    let message = stored_or_arg(arg)?;
    let draft = pending_draft()?;
    host::set_commit_message(draft.revision, &message)?;
    Ok(None)
}

/// Copies the last generated (or argument-provided) commit message to the clipboard.
///
/// # Errors
///
/// Returns [`Error::Failed`] if no message exists or clipboard permission is denied.
pub fn run_copy(arg: Option<&str>, plugin_name: &str) -> Result<Option<String>, Error> {
    let message = stored_or_arg(arg)?;
    copy_to_clipboard(&message, plugin_name)?;
    Ok(None)
}

/// Validates the prepared commit message against Conventional Commits rules when enabled.
#[must_use]
pub fn check_conventional(check: &str, draft: &CommitDraft) -> Vec<CommitProblem> {
    let style = CommitStyle::from_setting(&setting_string("style", "conventional"));
    if check != "conventional" || !style.is_conventional() {
        return Vec::new();
    }
    conventional::problem(&draft.message)
        .map(CommitProblem::new)
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use openidea_plugin::mock::MockHost;
    use openidea_plugin::{CommitChange, HttpResponse};

    use super::*;

    const TEST_CAPS: &[&str] = &[
        "vcs.read",
        "vcs.write",
        "storage",
        "net:127.0.0.1",
        "net:ask",
        "secrets",
        "clipboard",
    ];

    fn host_with_draft(message: &str) -> MockHost {
        let host = MockHost::install().with_capabilities(TEST_CAPS);
        host.set_project_root(Some("/repo"));
        host.set_commit_draft(Some(CommitDraft {
            revision: 7,
            message: message.to_owned(),
            amend: false,
            changes: vec![CommitChange {
                repository: "/repo".to_owned(),
                path: "/repo/src/login.rs".to_owned(),
                old_path: None,
                status: "modified".to_owned(),
                diff: Some(
                    "diff --git a/src/login.rs b/src/login.rs\nindex 111..222 100644\n--- a/src/login.rs\n+++ b/src/login.rs\n@@ -1 +1 @@\n-panic!()\n+return Err(e)\n"
                        .to_owned(),
                ),
                truncated: false,
            }],
        }));
        host
    }

    fn last_notice(host: &MockHost) -> (String, Vec<String>) {
        let (root, notice) = host
            .project_notifications()
            .pop()
            .expect("expected project notification");
        assert_eq!(root, "/repo");
        let commands = notice.actions.into_iter().map(|a| a.command).collect();
        (notice.notification.body, commands)
    }

    #[test]
    fn generates_using_fixed_default_openai_compatible_server() {
        let host = host_with_draft("");
        host.on_fetch(|req| {
            assert_eq!(req.url, "http://127.0.0.1:8045/v1/chat/completions");
            assert!(req.headers.contains(&(
                "authorization".to_owned(),
                "Bearer sk-035fcf9af54b4e45af8de0f750784c56".to_owned(),
            )));
            let body: Value = serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();
            assert_eq!(body["model"], "gemini-3.8-flash-low");
            assert_eq!(body["stream"], false);
            let user_msg = body["messages"][1]["content"].as_str().unwrap();
            assert!(!user_msg.contains("diff --git"));
            assert!(user_msg.contains("+return Err(e)"));
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: br#"{"choices":[{"message":{"content":"<think>check</think>\n```\nfix(login): return error instead of panicking\n```"}}]}"#
                    .to_vec(),
            })
        });

        run_generate(&host.invocation(), "AI Assistant").unwrap();

        assert_eq!(
            host.commit_draft().unwrap().message,
            "fix(login): return error instead of panicking"
        );
        assert_eq!(
            host.storage().get(LAST_MESSAGE_KEY).unwrap(),
            "fix(login): return error instead of panicking"
        );
        assert_eq!(host.project_notifications().len(), 0);
        assert_eq!(host.tasks().len(), 1);
    }

    #[test]
    fn keychain_secret_overrides_default_api_key_for_custom_server() {
        let host = host_with_draft("");
        host.set_setting("baseUrl", "\"https://api.example.com/v1\"");
        host.set_setting("model", "\"custom-model\"");
        host.set_secret("api-key", "sk-override");
        host.on_fetch(|req| {
            assert_eq!(req.url, "https://api.example.com/v1/chat/completions");
            assert!(
                req.headers
                    .contains(&("authorization".to_owned(), "Bearer sk-override".to_owned()))
            );
            let body: Value = serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();
            assert_eq!(body["model"], "custom-model");
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: br#"{"choices":[{"message":{"content":"feat: use custom server"}}]}"#
                    .to_vec(),
            })
        });

        let host = host.with_capabilities(&[TEST_CAPS, &["net:api.example.com"]].concat());
        run_generate(&host.invocation(), "AI Assistant").unwrap();
        assert_eq!(
            host.commit_draft().unwrap().message,
            "feat: use custom server"
        );
    }

    #[test]
    fn empty_api_key_omits_authorization_header() {
        let host = host_with_draft("");
        host.set_setting("apiKey", "\"\"");
        host.on_fetch(|req| {
            assert!(req.headers.iter().all(|(k, _)| k != "authorization"));
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: br#"{"choices":[{"message":{"content":"chore: local no auth"}}]}"#.to_vec(),
            })
        });

        run_generate(&host.invocation(), "AI Assistant").unwrap();
        assert_eq!(host.commit_draft().unwrap().message, "chore: local no auth");
    }

    #[test]
    fn cancelled_request_stays_quiet_and_preserves_draft() {
        let host = host_with_draft("my draft");
        host.on_fetch(|_| Err(Error::Cancelled));

        run_generate(&host.invocation(), "AI Assistant").unwrap();
        assert_eq!(host.commit_draft().unwrap().message, "my draft");
        assert_eq!(host.project_notifications().len(), 0);
    }

    #[test]
    fn stale_draft_offers_insert_and_copy_without_overwriting() {
        let host = host_with_draft("");
        host.edit_commit_message("user kept typing");
        let project = host.invocation().project();
        deliver_message("feat: x", 7, project.as_ref()).unwrap();

        assert_eq!(host.commit_draft().unwrap().message, "user kept typing");
        assert_eq!(last_notice(&host).1, ["insert", "copy"]);

        run_insert(Some("feat: x")).unwrap();
        assert_eq!(host.commit_draft().unwrap().message, "feat: x");
    }

    #[test]
    fn error_notification_offers_retry_and_test_connection() {
        let host = host_with_draft("");
        host.on_fetch(|_| {
            Ok(HttpResponse {
                status: 500,
                headers: vec![],
                body: br#"{"error":{"message":"upstream down"}}"#.to_vec(),
            })
        });

        run_generate(&host.invocation(), "AI Assistant").unwrap();
        let (body, actions) = last_notice(&host);
        assert!(body.contains("upstream down"), "{body}");
        assert_eq!(actions, ["generate", "test-connection"]);
    }

    #[test]
    fn conventional_check_tracks_style_setting() {
        let host = host_with_draft("Fixed stuff");
        let draft = host.commit_draft().unwrap();
        let problems = check_conventional("conventional", &draft);
        assert_eq!(problems.len(), 1);

        host.set_setting("style", "\"plain\"");
        assert_eq!(check_conventional("conventional", &draft).len(), 0);
    }
}
