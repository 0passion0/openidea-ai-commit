//! Curated postfix templates, stdlib/ecosystem auto-import entries, and structural snippets.

use openidea_plugin::providers::CompletionKind;

/// `(key, title, documentation)` metadata for postfix templates.
pub const POSTFIX_SPECS: &[(&str, &str, &str)] = &[
    (
        "if",
        "Wrap in if condition",
        "Wraps `<expr>` in a conditional `if` block.",
    ),
    (
        "ifn",
        "Check if None / null / nil",
        "Wraps `<expr>` in a null/None check.",
    ),
    (
        "nn",
        "Check if non-None / Some / non-null",
        "Unwraps or guards `<expr>` when present.",
    ),
    (
        "for",
        "Iterate elements",
        "Expands `<expr>` into a `for` loop.",
    ),
    (
        "var",
        "Introduce local variable",
        "Extracts `<expr>` into a local variable binding.",
    ),
    (
        "let",
        "Bind local variable",
        "Extracts `<expr>` into a `let` / variable binding.",
    ),
    (
        "ret",
        "Return expression",
        "Prepends `return` before `<expr>`.",
    ),
    (
        "print",
        "Print to stdout",
        "Prints `<expr>` using the language's standard printer.",
    ),
    (
        "dbg",
        "Debug print expression",
        "Prints `<expr>` together with its expression label.",
    ),
    (
        "await",
        "Await async value",
        "Applies `await` syntax for the active language.",
    ),
    ("not", "Boolean negation", "Negates `<expr>`."),
    (
        "ok",
        "Wrap in Ok(...)",
        "Wraps `<expr>` in `Ok(...)` (Rust).",
    ),
    (
        "err",
        "Wrap in Err(...)",
        "Wraps `<expr>` in `Err(...)` (Rust).",
    ),
    (
        "some",
        "Wrap in Some(...)",
        "Wraps `<expr>` in `Some(...)` (Rust).",
    ),
    (
        "match",
        "Pattern match / switch",
        "Expands `<expr>` into a `match` or `switch` statement.",
    ),
    (
        "try",
        "Wrap in error handler",
        "Wraps `<expr>` in a `try/catch` or `try/except` block.",
    ),
    (
        "assert",
        "Assert truthiness",
        "Asserts `<expr>` holds at runtime.",
    ),
    (
        "len",
        "Length of collection",
        "Computes collection length of `<expr>`.",
    ),
    (
        "par",
        "Parenthesize expression",
        "Surrounds `<expr>` in parentheses `(...)`.",
    ),
];

/// `(symbol, import_line, module_detail, kind)` entries for stdlib auto-import.
pub type ImportSpec = (&'static str, &'static str, &'static str, CompletionKind);

pub const PYTHON_IMPORTS: &[ImportSpec] = &[
    (
        "Path",
        "from pathlib import Path",
        "pathlib",
        CompletionKind::Class,
    ),
    (
        "dataclass",
        "from dataclasses import dataclass",
        "dataclasses",
        CompletionKind::Function,
    ),
    (
        "defaultdict",
        "from collections import defaultdict",
        "collections",
        CompletionKind::Class,
    ),
    (
        "Counter",
        "from collections import Counter",
        "collections",
        CompletionKind::Class,
    ),
    (
        "deque",
        "from collections import deque",
        "collections",
        CompletionKind::Class,
    ),
    (
        "Optional",
        "from typing import Optional",
        "typing",
        CompletionKind::TypeParameter,
    ),
    (
        "Any",
        "from typing import Any",
        "typing",
        CompletionKind::TypeParameter,
    ),
    (
        "Callable",
        "from typing import Callable",
        "typing",
        CompletionKind::TypeParameter,
    ),
    (
        "Protocol",
        "from typing import Protocol",
        "typing",
        CompletionKind::Interface,
    ),
    (
        "datetime",
        "from datetime import datetime",
        "datetime",
        CompletionKind::Class,
    ),
    (
        "timedelta",
        "from datetime import timedelta",
        "datetime",
        CompletionKind::Class,
    ),
    ("json", "import json", "stdlib", CompletionKind::Module),
    (
        "asyncio",
        "import asyncio",
        "stdlib",
        CompletionKind::Module,
    ),
    (
        "logging",
        "import logging",
        "stdlib",
        CompletionKind::Module,
    ),
    ("re", "import re", "stdlib", CompletionKind::Module),
    ("pytest", "import pytest", "test", CompletionKind::Module),
];

pub const RUST_IMPORTS: &[ImportSpec] = &[
    (
        "HashMap",
        "use std::collections::HashMap;",
        "std::collections",
        CompletionKind::Struct,
    ),
    (
        "HashSet",
        "use std::collections::HashSet;",
        "std::collections",
        CompletionKind::Struct,
    ),
    (
        "BTreeMap",
        "use std::collections::BTreeMap;",
        "std::collections",
        CompletionKind::Struct,
    ),
    (
        "VecDeque",
        "use std::collections::VecDeque;",
        "std::collections",
        CompletionKind::Struct,
    ),
    (
        "Arc",
        "use std::sync::Arc;",
        "std::sync",
        CompletionKind::Struct,
    ),
    (
        "Mutex",
        "use std::sync::Mutex;",
        "std::sync",
        CompletionKind::Struct,
    ),
    (
        "RwLock",
        "use std::sync::RwLock;",
        "std::sync",
        CompletionKind::Struct,
    ),
    (
        "PathBuf",
        "use std::path::PathBuf;",
        "std::path",
        CompletionKind::Struct,
    ),
    (
        "Duration",
        "use std::time::Duration;",
        "std::time",
        CompletionKind::Struct,
    ),
    (
        "Instant",
        "use std::time::Instant;",
        "std::time",
        CompletionKind::Struct,
    ),
    (
        "Serialize",
        "use serde::Serialize;",
        "serde",
        CompletionKind::Interface,
    ),
    (
        "Deserialize",
        "use serde::Deserialize;",
        "serde",
        CompletionKind::Interface,
    ),
];

pub const TS_JS_IMPORTS: &[ImportSpec] = &[
    (
        "useState",
        "import { useState } from \"react\";",
        "react",
        CompletionKind::Function,
    ),
    (
        "useEffect",
        "import { useEffect } from \"react\";",
        "react",
        CompletionKind::Function,
    ),
    (
        "useMemo",
        "import { useMemo } from \"react\";",
        "react",
        CompletionKind::Function,
    ),
    (
        "useCallback",
        "import { useCallback } from \"react\";",
        "react",
        CompletionKind::Function,
    ),
    (
        "useRef",
        "import { useRef } from \"react\";",
        "react",
        CompletionKind::Function,
    ),
    (
        "path",
        "import path from \"node:path\";",
        "node:path",
        CompletionKind::Module,
    ),
    (
        "readFile",
        "import { readFile } from \"node:fs/promises\";",
        "node:fs/promises",
        CompletionKind::Function,
    ),
];

pub const JAVA_IMPORTS: &[ImportSpec] = &[
    (
        "List",
        "import java.util.List;",
        "java.util",
        CompletionKind::Interface,
    ),
    (
        "Map",
        "import java.util.Map;",
        "java.util",
        CompletionKind::Interface,
    ),
    (
        "HashMap",
        "import java.util.HashMap;",
        "java.util",
        CompletionKind::Class,
    ),
    (
        "ArrayList",
        "import java.util.ArrayList;",
        "java.util",
        CompletionKind::Class,
    ),
    (
        "Set",
        "import java.util.Set;",
        "java.util",
        CompletionKind::Interface,
    ),
    (
        "Optional",
        "import java.util.Optional;",
        "java.util",
        CompletionKind::Class,
    ),
    (
        "Collectors",
        "import java.util.stream.Collectors;",
        "java.util.stream",
        CompletionKind::Class,
    ),
];

#[must_use]
pub fn auto_import_table(lang: &str) -> &'static [ImportSpec] {
    match lang {
        "python" => PYTHON_IMPORTS,
        "rust" => RUST_IMPORTS,
        "typescript" | "javascript" => TS_JS_IMPORTS,
        "java" => JAVA_IMPORTS,
        _ => &[],
    }
}

/// Expands a postfix template `key` for `expr` in normalized `lang`.
#[must_use]
pub fn expand_postfix_template(lang: &str, key: &str, expr: &str, indent: &str) -> Option<String> {
    let sub = format!("{indent}    ");
    expand_control_flow_postfix(lang, key, expr, indent, &sub)
        .or_else(|| expand_expr_postfix(lang, key, expr))
}

fn expand_control_flow_postfix(
    lang: &str,
    key: &str,
    expr: &str,
    indent: &str,
    sub: &str,
) -> Option<String> {
    match key {
        "if" => match lang {
            "python" => Some(format!("if {expr}:\n{sub}$0")),
            "rust" | "go" => Some(format!("if {expr} {{\n{sub}$0\n{indent}}}")),
            "typescript" | "javascript" | "java" | "kotlin" | "csharp" => {
                Some(format!("if ({expr}) {{\n{sub}$0\n{indent}}}"))
            }
            _ => None,
        },
        "ifn" => match lang {
            "python" => Some(format!("if {expr} is None:\n{sub}$0")),
            "rust" => Some(format!("if {expr}.is_none() {{\n{sub}$0\n{indent}}}")),
            "typescript" | "javascript" | "java" | "kotlin" | "csharp" => {
                Some(format!("if ({expr} == null) {{\n{sub}$0\n{indent}}}"))
            }
            "go" => Some(format!("if {expr} == nil {{\n{sub}$0\n{indent}}}")),
            _ => None,
        },
        "nn" => match lang {
            "python" => Some(format!("if {expr} is not None:\n{sub}$0")),
            "rust" => Some(format!(
                "if let Some(${{1:val}}) = {expr} {{\n{sub}$0\n{indent}}}"
            )),
            "typescript" | "javascript" | "java" | "kotlin" | "csharp" => {
                Some(format!("if ({expr} != null) {{\n{sub}$0\n{indent}}}"))
            }
            "go" => Some(format!("if {expr} != nil {{\n{sub}$0\n{indent}}}")),
            _ => None,
        },
        "for" => match lang {
            "python" => Some(format!("for ${{1:item}} in {expr}:\n{sub}$0")),
            "rust" => Some(format!("for ${{1:item}} in {expr} {{\n{sub}$0\n{indent}}}")),
            "typescript" | "javascript" => Some(format!(
                "for (const ${{1:item}} of {expr}) {{\n{sub}$0\n{indent}}}"
            )),
            "go" => Some(format!(
                "for _, ${{1:item}} := range {expr} {{\n{sub}$0\n{indent}}}"
            )),
            "java" | "kotlin" | "csharp" => Some(format!(
                "for (var ${{1:item}} : {expr}) {{\n{sub}$0\n{indent}}}"
            )),
            _ => None,
        },
        "match" => match lang {
            "python" => Some(format!("match {expr}:\n{sub}case ${{1:_}}:\n{sub}    $0")),
            "rust" => Some(format!("match {expr} {{\n{sub}${{1:_}} => $0,\n{indent}}}")),
            "typescript" | "javascript" => Some(format!(
                "switch ({expr}) {{\n{sub}case ${{1:value}}:\n{sub}    $0\n{sub}    break;\n{indent}}}"
            )),
            _ => None,
        },
        "try" => match lang {
            "python" => Some(format!(
                "try:\n{sub}{expr}\n{indent}except ${{1:Exception}} as ${{2:exc}}:\n{sub}$0"
            )),
            "typescript" | "javascript" => Some(format!(
                "try {{\n{sub}{expr}\n{indent}}} catch (${{1:err}}) {{\n{sub}$0\n{indent}}}"
            )),
            _ => None,
        },
        _ => None,
    }
}

fn expand_expr_postfix(lang: &str, key: &str, expr: &str) -> Option<String> {
    match key {
        "par" if lang != "generic" => Some(format!("({expr})")),
        "var" | "let" => match lang {
            "python" => Some(format!("${{1:value}} = {expr}")),
            "rust" => Some(format!("let ${{1:value}} = {expr};")),
            "typescript" | "javascript" => Some(format!("const ${{1:value}} = {expr};")),
            "go" => Some(format!("${{1:value}} := {expr}")),
            "java" | "kotlin" | "csharp" => Some(format!("var ${{1:value}} = {expr};")),
            _ => None,
        },
        "ret" => match lang {
            "python" | "go" => Some(format!("return {expr}")),
            "rust" | "typescript" | "javascript" | "java" | "kotlin" | "csharp" => {
                Some(format!("return {expr};"))
            }
            _ => None,
        },
        "print" => match lang {
            "python" => Some(format!("print({expr})")),
            "rust" => Some(format!("println!(\"{{:?}}\", {expr});")),
            "typescript" | "javascript" => Some(format!("console.log({expr});")),
            "go" => Some(format!("fmt.Println({expr})")),
            "java" | "kotlin" | "csharp" => Some(format!("System.out.println({expr});")),
            _ => None,
        },
        "dbg" => match lang {
            "python" => Some(format!("print(f\"{{{expr}=}}\")")),
            "rust" => Some(format!("dbg!({expr})")),
            "typescript" | "javascript" => Some(format!("console.log(\"{expr}:\", {expr});")),
            _ => None,
        },
        "await" => match lang {
            "python" | "typescript" | "javascript" => Some(format!("await {expr}")),
            "rust" => Some(format!("{expr}.await")),
            _ => None,
        },
        "not" => match lang {
            "python" => Some(format!("not {expr}")),
            "rust" | "typescript" | "javascript" | "go" | "java" | "kotlin" | "csharp" => {
                Some(format!("!{expr}"))
            }
            _ => None,
        },
        "ok" if lang == "rust" => Some(format!("Ok({expr})")),
        "err" if lang == "rust" => Some(format!("Err({expr})")),
        "some" if lang == "rust" => Some(format!("Some({expr})")),
        "assert" => match lang {
            "python" => Some(format!("assert {expr}")),
            "rust" => Some(format!("assert!({expr});")),
            _ => None,
        },
        "len" => match lang {
            "python" | "go" => Some(format!("len({expr})")),
            "rust" => Some(format!("{expr}.len()")),
            "typescript" | "javascript" => Some(format!("{expr}.length")),
            _ => None,
        },
        _ => None,
    }
}

/// Returns `(trigger, description, body)` live snippets for the active language.
#[must_use]
pub fn language_snippets(
    lang: &str,
    indent: &str,
    sub: &str,
) -> Vec<(&'static str, &'static str, String)> {
    match lang {
        "python" => vec![
            (
                "def",
                "Function definition",
                format!("def ${{1:name}}(${{2:args}}) -> ${{3:None}}:\n{sub}$0"),
            ),
            (
                "afn",
                "Async function definition",
                format!("async def ${{1:name}}(${{2:args}}) -> ${{3:None}}:\n{sub}$0"),
            ),
            (
                "cls",
                "Class definition",
                format!(
                    "class ${{1:Name}}:\n{sub}def __init__(self, ${{2:args}}) -> None:\n{sub}    $0"
                ),
            ),
            (
                "ifmain",
                "Main entrypoint guard",
                format!("if __name__ == \"__main__\":\n{sub}$0"),
            ),
        ],
        "rust" => vec![
            (
                "fn",
                "Function definition",
                format!("fn ${{1:name}}(${{2:args}}) -> ${{3:()}} {{\n{sub}$0\n{indent}}}"),
            ),
            (
                "pfn",
                "Public function definition",
                format!("pub fn ${{1:name}}(${{2:args}}) -> ${{3:()}} {{\n{sub}$0\n{indent}}}"),
            ),
            (
                "test",
                "Unit test function",
                format!("#[test]\n{indent}fn ${{1:test_name}}() {{\n{sub}$0\n{indent}}}"),
            ),
            (
                "impl",
                "Impl block",
                format!("impl ${{1:Type}} {{\n{sub}$0\n{indent}}}"),
            ),
        ],
        "typescript" | "javascript" => vec![
            (
                "fn",
                "Function declaration",
                format!("function ${{1:name}}(${{2:args}}) {{\n{sub}$0\n{indent}}}"),
            ),
            (
                "afn",
                "Async arrow function",
                format!("const ${{1:name}} = async (${{2:args}}) => {{\n{sub}$0\n{indent}}};"),
            ),
            (
                "tryc",
                "Try / catch block",
                format!("try {{\n{sub}$0\n{indent}}} catch (${{1:error}}) {{\n{sub}\n{indent}}}"),
            ),
        ],
        _ => Vec::new(),
    }
}
