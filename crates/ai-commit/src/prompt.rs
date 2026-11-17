//! Prompt assembly strictly bounded within the model input limit, plus response cleanup.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use ai_core::{
    PromptPayload, cut_utf8, strip_reasoning_tags, unwrap_code_fence, unwrap_outer_quotes,
};
use openidea_plugin::{CommitChange, CommitDraft};

use crate::config::{
    BodyMode, CommitStyle, DEFAULT_LANGUAGE, DraftUsage, PromptConfig, is_chinese_language,
};
use crate::conventional::TYPES;
use crate::diff::{self, FileStats, FileTier, relative_path, status_code};

/// Default Chinese system instruction sent to the OpenAI-compatible chat completions endpoint.
pub const SYSTEM: &str = "你是一名严谨的资深研发工程师，负责根据 Git 变更清单与 diff 生成高质量、规范、详细的中文 Git Commit Message。\n\n\
必须遵守：\n\
1. 严格以提供的文件清单和 diff 事实为依据，严禁编造未出现的类名、函数、字段、需求单号或原因。\n\
2. 输出必须规整、具体、有信息量，严禁空洞概括（如“修改代码”、“优化逻辑”、“修复问题”）。\n\
3. 除 Conventional Commits 的英文 type、scope 以及代码原有技术标识符（文件名、类名、方法名、字段名等）外，标题摘要与正文描述必须全部使用简体中文。\n\
4. 仅输出最终 Git 提交信息纯文本，禁止输出思考过程、<think> 标签、Markdown 代码块（```）、引号或任何前缀引导语。";

const SYSTEM_OTHER: &str = "You are a senior software engineer writing accurate, well-structured, detailed git commit messages strictly from the provided file list and diff. \
Be concrete about modified symbols, behaviors, and reasons. Reply with the commit message only: no preamble, no explanation, no quotes, no code fences, and no thinking tags.";

const MAX_HINT_BYTES: usize = 4_096;
const MAX_INSTRUCTIONS_BYTES: usize = 4_096;
const MIN_BUDGET_FOR_EXAMPLES: usize = 12_000;
const DIFF_FENCE_OPEN: &str = "\nDiff:\n```diff\n";
const DIFF_FENCE_CLOSE: &str = "```\n";
const FOOTER_RESERVE_BYTES: usize = 200;
const MANIFEST_OVERFLOW_RESERVE: usize = 220;

const RESPONSE_LABELS: &[&str] = &[
    "Commit message:",
    "commit message:",
    "Commit Message:",
    "Message:",
    "Commit:",
    "提交信息：",
    "提交信息:",
    "提交说明：",
    "提交说明:",
    "以下是生成的提交信息：",
    "以下是生成的提交信息:",
    "以下是提交信息：",
    "以下是提交信息:",
    "生成的提交信息：",
    "生成的提交信息:",
];

/// Builds the single-shot prompt while keeping `system + user <= cfg.max_input_bytes`.
#[must_use]
pub fn build(draft: &CommitDraft, cfg: &PromptConfig) -> PromptPayload {
    let chinese = is_chinese_language(&cfg.language);
    let system = if chinese { SYSTEM } else { SYSTEM_OTHER };
    let max_user_bytes = cfg.max_input_bytes.saturating_sub(system.len());
    let mut user = String::with_capacity(max_user_bytes.min(32 * 1024));

    append_rules(&mut user, draft, cfg, chinese, max_user_bytes);
    append_hint_and_instructions(&mut user, draft, cfg, chinese);

    let remaining_after_preamble = max_user_bytes.saturating_sub(user.len());
    // Reserve up to 35% of remaining space for the file manifest when commits touch
    // hundreds or thousands of files, preserving most bytes for actual diffs.
    let manifest_budget = (remaining_after_preamble * 35 / 100)
        .max(256)
        .min(remaining_after_preamble);
    append_manifest(&mut user, &draft.changes, manifest_budget, chinese);

    let wrapper_cost = DIFF_FENCE_OPEN.len() + DIFF_FENCE_CLOSE.len() + FOOTER_RESERVE_BYTES;
    let diff_budget = max_user_bytes
        .saturating_sub(user.len())
        .saturating_sub(wrapper_cost);

    let optimized = diff::render_optimized(&draft.changes, diff_budget);
    user.push_str(DIFF_FENCE_OPEN);
    user.push_str(&optimized.text);
    if !optimized.text.is_empty() && !optimized.text.ends_with('\n') {
        user.push('\n');
    }
    user.push_str(DIFF_FENCE_CLOSE);

    append_truncation_note(&mut user, &optimized, max_user_bytes, chinese);

    if user.len() > max_user_bytes {
        user.truncate(cut_utf8(&user, max_user_bytes).len());
    }

    PromptPayload {
        system: system.to_owned(),
        user,
    }
}

fn append_rules(
    out: &mut String,
    draft: &CommitDraft,
    cfg: &PromptConfig,
    chinese: bool,
    max_user_bytes: usize,
) {
    if chinese {
        append_chinese_rules(out, draft, cfg, max_user_bytes);
    } else {
        append_generic_rules(out, draft, cfg);
    }
}

fn append_chinese_rules(
    out: &mut String,
    draft: &CommitDraft,
    cfg: &PromptConfig,
    max_user_bytes: usize,
) {
    out.push_str("请根据下方的变更文件清单与 Git diff，生成规范、详细且明确的中文提交信息。\n\n输出格式与内容要求：\n");
    match cfg.style {
        CommitStyle::Conventional => {
            out.push_str(
                "- 标题行（第 1 行）严格采用 `<type>(<scope>): <中文摘要>` 格式：\n\
                 - type 仅限 `feat`、`fix`、`refactor`、`perf`、`docs`、`style`、`test`、`build`、`ci`、`chore`、`revert`；`refactor` 不用于新增功能或修复 Bug，`style` 不用于业务逻辑修改；若存在破坏性不兼容变更在 `)` 后加 `!`。\n\
                 - scope 必须填写：使用简短小写英文标识主要受影响的模块、包名或功能域（如 `article_ch`、`redis-queue`、`parser`、`auth`、`db`、`tasks`）。\n\
                 - 冒号必须使用半角英文冒号加一个空格（`: `），禁止使用全角冒号 `：`。\n\
                 - 中文摘要以明确动词开头（如`新增`、`引入`、`修复`、`重构`、`优化`、`修正`、`移除`、`完善`），概括核心改动与目标，末尾不加句号，禁止空洞措辞。\n",
            );
        }
        CommitStyle::Plain => {
            out.push_str(
                "- 标题行（第 1 行）使用明确动词开头的简体中文一句话摘要，说明核心变更与范围，末尾不加句号。\n",
            );
        }
        CommitStyle::Gitmoji => {
            out.push_str(
                "- 标题行（第 1 行）以单个最贴合的 gitmoji 开头，空一格后写明确动词开头的简体中文摘要，末尾不加句号。\n",
            );
        }
    }

    match cfg.body {
        BodyMode::Never => out.push_str("- 仅输出第 1 行标题，不输出正文。\n"),
        BodyMode::Always | BodyMode::Auto => {
            out.push_str(
                "- 正文（第 2 行空行后从第 3 行起）：必须输出结构化的要点列表，禁止只输出单行标题，也不要简单复述标题。\n\
                 - 列表格式：每条严格以半角 `- ` 开头，保持单层扁平列表，禁用 `1.` 编号、`*`、缩进子列表、分段小标题或散文段落。\n\
                 - 语言要求：除代码原有文件名、类名、方法/函数名、字段名、配置键名等技术标识符外，所有描述必须全部使用简体中文。\n\
                 - 详细度要求：围绕“改了什么（位置/关键符号）+ 为什么改/背景 + 采用什么实现方案 + 影响或边界防护”展开说明；禁止“修改部分代码”、“优化逻辑”、“修复问题”等空洞概括。\n\
                 - 条目覆盖：小改动也需写出 2~4 条具体实现或校验细节；中大改动写出 4~10 条，按“核心逻辑 -> 模型/接口/辅助函数 -> 配置/脚本 -> 测试/文档”排序；涉及多模块时可用 `- 模块名：具体改动...` 前缀区分。\n\
                 - 脚注规则（可选）：仅当 diff 确含破坏性不兼容变更，或草稿/上下文中明确提及 Issue 编号时，在正文空一行后补充 `BREAKING CHANGE: <中文说明>` 或 `Closes/Fixes/Refs #<编号>`；严禁虚构 Issue 编号。\n",
            );
        }
    }

    out.push_str(
        "- 事实约束：严格依据文件清单与 diff 编写，不猜测或编造未出现的需求、指标或未修改内容。\n",
    );
    if draft.amend {
        out.push_str(
            "- 当前为 amend 修改上次提交：diff 仅体现相对上次提交的增量改动，请结合下方原提交信息整合出完整的新提交信息。\n",
        );
    }

    if cfg.body != BodyMode::Never
        && cfg.style == CommitStyle::Conventional
        && max_user_bytes >= MIN_BUDGET_FOR_EXAMPLES
    {
        out.push_str(
            "\n输出风格示例（仅参考结构、中文表达与详细程度，严禁照抄内容）：\n\
             示例 1：\n\
             fix(parser): 修复字段展开后含空值导致的校验异常\n\n\
             - 在 parse_items 解析阶段过滤空列表元素，防止产生 NaN 空值条目\n\
             - 将期刊别名字段由 old_name 更正为 canonical_name 并同步更新映射\n\
             - 当必填字段缺失时抛出包含字段名的详细错误，避免静默写入脏数据\n\n\
             示例 2：\n\
             refactor(redis-queue): 基于 Lua 脚本重构消息认领与过期清理逻辑\n\n\
             - 新增 claim_pending Lua 脚本原子实现消息认领与超时转移，消除并发消费竞态\n\
             - 将过期清理从每次 publish 触发改为初始化与周期任务执行，降低高频网络开销\n\
             - 模型契约：定义 Pydantic 数据模型校验负载并在失败时输出明确提示\n\
             - 测试与清理：移除旧版兼容入口并补充异常回滚测试用例\n",
        );
    }
}

fn append_generic_rules(out: &mut String, draft: &CommitDraft, cfg: &PromptConfig) {
    let language = {
        let trimmed = cfg.language.trim();
        if trimmed.is_empty() {
            DEFAULT_LANGUAGE
        } else {
            trimmed
        }
    };

    out.push_str("Write a clear, well-structured, detailed commit message for the changes below.\n\nRules:\n");
    let _ = writeln!(out, "- Write all descriptions strictly in {language}.");
    match cfg.style {
        CommitStyle::Plain => {
            out.push_str(
                "- Subject line: a concise imperative summary stating the main change and scope, with no trailing period.\n",
            );
        }
        CommitStyle::Gitmoji => {
            out.push_str(
                "- Subject line: start with one fitting gitmoji, a space, and a clear imperative summary, with no trailing period.\n",
            );
        }
        CommitStyle::Conventional => {
            out.push_str(
                "- Follow Conventional Commits: `type(scope): subject`, type one of feat, fix, refactor, perf, docs, style, test, build, ci, chore, revert (`refactor` is not for features/fixes; `style` is not for behavior); include a concise lowercase scope; add `!` or a `BREAKING CHANGE:` footer when incompatible; use ASCII `: `; no trailing period.\n",
            );
        }
    }

    match cfg.body {
        BodyMode::Never => out.push_str("- Output the subject line only, with no body.\n"),
        BodyMode::Always | BodyMode::Auto => {
            out.push_str(
                "- After one blank line, provide a structured flat bullet list using `- ` prefixes covering every important change.\n\
                 - Be concrete and specific: name the affected modules, symbols, fields, or configs, what changed, and why; avoid vague fillers.\n\
                 - Provide 2-4 specific bullets for small changes and 4-10 bullets for larger multi-module changes.\n",
            );
        }
    }

    out.push_str(
        "- Base everything strictly on the file list and diff; never invent unverified details.\n",
    );
    if draft.amend {
        out.push_str(
            "- Amending previous commit: combine the previous message below with the incremental diff into one complete message.\n",
        );
    }
}

fn append_hint_and_instructions(
    out: &mut String,
    draft: &CommitDraft,
    cfg: &PromptConfig,
    chinese: bool,
) {
    let hint = draft.message.trim();
    if draft.amend && !hint.is_empty() {
        let bounded = cut_utf8(hint, MAX_HINT_BYTES);
        let label = if chinese {
            "\n待修改的原提交信息（需保留并融合增量改动）："
        } else {
            "\nPrevious commit message being amended (keep and extend):"
        };
        let _ = write!(out, "{label}\n<<<\n{bounded}\n>>>\n");
    } else if cfg.draft_usage == DraftUsage::UseAsHint && !hint.is_empty() {
        let bounded = cut_utf8(hint, MAX_HINT_BYTES);
        let label = if chinese {
            "\n作者草稿提示（请结合 diff 按上述规范润色并补全细节）："
        } else {
            "\nAuthor draft notes (rewrite to satisfy rules):"
        };
        let _ = write!(out, "{label}\n<<<\n{bounded}\n>>>\n");
    }

    let extra = cfg.instructions.trim();
    if !extra.is_empty() {
        let bounded = cut_utf8(extra, MAX_INSTRUCTIONS_BYTES);
        let label = if chinese {
            "\n附加要求："
        } else {
            "\nExtra instructions:"
        };
        let _ = write!(out, "{label}\n{bounded}\n");
    }
}

fn append_manifest(out: &mut String, changes: &[CommitChange], budget: usize, chinese: bool) {
    let mut entries: Vec<(FileTier, usize, FileStats, String)> = Vec::with_capacity(changes.len());
    let mut total_added = 0usize;
    let mut total_deleted = 0usize;

    for (idx, change) in changes.iter().enumerate() {
        let rel = relative_path(&change.repository, &change.path);
        let tier = diff::classify_tier(&rel, &change.status, change.diff.is_none());
        let code = status_code(&change.status);
        let stats = change
            .diff
            .as_deref()
            .map_or_else(FileStats::default, diff::summarize_stats);
        total_added += stats.added;
        total_deleted += stats.deleted;

        let path_part = match change.old_path.as_deref() {
            Some(old) => format!("{} -> {rel}", relative_path(&change.repository, old)),
            None => rel,
        };
        let mut line = format!("{code} {path_part}");
        if change.diff.is_none() {
            line.push_str(" [binary]");
        } else if stats.added > 0 || stats.deleted > 0 {
            let _ = write!(line, " (+{}/-{})", stats.added, stats.deleted);
        }
        if change.truncated {
            line.push_str(" [host-truncated]");
        }
        line.push('\n');
        entries.push((tier, idx, stats, line));
    }

    // Prioritize high-signal files if the manifest itself must be summarized.
    entries.sort_by_key(|(tier, idx, _, _)| (*tier, *idx));

    let mut section = String::with_capacity(budget.min(4096));
    if chinese {
        let _ = writeln!(
            section,
            "\n变更文件清单（共 {} 个文件，总计 +{total_added}/-{total_deleted} 行）：",
            changes.len()
        );
    } else {
        let _ = writeln!(
            section,
            "\nChanged files ({}, +{total_added}/-{total_deleted} lines):",
            changes.len()
        );
    }

    let list_cap = budget.saturating_sub(MANIFEST_OVERFLOW_RESERVE);
    let mut omitted_start: Option<usize> = None;
    for (pos, (_, _, _, line)) in entries.iter().enumerate() {
        if section.len() + line.len() > list_cap && pos > 0 {
            omitted_start = Some(pos);
            break;
        }
        if section.len() + line.len() <= budget {
            section.push_str(line);
        } else {
            omitted_start = Some(pos);
            break;
        }
    }

    if let Some(start) = omitted_start
        && let Some(omitted_slice) = entries.get(start..)
    {
        let summary = summarize_omitted_manifest(omitted_slice, changes);
        if section.len() + summary.len() <= budget {
            section.push_str(&summary);
        }
    }

    out.push_str(&section);
}

fn summarize_omitted_manifest(
    omitted: &[(FileTier, usize, FileStats, String)],
    changes: &[CommitChange],
) -> String {
    let mut dir_counts: BTreeMap<String, usize> = BTreeMap::new();
    for (_, orig_idx, _, _) in omitted {
        let dir = changes.get(*orig_idx).map_or_else(
            || ".".to_owned(),
            |c| {
                let rel = relative_path(&c.repository, &c.path);
                rel.split('/').next().unwrap_or(".").to_owned()
            },
        );
        *dir_counts.entry(dir).or_insert(0) += 1;
    }

    let mut groups: Vec<(String, usize)> = dir_counts.into_iter().collect();
    groups.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut summary = format!("... and {} more files (", omitted.len());
    for (i, (dir, count)) in groups.iter().take(5).enumerate() {
        if i > 0 {
            summary.push_str(", ");
        }
        let _ = write!(summary, "{dir}/: {count}");
    }
    if groups.len() > 5 {
        summary.push_str(", ...");
    }
    summary.push_str(")\n");
    summary
}

fn append_truncation_note(
    out: &mut String,
    optimized: &diff::OptimizedDiff,
    max_user_bytes: usize,
    chinese: bool,
) {
    if optimized.truncated_files == 0 && optimized.omitted_files == 0 && !optimized.host_truncated {
        return;
    }

    let note = if chinese {
        let mut s = String::from("（注：为控制在输入限制内，diff 已压缩/截断");
        if optimized.truncated_files > 0 {
            let _ = write!(s, "，裁剪 {} 个文件", optimized.truncated_files);
        }
        if optimized.omitted_files > 0 {
            let _ = write!(s, "，{} 个文件仅保留在清单中", optimized.omitted_files);
        }
        s.push_str("；请结合变更文件清单与可见 diff 归纳，切勿臆测未展示细节。）\n");
        s
    } else {
        let mut s = String::from("(Input diff compacted/truncated to stay within limit");
        if optimized.truncated_files > 0 {
            let _ = write!(s, "; {} file diff(s) trimmed", optimized.truncated_files);
        }
        if optimized.omitted_files > 0 {
            let _ = write!(
                s,
                "; {} file diff(s) summarized in file list",
                optimized.omitted_files
            );
        }
        s.push_str(".)\n");
        s
    };

    if out.len() + note.len() <= max_user_bytes {
        out.push_str(&note);
    }
}

/// Cleans and normalizes model output into a uniform Git commit message.
#[must_use]
pub fn clean(answer: &str) -> String {
    let no_reasoning = strip_reasoning_tags(answer);

    let mut text = strip_leading_label(no_reasoning.trim());
    text = unwrap_code_fence(&text);
    text = strip_leading_label(&text);
    text = unwrap_outer_quotes(&text);

    normalize_commit_structure(&text)
}

fn strip_leading_label(input: &str) -> String {
    let trimmed = input.trim();
    for label in RESPONSE_LABELS {
        if let Some(rest) = trimmed.strip_prefix(label) {
            return rest.trim().to_owned();
        }
    }
    trimmed.to_owned()
}

fn normalize_commit_structure(input: &str) -> String {
    let raw_lines: Vec<String> = input
        .lines()
        .map(|l| l.trim_end().to_owned())
        .skip_while(String::is_empty)
        .collect();

    let Some(first_raw) = raw_lines.first() else {
        return String::new();
    };

    let mut out: Vec<String> = Vec::with_capacity(raw_lines.len() + 1);
    out.push(normalize_subject_line(first_raw));

    let remaining = raw_lines.get(1..).unwrap_or(&[]);
    let first_non_empty_idx = remaining.iter().position(|l| !l.is_empty());
    let Some(start_idx) = first_non_empty_idx else {
        return out.join("\n");
    };

    // Separate subject from body with one blank line if the model omitted it before bullets.
    if start_idx == 0 {
        if remaining
            .first()
            .is_some_and(|line| is_list_item(line.trim_start()))
        {
            out.push(String::new());
        }
    } else {
        out.push(String::new());
    }

    for line in remaining.get(start_idx..).unwrap_or(&[]) {
        if line.is_empty() {
            if out.last().is_some_and(|prev| !prev.is_empty()) {
                out.push(String::new());
            }
            continue;
        }
        out.push(normalize_body_line(line));
    }

    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out.join("\n")
}

fn normalize_subject_line(raw: &str) -> String {
    let trimmed = raw.trim();
    let unbolded = trimmed
        .strip_prefix("**")
        .and_then(|s| s.strip_suffix("**"))
        .map_or(trimmed, str::trim);

    let normalized_colon = normalize_conventional_separator(unbolded);
    let no_period = if normalized_colon.ends_with("..") {
        normalized_colon.as_str()
    } else {
        normalized_colon.trim_end_matches(['.', '。']).trim_end()
    };
    no_period.to_owned()
}

fn normalize_conventional_separator(subject: &str) -> String {
    let Some((sep_idx, sep_char)) = subject
        .char_indices()
        .find(|&(_, ch)| ch == ':' || ch == '：')
    else {
        return subject.to_owned();
    };

    let prefix = subject.get(..sep_idx).unwrap_or("").trim();
    let head = prefix.strip_suffix('!').unwrap_or(prefix).trim_end();
    let kind = head.split_once('(').map_or(head, |(k, _)| k.trim());
    if !TYPES.contains(&kind) {
        return subject.to_owned();
    }

    let after_idx = sep_idx + sep_char.len_utf8();
    let rest = subject.get(after_idx..).unwrap_or("").trim_start();
    if rest.is_empty() {
        return subject.to_owned();
    }
    format!("{prefix}: {rest}")
}

fn is_list_item(trimmed_start: &str) -> bool {
    trimmed_start.starts_with("- ")
        || trimmed_start.starts_with("• ")
        || trimmed_start.starts_with("· ")
        || (trimmed_start.starts_with("* ") && !trimmed_start.starts_with("**"))
}

fn normalize_body_line(line: &str) -> String {
    let leading_spaces = line.len() - line.trim_start().len();
    if leading_spaces > 0 {
        return line.to_owned();
    }

    for marker in ["• ", "· ", "* "] {
        if let Some(rest) = line.strip_prefix(marker) {
            let clean_rest = rest.trim_start();
            if !clean_rest.is_empty() {
                return format!("- {clean_rest}");
            }
        }
    }

    if let Some(rest) = line.strip_prefix("- ") {
        let trimmed_rest = rest.trim_start();
        if trimmed_rest.len() != rest.len() && !trimmed_rest.is_empty() {
            return format!("- {trimmed_rest}");
        }
    }

    line.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(path: &str, diff: Option<&str>) -> CommitChange {
        CommitChange {
            repository: "/repo".to_owned(),
            path: format!("/repo/{path}"),
            old_path: None,
            status: "modified".to_owned(),
            diff: diff.map(str::to_owned),
            truncated: false,
        }
    }

    fn draft(message: &str, changes: Vec<CommitChange>) -> CommitDraft {
        CommitDraft {
            revision: 1,
            message: message.to_owned(),
            amend: false,
            changes,
        }
    }

    #[test]
    fn enforces_strict_150kb_input_cap_even_with_650_huge_files() {
        let mut large_diff = String::new();
        for i in 0..120 {
            let _ = writeln!(
                large_diff,
                "+pub fn generated_item_{i}() -> usize {{ {i} }}"
            );
        }
        let changes: Vec<CommitChange> = (0..650)
            .map(|i| {
                change(
                    &format!("src/module_{}/file_{i}.rs", i % 10),
                    Some(&large_diff),
                )
            })
            .collect();

        let payload = build(&draft("", changes), &PromptConfig::default());
        assert!(
            payload.total_bytes() <= 150_000,
            "total bytes {} exceeded 150,000",
            payload.total_bytes()
        );
        assert!(payload.user.contains("变更文件清单（共 650 个文件"));
        assert!(payload.user.contains("src/module_0/file_0.rs (+120/-0)"));
    }

    #[test]
    fn enforces_small_budget_by_skipping_optional_examples() {
        let cfg = PromptConfig {
            max_input_bytes: 4_000,
            ..PromptConfig::default()
        };
        let payload = build(
            &draft("", vec![change("src/lib.rs", Some("+pub fn x() {}\n"))]),
            &cfg,
        );
        assert!(payload.total_bytes() <= 4_000);
        assert!(!payload.user.contains("输出风格示例"));
        assert!(payload.user.contains("+pub fn x() {}"));
    }

    #[test]
    fn builds_detailed_chinese_prompt_by_default() {
        let payload = build(
            &draft("", vec![change("src/auth.rs", Some("+fn login() {}\n"))]),
            &PromptConfig::default(),
        );
        assert!(payload.system.contains("简体中文"));
        assert!(payload.user.contains("输出格式与内容要求"));
        assert!(payload.user.contains("输出风格示例"));
        assert!(payload.user.contains("M src/auth.rs (+1/-0)"));
    }

    #[test]
    fn respects_explicit_english_language_override() {
        let cfg = PromptConfig {
            language: "English".to_owned(),
            ..PromptConfig::default()
        };
        let payload = build(
            &draft("", vec![change("src/auth.rs", Some("+fn login() {}\n"))]),
            &cfg,
        );
        assert!(payload.system.contains("senior software engineer"));
        assert!(
            payload
                .user
                .contains("Write all descriptions strictly in English.")
        );
        assert!(payload.user.contains("Changed files (1, +1/-0 lines):"));
    }

    #[test]
    fn cleans_thinking_tags_fences_labels_quotes_and_normalizes_chinese_structure() {
        assert_eq!(
            clean("<think>analyzing diff</think>\n```\nfeat: 新增导出功能\n```"),
            "feat: 新增导出功能"
        );
        assert_eq!(
            clean(
                "<thought>reasoning</thought>\n提交信息：\n```gitcommit\n**fix(api)：修复参数为空导致崩溃的问题。**\n* 检查输入列表是否为空\n•  补充单元测试\n```"
            ),
            "fix(api): 修复参数为空导致崩溃的问题\n\n- 检查输入列表是否为空\n- 补充单元测试"
        );
        assert_eq!(
            clean("feat(core):新增缓存\n- 初始化 LRU 池"),
            "feat(core): 新增缓存\n\n- 初始化 LRU 池"
        );
        assert_eq!(clean("Commit message: fix: 修复异常"), "fix: 修复异常");
        assert_eq!(clean("\"docs: 更新文档\""), "docs: 更新文档");
        assert_eq!(clean("  chore: 调整配置。\r\n"), "chore: 调整配置");
    }

    #[test]
    fn respects_draft_usage_and_amend() {
        let mut d = draft("修复登录空指针异常", vec![change("a.rs", Some("+x\n"))]);
        let payload = build(&d, &PromptConfig::default());
        assert!(payload.user.contains("修复登录空指针异常"));

        let no_draft = PromptConfig {
            draft_usage: DraftUsage::Ignore,
            ..PromptConfig::default()
        };
        assert!(!build(&d, &no_draft).user.contains("修复登录空指针异常"));

        d.amend = true;
        let amend_payload = build(&d, &no_draft);
        assert!(amend_payload.user.contains("amend"));
        assert!(amend_payload.user.contains("修复登录空指针异常"));
    }
}
