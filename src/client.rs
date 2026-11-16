//! Single-shot OpenAI-compatible HTTP client.

use openidea_plugin::{Error, HttpRequest, host};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::ServerConfig;
use crate::prompt::PromptPayload;

#[derive(Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: [ChatMessage<'a>; 2],
    stream: bool,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Option<ResponseMessage>,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: Option<MessageContent>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Deserialize)]
struct ContentPart {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: Option<ApiErrorDetail>,
}

#[derive(Deserialize)]
struct ApiErrorDetail {
    message: Option<String>,
}

/// Sends a single-shot chat completion request to the configured OpenAI-compatible server.
///
/// # Errors
///
/// Returns [`Error::Cancelled`] when cancelled by the user/task, or [`Error::Failed`]
/// on permission denial, HTTP error, invalid JSON, or empty model response.
pub fn complete(server: &ServerConfig, prompt: &PromptPayload) -> Result<String, Error> {
    let payload = ChatCompletionRequest {
        model: &server.model,
        messages: [
            ChatMessage {
                role: "system",
                content: &prompt.system,
            },
            ChatMessage {
                role: "user",
                content: &prompt.user,
            },
        ],
        stream: false,
    };

    let body = serde_json::to_string(&payload)
        .map_err(|e| Error::Failed(format!("Failed to serialize chat request: {e}")))?;

    let mut req = HttpRequest::post(server.endpoint.as_str(), body)
        .header("content-type", "application/json");
    if let Some(auth) = server.auth.header_value() {
        req = req.header("authorization", &auth);
    }
    req.timeout_ms = Some(server.timeout_ms);

    let res = host::fetch(&req).map_err(|err| match err {
        Error::Denied(_) => Error::Failed(server.endpoint.denied_message()),
        other => other,
    })?;

    let text = res.text();
    if res.status != 200 {
        return Err(extract_api_error(res.status, &text));
    }

    extract_choice_text(&text)
}

/// Sends a minimal one-shot probe to verify URL, auth, and model availability.
///
/// # Errors
///
/// Propagates any transport, permission, or API error from [`complete`].
pub fn test_connection(server: &ServerConfig) -> Result<String, Error> {
    let probe = PromptPayload {
        system: "Reply with one word.".to_owned(),
        user: "Reply with OK.".to_owned(),
    };
    complete(server, &probe)
}

fn extract_api_error(status: u16, body: &str) -> Error {
    let detail = serde_json::from_str::<ErrorEnvelope>(body)
        .ok()
        .and_then(|env| env.error)
        .and_then(|err| err.message)
        .or_else(|| {
            serde_json::from_str::<Value>(body)
                .ok()
                .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_owned))
        })
        .unwrap_or_else(|| body.chars().take(300).collect());

    Error::Failed(format!("Server returned HTTP {status}: {detail}"))
}

fn extract_choice_text(body: &str) -> Result<String, Error> {
    let parsed: ChatCompletionResponse = serde_json::from_str(body)
        .map_err(|e| Error::Failed(format!("Invalid OpenAI-compatible JSON response: {e}")))?;

    let content = parsed
        .choices
        .into_iter()
        .next()
        .and_then(|choice| choice.message)
        .and_then(|msg| msg.content);

    let text = match content {
        Some(MessageContent::Text(s)) => s,
        Some(MessageContent::Parts(parts)) => parts
            .into_iter()
            .filter(|part| part.kind.as_deref().is_none_or(|k| k == "text"))
            .filter_map(|part| part.text)
            .collect::<String>(),
        None => String::new(),
    };

    if text.trim().is_empty() {
        return Err(Error::Failed(
            "The model returned an empty completion.".to_owned(),
        ));
    }

    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthHeader, EndpointUrl};

    #[test]
    fn extracts_string_and_multipart_responses() {
        let plain = r#"{"choices":[{"message":{"content":"feat: ok"}}]}"#;
        assert_eq!(extract_choice_text(plain).unwrap(), "feat: ok");

        let parts = r#"{"choices":[{"message":{"content":[{"type":"text","text":"fix: "},{"type":"text","text":"done"}]}}]}"#;
        assert_eq!(extract_choice_text(parts).unwrap(), "fix: done");

        let empty = r#"{"choices":[{"message":{"content":"   "}}]}"#;
        assert!(extract_choice_text(empty).is_err());
    }

    #[test]
    fn builds_expected_error_message() {
        let err = extract_api_error(401, r#"{"error":{"message":"invalid api key"}}"#);
        assert_eq!(
            err,
            Error::Failed("Server returned HTTP 401: invalid api key".to_owned())
        );

        let srv = ServerConfig {
            endpoint: EndpointUrl::parse("http://127.0.0.1:8045/v1").unwrap(),
            model: "gemini-3.8-flash-low".to_owned(),
            auth: AuthHeader::StaticToken("sk-test".to_owned()),
            timeout_ms: 55_000,
        };
        assert_eq!(
            srv.endpoint.as_str(),
            "http://127.0.0.1:8045/v1/chat/completions"
        );
    }
}
