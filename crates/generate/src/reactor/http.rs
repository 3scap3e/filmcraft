//! Reactor's HTTP side: the token exchange, the coordinator's session and WebRTC signaling calls,
//! uploads, and the clip download. Pure-Rust TLS (rustls + RustCrypto, OS certificate verifier).

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::GenerateError;

pub const API_URL: &str = "https://api.reactor.inc";
const API_VERSION: &str = "1";
const WEBRTC_VERSION: &str = "1.0";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest response body read from the coordinator (its answers are small JSON).
const MAX_JSON_BYTES: u64 = 4 << 20;
/// Longest clip segment or playlist accepted.
const MAX_SEGMENT_BYTES: u64 = 512 << 20;

#[derive(Clone, Debug, Serialize)]
pub struct ClientInfo {
    pub sdk_version: String,
    pub sdk_type: String,
}

impl ClientInfo {
    pub fn filmcraft() -> Self {
        ClientInfo { sdk_version: env!("CARGO_PKG_VERSION").to_string(), sdk_type: "rust".to_string() }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct TrackCapability {
    pub name: String,
    pub kind: String,
    pub direction: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Capabilities {
    #[serde(default)]
    pub tracks: Vec<TrackCapability>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SessionResponse {
    pub session_id: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub capabilities: Option<Capabilities>,
    #[serde(default)]
    pub selected_transport: Option<Value>,
}

impl SessionResponse {
    pub fn is_ready(&self) -> bool {
        self.capabilities.is_some() && self.selected_transport.is_some()
    }
    pub fn is_terminal(&self) -> bool {
        matches!(self.state.as_str(), "INACTIVE" | "CLOSED")
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct IceCredentials {
    pub username: String,
    pub password: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct IceServer {
    #[serde(default)]
    pub uris: Vec<String>,
    #[serde(default)]
    pub credentials: Option<IceCredentials>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TrackMappingEntry {
    pub name: String,
    pub kind: String,
    pub direction: String,
    pub mid: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IceCandidate {
    pub candidate: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sdp_mid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sdp_mline_index: Option<u16>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SdpAnswer {
    pub sdp_answer: String,
    #[serde(default)]
    pub connection_id: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreatedUpload {
    pub presigned_id: String,
    pub presigned_url: String,
}

/// What a non-success status means to the caller.
fn status_error(what: &str, status: u16, body: &str) -> GenerateError {
    let detail = body.chars().take(300).collect::<String>();
    match status {
        401 | 403 => GenerateError::Refused(format!("{what}: not authorized ({status}): {detail}")),
        402 => GenerateError::Refused(format!("{what}: payment required ({status}): {detail}")),
        404 | 410 => GenerateError::Protocol(format!("{what}: gone ({status})")),
        429 => GenerateError::Refused(format!("{what}: rate limited ({status}): {detail}")),
        426 | 501 => GenerateError::Protocol(format!("{what}: the server no longer speaks this API version ({status})")),
        _ => GenerateError::Network(format!("{what}: HTTP {status}: {detail}")),
    }
}

/// A plain response: status, `Retry-After` in milliseconds if any, body.
pub struct Response {
    pub status: u16,
    pub retry_after_ms: Option<u64>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub fn json<T: for<'de> Deserialize<'de>>(&self, what: &str) -> Result<T, GenerateError> {
        serde_json::from_slice(&self.body).map_err(|e| GenerateError::Protocol(format!("{what}: bad JSON: {e}")))
    }
}

pub struct Http {
    agent: ureq::Agent,
}

impl Http {
    pub fn new() -> Self {
        let provider = Arc::new(rustls_rustcrypto::provider());
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .provider(ureq::tls::TlsProvider::Rustls)
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .unversioned_rustls_crypto_provider(provider)
                    .build(),
            )
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(REQUEST_TIMEOUT))
            .http_status_as_error(false)
            .build()
            .into();
        Http { agent }
    }

    /// One request; any HTTP status comes back as `Ok` (transport failures are `Err`).
    pub fn request(&self, method: &str, url: &str, headers: &[(&str, String)], body: Option<&[u8]>, max_bytes: u64) -> Result<Response, GenerateError> {
        let send = |req: ureq::RequestBuilder<ureq::typestate::WithBody>, body: &[u8]| req.send(body);
        let result = match (method, body) {
            ("GET", _) => {
                let mut r = self.agent.get(url);
                for (k, v) in headers {
                    r = r.header(*k, v.as_str());
                }
                r.call()
            }
            ("DELETE", _) => {
                let mut r = self.agent.delete(url);
                for (k, v) in headers {
                    r = r.header(*k, v.as_str());
                }
                r.call()
            }
            (m, body) => {
                let mut r = match m {
                    "PUT" => self.agent.put(url),
                    _ => self.agent.post(url),
                };
                for (k, v) in headers {
                    r = r.header(*k, v.as_str());
                }
                send(r, body.unwrap_or(&[]))
            }
        };
        let resp = result.map_err(|e| GenerateError::Network(format!("{method} {}: {e}", redact(url))))?;
        let status = resp.status().as_u16();
        let retry_after_ms =
            resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok()).map(|s| s.saturating_mul(1000));
        let mut body = resp.into_body();
        let bytes = body
            .with_config()
            .limit(max_bytes)
            .read_to_vec()
            .map_err(|e| GenerateError::Network(format!("{method} {}: reading the body: {e}", redact(url))))?;
        Ok(Response { status, retry_after_ms, body: bytes })
    }

    fn api_headers(jwt: Option<&str>, json_body: bool, signaling: bool) -> Vec<(&'static str, String)> {
        let mut h = vec![("Reactor-API-Version", API_VERSION.to_string()), ("Reactor-API-Accept-Version", API_VERSION.to_string())];
        if signaling {
            h.push(("Reactor-WebRTC-Version", WEBRTC_VERSION.to_string()));
        }
        if json_body {
            h.push(("Content-Type", "application/json".to_string()));
        }
        if let Some(j) = jwt {
            h.push(("Authorization", format!("Bearer {j}")));
        }
        h
    }

    fn json_call(&self, method: &str, url: &str, jwt: Option<&str>, body: Option<&Value>, signaling: bool, what: &str) -> Result<Response, GenerateError> {
        let bytes = body.map(|b| serde_json::to_vec(b).unwrap_or_default());
        let headers = Self::api_headers(jwt, bytes.is_some(), signaling);
        self.request(method, url, &headers, bytes.as_deref(), MAX_JSON_BYTES).map_err(|e| match e {
            GenerateError::Network(m) => GenerateError::Network(format!("{what}: {m}")),
            e => e,
        })
    }

    /// `POST /tokens` with the API key: a JWT scoped to one model, for `expires_after` seconds.
    pub fn token(&self, api_url: &str, api_key: &str, model: &str, expires_after: u64) -> Result<String, GenerateError> {
        let body = json!({
            "expires_after": expires_after,
            "authorization_details": [{
                "type": "session",
                "resources": {"models": {"match": [model]}},
                "constraints": {"max_sessions": 4},
            }],
        });
        let bytes = serde_json::to_vec(&body).unwrap_or_default();
        let headers = [("Reactor-API-Key", api_key.to_string()), ("Content-Type", "application/json".to_string())];
        let r = self.request("POST", &format!("{api_url}/tokens"), &headers, Some(&bytes), MAX_JSON_BYTES)?;
        if r.status == 401 || r.status == 403 {
            return Err(GenerateError::Refused("the Reactor API key was rejected (check Settings ▸ Generate)".into()));
        }
        if !(200..300).contains(&r.status) {
            return Err(status_error("token", r.status, &r.text()));
        }
        let v: Value = r.json("token")?;
        v.get("jwt").and_then(Value::as_str).map(str::to_string).ok_or_else(|| GenerateError::Protocol("token: no `jwt` in the response".into()))
    }

    pub fn create_session(&self, api_url: &str, jwt: &str, model: &str, client: &ClientInfo) -> Result<SessionResponse, GenerateError> {
        let body = json!({
            "model": {"name": model},
            "client_info": client,
            "supported_transports": [{"protocol": "webrtc", "version": WEBRTC_VERSION}],
        });
        let r = self.json_call("POST", &format!("{api_url}/sessions"), Some(jwt), Some(&body), false, "create session")?;
        if !(200..300).contains(&r.status) {
            return Err(status_error("create session", r.status, &r.text()));
        }
        r.json("create session")
    }

    pub fn get_session(&self, api_url: &str, jwt: &str, session: &str) -> Result<SessionResponse, GenerateError> {
        let r = self.json_call("GET", &format!("{api_url}/sessions/{session}"), Some(jwt), None, false, "session")?;
        if !(200..300).contains(&r.status) {
            return Err(status_error("session", r.status, &r.text()));
        }
        r.json("session")
    }

    /// `DELETE /sessions/{id}`; a 404 is success (already gone).
    pub fn terminate_session(&self, api_url: &str, jwt: &str, session: &str) -> Result<(), GenerateError> {
        let r = self.json_call("DELETE", &format!("{api_url}/sessions/{session}"), Some(jwt), None, false, "end session")?;
        if r.status == 404 || (200..300).contains(&r.status) {
            return Ok(());
        }
        Err(status_error("end session", r.status, &r.text()))
    }

    fn signaling_base(api_url: &str, session: &str) -> String {
        format!("{api_url}/sessions/{session}/transport/webrtc")
    }

    pub fn ice_servers(&self, api_url: &str, jwt: &str, session: &str) -> Result<Vec<IceServer>, GenerateError> {
        let r = self.json_call("GET", &format!("{}/ice_servers", Self::signaling_base(api_url, session)), Some(jwt), None, true, "ice servers")?;
        if !(200..300).contains(&r.status) {
            return Err(status_error("ice servers", r.status, &r.text()));
        }
        let v: Value = r.json("ice servers")?;
        let servers = v.get("ice_servers").cloned().unwrap_or(Value::Array(Vec::new()));
        serde_json::from_value(servers).map_err(|e| GenerateError::Protocol(format!("ice servers: {e}")))
    }

    pub fn register_connection(&self, api_url: &str, jwt: &str, session: &str) -> Result<u32, GenerateError> {
        let r = self.json_call(
            "POST",
            &format!("{}/connections", Self::signaling_base(api_url, session)),
            Some(jwt),
            Some(&json!({})),
            true,
            "register connection",
        )?;
        if !(200..300).contains(&r.status) {
            return Err(status_error("register connection", r.status, &r.text()));
        }
        let v: Value = r.json("register connection")?;
        v.get("connection_id")
            .and_then(Value::as_u64)
            .and_then(|c| u32::try_from(c).ok())
            .ok_or_else(|| GenerateError::Protocol("register connection: no `connection_id`".into()))
    }

    pub fn send_offer(
        &self,
        api_url: &str,
        jwt: &str,
        session: &str,
        connection: u32,
        sdp: &str,
        mapping: &[TrackMappingEntry],
        client: &ClientInfo,
    ) -> Result<(), GenerateError> {
        let body = json!({"sdp_offer": sdp, "client_info": client, "track_mapping": mapping});
        let url = format!("{}/connections/{connection}/sdp_params", Self::signaling_base(api_url, session));
        let r = self.json_call("POST", &url, Some(jwt), Some(&body), true, "send offer")?;
        if !(200..300).contains(&r.status) {
            return Err(status_error("send offer", r.status, &r.text()));
        }
        Ok(())
    }

    /// One poll of the SDP answer: `None` while the server still answers 202.
    pub fn poll_answer(&self, api_url: &str, jwt: &str, session: &str, connection: u32) -> Result<Option<SdpAnswer>, GenerateError> {
        let url = format!("{}/connections/{connection}/sdp_params", Self::signaling_base(api_url, session));
        let r = self.json_call("GET", &url, Some(jwt), None, true, "sdp answer")?;
        if r.status == 202 {
            return Ok(None);
        }
        if !(200..300).contains(&r.status) {
            return Err(status_error("sdp answer", r.status, &r.text()));
        }
        r.json("sdp answer").map(Some)
    }

    pub fn send_ice(
        &self,
        api_url: &str,
        jwt: &str,
        session: &str,
        connection: u32,
        candidates: &[IceCandidate],
        is_final: bool,
        client: &ClientInfo,
    ) -> Result<(), GenerateError> {
        let body = json!({"candidates": candidates, "is_final": is_final, "client_info": client});
        let url = format!("{}/connections/{connection}/ice_candidates", Self::signaling_base(api_url, session));
        let r = self.json_call("POST", &url, Some(jwt), Some(&body), true, "ice candidates")?;
        if !(200..300).contains(&r.status) {
            return Err(status_error("ice candidates", r.status, &r.text()));
        }
        Ok(())
    }

    pub fn create_upload(&self, api_url: &str, jwt: &str, session: &str, name: &str, size: u64, mime: &str) -> Result<CreatedUpload, GenerateError> {
        let body = json!({"name": name, "size": size, "mime_type": mime});
        let r = self.json_call("POST", &format!("{api_url}/sessions/{session}/uploads"), Some(jwt), Some(&body), false, "create upload")?;
        if !(200..300).contains(&r.status) {
            return Err(status_error("create upload", r.status, &r.text()));
        }
        r.json("create upload")
    }

    /// PUT the file's bytes to the presigned URL.
    pub fn put_upload(&self, presigned_url: &str, mime: &str, bytes: &[u8]) -> Result<(), GenerateError> {
        let headers = [("Content-Type", mime.to_string())];
        let r = self.request("PUT", presigned_url, &headers, Some(bytes), MAX_JSON_BYTES)?;
        if !(200..300).contains(&r.status) {
            return Err(status_error("upload", r.status, &r.text()));
        }
        Ok(())
    }

    /// One fetch of the clip's playlist: `Ok(None)` while the server answers 202 (not ready).
    pub fn playlist(&self, url: &str, jwt: &str) -> Result<Option<(String, Option<u64>)>, GenerateError> {
        let headers = [("Authorization", format!("Bearer {jwt}"))];
        let r = self.request("GET", url, &headers, None, MAX_SEGMENT_BYTES)?;
        match r.status {
            202 => Ok(None),
            200..=299 => Ok(Some((r.text(), r.retry_after_ms))),
            404 | 410 => Err(GenerateError::Protocol("the clip is gone (its playlist expired)".into())),
            s => Err(status_error("playlist", s, &r.text())),
        }
    }

    /// A clip segment (presigned URL, fetched unauthenticated).
    pub fn segment(&self, url: &str) -> Result<Vec<u8>, GenerateError> {
        let r = self.request("GET", url, &[], None, MAX_SEGMENT_BYTES)?;
        if !(200..300).contains(&r.status) {
            return Err(status_error("segment", r.status, ""));
        }
        Ok(r.body)
    }
}

/// A URL without its query string (presigned URLs carry signatures).
fn redact(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_actionable_errors() {
        assert!(matches!(status_error("x", 401, ""), GenerateError::Refused(_)));
        assert!(matches!(status_error("x", 429, ""), GenerateError::Refused(_)));
        assert!(matches!(status_error("x", 404, ""), GenerateError::Protocol(_)));
        assert!(matches!(status_error("x", 500, "boom"), GenerateError::Network(m) if m.contains("boom")));
        assert_eq!(redact("https://a/b?X-Amz-Signature=secret"), "https://a/b");
    }

    #[test]
    fn session_readiness_follows_capabilities_and_transport() {
        let s: SessionResponse = serde_json::from_value(json!({"session_id": "s", "state": "ACTIVE", "capabilities": {"protocol_version": "1", "tracks": [{"name": "main_video", "kind": "video", "direction": "recvonly"}]}, "selected_transport": {"protocol": "webrtc", "version": "1.0"}})).unwrap();
        assert!(s.is_ready() && !s.is_terminal());
        let s: SessionResponse = serde_json::from_value(json!({"session_id": "s", "state": "CLOSED"})).unwrap();
        assert!(!s.is_ready() && s.is_terminal());
        let c = IceCandidate { candidate: "candidate:1 1 udp 1 1.2.3.4 5 typ host".into(), sdp_mid: Some("0".into()), sdp_mline_index: None };
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("sdp_mline_index").is_none() && v["sdp_mid"] == "0");
    }
}
