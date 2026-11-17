//! Native inline completion bridge to the loopback Gemini companion.

use openidea_plugin::providers::{Document, InlineSuggestion};
use openidea_plugin::{Error, HttpRequest, LogLevel, Position, host, text};
use serde::{Deserialize, Serialize};

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

fn build_request(doc: &Document, at: Position, limit: usize) -> Option<CompletionRequest<'_>> {
    let offset = text::offset(&doc.text, at)?;
    let (prefix, suffix) = doc.text.split_at(offset);
    let context = python_context(prefix, &doc.text, limit / 4);
    let after = ai_core::cut_utf8(suffix, limit / 4);
    let before_limit = limit - context.len() - after.len();
    let mut start = prefix.len().saturating_sub(before_limit);
    while !prefix.is_char_boundary(start) {
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
        before: prefix.split_at(start).1,
        after,
        context,
    })
}

fn is_scope(line: &str) -> bool {
    line.starts_with("def ") || line.starts_with("async def ") || line.starts_with("class ")
}

fn python_context<'a>(prefix: &'a str, source: &'a str, limit: usize) -> String {
    let mut scopes: Vec<(usize, &str)> = Vec::new();
    for line in prefix.lines() {
        let code = line.trim_start();
        if code.is_empty() || code.starts_with('#') {
            continue;
        }
        let indent = line.len() - code.len();
        while scopes.last().is_some_and(|(level, _)| *level >= indent) {
            scopes.pop();
        }
        if is_scope(code) {
            scopes.push((indent, line));
        }
    }

    // Source hints only; semantic types and diagnostics remain owned by the native LSP.
    let imports = source.lines().filter(|line| {
        let code = line.trim_start();
        code.starts_with("import ") || code.starts_with("from ")
    });
    let signatures = source.lines().filter(|line| is_scope(line.trim_start()));
    let annotations = prefix.lines().rev().filter(|line| {
        let code = line.trim_start();
        code.contains(':') && !code.starts_with('#') && !is_scope(code)
    });
    let mut context = String::new();
    for line in scopes
        .into_iter()
        .map(|(_, line)| line)
        .chain(imports)
        .chain(annotations)
        .chain(signatures)
    {
        if line.len() < limit.saturating_sub(context.len())
            && !context.lines().any(|existing| existing == line)
        {
            context.push_str(line);
            context.push('\n');
        }
    }
    context
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompletionResponse {
    insert_text: String,
}

fn complete(doc: &Document, at: Position) -> Result<Option<InlineSuggestion>, Error> {
    if !host::setting_bool("completionEnabled").unwrap_or(false) || doc.language != "python" {
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
    let result: CompletionResponse = serde_json::from_slice(&response.body)
        .map_err(|_| Error::Failed("Invalid completion service response".into()))?;
    if result.insert_text.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(InlineSuggestion {
        insert_text: result.insert_text,
        replace: None,
    }))
}

/// Computes an optional inline completion suggestion from the local Gemini companion.
#[must_use]
pub fn inline_completion(doc: &Document, at: Position) -> Option<InlineSuggestion> {
    match complete(doc, at) {
        Ok(suggestion) => suggestion,
        Err(Error::Cancelled | Error::Stale) => None,
        Err(_) => {
            host::log(
                LogLevel::Warn,
                "AI completion unavailable; check the local completion service and settings",
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openidea_plugin::HttpResponse;
    use openidea_plugin::mock::MockHost;
    use serde_json::Value;

    fn doc(text: &str) -> Document {
        Document {
            path: "/repo/app.py".into(),
            language: "python".into(),
            version: 7,
            text: text.into(),
        }
    }

    #[test]
    fn splits_at_utf16_caret_and_preserves_suffix_and_imports() {
        let document =
            doc("import pandas as pd\n\ndef process(df):\n    # 中文😀\n    \n    return df\n");
        let request = build_request(&document, Position::new(4, 4), 2048).unwrap();
        assert!(request.before.ends_with("# 中文😀\n    "));
        assert_eq!(request.after, "\n    return df\n");
        assert!(request.context.contains("import pandas as pd"));
        assert!(request.context.contains("def process(df):"));
        let unicode = doc("x = '😀'; ");
        let request = build_request(&unicode, Position::new(0, 10), 2048).unwrap();
        assert_eq!(request.before, unicode.text);
        assert_eq!(request.after, "");
    }

    #[test]
    fn bounds_total_code_context_and_keeps_both_caret_sides() {
        let document = doc(&format!(
            "{}\n    value = \n{}",
            "# 中文😀\n".repeat(500),
            "# 后文\n".repeat(500)
        ));
        let request = build_request(&document, Position::new(501, 12), 512).unwrap();
        assert!(request.before.len() + request.after.len() + request.context.len() <= 512);
        assert!(request.before.ends_with("    value = "));
        assert!(request.after.starts_with('\n'));
    }

    #[test]
    fn completion_is_opt_in_and_returns_ghost_without_editing() {
        let host = MockHost::install().with_capabilities(&["net:127.0.0.1"]);
        let document = doc("result = ");
        assert!(complete(&document, Position::new(0, 9)).unwrap().is_none());
        assert_eq!(host.requests().len(), 0);
        host.set_setting("completionEnabled", "true");
        host.on_fetch(|request| {
            assert_eq!(request.url, "http://127.0.0.1:8765/v1/completion");
            let body: Value = serde_json::from_slice(request.body.as_ref().unwrap()).unwrap();
            assert_eq!(body["path"], "/repo/app.py");
            assert_eq!(body["version"], 7);
            assert_eq!(body["before"], "result = ");
            assert_eq!(body["after"], "");
            assert_eq!(body["position"]["character"], 9);
            assert!(!body.to_string().contains("apiKey"));
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: br#"{"insertText":"process_data()"}"#.to_vec(),
            })
        });
        let suggestion = complete(&document, Position::new(0, 9)).unwrap().unwrap();
        assert_eq!(suggestion.insert_text, "process_data()");
        assert!(suggestion.replace.is_none());
        assert_eq!(document.text, "result = ");
    }

    #[test]
    fn empty_and_failed_predictions_do_not_break_local_completion() {
        let host = MockHost::install().with_capabilities(&["net:127.0.0.1"]);
        host.set_setting("completionEnabled", "true");
        host.on_fetch(|_| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: br#"{"insertText":""}"#.to_vec(),
            })
        });
        assert!(inline_completion(&doc("x = "), Position::new(0, 4)).is_none());
        host.on_fetch(|_| Err(Error::Cancelled));
        assert!(inline_completion(&doc("x = "), Position::new(0, 4)).is_none());
        host.on_fetch(|_| {
            Ok(HttpResponse {
                status: 429,
                headers: vec![],
                body: b"private upstream error".to_vec(),
            })
        });
        assert!(inline_completion(&doc("x = "), Position::new(0, 4)).is_none());
    }

    #[test]
    fn keeps_enclosing_class_and_annotations_when_prefix_is_truncated() {
        let document = doc(&format!(
            "import pandas as pd\nclass Processor:\n    def clean(self, df: pd.DataFrame) -> pd.DataFrame:\n        total: int = 1\n{}        ",
            "        # long body 中文😀\n".repeat(100)
        ));
        let request = build_request(&document, Position::new(104, 8), 2048).unwrap();
        assert!(!request.before.contains("class Processor"));
        assert!(request.context.contains("class Processor:"));
        assert!(
            request
                .context
                .contains("def clean(self, df: pd.DataFrame) -> pd.DataFrame:")
        );
        assert!(request.context.contains("import pandas as pd"));
        assert!(request.context.contains("total: int = 1"));
        for limit in 0..32 {
            let request = build_request(&document, Position::new(104, 8), limit).unwrap();
            assert!(request.before.len() + request.after.len() + request.context.len() <= limit);
        }
    }

    #[test]
    fn preserves_multiline_insertion_whitespace_and_local_settings() {
        let host = MockHost::install().with_capabilities(&["net:127.0.0.1"]);
        host.set_setting("completionEnabled", "true");
        host.set_setting("completionPort", "9000");
        host.set_setting("completionRequestTimeoutMs", "15000");
        host.on_fetch(|request| {
            assert_eq!(request.url, "http://127.0.0.1:9000/v1/completion");
            assert_eq!(request.timeout_ms, Some(15000));
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: br#"{"insertText":"    x = 1\n    return x\n"}"#.to_vec(),
            })
        });
        let suggestion = complete(&doc("def f():\n"), Position::new(1, 0))
            .unwrap()
            .unwrap();
        assert_eq!(suggestion.insert_text, "    x = 1\n    return x\n");
    }

    #[test]
    fn rejects_invalid_settings_and_malformed_service_contract() {
        let host = MockHost::install().with_capabilities(&["net:127.0.0.1"]);
        host.set_setting("completionEnabled", "true");
        host.set_setting("completionPort", "0");
        assert!(matches!(
            complete(&doc("x = "), Position::new(0, 4)),
            Err(Error::Invalid(_))
        ));
        assert_eq!(host.requests().len(), 0);
        host.set_setting("completionPort", "8765");
        host.on_fetch(|_| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: br#"{"text":"bad contract"}"#.to_vec(),
            })
        });
        assert!(matches!(
            complete(&doc("x = "), Position::new(0, 4)),
            Err(Error::Failed(_))
        ));
        let mut document = doc("x = ");
        document.language = "rust".into();
        assert!(complete(&document, Position::new(0, 4)).unwrap().is_none());
        assert_eq!(host.requests().len(), 1);
    }
}
