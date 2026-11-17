# Plugin AI Completion Implementation Plan

> **For agentic workers:** Execute these bounded tasks in the existing checkout. Backend ownership belongs to `completion_server`; Rust integration and verification stay with the main agent.

**Goal:** Add real Gemini inline completions to AI Code Helper without changing OpenIDEA or existing translation/naming contracts.

**Architecture:** The native inline provider supplies the document and UTF-16 caret. A bounded request goes to a loopback Node companion using the official Google SDK. Native Python LSP continues independently.

**Tech Stack:** Rust 2024/MSRV 1.85, OpenIDEA SDK v0.1.42, Node ESM, `@google/genai`.

**Spec:** Supplied completion requirements, narrowed by the user's explicit instruction to modify only this plugin repository.

**Workspace update:** The user confirmed a concurrent merge into `plugins/ai-assistant`, with domain code moved to `crates/ai-code-helper` and `crates/ai-commit`. Preserve that restructuring and forward the existing provider through the unified plugin entry point. Completion settings, server and guide now belong to `plugins/ai-assistant`.

## Constraints

- Preserve the public unit struct `AiCodeHelper`, commands and shared single-shot AI client.
- Completion requires user opt-in; keys stay in companion environment variables.
- Reuse native debounce, cancellation, document/selection staleness checks, Tab/Esc and next-word acceptance.
- The current ABI returns one final inline suggestion and one buffered HTTP response. Incremental Ghost Text, line acceptance and acceptance telemetry are unavailable to this plugin.
- Reuse native Python workspace/interpreter/LSP; never start Pyright on an isolated copy of the file.
- No project disk reads, bulk uploads, unrelated dependency upgrades or retries.
- Initial baseline had five invalid `NamingStyle::*Case` references. They were corrected by concurrent workspace work before this task edited any code; preserve that work.

## Tasks

- [ ] Rust: failing request/opt-in/Unicode tests, bounded prefix/suffix and Python context, response parsing, settings and provider registration.
- [ ] Node: tests first, official SDK, FIM prompt, validator, cancellable scheduler, LRU, boundary validation, environment config and sanitized metrics.
- [ ] Integration: exact local HTTP contract, workspace tests/static checks and WASI build; inspect only task changes.
- [ ] Delivery: deployment, native Pyright/interpreter setup, measured checks and host-imposed acceptance gaps. Live Gemini validation requires an available key.
