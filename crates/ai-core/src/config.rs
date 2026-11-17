//! Shared OpenAI-compatible server configuration and endpoint URL parsing.

use std::fmt;

use openidea_plugin::{Error, host};

/// Default local OpenAI-compatible server base URL.
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8045/v1";

/// Default fixed model name sent to the OpenAI-compatible endpoint.
pub const DEFAULT_MODEL: &str = "gemini-3.8-flash-low";

/// Default Bearer token used when no keychain secret override is stored.
pub const DEFAULT_API_KEY: &str = "sk-035fcf9af54b4e45af8de0f750784c56";

/// Default total model input cap in bytes (`150 KB = 150,000` bytes).
pub const DEFAULT_MAX_INPUT_BYTES: usize = 150_000;

/// Minimum configurable model input cap in bytes.
pub const MIN_MAX_INPUT_BYTES: usize = 4_000;

/// Maximum configurable model input cap in bytes.
pub const MAX_MAX_INPUT_BYTES: usize = 512_000;

/// Default HTTP request timeout in milliseconds.
pub const DEFAULT_TIMEOUT_MS: u32 = 55_000;

/// Authentication header source for OpenAI-compatible requests.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthHeader {
    /// Omit the `Authorization` header.
    None,
    /// Expand the plugin's keychain secret (`api-key`) on the host side.
    KeychainPlaceholder,
    /// Send a Bearer token configured in Server settings.
    StaticToken(String),
}

impl fmt::Debug for AuthHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("AuthHeader::None"),
            Self::KeychainPlaceholder => f.write_str("AuthHeader::KeychainPlaceholder"),
            Self::StaticToken(_) => f.write_str("AuthHeader::StaticToken(***)"),
        }
    }
}

impl AuthHeader {
    /// Returns the `Authorization` header value, if authentication is enabled.
    #[must_use]
    pub fn header_value(&self) -> Option<String> {
        match self {
            Self::None => None,
            Self::KeychainPlaceholder => Some("Bearer {{secret:api-key}}".to_owned()),
            Self::StaticToken(token) => Some(format!("Bearer {token}")),
        }
    }
}

/// Validated, normalized OpenAI-compatible `/chat/completions` URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointUrl {
    completions_url: String,
    scheme: String,
    host: String,
}

impl EndpointUrl {
    /// Normalizes any OpenAI-compatible base URL or `/chat/completions` URL.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Failed`] when the URL lacks an `http://`/`https://`
    /// scheme or has no host.
    pub fn parse(raw: &str) -> Result<Self, Error> {
        let trimmed = raw.trim().trim_end_matches('/');
        let Some((scheme_raw, rest)) = trimmed.split_once("://") else {
            return Err(Error::Failed(format!(
                "Invalid OpenAI-compatible URL {raw:?}: expected http:// or https://."
            )));
        };
        let scheme = scheme_raw.to_ascii_lowercase();
        if scheme != "http" && scheme != "https" {
            return Err(Error::Failed(format!(
                "Unsupported URL scheme {scheme:?}: use http:// or https://."
            )));
        }

        let (authority, path) = match rest.find(['/', '?', '#']) {
            Some(sep_idx) => rest.split_at(sep_idx),
            None => (rest, ""),
        };
        let host = extract_host(authority).ok_or_else(|| {
            Error::Failed(format!(
                "Invalid OpenAI-compatible URL {raw:?}: missing host."
            ))
        })?;

        let completions_url = normalize_completions_url(trimmed, path);
        Ok(Self {
            completions_url,
            scheme,
            host: host.to_owned(),
        })
    }

    /// Returns the normalized `/chat/completions` URL.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.completions_url
    }

    /// Returns `true` when the endpoint targets loopback (`127.0.0.1`, `localhost`, `::1`).
    #[must_use]
    pub fn is_loopback(&self) -> bool {
        let lower = self.host.to_ascii_lowercase();
        matches!(lower.as_str(), "localhost" | "127.0.0.1" | "::1") || lower.ends_with(".localhost")
    }

    /// Builds an actionable permission message naming `plugin_name` when the host denies network access.
    #[must_use]
    pub fn denied_message(&self, plugin_name: &str) -> String {
        if self.scheme == "http" && !self.is_loopback() {
            return format!(
                "Plain HTTP to remote host {} requires `net.http:{}` in plugin.toml, or switch to https://.",
                self.host, self.host
            );
        }
        match self.host.to_ascii_lowercase().as_str() {
            "127.0.0.1" => format!(
                "Network access to 127.0.0.1 was denied. Grant `net:127.0.0.1` in Settings › Plugins › {plugin_name}."
            ),
            "localhost" | "::1" => format!(
                "Network access to localhost was denied. Grant `net:localhost` or `net:ask` in Settings › Plugins › {plugin_name}."
            ),
            _ => format!(
                "Network access to {} was denied. Grant `net:ask` in Settings › Plugins › {plugin_name} and allow the host when prompted.",
                self.host
            ),
        }
    }
}

fn extract_host(authority: &str) -> Option<&str> {
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match host_port.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(v6),
        None => host_port.split(':').next().unwrap_or(host_port),
    };
    (!host.is_empty()).then_some(host)
}

fn normalize_completions_url(trimmed: &str, path_with_query: &str) -> String {
    let (before_query, query_suffix) = match path_with_query.find(['?', '#']) {
        Some(idx) => path_with_query.split_at(idx),
        None => (path_with_query, ""),
    };
    let path_only = before_query.trim_end_matches('/');
    let base_without_query = trimmed.strip_suffix(query_suffix).unwrap_or(trimmed);

    let full_path = if path_only.ends_with("/chat/completions") {
        base_without_query.to_owned()
    } else if path_only.is_empty() {
        format!("{base_without_query}/v1/chat/completions")
    } else {
        format!("{base_without_query}/chat/completions")
    };
    format!("{full_path}{query_suffix}")
}

/// Settings governing the OpenAI-compatible server connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConfig {
    /// Normalized chat completions endpoint.
    pub endpoint: EndpointUrl,
    /// Model identifier sent in the request payload.
    pub model: String,
    /// Optional Bearer authentication source.
    pub auth: AuthHeader,
    /// HTTP timeout in milliseconds, clamped to OpenIDEA's task budget.
    pub timeout_ms: u32,
}

impl ServerConfig {
    /// Reads server configuration from OpenIDEA plugin settings and keychain secrets.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Failed`] if `baseUrl` cannot be parsed as an HTTP(S) URL.
    pub fn from_host() -> Result<Self, Error> {
        let raw_url = setting_string("baseUrl", DEFAULT_BASE_URL);
        let endpoint = EndpointUrl::parse(&raw_url)?;

        let model_raw = setting_string("model", DEFAULT_MODEL);
        let model = {
            let trimmed = model_raw.trim();
            if trimmed.is_empty() {
                DEFAULT_MODEL.to_owned()
            } else {
                trimmed.to_owned()
            }
        };

        let auth = if host::secret_is_set("api-key") {
            AuthHeader::KeychainPlaceholder
        } else {
            let raw_key = setting_string("apiKey", DEFAULT_API_KEY);
            let trimmed_key = raw_key.trim();
            if trimmed_key.is_empty() {
                AuthHeader::None
            } else {
                AuthHeader::StaticToken(trimmed_key.to_owned())
            }
        };

        let timeout_ms =
            host::setting_number("timeoutSeconds").map_or(DEFAULT_TIMEOUT_MS, |secs| {
                #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let clamped = secs.clamp(5.0, 58.0) as u32;
                clamped * 1_000
            });

        Ok(Self {
            endpoint,
            model,
            auth,
            timeout_ms,
        })
    }
}

/// Reads the `maxInputKb` setting from OpenIDEA and bounds it to `[min_bytes, max_bytes]`.
#[must_use]
pub fn parse_max_input_bytes(default_bytes: usize) -> usize {
    host::setting_number("maxInputKb").map_or(default_bytes, |kb| {
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let bytes = (kb.clamp(4.0, 512.0) as usize) * 1_000;
        bytes.clamp(MIN_MAX_INPUT_BYTES, MAX_MAX_INPUT_BYTES)
    })
}

/// Reads a string setting from OpenIDEA with a fallback default.
#[must_use]
pub fn setting_string(key: &str, default: &str) -> String {
    host::setting_string(key).unwrap_or_else(|| default.to_owned())
}

/// Reads a boolean setting from OpenIDEA with a fallback default.
#[must_use]
pub fn setting_bool(key: &str, default: bool) -> bool {
    host::setting_bool(key).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_openai_compatible_urls() {
        assert_eq!(
            EndpointUrl::parse("http://127.0.0.1:8045/v1")
                .unwrap()
                .as_str(),
            "http://127.0.0.1:8045/v1/chat/completions"
        );
        assert_eq!(
            EndpointUrl::parse("http://127.0.0.1:8045/v1/")
                .unwrap()
                .as_str(),
            "http://127.0.0.1:8045/v1/chat/completions"
        );
        assert_eq!(
            EndpointUrl::parse("http://127.0.0.1:8045/v1/chat/completions")
                .unwrap()
                .as_str(),
            "http://127.0.0.1:8045/v1/chat/completions"
        );
        assert_eq!(
            EndpointUrl::parse("http://127.0.0.1:8045")
                .unwrap()
                .as_str(),
            "http://127.0.0.1:8045/v1/chat/completions"
        );
        assert_eq!(
            EndpointUrl::parse("https://generativelanguage.googleapis.com/v1beta/openai")
                .unwrap()
                .as_str(),
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
        );
        assert_eq!(
            EndpointUrl::parse("https://api.example.com/v1?api-version=1")
                .unwrap()
                .as_str(),
            "https://api.example.com/v1/chat/completions?api-version=1"
        );
    }

    #[test]
    fn rejects_invalid_urls() {
        assert!(EndpointUrl::parse("file:///etc/passwd").is_err());
        assert!(EndpointUrl::parse("127.0.0.1:8045/v1").is_err());
        assert!(EndpointUrl::parse("http:///v1").is_err());
    }

    #[test]
    fn redacts_api_key_in_debug_output() {
        let auth = AuthHeader::StaticToken("sk-secret-value".to_owned());
        let formatted = format!("{auth:?}");
        assert!(!formatted.contains("sk-secret-value"));
        assert!(formatted.contains("***"));
    }

    #[test]
    fn denied_message_mentions_expected_capability_and_plugin() {
        let local = EndpointUrl::parse(DEFAULT_BASE_URL).unwrap();
        let msg = local.denied_message("AI Code Helper");
        assert!(msg.contains("net:127.0.0.1"));
        assert!(msg.contains("AI Code Helper"));

        let remote_https = EndpointUrl::parse("https://llm.example.com/v1").unwrap();
        assert!(remote_https.denied_message("AI Commit").contains("net:ask"));

        let remote_http = EndpointUrl::parse("http://192.168.1.20:8000/v1").unwrap();
        assert!(
            remote_http
                .denied_message("AI Commit")
                .contains("net.http:192.168.1.20")
        );
    }
}
