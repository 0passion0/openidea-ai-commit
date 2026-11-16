//! AI Commit plugin for OpenIDEA.
//!
//! Generates commit messages in a single shot against any OpenAI-compatible
//! server, optimizing `git diff` input and bounding the total prompt to 150 KB.

mod client;
mod config;
mod conventional;
mod diff;
mod prompt;

use openidea_plugin::commit::{CommitChecks, CommitProblem};
use openidea_plugin::{
    CommitDraft, Error, Invocation, NotificationAction, NotificationLevel, Plugin, Project, host,
    register_plugin,
};

use config::{CommitStyle, PromptConfig, ServerConfig, setting_string};

const LAST_MESSAGE_KEY: &str = "last-message";

/// Main OpenIDEA plugin entrypoint.
#[derive(Default)]
pub struct AiCommit;

fn notify(
    project: Option<&Project>,
    level: NotificationLevel,
    title: &str,
    body: &str,
    actions: &[NotificationAction],
) -> Result<(), Error> {
    if let Some(proj) = project {
        match proj.notify(level, title, body, actions) {
            Err(Error::NotFound(_)) => {}
            other => return other,
        }
    }
    host::notify_with_actions(level, title, body, actions)
}

fn report_error(
    project: Option<&Project>,
    title: &str,
    retry_command: &str,
    err: Error,
) -> Result<(), Error> {
    if matches!(err, Error::Cancelled) {
        return Ok(());
    }
    let body = match &err {
        Error::Failed(msg) => msg.clone(),
        other => other.to_string(),
    };
    let mut actions = vec![NotificationAction::new("Retry", retry_command)];
    if retry_command != "test-connection" {
        actions.push(NotificationAction::new(
            "Test connection",
            "test-connection",
        ));
    }
    notify(project, NotificationLevel::Error, title, &body, &actions).or(Err(err))
}

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

impl AiCommit {
    fn run_generate(ctx: &Invocation) -> Result<Option<String>, Error> {
        let project = ctx.project();
        if let Err(err) = Self::write_commit_message(project.as_ref()) {
            report_error(
                project.as_ref(),
                "Could not generate commit message",
                "generate",
                err,
            )?;
        }
        Ok(None)
    }

    fn write_commit_message(project: Option<&Project>) -> Result<(), Error> {
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

        let raw_answer = client::complete(&server, &payload)?;
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

    fn run_insert(arg: Option<&str>) -> Result<Option<String>, Error> {
        let message = stored_or_arg(arg)?;
        let draft = pending_draft()?;
        host::set_commit_message(draft.revision, &message)?;
        Ok(None)
    }

    fn run_copy(arg: Option<&str>) -> Result<Option<String>, Error> {
        let message = stored_or_arg(arg)?;
        host::copy_to_clipboard(&message).map_err(|err| match err {
            Error::Denied(_) => Error::Failed(
                "Grant the `clipboard` capability in Settings › Plugins › AI Commit to copy messages."
                    .to_owned(),
            ),
            other => other,
        })?;
        Ok(None)
    }

    fn run_test_connection(ctx: &Invocation) -> Result<Option<String>, Error> {
        let project = ctx.project();
        let server = ServerConfig::from_host()?;
        let task = host::begin_task(&format!("Testing {}", server.model), true);
        task.report(30, Some(100), Some("Connecting..."));

        let answer = match client::test_connection(&server) {
            Ok(reply) => reply,
            Err(err) => {
                drop(task);
                report_error(
                    project.as_ref(),
                    "Server connection failed",
                    "test-connection",
                    err,
                )?;
                return Ok(None);
            }
        };

        drop(task);
        let snippet: String = prompt::clean(&answer).chars().take(80).collect();
        notify(
            project.as_ref(),
            NotificationLevel::Info,
            "OpenAI-compatible server connected",
            &format!("Model `{}` replied: {snippet}", server.model),
            &[],
        )?;
        Ok(None)
    }
}

impl Plugin for AiCommit {
    fn run_command(
        &mut self,
        id: &str,
        ctx: &Invocation,
        arg: Option<&str>,
    ) -> Result<Option<String>, Error> {
        match id {
            "generate" => Self::run_generate(ctx),
            "insert" => Self::run_insert(arg),
            "copy" => Self::run_copy(arg),
            "test-connection" => Self::run_test_connection(ctx),
            _ => Err(Error::NotFound(format!("unknown command {id}"))),
        }
    }
}

impl CommitChecks for AiCommit {
    fn check(
        &mut self,
        check: &str,
        draft: &CommitDraft,
        _ctx: &Invocation,
    ) -> Result<Vec<CommitProblem>, Error> {
        let style = CommitStyle::from_setting(&setting_string("style", "conventional"));
        if check != "conventional" || !style.is_conventional() {
            return Ok(Vec::new());
        }
        Ok(conventional::problem(&draft.message)
            .map(CommitProblem::new)
            .into_iter()
            .collect())
    }
}

register_plugin!(AiCommit, commit_checks);

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

        AiCommit
            .run_command("generate", &host.invocation(), None)
            .unwrap();

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
        AiCommit
            .run_command("generate", &host.invocation(), None)
            .unwrap();
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

        AiCommit
            .run_command("generate", &host.invocation(), None)
            .unwrap();
        assert_eq!(host.commit_draft().unwrap().message, "chore: local no auth");
    }

    #[test]
    fn cancelled_request_stays_quiet_and_preserves_draft() {
        let host = host_with_draft("my draft");
        host.on_fetch(|_| Err(Error::Cancelled));

        AiCommit
            .run_command("generate", &host.invocation(), None)
            .unwrap();
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

        AiCommit
            .run_command("insert", &host.invocation(), Some("feat: x"))
            .unwrap();
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

        AiCommit
            .run_command("generate", &host.invocation(), None)
            .unwrap();
        let (body, actions) = last_notice(&host);
        assert!(body.contains("upstream down"), "{body}");
        assert_eq!(actions, ["generate", "test-connection"]);
    }

    #[test]
    fn test_connection_reports_model_reply() {
        let host = host_with_draft("");
        host.on_fetch(|req| {
            let body: Value = serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();
            assert_eq!(body["model"], "gemini-3.8-flash-low");
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: br#"{"choices":[{"message":{"content":"OK"}}]}"#.to_vec(),
            })
        });

        AiCommit
            .run_command("test-connection", &host.invocation(), None)
            .unwrap();
        assert!(last_notice(&host).0.contains("OK"));
    }

    #[test]
    fn conventional_check_tracks_style_setting() {
        let host = host_with_draft("Fixed stuff");
        let draft = host.commit_draft().unwrap();
        let problems = AiCommit
            .check("conventional", &draft, &host.invocation())
            .unwrap();
        assert_eq!(problems.len(), 1);

        host.set_setting("style", "\"plain\"");
        assert_eq!(
            AiCommit
                .check("conventional", &draft, &host.invocation())
                .unwrap()
                .len(),
            0
        );
    }
}
