//! A client of mihomo's external controller: REST calls and the WebSocket
//! streams (`/traffic`, `/memory`, `/logs`, `/connections`).

use std::time::Duration;

use futures_util::StreamExt;
use serde::de::DeserializeOwned;
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};

use crate::model::*;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("The core is not reachable")]
    Unreachable,
    #[error("{0}")]
    Message(String),
    #[error("Timed out")]
    Timeout,
}

impl From<reqwest::Error> for ApiError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            ApiError::Timeout
        } else if e.is_connect() {
            ApiError::Unreachable
        } else {
            ApiError::Message(e.to_string())
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, Clone)]
pub struct Controller {
    base: String,
    ws_base: String,
    secret: String,
    http: reqwest::Client,
}

/// Percent-encodes a path segment (group and proxy names have spaces, emoji, CJK).
pub fn segment(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for b in name.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl Controller {
    pub fn new(port: u16, secret: &str) -> Controller {
        Controller {
            base: format!("http://127.0.0.1:{port}"),
            ws_base: format!("ws://127.0.0.1:{port}"),
            secret: secret.to_owned(),
            http: reqwest::Client::builder()
                // The controller is local: never through a proxy from the environment.
                .no_proxy()
                .timeout(Duration::from_secs(15))
                .build()
                .expect("http client"),
        }
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut r = self.http.request(method, format!("{}{}", self.base, path));
        if !self.secret.is_empty() {
            r = r.bearer_auth(&self.secret);
        }
        r
    }

    async fn check(resp: reqwest::Response) -> ApiResult<reqwest::Response> {
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let message = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_owned))
            .unwrap_or_else(|| if text.is_empty() { status.to_string() } else { text });
        Err(ApiError::Message(message))
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> ApiResult<T> {
        let resp = Self::check(self.req(reqwest::Method::GET, path).send().await?).await?;
        resp.json::<T>().await.map_err(|e| ApiError::Message(e.to_string()))
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Option<serde_json::Value>) -> ApiResult<()> {
        let mut r = self.req(method, path);
        if let Some(body) = body {
            r = r.json(&body);
        }
        Self::check(r.send().await?).await.map(|_| ())
    }

    pub async fn version(&self) -> ApiResult<Version> {
        self.get("/version").await
    }

    pub async fn configs(&self) -> ApiResult<RunningConfig> {
        self.get("/configs").await
    }

    /// Changes parts of the running config (`mode`, `log-level`, `allow-lan`, ...).
    pub async fn patch_configs(&self, body: serde_json::Value) -> ApiResult<()> {
        self.send(reqwest::Method::PATCH, "/configs", Some(body)).await
    }

    /// Loads the config file at `path` into the running core.
    pub async fn reload(&self, path: &str) -> ApiResult<()> {
        let r = self
            .req(reqwest::Method::PUT, "/configs?force=true")
            .timeout(Duration::from_secs(90))
            .json(&serde_json::json!({ "path": path, "payload": "" }));
        Self::check(r.send().await?).await.map(|_| ())
    }

    pub async fn proxies(&self) -> ApiResult<ProxiesResponse> {
        self.get("/proxies").await
    }

    pub async fn select(&self, group: &str, name: &str) -> ApiResult<()> {
        self.send(
            reqwest::Method::PUT,
            &format!("/proxies/{}", segment(group)),
            Some(serde_json::json!({ "name": name })),
        )
        .await
    }

    /// Tests one proxy; the delay in ms, or an error ("Timeout", ...).
    pub async fn delay(&self, name: &str, url: &str, timeout_ms: u32) -> ApiResult<u32> {
        #[derive(serde::Deserialize)]
        struct D {
            delay: u32,
        }
        let path = format!(
            "/proxies/{}/delay?timeout={timeout_ms}&url={}",
            segment(name),
            segment(url)
        );
        let r = self
            .req(reqwest::Method::GET, &path)
            .timeout(Duration::from_millis(u64::from(timeout_ms) + 3000));
        let resp = Self::check(r.send().await?).await?;
        let d: D = resp.json().await.map_err(|e| ApiError::Message(e.to_string()))?;
        Ok(d.delay)
    }

    /// Tests every member of a group at once; delays by name (failed ones missing).
    pub async fn group_delay(
        &self,
        group: &str,
        url: &str,
        timeout_ms: u32,
    ) -> ApiResult<std::collections::HashMap<String, u32>> {
        let path = format!(
            "/group/{}/delay?timeout={timeout_ms}&url={}",
            segment(group),
            segment(url)
        );
        let r = self
            .req(reqwest::Method::GET, &path)
            .timeout(Duration::from_millis(u64::from(timeout_ms) + 5000));
        let resp = Self::check(r.send().await?).await?;
        resp.json().await.map_err(|e| ApiError::Message(e.to_string()))
    }

    pub async fn proxy_providers(&self) -> ApiResult<Vec<ProxyProvider>> {
        Ok(proxy_providers_of(self.get("/providers/proxies").await?))
    }

    pub async fn update_proxy_provider(&self, name: &str) -> ApiResult<()> {
        self.send(reqwest::Method::PUT, &format!("/providers/proxies/{}", segment(name)), None).await
    }

    pub async fn healthcheck_provider(&self, name: &str) -> ApiResult<()> {
        let r = self
            .req(reqwest::Method::GET, &format!("/providers/proxies/{}/healthcheck", segment(name)))
            .timeout(Duration::from_secs(60));
        Self::check(r.send().await?).await.map(|_| ())
    }

    pub async fn rule_providers(&self) -> ApiResult<Vec<RuleProvider>> {
        let resp: ProvidersResponse<RuleProvider> = self.get("/providers/rules").await?;
        let mut list: Vec<RuleProvider> = resp.providers.into_values().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    pub async fn update_rule_provider(&self, name: &str) -> ApiResult<()> {
        self.send(reqwest::Method::PUT, &format!("/providers/rules/{}", segment(name)), None).await
    }

    pub async fn rules(&self) -> ApiResult<Vec<Rule>> {
        Ok(self.get::<RulesResponse>("/rules").await?.rules)
    }

    pub async fn connections(&self) -> ApiResult<ConnectionsResponse> {
        self.get("/connections").await
    }

    pub async fn close_connection(&self, id: &str) -> ApiResult<()> {
        self.send(reqwest::Method::DELETE, &format!("/connections/{}", segment(id)), None).await
    }

    pub async fn close_all_connections(&self) -> ApiResult<()> {
        self.send(reqwest::Method::DELETE, "/connections", None).await
    }

    pub async fn dns_query(&self, name: &str, kind: &str) -> ApiResult<DnsResponse> {
        self.get(&format!("/dns/query?name={}&type={}", segment(name), segment(kind))).await
    }

    pub async fn flush_fakeip(&self) -> ApiResult<()> {
        self.send(reqwest::Method::POST, "/cache/fakeip/flush", None).await
    }

    /// Downloads the GeoIP / GeoSite databases again.
    pub async fn update_geo(&self) -> ApiResult<()> {
        let r = self
            .req(reqwest::Method::POST, "/configs/geo")
            .timeout(Duration::from_secs(180))
            .json(&serde_json::json!({}));
        Self::check(r.send().await?).await.map(|_| ())
    }

    /// Waits until the controller answers, up to `limit`.
    pub async fn wait_ready(&self, limit: Duration) -> ApiResult<Version> {
        let start = std::time::Instant::now();
        loop {
            match self.version().await {
                Ok(v) => return Ok(v),
                Err(ApiError::Message(m)) if m.contains("Unauthorized") => {
                    return Err(ApiError::Message("The controller rejected Kairo's secret".into()));
                }
                Err(e) if start.elapsed() > limit => return Err(e),
                Err(_) => tokio::time::sleep(Duration::from_millis(150)).await,
            }
        }
    }

    /// Connects to a streaming endpoint (`/traffic`, `/logs?level=info`, ...).
    pub async fn connect(
        &self,
        path_and_query: &str,
    ) -> ApiResult<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>> {
        let mut request = format!("{}{}", self.ws_base, path_and_query)
            .into_client_request()
            .map_err(|e| ApiError::Message(e.to_string()))?;
        if !self.secret.is_empty() {
            request.headers_mut().insert(
                "Authorization",
                format!("Bearer {}", self.secret).parse().expect("header value"),
            );
        }
        match tokio::time::timeout(Duration::from_secs(5), tokio_tungstenite::connect_async(request)).await {
            Ok(Ok((ws, _))) => Ok(ws),
            Ok(Err(tungstenite::Error::Io(_))) => Err(ApiError::Unreachable),
            Ok(Err(e)) => Err(ApiError::Message(e.to_string())),
            Err(_) => Err(ApiError::Timeout),
        }
    }

    /// Streams `path` and hands each decoded message to `each` until the
    /// connection ends or `each` returns false.
    pub async fn stream<T: DeserializeOwned>(
        &self,
        path_and_query: &str,
        mut each: impl FnMut(T) -> bool,
    ) -> ApiResult<()> {
        let mut ws = self.connect(path_and_query).await?;
        while let Some(message) = ws.next().await {
            let message = message.map_err(|e| ApiError::Message(e.to_string()))?;
            let text = match message {
                tungstenite::Message::Text(t) => t,
                tungstenite::Message::Binary(b) => match String::from_utf8(b.to_vec()) {
                    Ok(s) => s.into(),
                    Err(_) => continue,
                },
                tungstenite::Message::Close(_) => break,
                _ => continue,
            };
            match serde_json::from_str::<T>(&text) {
                Ok(value) => {
                    if !each(value) {
                        break;
                    }
                }
                Err(err) => log::debug!("{path_and_query}: undecodable message: {err}"),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::segment;

    #[test]
    fn encodes_names() {
        assert_eq!(segment("Proxy"), "Proxy");
        assert_eq!(segment("🇯🇵 Tokyo 01"), "%F0%9F%87%AF%F0%9F%87%B5%20Tokyo%2001");
        assert_eq!(segment("a/b?c"), "a%2Fb%3Fc");
    }
}
