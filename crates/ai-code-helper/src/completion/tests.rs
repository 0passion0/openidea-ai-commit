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
fn keeps_referenced_variable_definition_when_prefix_is_truncated() {
    let document = doc(&format!(
        "import pandas as pd\ndf = pd.read_csv(\"data.csv\")\n{}df.",
        "# long body 中文😀\n".repeat(200)
    ));
    let request = build_request(&document, Position::new(202, 3), 2048).unwrap();
    assert!(!request.before.contains("df = pd.read_csv"));
    assert!(request.context.contains("df = pd.read_csv(\"data.csv\")"));
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

#[test]
fn openai_fim_works_across_languages_and_uses_zero_ms_prefix_cache() {
    cache::clear();
    let host = MockHost::install().with_capabilities(&["net:127.0.0.1"]);
    host.set_setting("completionEnabled", "true");
    host.set_setting("completionBackend", "openai");
    host.set_setting("completionModel", "gemini-fast-fim");
    host.on_fetch(|request| {
        assert_eq!(request.url, "http://127.0.0.1:8045/v1/chat/completions");
        let body: Value = serde_json::from_slice(request.body.as_ref().unwrap()).unwrap();
        assert_eq!(body["model"], "gemini-fast-fim");
        assert!(
            body["messages"][1]["content"]
                .as_str()
                .unwrap()
                .contains("<PREFIX>")
        );
        Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::json!({
                "choices": [{
                    "message": {
                        "content": "<think>plan</think>\n```rust\nlet total = calculate_discount(items)\n```"
                    }
                }]
            })
            .to_string()
            .into_bytes(),
        })
    });

    let rust_doc = Document {
        path: "/repo/src/lib.rs".into(),
        language: "rust".into(),
        version: 1,
        text: "fn pay(items: &[i32]) {\n    let total = \n}\n".into(),
    };
    let first = complete(&rust_doc, Position::new(1, 16)).unwrap().unwrap();
    assert_eq!(first.insert_text, "calculate_discount(items)");
    assert_eq!(host.requests().len(), 1);

    let typed_further = Document {
        path: "/repo/src/lib.rs".into(),
        language: "rust".into(),
        version: 2,
        text: "fn pay(items: &[i32]) {\n    let total = calc\n}\n".into(),
    };
    let cached = complete(&typed_further, Position::new(1, 20))
        .unwrap()
        .unwrap();
    assert_eq!(cached.insert_text, "ulate_discount(items)");
    assert_eq!(host.requests().len(), 1);
    cache::clear();
}

#[test]
fn smart_complete_command_inserts_picked_candidate_at_cursor() {
    let host = MockHost::install().with_capabilities(&[
        "editor.read",
        "editor.write",
        "clipboard",
        "storage",
        "net:127.0.0.1",
    ]);
    host.set_project_root(Some("/repo"));
    host.open_editor(
        "/repo/src/calc.py",
        "python",
        "def total_price(items):\n    ",
        &[(1, 4, 1, 4)],
    );
    host.on_fetch(|_| {
        Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::json!({
                "choices": [{
                    "message": {
                        "content": "{\"candidates\":[{\"label\":\"简洁求和\",\"description\":\"使用内置 sum\",\"code\":\"return sum(item.price for item in items)\"},{\"label\":\"带空值保护\",\"description\":\"判空后求和\",\"code\":\"if not items:\\n        return 0\\n    return sum(i.price for i in items)\"}]}"
                    }
                }]
            })
            .to_string()
            .into_bytes(),
        })
    });
    host.answer_next_prompt(Some(&["smart:0"]));

    run_smart_complete(&host.invocation(), "AI Assistant").unwrap();
    assert_eq!(
        host.editor_text(),
        "def total_price(items):\n    return sum(item.price for item in items)"
    );
}
