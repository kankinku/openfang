//! Production middleware for the OpenFang API server.
//!
//! Provides:
//! - Request ID generation and propagation
//! - Per-endpoint structured request logging
//! - In-memory rate limiting (per IP)

use axum::body::Body;
use axum::http::{HeaderMap, Request, Response, StatusCode, Uri};
use axum::middleware::Next;
use openfang_types::config::{ApiAuthMode, KernelConfig};
use std::net::IpAddr;
use std::time::Instant;
use tracing::info;

/// Request ID header name (standard).
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Runtime API authentication mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiAuthRuntimeMode {
    /// No remote auth configured: allow loopback only.
    LocalhostOnly,
    /// Bearer token auth.
    Token,
    /// Header password auth.
    Password,
    /// Trusted proxy header auth.
    TrustedProxy,
}

/// Resolved runtime auth state used by middleware and WebSocket upgrades.
#[derive(Debug, Clone)]
pub struct ApiAuthState {
    mode: ApiAuthRuntimeMode,
    token: Option<String>,
    password: Option<String>,
    trusted_proxy_user_header: String,
    trusted_proxy_ips: Vec<IpAddr>,
}

impl ApiAuthState {
    /// Resolve runtime auth state from kernel config + environment.
    pub fn from_kernel_config(config: &KernelConfig) -> Self {
        let token = if !config.api_key.trim().is_empty() {
            Some(config.api_key.clone())
        } else if config.api_auth.token_env.trim().is_empty() {
            None
        } else {
            std::env::var(&config.api_auth.token_env)
                .ok()
                .filter(|v| !v.trim().is_empty())
        };

        let password = if config.api_auth.password_env.trim().is_empty() {
            None
        } else {
            std::env::var(&config.api_auth.password_env)
                .ok()
                .filter(|v| !v.trim().is_empty())
        };

        let trusted_proxy_ips: Vec<IpAddr> = config
            .api_auth
            .trusted_proxy
            .trusted_ips
            .iter()
            .filter_map(|v| v.parse::<IpAddr>().ok())
            .collect();

        let mode = match config.api_auth.mode {
            ApiAuthMode::None => ApiAuthRuntimeMode::LocalhostOnly,
            ApiAuthMode::Token => {
                if token.is_some() {
                    ApiAuthRuntimeMode::Token
                } else {
                    ApiAuthRuntimeMode::LocalhostOnly
                }
            }
            ApiAuthMode::Password => {
                if password.is_some() {
                    ApiAuthRuntimeMode::Password
                } else {
                    ApiAuthRuntimeMode::LocalhostOnly
                }
            }
            ApiAuthMode::TrustedProxy => ApiAuthRuntimeMode::TrustedProxy,
        };

        Self {
            mode,
            token,
            password,
            trusted_proxy_user_header: if config.api_auth.trusted_proxy.user_header.trim().is_empty()
            {
                "x-openfang-user".to_string()
            } else {
                config.api_auth.trusted_proxy.user_header.clone()
            },
            trusted_proxy_ips,
        }
    }

    /// Whether API auth is configured for remote access.
    pub fn auth_enabled(&self) -> bool {
        !matches!(self.mode, ApiAuthRuntimeMode::LocalhostOnly)
    }

    /// Human-readable mode label.
    pub fn mode_label(&self) -> &'static str {
        match self.mode {
            ApiAuthRuntimeMode::LocalhostOnly => "localhost_only",
            ApiAuthRuntimeMode::Token => "bearer_token",
            ApiAuthRuntimeMode::Password => "password",
            ApiAuthRuntimeMode::TrustedProxy => "trusted_proxy",
        }
    }

    /// Whether a token secret is resolved.
    pub fn token_set(&self) -> bool {
        self.token.is_some()
    }

    /// Whether a password secret is resolved.
    pub fn password_set(&self) -> bool {
        self.password.is_some()
    }

    fn get_remote_ip(request: &Request<Body>) -> Option<IpAddr> {
        request
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|ci| ci.0.ip())
    }

    fn safe_equal_secret(input: &str, expected: &str) -> bool {
        use subtle::ConstantTimeEq;
        if input.len() != expected.len() {
            return false;
        }
        input.as_bytes().ct_eq(expected.as_bytes()).into()
    }

    fn query_value<'a>(uri: &'a Uri, key: &str) -> Option<&'a str> {
        uri.query()
            .and_then(|q| q.split('&').find_map(|pair| pair.strip_prefix(&format!("{key}="))))
    }

    fn trusted_proxy_ok(&self, headers: &HeaderMap, remote_ip: IpAddr) -> bool {
        if self.trusted_proxy_ips.is_empty() {
            return false;
        }
        if !self.trusted_proxy_ips.contains(&remote_ip) {
            return false;
        }
        headers
            .get(self.trusted_proxy_user_header.as_str())
            .and_then(|v| v.to_str().ok())
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
    }

    /// Authorize a WebSocket upgrade request.
    pub fn authorize_ws_upgrade(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
        remote_ip: IpAddr,
    ) -> bool {
        match self.mode {
            ApiAuthRuntimeMode::LocalhostOnly => remote_ip.is_loopback(),
            ApiAuthRuntimeMode::Token => {
                let Some(token) = self.token.as_deref() else {
                    return remote_ip.is_loopback();
                };
                let header_auth = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.strip_prefix("Bearer "))
                    .map(|candidate| Self::safe_equal_secret(candidate, token))
                    .unwrap_or(false);
                let query_auth = Self::query_value(uri, "token")
                    .map(|candidate| Self::safe_equal_secret(candidate, token))
                    .unwrap_or(false);
                header_auth || query_auth
            }
            ApiAuthRuntimeMode::Password => {
                let Some(password) = self.password.as_deref() else {
                    return remote_ip.is_loopback();
                };
                let header_auth = headers
                    .get("x-openfang-password")
                    .and_then(|v| v.to_str().ok())
                    .map(|candidate| Self::safe_equal_secret(candidate, password))
                    .unwrap_or(false);
                let query_auth = Self::query_value(uri, "password")
                    .map(|candidate| Self::safe_equal_secret(candidate, password))
                    .unwrap_or(false);
                header_auth || query_auth
            }
            ApiAuthRuntimeMode::TrustedProxy => self.trusted_proxy_ok(headers, remote_ip),
        }
    }
}

/// Middleware: inject a unique request ID and log the request/response.
pub async fn request_logging(request: Request<Body>, next: Next) -> Response<Body> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let method = request.method().clone();
    let uri = request.uri().path().to_string();
    let start = Instant::now();

    let mut response = next.run(request).await;

    let elapsed = start.elapsed();
    let status = response.status().as_u16();

    info!(
        request_id = %request_id,
        method = %method,
        path = %uri,
        status = status,
        latency_ms = elapsed.as_millis() as u64,
        "API request"
    );

    // Inject the request ID into the response
    if let Ok(header_val) = request_id.parse() {
        response.headers_mut().insert(REQUEST_ID_HEADER, header_val);
    }

    response
}

/// API authentication middleware.
pub async fn auth(
    axum::extract::State(auth_state): axum::extract::State<ApiAuthState>,
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    // Public endpoints that don't require auth (dashboard needs these)
    let path = request.uri().path();
    if path == "/"
        || path == "/api/health"
        || path == "/api/health/detail"
        || path == "/api/status"
        || path == "/api/version"
        || path == "/api/agents"
        || path == "/api/profiles"
        || path == "/api/config"
        || path.starts_with("/api/uploads/")
    {
        return next.run(request).await;
    }

    let remote_ip = ApiAuthState::get_remote_ip(&request).unwrap_or(IpAddr::from([127, 0, 0, 1]));

    match auth_state.mode {
        ApiAuthRuntimeMode::LocalhostOnly => {
            if remote_ip.is_loopback() {
                return next.run(request).await;
            }
            tracing::warn!(
                ip = %remote_ip,
                "Rejected non-localhost request: no API auth credential configured."
            );
            Response::builder()
                .status(StatusCode::FORBIDDEN)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "error": "No API auth configured. Remote access denied. Configure [api_auth] in ~/.openfang/config.toml"
                    })
                    .to_string(),
                ))
                .unwrap_or_default()
        }
        ApiAuthRuntimeMode::Token => {
            let token = auth_state.token.as_deref().unwrap_or("");
            let header_auth = request
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "));
            let query_auth = ApiAuthState::query_value(request.uri(), "token");

            let ok = header_auth
                .map(|candidate| ApiAuthState::safe_equal_secret(candidate, token))
                .unwrap_or(false)
                || query_auth
                    .map(|candidate| ApiAuthState::safe_equal_secret(candidate, token))
                    .unwrap_or(false);

            if ok {
                return next.run(request).await;
            }

            let credential_provided = header_auth.is_some() || query_auth.is_some();
            let error_msg = if credential_provided {
                "Invalid API token"
            } else {
                "Missing Authorization: Bearer <token> header"
            };

            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header("www-authenticate", "Bearer")
                .body(Body::from(
                    serde_json::json!({"error": error_msg}).to_string(),
                ))
                .unwrap_or_default()
        }
        ApiAuthRuntimeMode::Password => {
            let password = auth_state.password.as_deref().unwrap_or("");
            let header_auth = request
                .headers()
                .get("x-openfang-password")
                .and_then(|v| v.to_str().ok());
            let query_auth = ApiAuthState::query_value(request.uri(), "password");

            let ok = header_auth
                .map(|candidate| ApiAuthState::safe_equal_secret(candidate, password))
                .unwrap_or(false)
                || query_auth
                    .map(|candidate| ApiAuthState::safe_equal_secret(candidate, password))
                    .unwrap_or(false);

            if ok {
                return next.run(request).await;
            }

            let credential_provided = header_auth.is_some() || query_auth.is_some();
            let error_msg = if credential_provided {
                "Invalid API password"
            } else {
                "Missing x-openfang-password header"
            };

            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(Body::from(
                    serde_json::json!({"error": error_msg}).to_string(),
                ))
                .unwrap_or_default()
        }
        ApiAuthRuntimeMode::TrustedProxy => {
            if auth_state.trusted_proxy_ok(request.headers(), remote_ip) {
                return next.run(request).await;
            }
            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(Body::from(
                    serde_json::json!({
                        "error": format!(
                            "Trusted proxy auth failed (expected header '{}')",
                            auth_state.trusted_proxy_user_header
                        )
                    })
                    .to_string(),
                ))
                .unwrap_or_default()
        }
    }
}

/// Security headers middleware — applied to ALL API responses.
pub async fn security_headers(request: Request<Body>, next: Next) -> Response<Body> {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    headers.insert("x-frame-options", "DENY".parse().unwrap());
    headers.insert("x-xss-protection", "1; mode=block".parse().unwrap());
    // All JS/CSS is bundled inline — only external resource is Google Fonts.
    headers.insert(
        "content-security-policy",
        "default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com https://fonts.gstatic.com; img-src 'self' data: blob:; connect-src 'self' ws://localhost:* ws://127.0.0.1:* wss://localhost:* wss://127.0.0.1:*; font-src 'self' https://fonts.gstatic.com; media-src 'self' blob:; frame-src 'self' blob:; object-src 'none'; base-uri 'self'; form-action 'self'"
            .parse()
            .unwrap(),
    );
    headers.insert(
        "referrer-policy",
        "strict-origin-when-cross-origin".parse().unwrap(),
    );
    headers.insert(
        "cache-control",
        "no-store, no-cache, must-revalidate".parse().unwrap(),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_id_header_constant() {
        assert_eq!(REQUEST_ID_HEADER, "x-request-id");
    }
}
