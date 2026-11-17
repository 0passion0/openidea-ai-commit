//! Conventional Commits validation for the before-commit check.

/// Recognized Conventional Commits types.
pub const TYPES: &[&str] = &[
    "feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore", "revert",
];

const GIT_GENERATED_PREFIXES: &[&str] = &["Merge ", "Revert \"", "fixup! ", "squash! ", "amend! "];

/// Validates the subject line against `type(scope)!: subject`.
///
/// Returns `None` when valid (or when the message is empty or git-generated).
#[must_use]
pub fn problem(message: &str) -> Option<String> {
    let subject = message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))?;

    if GIT_GENERATED_PREFIXES
        .iter()
        .any(|prefix| subject.starts_with(prefix))
    {
        return None;
    }

    let Some((head, rest)) = subject.split_once(':') else {
        return Some(format!(
            "the subject should start with a type, like \"feat: ...\" or \"fix(scope): ...\" (types: {})",
            TYPES.join(", ")
        ));
    };

    let head = head.strip_suffix('!').unwrap_or(head);
    let kind = match head.split_once('(') {
        Some((kind, scope)) => match scope.strip_suffix(')') {
            Some(scope) if !scope.trim().is_empty() && !scope.contains(['(', ')']) => kind,
            _ => {
                return Some(format!(
                    "the scope in \"{head}\" should be a non-empty word in parentheses, like \"fix(api): ...\""
                ));
            }
        },
        None => head,
    };

    if !TYPES.contains(&kind) {
        return Some(format!(
            "\"{kind}\" is not a Conventional Commits type (use one of {})",
            TYPES.join(", ")
        ));
    }

    if !rest.starts_with(' ') || rest.trim().is_empty() {
        return Some(
            "put a space and a description after the colon, like \"feat: add export\"".to_owned(),
        );
    }

    None
}

#[cfg(test)]
mod tests {
    use super::problem;

    #[test]
    fn accepts_conventional_subjects() {
        for ok in [
            "feat: add export",
            "refactor(article_ch): 引入强类型模型重构中文文献处理流水线",
            "fix(redis-queue)!: 移除兼容入口",
            "fix(api): handle 404\n\nbody",
            "refactor!: drop old config",
            "feat(ui)!: new layout",
            "\n# comment\nchore: bump deps",
            "Merge branch 'main' into dev",
            "Revert \"feat: x\"",
            "fixup! feat: x",
            "",
        ] {
            assert_eq!(problem(ok), None, "{ok:?}");
        }
    }

    #[test]
    fn rejects_invalid_subjects() {
        assert!(
            problem("Add export")
                .unwrap()
                .contains("should start with a type")
        );
        assert!(
            problem("feature: add export")
                .unwrap()
                .contains("not a Conventional Commits type")
        );
        assert!(problem("fix(): x").unwrap().contains("scope"));
        assert!(problem("fix(api: x").unwrap().contains("scope"));
        assert!(problem("fix:x").unwrap().contains("space"));
        assert!(problem("fix: ").unwrap().contains("space"));
    }
}
