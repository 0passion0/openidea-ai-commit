//! Shared OpenIDEA notification, error reporting, clipboard, and connection-probe UI flows.

use openidea_plugin::{Error, NotificationAction, NotificationLevel, Project, host};

use crate::client::test_connection;
use crate::config::ServerConfig;

/// Sends a notification scoped to `project` when available, falling back to global UI notice.
///
/// # Errors
///
/// Returns host notification errors if delivery fails.
pub fn notify(
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

/// Reports a failed command with actionable retry and connection-test buttons, suppressing cancellations.
///
/// # Errors
///
/// Propagates `err` if showing the error notification itself fails.
pub fn report_error(
    project: Option<&Project>,
    title: &str,
    retry_command: &str,
    err: Error,
    retry_label: &str,
    test_label: &str,
) -> Result<(), Error> {
    if matches!(err, Error::Cancelled) {
        return Ok(());
    }
    let body = match &err {
        Error::Failed(msg) => msg.clone(),
        other => other.to_string(),
    };
    let mut actions = vec![NotificationAction::new(retry_label, retry_command)];
    if retry_command != "test-connection" {
        actions.push(NotificationAction::new(test_label, "test-connection"));
    }
    notify(project, NotificationLevel::Error, title, &body, &actions).or(Err(err))
}

/// Copies `text` to the system clipboard with an actionable capability error message.
///
/// # Errors
///
/// Returns [`Error::Failed`] if clipboard permission is denied, or propagates other host errors.
pub fn copy_to_clipboard(text: &str, plugin_name: &str) -> Result<(), Error> {
    host::copy_to_clipboard(text).map_err(|err| match err {
        Error::Denied(_) => Error::Failed(format!(
            "Grant the `clipboard` capability in Settings › Plugins › {plugin_name} to copy text."
        )),
        other => other,
    })
}

/// Runs a single-shot connectivity probe against the configured OpenAI-compatible server.
///
/// # Errors
///
/// Propagates configuration errors or unexpected notification delivery failures.
pub fn run_test_connection(project: Option<&Project>, plugin_name: &str) -> Result<(), Error> {
    let server = ServerConfig::from_host()?;
    let task = host::begin_task(&format!("Testing {}", server.model), true);
    task.report(30, Some(100), Some("Connecting..."));

    let answer = match test_connection(&server, plugin_name) {
        Ok(reply) => reply,
        Err(err) => {
            drop(task);
            report_error(
                project,
                "Server connection failed",
                "test-connection",
                err,
                "Retry",
                "Test connection",
            )?;
            return Ok(());
        }
    };

    drop(task);
    let snippet: String = answer.trim().chars().take(80).collect();
    notify(
        project,
        NotificationLevel::Info,
        "OpenAI-compatible server connected",
        &format!("Model `{}` replied: {snippet}", server.model),
        &[],
    )
}
