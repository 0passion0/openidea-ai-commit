# AI Commit for OpenIDEA

A streamlined, single-shot OpenIDEA plugin that generates commit messages from the changes selected in the Commit panel using **any OpenAI-compatible server**.

## Key Features

- **Commit toolbar button (`commit.message`)**: Places the AI Commit button directly above the commit message box (Commit tool window and Commit dialog).
  - Shows a spinner on the button while generating (`indicatesRunning`).
  - Opens a cancellable background task (`host::begin_task`) so you can cancel/stop an in-flight request from OpenIDEA's Background Tasks status bar or **Stop Background Processes**, aborting the HTTP call immediately without overwriting your draft.
- **OpenAI-compatible only (single-shot)**: Direct HTTP `POST /chat/completions` (`stream: false`) with zero CLI agents or multi-turn loops.
  - Default server: `http://127.0.0.1:8045/v1`
  - Default model: `gemini-3.8-flash-low`
  - Default API key: configurable in **Settings › Plugins › AI Commit › Server** or overridden securely via the **Secrets** (`api-key`) field.
- **Git diff optimization & strict 150 KB cap**:
  - Strips redundant git headers (`diff --git`, `index`, `---`, `+++`, mode headers, `\ No newline at end of file`).
  - Classifies files (`Source` > `DocsAndTests` > `Deleted` > `LowSignal` such as lockfiles, minified assets, generated files, binaries).
  - Compacts excessive context runs and long lines when diffs are large, and distributes remaining budget fairly across source files via water-filling so later files are never starved.
  - Strictly keeps the total model input (`system + user`) within `150 KB` (`150,000` bytes by default).
- **Structured Chinese Conventional Commits output**:
  - Defaults to Simplified Chinese (`中文`) with mandatory detailed `- ` bullet-list body (`body = "always"`), producing `<type>(<scope>): <中文摘要>` plus concrete symbol/module/impact bullets.
  - Normalizes full-width colons (`：` -> `: `), trailing periods on subject lines, `*` / `•` bullet markers to `- `, missing subject-body blank lines, `<think>` / `<thought>` tags, code fences, and response labels.
- **Safe draft handling & Conventional Commits check**:
  - Never overwrites edits typed while generating (`Error::Stale` surfaces **Insert anyway** and **Copy** actions).
  - Strips `<think>...</think>` tags, code fences, labels, and outer quotes.
  - Includes a before-commit **Conventional Commits message** check active when `style = "conventional"`.

## Build & Verify

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --target wasm32-wasip2 --release
```

Copy `target/wasm32-wasip2/release/openidea_plugin_local_ai_commit.wasm` to `dist/plugin.wasm`, then link or pack with `openidea-plugin`:

```sh
openidea-plugin check
openidea-plugin link
```
