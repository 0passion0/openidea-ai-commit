# OpenIDEA AI Assistant (`local.ai-commit`)

A unified, lightweight Cargo Workspace producing a single OpenIDEA WebAssembly plugin (`plugins/ai-assistant`) with **one shared OpenAI-compatible server configuration**:

1. **AI Commit (`commit.message`)**: Single-shot Chinese Conventional Commits generator with git-diff compaction, tiered file prioritization, `<= 150 KB` input bounding, cancellable progress, stale draft protection, and pre-commit Conventional check.
2. **Editor Selection Translation (`editor.context` › AI 翻译与命名 / `Alt+Shift+T`)**: Automatic Chinese <-> English translation preserving inline code identifiers, placeholders, and comment markers, shown in a lightweight multiline preview dialog (OK replaces selection; Cancel leaves source intact and offers **Copy** / **Replace** notification actions).
3. **Variable Naming Optimization (`editor.context` › AI 翻译与命名 / `Alt+Shift+N`)**: Accepts Chinese phrases, imperfect English, Pinyin, or mixed phrases and opens a structured Quick Pick list across `camelCase`, `PascalCase`, `snake_case`, `SCREAMING_SNAKE_CASE`, and `kebab-case` (plus a **Copy All Markdown** row).
4. **Python Inline Completion (Optional)**: Opt-in `Providers::inline_completion` bridge to the local companion service in `plugins/ai-assistant/server/`.

## Workspace Architecture

- `crates/ai-core`: Shared OpenAI-compatible HTTP client, URL parser, secret-redacted `AuthHeader`, notifications/clipboard UI helpers, and UTF-8/reasoning-tag sanitizer.
- `crates/ai-commit`: Commit message domain crate.
- `crates/ai-code-helper`: Editor selection translation, variable naming optimizer, and inline completion domain crate.
- `plugins/ai-assistant`: Unified Wasm plugin package (`dist/plugin.wasm`).

## Build, Verify & Install in OpenIDEA

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --target wasm32-wasip2 -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
./scripts/build.ps1
```

In OpenIDEA: **Settings › Plugins › ⚙ › Install Plugin from Disk...** and select:

`d:\tmp\ai-commit-rust\plugins\ai-assistant`
