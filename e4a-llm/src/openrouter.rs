//! OpenAI-compatible chat client for OpenRouter.
//!
//! Dials `openrouter.ai:443` only after a SOCKS5 CONNECT. `socks5h://` means the proxy
//! resolves that name (remote DNS). There is no default proxy host. The API key is never
//! written into logs or error text.

use std::collections::BTreeMap;
use std::future::Future;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::{ChatMessage, Error, EventStream, Llm, LlmEvent, ToolCall, TurnRequest, Usage};

const OPENROUTER_HOST: &str = "openrouter.ai";
const OPENROUTER_PORT: u16 = 443;
const OPENROUTER_PATH: &str = "/api/v1/chat/completions";
const MAX_BODY: usize = 8 * 1024 * 1024;
const DIAL_TIMEOUT: Duration = Duration::from_secs(20);
const TURN_TIMEOUT: Duration = Duration::from_secs(180);

/// Chat client that reaches OpenRouter through a caller-supplied SOCKS5 proxy.
///
/// Construct with [`OpenRouterClient::new`], then [`OpenRouterClient::with_socks`] and
/// [`OpenRouterClient::with_key`]. An empty field is filled from `E4A_SOCKS` or
/// `E4A_OPENROUTER_KEY` when [`Llm::stream`] runs. A set field wins over the environment.
#[derive(Clone)]
pub struct OpenRouterClient {
    model: String,
    socks: String,
    key: String,
}

impl OpenRouterClient {
    /// `model` is the id stored on the client. A leading `openrouter::` is stripped on the wire.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            socks: String::new(),
            key: String::new(),
        }
    }

    /// SOCKS URL (`socks5h://host:port` or `socks5://host:port`). No default host or port.
    pub fn with_socks(mut self, url: impl Into<String>) -> Self {
        self.socks = url.into();
        self
    }

    /// Bearer token. Empty defers to `E4A_OPENROUTER_KEY`.
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

impl Llm for OpenRouterClient {
    fn stream(
        &self,
        request: &TurnRequest,
    ) -> impl Future<Output = Result<EventStream, Error>> + Send {
        let model = self.model.clone();
        let socks = self.socks.clone();
        let key = self.key.clone();
        let request = request.clone();
        async move { stream_turn(model, socks, key, request).await }
    }
}

async fn stream_turn(
    model: String,
    socks: String,
    key_field: String,
    request: TurnRequest,
) -> Result<EventStream, Error> {
    let socks_url = setting_or_env(&socks, env_value("E4A_SOCKS").as_deref(), "E4A_SOCKS is unset")?;
    let endpoint = parse_socks(&socks_url)?;
    let key = setting_or_env(
        &key_field,
        env_value("E4A_OPENROUTER_KEY").as_deref(),
        "E4A_OPENROUTER_KEY is unset",
    )?;
    validate_key(&key)?;
    let body = chat_body(&model, &request)?;
    let bytes = serde_json::to_vec(&body).map_err(|err| Error::message(format!("chat request: {err}")))?;
    let text = match exchange(&endpoint, &key, &bytes).await {
        Ok(text) => text,
        Err(err) => return Err(Error::message(redact(&err.to_string(), &key))),
    };
    let events = events_from_body(&text, &model)
        .map_err(|err| Error::message(redact(&err.to_string(), &key)))?;
    Ok(EventStream::from_events(events))
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn nonempty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Field wins. The environment is consulted only when the field is empty.
fn setting_or_env(field: &str, from_env: Option<&str>, missing: &str) -> Result<String, Error> {
    if let Some(value) = nonempty(field) {
        return Ok(value.to_string());
    }
    if let Some(value) = from_env.and_then(nonempty) {
        return Ok(value.to_string());
    }
    Err(Error::message(missing))
}

fn validate_key(key: &str) -> Result<(), Error> {
    if key.bytes().any(|byte| byte.is_ascii_whitespace() || byte < 0x20 || byte == 0x7f) {
        return Err(Error::message("E4A_OPENROUTER_KEY is invalid"));
    }
    Ok(())
}

fn redact(text: &str, key: &str) -> String {
    if key.len() < 4 {
        return text.to_string();
    }
    text.replace(key, "[redacted]")
}

fn chat_body(model: &str, request: &TurnRequest) -> Result<Value, Error> {
    let trimmed = model.trim();
    let wire_model = trimmed.strip_prefix("openrouter::").unwrap_or(trimmed);
    if wire_model.is_empty() {
        return Err(Error::message("model is empty"));
    }
    if request.messages.is_empty() {
        return Err(Error::message("chat request has no messages"));
    }
    let mut messages = Vec::with_capacity(request.messages.len());
    for message in &request.messages {
        messages.push(wire_message(message)?);
    }
    let mut body = json!({
        "model": wire_model,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if !request.tools.is_empty() {
        let mut tools = Vec::with_capacity(request.tools.len());
        for tool in &request.tools {
            if tool.name.is_empty() {
                return Err(Error::message("tool schema is missing a name"));
            }
            tools.push(json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                }
            }));
        }
        body["tools"] = Value::Array(tools);
    }
    Ok(body)
}

fn wire_message(message: &ChatMessage) -> Result<Value, Error> {
    let role = message.role.trim();
    if !message.tool_calls.is_empty() {
        if role.is_empty() {
            return Err(Error::message("chat message is missing a role"));
        }
        let mut calls = Vec::with_capacity(message.tool_calls.len());
        for call in &message.tool_calls {
            if call.id.is_empty() || call.name.is_empty() {
                return Err(Error::message("tool call is missing an id or a name"));
            }
            let arguments = serde_json::to_string(&call.arguments)
                .map_err(|err| Error::message(format!("tool arguments: {err}")))?;
            calls.push(json!({
                "id": call.id,
                "type": "function",
                "function": {
                    "name": call.name,
                    "arguments": arguments,
                }
            }));
        }
        let content = if message.text.is_empty() {
            Value::Null
        } else {
            Value::String(message.text.clone())
        };
        return Ok(json!({
            "role": role,
            "content": content,
            "tool_calls": calls,
        }));
    }
    if role == "tool" || role == "toolResult" || message.tool_call_id.is_some() {
        let Some(id) = message.tool_call_id.as_deref().filter(|id| !id.is_empty()) else {
            return Err(Error::message("tool result is missing tool_call_id"));
        };
        return Ok(json!({
            "role": "tool",
            "tool_call_id": id,
            "content": message.text,
        }));
    }
    if role.is_empty() {
        return Err(Error::message("chat message is missing a role"));
    }
    Ok(json!({
        "role": role,
        "content": message.text,
    }))
}

struct SocksEndpoint {
    remote_dns: bool,
    host: String,
    port: u16,
    username: Option<String>,
    password: Option<String>,
}

fn parse_socks(url: &str) -> Result<SocksEndpoint, Error> {
    let url = url.trim();
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(Error::message("socks url needs a scheme"));
    };
    let remote_dns = match scheme.to_ascii_lowercase().as_str() {
        "socks5h" => true,
        "socks5" => false,
        _ => return Err(Error::message("socks url scheme must be socks5 or socks5h")),
    };
    let rest = rest.split(['/', '?', '#']).next().unwrap_or("");
    if rest.is_empty() {
        return Err(Error::message("socks url is missing a host"));
    }
    let (userinfo, hostport) = match rest.rfind('@') {
        Some(at) => (&rest[..at], &rest[at + 1..]),
        None => ("", rest),
    };
    if hostport.is_empty() {
        return Err(Error::message("socks url is missing a host"));
    }
    let (host, port) = split_host_port(hostport)?;
    let (username, password) = split_userinfo(userinfo)?;
    Ok(SocksEndpoint {
        remote_dns,
        host,
        port,
        username,
        password,
    })
}

fn split_host_port(hostport: &str) -> Result<(String, u16), Error> {
    let (host, port_text) = if let Some(rest) = hostport.strip_prefix('[') {
        let Some((host, after)) = rest.split_once(']') else {
            return Err(Error::message("socks url has a bad host"));
        };
        let Some(port_text) = after.strip_prefix(':') else {
            return Err(Error::message("socks url is missing a port"));
        };
        (host, port_text)
    } else {
        let Some((host, port_text)) = hostport.rsplit_once(':') else {
            return Err(Error::message("socks url is missing a port"));
        };
        (host, port_text)
    };
    if host.is_empty() || port_text.is_empty() {
        return Err(Error::message("socks url is missing a host"));
    }
    let port: u16 = port_text
        .parse()
        .map_err(|_| Error::message("socks url has a bad port"))?;
    if port == 0 {
        return Err(Error::message("socks url has a bad port"));
    }
    let host = percent_decode(host)?;
    if host.is_empty() || host.bytes().any(|byte| byte.is_ascii_control() || byte == b' ' || byte == b'/') {
        return Err(Error::message("socks url has a bad host"));
    }
    Ok((host, port))
}

fn split_userinfo(raw: &str) -> Result<(Option<String>, Option<String>), Error> {
    if raw.is_empty() {
        return Ok((None, None));
    }
    let (user, pass) = match raw.split_once(':') {
        Some((user, pass)) => (user, pass),
        None => (raw, ""),
    };
    let user = percent_decode(user)?;
    let pass = percent_decode(pass)?;
    if user.is_empty() {
        return Err(Error::message("socks url is missing a username"));
    }
    Ok((Some(user), Some(pass)))
}

fn percent_decode(input: &str) -> Result<String, Error> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index.saturating_add(2) >= bytes.len() {
                return Err(Error::message("socks url has a bad escape"));
            }
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
                .map_err(|_| Error::message("socks url has a bad escape"))?;
            let value = u8::from_str_radix(hex, 16)
                .map_err(|_| Error::message("socks url has a bad escape"))?;
            out.push(value);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).map_err(|_| Error::message("socks url userinfo is not utf-8"))
}

fn proxy_label(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn socks5_greeting(userinfo: bool) -> [u8; 3] {
    if userinfo {
        [0x05, 0x01, 0x02]
    } else {
        [0x05, 0x01, 0x00]
    }
}

fn socks5_userpass(user: &str, pass: &str) -> Result<Vec<u8>, Error> {
    let user = user.as_bytes();
    let pass = pass.as_bytes();
    if user.is_empty() || user.len() > 255 || pass.len() > 255 || user.contains(&0) || pass.contains(&0) {
        return Err(Error::message("socks5 userinfo is invalid"));
    }
    let mut out = Vec::with_capacity(3 + user.len() + pass.len());
    out.push(0x01);
    out.push(u8::try_from(user.len()).unwrap_or(255));
    out.extend_from_slice(user);
    out.push(u8::try_from(pass.len()).unwrap_or(255));
    out.extend_from_slice(pass);
    Ok(out)
}

/// SOCKS5 CONNECT (RFC 1928) for a domain name. `socks5h` sends this for `openrouter.ai`.
fn socks5_connect_request(host: &str, port: u16) -> Result<Vec<u8>, Error> {
    let host_bytes = host.as_bytes();
    if host_bytes.is_empty() || host_bytes.len() > 255 {
        return Err(Error::message("socks5 destination host length is invalid"));
    }
    let mut out = Vec::with_capacity(7 + host_bytes.len());
    out.push(0x05);
    out.push(0x01);
    out.push(0x00);
    out.push(0x03);
    out.push(u8::try_from(host_bytes.len()).unwrap_or(255));
    out.extend_from_slice(host_bytes);
    out.extend_from_slice(&port.to_be_bytes());
    Ok(out)
}

fn socks5_connect_ipv4(octets: [u8; 4], port: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(10);
    out.extend_from_slice(&[0x05, 0x01, 0x00, 0x01]);
    out.extend_from_slice(&octets);
    out.extend_from_slice(&port.to_be_bytes());
    out
}

fn socks5_connect_ipv6(octets: [u8; 16], port: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(22);
    out.extend_from_slice(&[0x05, 0x01, 0x00, 0x04]);
    out.extend_from_slice(&octets);
    out.extend_from_slice(&port.to_be_bytes());
    out
}

fn socks_reply_error(rep: u8) -> Error {
    let reason = match rep {
        0x01 => "general failure",
        0x02 => "connection not allowed",
        0x03 => "network unreachable",
        0x04 => "host unreachable",
        0x05 => "connection refused",
        0x06 => "ttl expired",
        0x07 => "command not supported",
        0x08 => "address type not supported",
        _ => "rejected",
    };
    Error::message(format!("socks5 connect {reason}"))
}

async fn within<T>(label: &str, limit: Duration, fut: impl Future<Output = Result<T, Error>>) -> Result<T, Error> {
    match tokio::time::timeout(limit, fut).await {
        Ok(result) => result,
        Err(_) => Err(Error::message(format!("{label} timed out"))),
    }
}

async fn exchange(endpoint: &SocksEndpoint, key: &str, body: &[u8]) -> Result<String, Error> {
    let mut tls = within("socks5 dial", DIAL_TIMEOUT, dial_tls(endpoint)).await?;
    within("openrouter", TURN_TIMEOUT, http_roundtrip(&mut tls, key, body)).await
}

async fn dial_tls(endpoint: &SocksEndpoint) -> Result<tokio_rustls::client::TlsStream<TcpStream>, Error> {
    let mut tcp = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
        .await
        .map_err(|err| Error::message(format!("socks5 dial {}: {err}", proxy_label(&endpoint.host, endpoint.port))))?;
    tcp.set_nodelay(true)
        .map_err(|err| Error::message(format!("socks5 dial: {err}")))?;
    socks_handshake(&mut tcp, endpoint).await?;
    let config = tls_config()?;
    let server_name = rustls::pki_types::ServerName::try_from(OPENROUTER_HOST)
        .map_err(|_| Error::message("openrouter host is not a dns name"))?
        .to_owned();
    tokio_rustls::TlsConnector::from(config)
        .connect(server_name, tcp)
        .await
        .map_err(|err| Error::message(format!("tls handshake: {err}")))
}

fn tls_config() -> Result<Arc<rustls::ClientConfig>, Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|err| Error::message(format!("tls config: {err}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

async fn socks_handshake(stream: &mut TcpStream, endpoint: &SocksEndpoint) -> Result<(), Error> {
    let greeting = socks5_greeting(endpoint.username.is_some());
    write_all(stream, &greeting).await?;
    let mut choice = [0u8; 2];
    read_exact(stream, &mut choice).await?;
    if choice[0] != 0x05 {
        return Err(Error::message("socks5 greeting was rejected"));
    }
    match (choice[1], endpoint.username.as_deref()) {
        (0x00, None) => {}
        (0x02, Some(user)) => {
            let pass = endpoint.password.as_deref().unwrap_or("");
            let packet = socks5_userpass(user, pass)?;
            write_all(stream, &packet).await?;
            let mut status = [0u8; 2];
            read_exact(stream, &mut status).await?;
            if status[1] != 0x00 {
                return Err(Error::message("socks5 authentication failed"));
            }
        }
        (0xff, _) => return Err(Error::message("socks5 proxy rejected the auth methods")),
        _ => return Err(Error::message("socks5 proxy selected an unsupported auth method")),
    }
    let request = destination_request(endpoint.remote_dns).await?;
    write_all(stream, &request).await?;
    read_connect_reply(stream).await
}

async fn destination_request(remote_dns: bool) -> Result<Vec<u8>, Error> {
    if remote_dns {
        return socks5_connect_request(OPENROUTER_HOST, OPENROUTER_PORT);
    }
    let addr = tokio::net::lookup_host((OPENROUTER_HOST, OPENROUTER_PORT))
        .await
        .map_err(|err| Error::message(format!("resolve openrouter.ai: {err}")))?
        .next()
        .ok_or_else(|| Error::message("resolve openrouter.ai returned no address"))?;
    Ok(match addr.ip() {
        IpAddr::V4(ip) => socks5_connect_ipv4(ip.octets(), OPENROUTER_PORT),
        IpAddr::V6(ip) => socks5_connect_ipv6(ip.octets(), OPENROUTER_PORT),
    })
}

async fn read_connect_reply(stream: &mut TcpStream) -> Result<(), Error> {
    let mut head = [0u8; 4];
    read_exact(stream, &mut head).await?;
    if head[0] != 0x05 {
        return Err(Error::message("socks5 connect reply is invalid"));
    }
    if head[1] != 0x00 {
        return Err(socks_reply_error(head[1]));
    }
    let addr_len = match head[3] {
        0x01 => 4,
        0x04 => 16,
        0x03 => {
            let mut len = [0u8; 1];
            read_exact(stream, &mut len).await?;
            usize::from(len[0])
        }
        _ => return Err(Error::message("socks5 connect reply address is invalid")),
    };
    let mut rest = vec![0u8; addr_len.saturating_add(2)];
    read_exact(stream, &mut rest).await?;
    Ok(())
}

async fn write_all(stream: &mut TcpStream, bytes: &[u8]) -> Result<(), Error> {
    stream
        .write_all(bytes)
        .await
        .map_err(|err| Error::message(format!("socks5 write: {err}")))?;
    stream
        .flush()
        .await
        .map_err(|err| Error::message(format!("socks5 write: {err}")))?;
    Ok(())
}

async fn read_exact(stream: &mut TcpStream, buf: &mut [u8]) -> Result<(), Error> {
    stream
        .read_exact(buf)
        .await
        .map_err(|err| Error::message(format!("socks5 read: {err}")))?;
    Ok(())
}

async fn http_roundtrip<S>(stream: &mut S, key: &str, body: &[u8]) -> Result<String, Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut request = http_request(key, body);
    stream
        .write_all(&request)
        .await
        .map_err(|err| Error::message(format!("openrouter write: {err}")))?;
    stream
        .flush()
        .await
        .map_err(|err| Error::message(format!("openrouter write: {err}")))?;
    for byte in &mut request {
        *byte = 0;
    }
    drop(request);

    let mut io = Io {
        stream,
        pending: Vec::new(),
    };
    let head_bytes = io.read_header_block().await?;
    let head_text = String::from_utf8(head_bytes)
        .map_err(|_| Error::message("openrouter headers are not utf-8"))?;
    let head = parse_head(&head_text)?;
    let bytes = read_http_body(&mut io, &head).await?;
    if !(200..300).contains(&head.status) {
        let text = String::from_utf8_lossy(&bytes);
        return Err(public_error(&head, &text, key));
    }
    String::from_utf8(bytes).map_err(|_| Error::message("openrouter response is not utf-8"))
}

fn http_request(key: &str, body: &[u8]) -> Vec<u8> {
    let header = format!(
        "POST {OPENROUTER_PATH} HTTP/1.1\r\nHost: {OPENROUTER_HOST}\r\nAuthorization: Bearer {key}\r\nContent-Type: application/json\r\nAccept: text/event-stream, application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut out = header.into_bytes();
    out.extend_from_slice(body);
    out
}

struct Io<'a, S> {
    stream: &'a mut S,
    pending: Vec<u8>,
}

impl<S> Io<'_, S>
where
    S: AsyncRead + Unpin,
{
    async fn fill(&mut self) -> Result<bool, Error> {
        let mut buf = [0u8; 8192];
        let n = self
            .stream
            .read(&mut buf)
            .await
            .map_err(|err| Error::message(format!("openrouter read: {err}")))?;
        if n == 0 {
            return Ok(false);
        }
        self.pending.extend_from_slice(&buf[..n]);
        Ok(true)
    }

    async fn read_header_block(&mut self) -> Result<Vec<u8>, Error> {
        const MARK: &[u8] = b"\r\n\r\n";
        loop {
            if let Some(pos) = self.pending.windows(MARK.len()).position(|window| window == MARK) {
                let end = pos + MARK.len();
                return Ok(self.pending.drain(..end).collect());
            }
            if self.pending.len() > 64 * 1024 {
                return Err(Error::message("openrouter headers are too large"));
            }
            if !self.fill().await? {
                return Err(Error::message("openrouter connection closed before headers"));
            }
        }
    }
}

struct ResponseHead {
    status: u16,
    headers: Vec<(String, String)>,
}

impl ResponseHead {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn chunked(&self) -> bool {
        self.header("transfer-encoding").is_some_and(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("chunked"))
        })
    }

    fn content_length(&self) -> Result<Option<usize>, Error> {
        let Some(value) = self.header("content-length") else {
            return Ok(None);
        };
        let len: u64 = value
            .trim()
            .parse()
            .map_err(|_| Error::message("openrouter content-length is invalid"))?;
        let len = usize::try_from(len).map_err(|_| Error::message("openrouter response is too large"))?;
        Ok(Some(len))
    }
}

fn parse_head(block: &str) -> Result<ResponseHead, Error> {
    let mut lines = block.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let mut parts = status_line.split(' ');
    let version = parts.next().unwrap_or("");
    if !version.starts_with("HTTP/1.") {
        return Err(Error::message("openrouter status line is not http/1"));
    }
    let code = parts.next().unwrap_or("");
    let status: u16 = code
        .parse()
        .map_err(|_| Error::message("openrouter status is invalid"))?;
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(Error::message("openrouter header is invalid"));
        };
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    Ok(ResponseHead { status, headers })
}

async fn read_http_body<S>(io: &mut Io<'_, S>, head: &ResponseHead) -> Result<Vec<u8>, Error>
where
    S: AsyncRead + Unpin,
{
    if head.chunked() {
        loop {
            match try_decode_chunked(&io.pending)? {
                Some((body, consumed)) => {
                    io.pending.drain(..consumed);
                    return Ok(body);
                }
                None => {
                    if io.pending.len() > MAX_BODY.saturating_add(1024 * 1024) {
                        return Err(Error::message("openrouter response is too large"));
                    }
                    if !io.fill().await? {
                        return Err(Error::message("openrouter connection closed early"));
                    }
                }
            }
        }
    }
    if let Some(len) = head.content_length()? {
        if len > MAX_BODY {
            return Err(Error::message("openrouter response is too large"));
        }
        while io.pending.len() < len {
            if !io.fill().await? {
                return Err(Error::message("openrouter connection closed early"));
            }
        }
        return Ok(io.pending.drain(..len).collect());
    }
    while io.fill().await? {
        if io.pending.len() > MAX_BODY {
            return Err(Error::message("openrouter response is too large"));
        }
    }
    Ok(std::mem::take(&mut io.pending))
}

/// `Ok(None)` means the buffer does not yet contain a full chunked body.
fn try_decode_chunked(input: &[u8]) -> Result<Option<(Vec<u8>, usize)>, Error> {
    let mut pos = 0;
    let mut body = Vec::new();
    loop {
        let Some(line_end) = find_crlf(input, pos) else {
            return Ok(None);
        };
        let line = std::str::from_utf8(&input[pos..line_end])
            .map_err(|_| Error::message("openrouter chunk size is invalid"))?;
        let size_hex = line.split(';').next().unwrap_or("").trim();
        if size_hex.is_empty() {
            return Err(Error::message("openrouter chunk size is missing"));
        }
        let size = usize::from_str_radix(size_hex, 16)
            .map_err(|_| Error::message("openrouter chunk size is invalid"))?;
        pos = line_end + 2;
        if size == 0 {
            loop {
                let Some(trailer_end) = find_crlf(input, pos) else {
                    return Ok(None);
                };
                let trailer = &input[pos..trailer_end];
                pos = trailer_end + 2;
                if trailer.is_empty() {
                    return Ok(Some((body, pos)));
                }
            }
        }
        if size > MAX_BODY || body.len().saturating_add(size) > MAX_BODY {
            return Err(Error::message("openrouter response is too large"));
        }
        if input.len() < pos.saturating_add(size).saturating_add(2) {
            return Ok(None);
        }
        body.extend_from_slice(&input[pos..pos + size]);
        if &input[pos + size..pos + size + 2] != b"\r\n" {
            return Err(Error::message("openrouter chunk is truncated"));
        }
        pos += size + 2;
    }
}

fn find_crlf(input: &[u8], start: usize) -> Option<usize> {
    input
        .get(start..)?
        .windows(2)
        .position(|window| window == b"\r\n")
        .map(|offset| start + offset)
}

/// The error a non-2xx response becomes. A 429 says "rate limited" and carries
/// the `X-RateLimit-*` counters when the response has them, so a caller can
/// pick a fallback model without parsing prose.
fn public_error(head: &ResponseHead, body: &str, key: &str) -> Error {
    let detail = truncate(&redact(&error_snippet(body), key), 300);
    let prefix = if head.status == 429 {
        rate_limit_note(head).unwrap_or_else(|| "openrouter rate limited".into())
    } else {
        format!("openrouter status {}", head.status)
    };
    if detail.is_empty() {
        Error::message(prefix)
    } else {
        Error::message(format!("{prefix}: {detail}"))
    }
}

/// `requests left N, tokens left M, resets DATE` from the rate-limit headers.
fn rate_limit_note(head: &ResponseHead) -> Option<String> {
    let mut parts = Vec::new();
    for (header, label) in [
        ("x-ratelimit-requests-remaining", "requests left"),
        ("x-ratelimit-tokens-remaining", "tokens left"),
    ] {
        if let Some(value) = head.header(header) {
            parts.push(format!("{label} {value}"));
        }
    }
    if let Some(reset) = head.header("x-ratelimit-reset") {
        parts.push(format!("resets {reset}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(format!("openrouter rate limited ({})", parts.join(", ")))
    }
}

fn error_snippet(body: &str) -> String {
    let text = body.trim();
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        if let Some(error) = value.get("error") {
            return error_text(error);
        }
    }
    text.to_string()
}

fn error_text(error: &Value) -> String {
    match error {
        Value::String(text) => text.clone(),
        Value::Object(_) => error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("request failed")
            .to_string(),
        _ => "request failed".to_string(),
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str("...");
    out
}

#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
struct Accum {
    text: String,
    calls: BTreeMap<u32, PartialCall>,
    usage: Option<(i64, i64, f64)>,
    reported_model: String,
    finish: Option<String>,
}

fn events_from_body(body: &str, model: &str) -> Result<Vec<LlmEvent>, Error> {
    let trimmed = body.trim_start_matches('\u{feff}').trim();
    if trimmed.is_empty() {
        return Err(Error::message("openrouter returned an empty body"));
    }
    if trimmed.starts_with('{') {
        let value: Value = serde_json::from_str(trimmed)
            .map_err(|err| Error::message(format!("openrouter json: {err}")))?;
        return events_from_completion(&value, model);
    }
    events_from_sse(trimmed, model)
}

fn events_from_completion(value: &Value, model: &str) -> Result<Vec<LlmEvent>, Error> {
    let mut accum = Accum::default();
    absorb_chunk(&mut accum, value)?;
    finish_accum(accum, model)
}

fn events_from_sse(body: &str, model: &str) -> Result<Vec<LlmEvent>, Error> {
    let mut accum = Accum::default();
    let mut data_lines: Vec<&str> = Vec::new();
    let mut done = false;
    for line in body.split('\n') {
        if done {
            break;
        }
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if !data_lines.is_empty() {
                let joined = data_lines.join("\n");
                data_lines.clear();
                if apply_data(&mut accum, &joined)? {
                    done = true;
                }
            }
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    if !done && !data_lines.is_empty() {
        let joined = data_lines.join("\n");
        let _ = apply_data(&mut accum, &joined)?;
    }
    finish_accum(accum, model)
}

fn apply_data(accum: &mut Accum, data: &str) -> Result<bool, Error> {
    let data = data.trim();
    if data.is_empty() {
        return Ok(false);
    }
    if data == "[DONE]" {
        return Ok(true);
    }
    let value: Value = serde_json::from_str(data)
        .map_err(|err| Error::message(format!("openrouter event: {err}")))?;
    absorb_chunk(accum, &value)?;
    Ok(false)
}

fn absorb_chunk(accum: &mut Accum, chunk: &Value) -> Result<(), Error> {
    if let Some(error) = chunk.get("error").filter(|value| !value.is_null()) {
        return Err(Error::message(format!("openrouter: {}", error_text(error))));
    }
    if accum.reported_model.is_empty() {
        if let Some(model) = chunk.get("model").and_then(Value::as_str) {
            if !model.is_empty() {
                accum.reported_model = model.to_string();
            }
        }
    }
    if let Some(usage) = chunk.get("usage").filter(|value| value.is_object()) {
        accum.usage = Some(parse_usage(usage)?);
    }
    let Some(choice) = chunk
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    else {
        return Ok(());
    };
    if let Some(usage) = choice.get("usage").filter(|value| value.is_object()) {
        accum.usage = Some(parse_usage(usage)?);
    }
    if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
        if !reason.is_empty() {
            accum.finish = Some(reason.to_string());
        }
    }
    if let Some(message) = choice.get("message").filter(|value| value.is_object()) {
        absorb_message(accum, message)?;
    } else if let Some(delta) = choice.get("delta").filter(|value| value.is_object()) {
        absorb_delta(accum, delta)?;
    }
    Ok(())
}

fn absorb_message(accum: &mut Accum, message: &Value) -> Result<(), Error> {
    if let Some(content) = message.get("content") {
        push_content(accum, content);
    }
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for (offset, call) in calls.iter().enumerate() {
            let index = call_index(call, u32::try_from(offset).unwrap_or(u32::MAX))?;
            absorb_one_call(accum, index, call)?;
        }
    }
    Ok(())
}

fn absorb_delta(accum: &mut Accum, delta: &Value) -> Result<(), Error> {
    if let Some(content) = delta.get("content") {
        push_content(accum, content);
    }
    if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for (offset, call) in calls.iter().enumerate() {
            let index = call_index(call, u32::try_from(offset).unwrap_or(u32::MAX))?;
            absorb_one_call(accum, index, call)?;
        }
    }
    Ok(())
}

fn push_content(accum: &mut Accum, content: &Value) {
    match content {
        Value::String(text) => accum.text.push_str(text),
        Value::Array(parts) => {
            for part in parts {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    accum.text.push_str(text);
                } else if let Some(text) = part.as_str() {
                    accum.text.push_str(text);
                }
            }
        }
        _ => {}
    }
}

fn call_index(call: &Value, fallback: u32) -> Result<u32, Error> {
    match call.get("index").filter(|value| !value.is_null()) {
        Some(value) => {
            let n = json_i64(value).ok_or_else(|| Error::message("tool call index is invalid"))?;
            u32::try_from(n).map_err(|_| Error::message("tool call index is invalid"))
        }
        None => Ok(fallback),
    }
}

fn absorb_one_call(accum: &mut Accum, index: u32, call: &Value) -> Result<(), Error> {
    let entry = accum.calls.entry(index).or_default();
    if let Some(id) = call.get("id").and_then(Value::as_str) {
        if !id.is_empty() {
            if !entry.id.is_empty() && entry.id != id {
                return Err(Error::message("tool call id changed"));
            }
            entry.id = id.to_string();
        }
    }
    let function = call.get("function");
    if let Some(name) = function.and_then(|value| value.get("name")).and_then(Value::as_str) {
        if !name.is_empty() {
            if !entry.name.is_empty() && entry.name != name {
                return Err(Error::message("tool call name changed"));
            }
            entry.name = name.to_string();
        }
    }
    if let Some(arguments) = function.and_then(|value| value.get("arguments")) {
        match arguments {
            Value::String(text) => entry.arguments.push_str(text),
            Value::Null => {}
            other => {
                let text = serde_json::to_string(other)
                    .map_err(|err| Error::message(format!("tool arguments: {err}")))?;
                entry.arguments.push_str(&text);
            }
        }
    }
    Ok(())
}

fn parse_usage(value: &Value) -> Result<(i64, i64, f64), Error> {
    let cost = value
        .get("cost")
        .and_then(Value::as_f64)
        .filter(|cost| cost.is_finite() && *cost >= 0.0)
        .unwrap_or(0.0);
    Ok((
        token_count(value, &["prompt_tokens", "input_tokens"])?,
        token_count(value, &["completion_tokens", "output_tokens"])?,
        cost,
    ))
}

fn token_count(value: &Value, keys: &[&str]) -> Result<i64, Error> {
    for key in keys {
        if let Some(part) = value.get(*key).filter(|part| !part.is_null()) {
            return json_i64(part).ok_or_else(|| Error::message("openrouter usage is invalid"));
        }
    }
    Ok(0)
}

fn json_i64(value: &Value) -> Option<i64> {
    if let Some(n) = value.as_i64() {
        return (n >= 0).then_some(n);
    }
    if let Some(n) = value.as_u64() {
        return i64::try_from(n).ok();
    }
    let n = value.as_f64()?;
    if n.is_finite() && n >= 0.0 && n <= i64::MAX as f64 {
        Some(n as i64)
    } else {
        None
    }
}

fn finish_accum(accum: Accum, configured_model: &str) -> Result<Vec<LlmEvent>, Error> {
    match accum.finish.as_deref() {
        Some("error") => return Err(Error::message("openrouter finish_reason error")),
        Some("content_filter") => {
            return Err(Error::message("openrouter finish_reason content_filter"))
        }
        _ => {}
    }
    if accum.calls.len() > 1 {
        return Err(Error::message("only one tool call is committed in this slice"));
    }
    let mut events = Vec::new();
    if !accum.text.is_empty() {
        events.push(LlmEvent::Text(accum.text));
    }
    if let Some((_, call)) = accum.calls.iter().next() {
        if call.id.is_empty() || call.name.is_empty() {
            return Err(Error::message("tool call was incomplete"));
        }
        let arguments = if call.arguments.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&call.arguments)
                .map_err(|_| Error::message("tool arguments are not json"))?
        };
        events.push(LlmEvent::ToolCall(ToolCall {
            id: call.id.clone(),
            name: call.name.clone(),
            arguments,
        }));
    }
    if events.is_empty() {
        return Err(Error::message("model turn was empty"));
    }
    let Some((input_tokens, output_tokens, cost_usd)) = accum.usage else {
        return Err(Error::message("model turn did not report usage"));
    };
    let model = if accum.reported_model.is_empty() {
        configured_model.to_string()
    } else {
        accum.reported_model
    };
    events.push(LlmEvent::Usage(Usage {
        input_tokens,
        output_tokens,
        provider: "openrouter".into(),
        model,
        cost_usd,
    }));
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn socks5_connect_for_openrouter_uses_remote_dns_form() {
        let bytes = socks5_connect_request("openrouter.ai", 443).unwrap();
        assert_eq!(&bytes[..5], &[0x05, 0x01, 0x00, 0x03, 13]);
        assert_eq!(&bytes[5..18], b"openrouter.ai");
        assert_eq!(&bytes[18..], &443u16.to_be_bytes());
    }

    #[test]
    fn socks5h_keeps_remote_dns_and_userinfo() {
        let endpoint = parse_socks("socks5h://user:p%40ss@[::1]:1080").unwrap();
        assert!(endpoint.remote_dns);
        assert_eq!(endpoint.host, "::1");
        assert_eq!(endpoint.port, 1080);
        assert_eq!(endpoint.username.as_deref(), Some("user"));
        assert_eq!(endpoint.password.as_deref(), Some("p@ss"));
        assert!(parse_socks("socks5h://:1080").is_err());
        assert!(parse_socks("socks5://127.0.0.1").is_err());
        let local = parse_socks("socks5://127.0.0.1:9050").unwrap();
        assert!(!local.remote_dns);
        assert_eq!(socks5_greeting(false), [0x05, 0x01, 0x00]);
        assert_eq!(socks5_greeting(true), [0x05, 0x01, 0x02]);
    }

    #[test]
    fn empty_field_uses_env_only_as_fallback() {
        let missing = setting_or_env("", None, "E4A_SOCKS is unset").unwrap_err();
        assert_eq!(missing.to_string(), "E4A_SOCKS is unset");
        let from_env = setting_or_env("", Some("socks5h://127.0.0.1:1"), "E4A_SOCKS is unset").unwrap();
        assert_eq!(from_env, "socks5h://127.0.0.1:1");
        let field = setting_or_env("socks5h://10.0.0.1:2", Some("ignored"), "E4A_SOCKS is unset").unwrap();
        assert_eq!(field, "socks5h://10.0.0.1:2");
        let key = setting_or_env("", None, "E4A_OPENROUTER_KEY is unset").unwrap_err();
        assert_eq!(key.to_string(), "E4A_OPENROUTER_KEY is unset");
    }

    #[test]
    fn chat_body_maps_messages_and_tools() {
        let request = TurnRequest {
            messages: vec![
                ChatMessage {
                    role: "user".into(),
                    text: "hi".into(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    is_error: false,
                },
                ChatMessage {
                    role: "assistant".into(),
                    text: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "call-1".into(),
                        name: "read".into(),
                        arguments: json!({ "path": "note.txt" }),
                    }],
                    tool_call_id: None,
                    is_error: false,
                },
                ChatMessage {
                    role: "tool".into(),
                    text: "ok".into(),
                    tool_calls: Vec::new(),
                    tool_call_id: Some("call-1".into()),
                    is_error: false,
                },
            ],
            tools: vec![crate::ToolSchema {
                name: "read".into(),
                description: "Read a file".into(),
                parameters: json!({ "type": "object" }),
            }],
        };
        let body = chat_body("openrouter::vendor/model", &request).unwrap();
        assert_eq!(body["model"], "vendor/model");
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["content"], "hi");
        assert_eq!(body["messages"][1]["tool_calls"][0]["function"]["name"], "read");
        assert_eq!(
            body["messages"][1]["tool_calls"][0]["function"]["arguments"],
            "{\"path\":\"note.txt\"}"
        );
        assert_eq!(body["messages"][2]["role"], "tool");
        assert_eq!(body["messages"][2]["tool_call_id"], "call-1");
        assert_eq!(body["tools"][0]["function"]["name"], "read");
    }

    #[test]
    fn sse_text_and_one_tool_call_become_events() {
        let text = "data: {\"model\":\"vendor/model\",\"choices\":[{\"delta\":{\"content\":\"pon\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"g\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":19,\"completion_tokens\":5}}\n\n\
data: [DONE]\n";
        let events = events_from_body(text, "openrouter::vendor/model").unwrap();
        assert_eq!(
            events,
            vec![
                LlmEvent::Text("pong".into()),
                LlmEvent::Usage(Usage {
                    input_tokens: 19,
                    output_tokens: 5,
                    provider: "openrouter".into(),
                    model: "vendor/model".into(),
                    cost_usd: 0.0,
                }),
            ]
        );

        let tool = "data: {\"model\":\"vendor/model\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"read\",\"arguments\":\"\"}}]}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"path\\\":\\\"note.txt\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":4}}\n\n\
data: [DONE]\n";
        let events = events_from_body(tool, "configured").unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0],
            LlmEvent::ToolCall(ToolCall {
                id: "call_1".into(),
                name: "read".into(),
                arguments: json!({ "path": "note.txt" }),
            })
        );
    }

    #[test]
    fn two_tool_calls_are_rejected() {
        let body = r#"{"model":"vendor/model","choices":[{"message":{"content":null,"tool_calls":[{"id":"a","type":"function","function":{"name":"read","arguments":"{}"}},{"id":"b","type":"function","function":{"name":"read","arguments":"{}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#;
        let error = events_from_body(body, "configured").unwrap_err();
        assert_eq!(error.to_string(), "only one tool call is committed in this slice");
    }

    #[test]
    fn chunked_body_stops_at_the_zero_chunk() {
        let raw = b"5\r\nhello\r\n0\r\n\r\n";
        let (body, consumed) = try_decode_chunked(raw).unwrap().unwrap();
        assert_eq!(body, b"hello");
        assert_eq!(consumed, raw.len());
        assert!(try_decode_chunked(b"5\r\nhel").unwrap().is_none());
    }

    #[test]
    fn status_error_drops_the_bearer_token() {
        let key = "test-token-value";
        let head = parse_head("HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\n\r\n").unwrap();
        let err = public_error(&head, &format!("{{\"error\":{{\"message\":\"bad {key}\"}}}}"), key);
        assert!(!err.to_string().contains(key));
        assert!(err.to_string().contains("[redacted]"));
        assert!(err.to_string().starts_with("openrouter status 401"));
    }

    #[test]
    fn a_429_names_the_rate_limit_and_its_counters() {
        let raw = "HTTP/1.1 429 Too Many Requests\r\n\
X-RateLimit-Requests-Remaining: 0\r\n\
X-RateLimit-Tokens-Remaining: 12\r\n\
X-RateLimit-Reset: 2026-11-17T00:00:00.000Z\r\n\r\n";
        let head = parse_head(raw).unwrap();
        let err = public_error(&head, "{\"error\":{\"message\":\"free-model limit\"}}", "k");
        let text = err.to_string();
        assert!(text.starts_with("openrouter rate limited (requests left 0, tokens left 12"), "{text}");
        assert!(text.contains("resets 2026-11-17"), "{text}");
        assert!(text.contains("free-model limit"), "{text}");
    }

    #[test]
    fn a_429_without_headers_still_names_the_limit() {
        let head = parse_head("HTTP/1.1 429 Too Many Requests\r\nContent-Length: 2\r\n\r\n").unwrap();
        let err = public_error(&head, "", "k");
        assert_eq!(err.to_string(), "openrouter rate limited");
    }

    #[test]
    fn usage_carries_the_reported_cost() {
        let body = "data: {\"model\":\"vendor/model\",\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":2,\"cost\":0.0000031}}\n\ndata: [DONE]\n";
        let events = events_from_body(body, "configured").unwrap();
        let LlmEvent::Usage(usage) = events.last().unwrap() else {
            panic!("no usage event");
        };
        assert_eq!(usage.cost_usd, 0.0000031);
    }
}
