//! Shared core library for OpenIDEA single-shot OpenAI-compatible plugins.
//!
//! Provides validated endpoint and server configuration ([`ServerConfig`],
//! [`EndpointUrl`], [`AuthHeader`]), a bounded single-shot HTTP client
//! ([`complete`], [`test_connection`]), shared host notification/clipboard UI
//! helpers ([`notify`], [`report_error`], [`copy_to_clipboard`],
//! [`run_test_connection`]), and model output / UTF-8 sanitization helpers
//! ([`cut_utf8`], [`strip_reasoning_tags`], [`unwrap_code_fence`]).
//!
//! # Target & MSRV
//!
//! Designed for both native unit tests (with `openidea_plugin::mock::MockHost`)
//! and `wasm32-wasip2` OpenIDEA plugin components on Rust 1.85+ (Edition 2024).

mod client;
mod config;
mod sanitize;
mod ui;

pub use client::{PromptPayload, complete, test_connection};
pub use config::{
    AuthHeader, DEFAULT_API_KEY, DEFAULT_BASE_URL, DEFAULT_MAX_INPUT_BYTES, DEFAULT_MODEL,
    DEFAULT_TIMEOUT_MS, EndpointUrl, MAX_MAX_INPUT_BYTES, MIN_MAX_INPUT_BYTES, ServerConfig,
    parse_max_input_bytes, setting_bool, setting_string,
};
pub use sanitize::{cut_utf8, strip_reasoning_tags, unwrap_code_fence, unwrap_outer_quotes};
pub use ui::{copy_to_clipboard, notify, report_error, run_test_connection};
