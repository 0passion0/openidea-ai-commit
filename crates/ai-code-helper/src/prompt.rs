//! Single-shot prompts strictly bounded within the configured input byte limit.

use std::fmt::Write as _;

use ai_core::{PromptPayload, cut_utf8};

use crate::config::{NamingStyle, TranslationDirection};
use crate::selection::SelectionTarget;

const SYSTEM_TRANSLATE_ZH: &str = "你是专业软件本地化与技术文档翻译专家，负责将输入内容准确翻译为自然流畅的简体中文。\n\n\
必须遵守：\n\
1. 仅输出最终译文纯文本，禁止输出解释、<think> 标签、多余 Markdown 代码块（```）或前后缀说明。\n\
2. 保留代码语法与技术元素：反引号内标识符（如 `fooBar`）、格式化占位符（%s、{0}、{name}）、URL、文件路径不变。\n\
3. 若选中内容本身带有注释符号（如 //、///、#）或引号格式，请在译文中保留相同前缀结构。\n\
4. 如果附带了上下文参考代码，仅用于消除歧义，严禁把上下文一并输出。";

const SYSTEM_TRANSLATE_EN: &str = "You are a senior software localization expert translating technical text and Chinese descriptions into clear, idiomatic English.\n\n\
Rules:\n\
1. Reply with the final translation text only: no explanations, no <think> tags, no extra markdown fences, and no wrapper labels.\n\
2. Preserve inline code identifiers (`fooBar`), format placeholders (%s, {0}, {name}), URLs, file paths, and comment markers (//, ///, #) if present.\n\
3. Keep the translation concise, accurate, and natural for source code comments, docs, or UI copy.\n\
4. Any surrounding code context is read-only reference for disambiguation; do not translate or include the context itself.";

const SYSTEM_NAMING: &str = "你是一名精通多语言编程规范的资深架构师，负责将用户的输入（可能是中文描述、不规范英文短语、拼音或中英混合表达）优化为高质量的标准化英文编程命名。\n\n\
必须严格只输出一个 JSON 对象，格式如下：\n\
{\"candidates\":[{\"tokens\":[\"word1\",\"word2\"],\"meaning\":\"中文语义简述（15字以内）\",\"kind\":\"variable\"}]}\n\n\
约束规则：\n\
1. `tokens` 必须是纯小写英文单词数组（只含 a-z 和 0-9，按语义从主要到次要排列，禁止包含拼音或无关虚词 a/the）。\n\
2. 按准确度与简洁度从高到低返回指定数量的不同语义方案（例如：完整语义方案、简洁常用方案、领域专业方案）。\n\
3. 结合提供的编程语言和上下文代码判断角色（变量、字段、函数动词前缀、类型名、常量等），确保词组精准地道。\n\
4. 禁止输出 <think> 标签、Markdown 围栏或 JSON 之外的任何文本。";

/// Builds a strictly byte-bounded single-shot translation prompt (`system + user <= max_input_bytes`).
#[must_use]
pub fn build_translation_prompt(
    target: &SelectionTarget,
    direction: TranslationDirection,
    max_input_bytes: usize,
) -> PromptPayload {
    let system = match direction {
        TranslationDirection::ZhToEn => SYSTEM_TRANSLATE_EN,
        TranslationDirection::EnToZh | TranslationDirection::Auto => SYSTEM_TRANSLATE_ZH,
    };
    let max_user_bytes = max_input_bytes.saturating_sub(system.len());

    let mut header = format!(
        "File language: {}\nDirection: {}\n",
        target.language,
        match direction {
            TranslationDirection::ZhToEn => "Chinese / mixed intent -> English",
            TranslationDirection::EnToZh | TranslationDirection::Auto => {
                "English -> Simplified Chinese"
            }
        }
    );
    let selected_open = "Selected text to translate:\n<<<\n";
    let selected_close = "\n>>>\n";

    let mut user = String::with_capacity(max_user_bytes.min(8_192));
    append_bounded_section(
        &mut user,
        &mut header,
        selected_open,
        &target.primary_text,
        selected_close,
        &target.context_before,
        &target.context_after,
        max_user_bytes,
    );

    PromptPayload {
        system: system.to_owned(),
        user,
    }
}

/// Builds a strictly byte-bounded single-shot variable naming optimization prompt.
#[must_use]
pub fn build_naming_prompt(
    target: &SelectionTarget,
    candidate_count: usize,
    styles: &[NamingStyle],
    max_input_bytes: usize,
) -> PromptPayload {
    let system = SYSTEM_NAMING;
    let max_user_bytes = max_input_bytes.saturating_sub(system.len());

    let style_names: Vec<&str> = styles.iter().map(|s| s.code()).collect();
    let mut header = format!(
        "编程语言: {}\n需要生成的候选方案数量: {candidate_count}\n目标将用于的命名风格: {}\n",
        target.language,
        style_names.join(", ")
    );
    let selected_open = "待优化命名的输入文本（可能是中文、不规范英文、拼音或中英混合）:\n<<<\n";
    let selected_close = "\n>>>\n";

    let mut user = String::with_capacity(max_user_bytes.min(8_192));
    append_bounded_section(
        &mut user,
        &mut header,
        selected_open,
        &target.primary_text,
        selected_close,
        &target.context_before,
        &target.context_after,
        max_user_bytes,
    );

    PromptPayload {
        system: system.to_owned(),
        user,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "single private helper assembling bounded prompt sections"
)]
fn append_bounded_section(
    out: &mut String,
    header: &mut String,
    selected_open: &str,
    selected_text: &str,
    selected_close: &str,
    context_before: &str,
    context_after: &str,
    max_user_bytes: usize,
) {
    let fixed_header = cut_utf8(header, max_user_bytes);
    out.push_str(fixed_header);
    header.clear();

    let remaining_for_body = max_user_bytes.saturating_sub(out.len());
    let wrapper_len = selected_open.len() + selected_close.len();
    let selected_budget = remaining_for_body.saturating_sub(wrapper_len);
    let bounded_selected = cut_utf8(selected_text, selected_budget);

    out.push_str(selected_open);
    out.push_str(bounded_selected);
    out.push_str(selected_close);

    let context_budget = max_user_bytes.saturating_sub(out.len());
    if context_budget <= 48 || (context_before.is_empty() && context_after.is_empty()) {
        return;
    }

    let mut context_block = String::from("\nSurrounding context (reference only):\n");
    if !context_before.is_empty() {
        let _ = writeln!(context_block, "--- before ---\n{context_before}");
    }
    if !context_after.is_empty() {
        let _ = writeln!(context_block, "--- after ---\n{context_after}");
    }

    let bounded_ctx = cut_utf8(&context_block, context_budget);
    out.push_str(bounded_ctx);
    if out.len() > max_user_bytes {
        out.truncate(cut_utf8(out, max_user_bytes).len());
    }
}

#[cfg(test)]
mod tests {
    use openidea_plugin::{Position, Range};

    use super::*;

    fn target(text: &str, before: &str, after: &str) -> SelectionTarget {
        SelectionTarget {
            path: "/repo/src/order.rs".to_owned(),
            language: "rust".to_owned(),
            doc_version: 1,
            primary_text: text.to_owned(),
            ranges: vec![Range::new(Position::new(1, 0), Position::new(1, 4))],
            context_before: before.to_owned(),
            context_after: after.to_owned(),
            truncated: false,
        }
    }

    #[test]
    fn enforces_150kb_cap_on_large_selections_and_contexts() {
        let huge_text = "x".repeat(200_000);
        let huge_ctx = "y".repeat(40_000);
        let t = target(&huge_text, &huge_ctx, &huge_ctx);

        let tr = build_translation_prompt(&t, TranslationDirection::EnToZh, 150_000);
        assert!(tr.total_bytes() <= 150_000);

        let nm = build_naming_prompt(&t, 3, &NamingStyle::ALL, 150_000);
        assert!(nm.total_bytes() <= 150_000);
    }

    #[test]
    fn builds_structured_naming_prompt_with_context() {
        let t = target("用户订单总金额", "struct Order {", "}");
        let payload = build_naming_prompt(&t, 3, &NamingStyle::ALL, 150_000);
        assert!(payload.system.contains("\"candidates\""));
        assert!(payload.user.contains("用户订单总金额"));
        assert!(payload.user.contains("struct Order {"));
    }
}
