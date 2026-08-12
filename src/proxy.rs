//! Opt-in diagnostic proxies that persist timing and byte counts, never content.

use crate::network::NetworkFlow;
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use chrono::{SecondsFormat, Utc};
use rustls::pki_types::ServerName;
use rustls::{
    ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection, StreamOwned,
};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const MAX_HEADER_BYTES: usize = 65_536;
const MAX_REQUEST_BYTES: u64 = 64 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(120);

pub type FlowHandler = Arc<dyn Fn(NetworkFlow) + Send + Sync + 'static>;

/// Run a conventional HTTP CONNECT proxy. Tunnel bytes remain opaque.
pub fn serve_tunnel(bind: SocketAddr, handler: FlowHandler) -> Result<()> {
    let listener =
        TcpListener::bind(bind).with_context(|| format!("bind CONNECT proxy at {bind}"))?;
    for client in listener.incoming() {
        let handler = Arc::clone(&handler);
        match client {
            Ok(client) => {
                thread::spawn(move || handler(handle_tunnel(client)));
            }
            Err(error) => eprintln!("proxy accept warning: {error}"),
        }
    }
    Ok(())
}

fn handle_tunnel(mut client: TcpStream) -> NetworkFlow {
    let wall = Utc::now();
    let started = Instant::now();
    let mut response_sent = false;
    let mut flow = base_flow("tunnel_proxy", "local_connect_proxy", wall);
    flow.destination_port = Some(443);
    flow.protocol = Some("tls-opaque".into());
    let result = (|| -> Result<()> {
        let (header, pending) = read_header(&mut client, MAX_HEADER_BYTES)?;
        let first = String::from_utf8_lossy(&header)
            .lines()
            .next()
            .context("empty proxy request")?
            .to_owned();
        let mut fields = first.split_whitespace();
        if !fields
            .next()
            .is_some_and(|method| method.eq_ignore_ascii_case("CONNECT"))
        {
            client.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
            response_sent = true;
            bail!("CONNECT required");
        }
        let (host, port) = parse_connect_target(fields.next().context("CONNECT target missing")?)?;
        flow.destination_host = Some(host.clone());
        flow.destination_port = Some(port);
        let dns = Instant::now();
        let address = (host.as_str(), port)
            .to_socket_addrs()?
            .next()
            .context("no addresses")?;
        flow.dns_ms = Some(dns.elapsed().as_secs_f64() * 1000.0);
        flow.destination_ip = Some(address.ip().to_string());
        let tcp = Instant::now();
        let mut remote = TcpStream::connect_timeout(&address, Duration::from_secs(15))?;
        flow.tcp_ms = Some(tcp.elapsed().as_secs_f64() * 1000.0);
        remote.set_read_timeout(Some(IO_TIMEOUT))?;
        remote.set_write_timeout(Some(IO_TIMEOUT))?;
        client.set_read_timeout(Some(IO_TIMEOUT))?;
        client.set_write_timeout(Some(IO_TIMEOUT))?;
        client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")?;
        response_sent = true;
        if !pending.is_empty() {
            remote.write_all(&pending)?;
            flow.request_bytes = pending.len() as u64;
        }
        let (outgoing, incoming) = relay(client.try_clone()?, remote)?;
        flow.request_bytes += outgoing;
        flow.response_bytes = incoming;
        flow.success = Some(true);
        Ok(())
    })();
    if let Err(error) = result {
        flow.error_type = Some(error_kind(&error));
        if !response_sent {
            let _ = client.write_all(
                b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    }
    finish_flow(&mut flow, started);
    flow
}

fn relay(client: TcpStream, remote: TcpStream) -> Result<(u64, u64)> {
    let mut client_reader = client.try_clone()?;
    let mut client_writer = client;
    let mut remote_reader = remote.try_clone()?;
    let mut remote_writer = remote;
    let outgoing = thread::spawn(move || {
        let result = std::io::copy(&mut client_reader, &mut remote_writer);
        let _ = remote_writer.shutdown(Shutdown::Write);
        result
    });
    let incoming = std::io::copy(&mut remote_reader, &mut client_writer);
    let _ = client_writer.shutdown(Shutdown::Write);
    let outgoing = outgoing
        .join()
        .map_err(|_| anyhow!("relay thread failed"))??;
    Ok((outgoing, incoming?))
}

#[derive(Debug, Clone)]
struct ReverseTarget {
    scheme: String,
    host: String,
    port: u16,
    prefix: String,
}

struct ProxyTarget {
    host: String,
    port: u16,
    authorization: Option<String>,
}

impl ProxyTarget {
    fn parse(value: &str) -> Result<Self> {
        let (scheme, remainder) = value
            .split_once("://")
            .context("invalid upstream proxy URL")?;
        if scheme != "http" {
            bail!("only http:// upstream proxies are supported");
        }
        let authority = remainder.split('/').next().unwrap_or("");
        let (credentials, authority) = authority
            .rsplit_once('@')
            .map(|(credentials, authority)| (Some(credentials), authority))
            .unwrap_or((None, authority));
        let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
            let (host, suffix) = bracketed
                .split_once(']')
                .context("invalid IPv6 proxy URL")?;
            let port = suffix
                .strip_prefix(':')
                .map(str::parse)
                .transpose()?
                .unwrap_or(80);
            (host.to_owned(), port)
        } else if let Some((host, port)) = authority.rsplit_once(':') {
            (host.to_owned(), port.parse()?)
        } else {
            (authority.to_owned(), 80)
        };
        if host.is_empty() || host.contains(['\r', '\n', '\0']) {
            bail!("invalid upstream proxy host");
        }
        let authorization = credentials.map(|credentials| {
            let decoded = credentials
                .split_once(':')
                .map(|(user, password)| {
                    format!("{}:{}", percent_decode(user), percent_decode(password))
                })
                .unwrap_or_else(|| format!("{}:", percent_decode(credentials)));
            format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(decoded)
            )
        });
        Ok(Self {
            host,
            port,
            authorization,
        })
    }
}

impl ReverseTarget {
    fn parse(value: &str) -> Result<Self> {
        let (scheme, remainder) = value
            .split_once("://")
            .context("upstream must be an http(s) URL")?;
        if !matches!(scheme, "http" | "https") {
            bail!("upstream must be an http(s) URL");
        }
        let (authority, prefix) = remainder
            .split_once('/')
            .map(|(authority, path)| (authority, format!("/{path}")))
            .unwrap_or((remainder, String::new()));
        if authority.is_empty() || authority.contains('@') || authority.contains(['\r', '\n']) {
            bail!("invalid upstream authority");
        }
        let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
            let (host, suffix) = bracketed.split_once(']').context("invalid IPv6 upstream")?;
            let port = suffix
                .strip_prefix(':')
                .map(str::parse)
                .transpose()?
                .unwrap_or(if scheme == "https" { 443 } else { 80 });
            (host.to_owned(), port)
        } else if let Some((host, port)) = authority.rsplit_once(':') {
            (host.to_owned(), port.parse()?)
        } else {
            (
                authority.to_owned(),
                if scheme == "https" { 443 } else { 80 },
            )
        };
        if host.is_empty() || prefix.contains(['\r', '\n']) {
            bail!("invalid upstream URL");
        }
        Ok(Self {
            scheme: scheme.into(),
            host,
            port,
            prefix: prefix.trim_end_matches('/').into(),
        })
    }

    fn authority(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if (self.scheme == "https" && self.port == 443)
            || (self.scheme == "http" && self.port == 80)
        {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }
}

/// Run an HTTP/1.1 reverse proxy, including streaming/SSE and WebSocket upgrade.
pub fn serve_reverse(bind: SocketAddr, upstream: &str, handler: FlowHandler) -> Result<()> {
    serve_reverse_with_tls(bind, upstream, None, handler)
}

/// Run the reverse proxy with optional localhost TLS termination.
pub fn serve_reverse_with_tls(
    bind: SocketAddr,
    upstream: &str,
    tls: Option<Arc<ServerConfig>>,
    handler: FlowHandler,
) -> Result<()> {
    let target = Arc::new(ReverseTarget::parse(upstream)?);
    let listener =
        TcpListener::bind(bind).with_context(|| format!("bind reverse proxy at {bind}"))?;
    for client in listener.incoming() {
        let target = Arc::clone(&target);
        let handler = Arc::clone(&handler);
        let tls = tls.clone();
        match client {
            Ok(client) => {
                thread::spawn(move || {
                    let _ = client.set_read_timeout(Some(IO_TIMEOUT));
                    let _ = client.set_write_timeout(Some(IO_TIMEOUT));
                    let flow = if let Some(config) = tls {
                        match ServerConnection::new(config) {
                            Ok(connection) => {
                                handle_reverse(StreamOwned::new(connection, client), &target, true)
                            }
                            Err(error) => failed_reverse(&target, true, error.to_string()),
                        }
                    } else {
                        handle_reverse(client, &target, false)
                    };
                    handler(flow);
                });
            }
            Err(error) => eprintln!("proxy accept warning: {error}"),
        }
    }
    Ok(())
}

fn handle_reverse<S: ProxyIo>(
    mut client: S,
    target: &ReverseTarget,
    client_tls: bool,
) -> NetworkFlow {
    let wall = Utc::now();
    let started = Instant::now();
    let mut flow = base_flow(
        if client_tls {
            "tls_reverse_proxy"
        } else {
            "reverse_proxy"
        },
        "local_reverse_proxy",
        wall,
    );
    flow.destination_host = Some(target.host.clone());
    flow.destination_port = Some(target.port);
    flow.protocol = Some(format!("{}/http1.1", target.scheme));
    let result = forward_http(&mut client, target, started, &mut flow);
    if let Err(error) = result {
        flow.error_type = Some(error_kind(&error));
        let _ = client.write_all(
            b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
    }
    finish_flow(&mut flow, started);
    flow
}

fn failed_reverse(target: &ReverseTarget, tls: bool, error: String) -> NetworkFlow {
    let mut flow = base_flow(
        if tls {
            "tls_reverse_proxy"
        } else {
            "reverse_proxy"
        },
        "local_reverse_proxy",
        Utc::now(),
    );
    flow.destination_host = Some(target.host.clone());
    flow.destination_port = Some(target.port);
    flow.protocol = Some(format!("{}/http1.1", target.scheme));
    flow.error_type = Some(error.chars().take(128).collect());
    finish_flow(&mut flow, Instant::now());
    flow
}

fn forward_http<S: ProxyIo>(
    client: &mut S,
    target: &ReverseTarget,
    started: Instant,
    flow: &mut NetworkFlow,
) -> Result<()> {
    let (request_header, pending) = read_header(client, MAX_HEADER_BYTES)?;
    let request_text = std::str::from_utf8(&request_header).context("HTTP header is not UTF-8")?;
    let mut lines = request_text.split("\r\n");
    let first = lines.next().context("empty HTTP request")?;
    let mut fields = first.split_whitespace();
    let method = fields.next().context("HTTP method missing")?;
    let path = fields.next().context("HTTP path missing")?;
    if !path.starts_with('/') || path.contains(['\r', '\n']) {
        bail!("invalid HTTP request target");
    }
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect();
    let websocket = headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("upgrade") && value.eq_ignore_ascii_case("websocket")
    });
    let content_length = header_value(&headers, "content-length")
        .map(str::parse::<u64>)
        .transpose()
        .context("invalid Content-Length")?
        .unwrap_or(0);
    if content_length > MAX_REQUEST_BYTES {
        bail!("request body too large");
    }
    flow.request_bytes = content_length;

    let mut connection = connect_upstream(target)?;
    let upstream = &mut connection.stream;
    let destination_path = format!("{}{}", target.prefix, path);
    let request_target = if connection.absolute_form {
        format!(
            "{}://{}{}",
            target.scheme,
            target.authority(),
            destination_path
        )
    } else {
        destination_path
    };
    write!(upstream, "{method} {request_target} HTTP/1.1\r\n")?;
    write!(upstream, "Host: {}\r\n", target.authority())?;
    if let Some(authorization) = &connection.proxy_authorization {
        write!(upstream, "Proxy-Authorization: {authorization}\r\n")?;
    }
    for (name, value) in &headers {
        if name.eq_ignore_ascii_case("host")
            || name.eq_ignore_ascii_case("proxy-authorization")
            || (!websocket && is_hop_by_hop(name))
        {
            continue;
        }
        write!(upstream, "{name}: {value}\r\n")?;
    }
    if !websocket {
        upstream.write_all(b"Connection: close\r\n")?;
    }
    upstream.write_all(b"\r\n")?;
    copy_exact_with_prefix(client, upstream, pending, content_length)?;
    upstream.flush()?;

    let (response_header, response_pending) = read_header(upstream, MAX_HEADER_BYTES)?;
    flow.ttfb_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
    let response_text =
        std::str::from_utf8(&response_header).context("upstream header is not UTF-8")?;
    let mut response_lines = response_text.split("\r\n");
    let status_line = response_lines.next().context("empty upstream response")?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok());
    flow.http_status = status;
    if let Some(status) = status {
        debug_proxy_response(method, path, status);
    }
    let response_headers: Vec<(String, String)> = response_lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect();
    write!(client, "{status_line}\r\n")?;
    for (name, value) in &response_headers {
        if [
            "connection",
            "keep-alive",
            "proxy-authenticate",
            "proxy-authorization",
        ]
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
        {
            continue;
        }
        write!(client, "{name}: {value}\r\n")?;
    }
    if websocket {
        client.write_all(b"Connection: Upgrade\r\n\r\n")?;
        client.write_all(&response_pending)?;
        flow.response_bytes = response_pending.len() as u64;
        flow.mode = "websocket_reverse_proxy".into();
        flow.protocol = Some(
            if target.scheme == "https" {
                "wss"
            } else {
                "ws"
            }
            .into(),
        );
        if status == Some(101) {
            client.set_relay_timeout(Duration::from_millis(100))?;
            upstream.set_relay_timeout(Duration::from_millis(100))?;
            let (out, incoming) = relay_duplex(client, upstream)?;
            flow.request_bytes += out;
            flow.response_bytes += incoming;
        }
        flow.success = Some(status == Some(101));
        return Ok(());
    }
    client.write_all(b"Connection: close\r\n\r\n")?;
    let mut scanner = SseTimingScanner::new(started);
    let chunked = header_value(&response_headers, "transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"));
    let response_length = header_value(&response_headers, "content-length")
        .and_then(|value| value.parse::<u64>().ok());
    flow.response_bytes = if chunked {
        copy_chunked(upstream, client, response_pending, &mut scanner)?
    } else if let Some(length) = response_length {
        copy_response_exact(upstream, client, response_pending, length, &mut scanner)?
    } else {
        copy_response_to_end(upstream, client, response_pending, &mut scanner)?
    };
    flow.first_event_ms = scanner.first_event_ms;
    flow.first_output_ms = scanner.first_output_ms;
    flow.success = status
        .map(|value| (200..400).contains(&value))
        .or(Some(false));
    Ok(())
}

trait ProxyIo: Read + Write {
    fn set_relay_timeout(&self, timeout: Duration) -> std::io::Result<()>;
}

impl ProxyIo for TcpStream {
    fn set_relay_timeout(&self, timeout: Duration) -> std::io::Result<()> {
        self.set_read_timeout(Some(timeout))
    }
}

impl ProxyIo for StreamOwned<ServerConnection, TcpStream> {
    fn set_relay_timeout(&self, timeout: Duration) -> std::io::Result<()> {
        self.sock.set_read_timeout(Some(timeout))
    }
}

enum UpstreamStream {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

struct UpstreamConnection {
    stream: UpstreamStream,
    absolute_form: bool,
    proxy_authorization: Option<String>,
}

impl Read for UpstreamStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buffer),
            Self::Tls(stream) => stream.read(buffer),
        }
    }
}

impl Write for UpstreamStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buffer),
            Self::Tls(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

impl ProxyIo for UpstreamStream {
    fn set_relay_timeout(&self, timeout: Duration) -> std::io::Result<()> {
        match self {
            Self::Plain(stream) => stream.set_read_timeout(Some(timeout)),
            Self::Tls(stream) => stream.sock.set_read_timeout(Some(timeout)),
        }
    }
}

fn connect_upstream(target: &ReverseTarget) -> Result<UpstreamConnection> {
    let proxy = configured_proxy(target)?;
    let (connect_host, connect_port) = proxy
        .as_ref()
        .map(|proxy| (proxy.host.as_str(), proxy.port))
        .unwrap_or((target.host.as_str(), target.port));
    let address = (connect_host, connect_port)
        .to_socket_addrs()?
        .next()
        .context("no upstream addresses")?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(30))?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    if target.scheme == "http" {
        return Ok(UpstreamConnection {
            stream: UpstreamStream::Plain(stream),
            absolute_form: proxy.is_some(),
            proxy_authorization: proxy.and_then(|proxy| proxy.authorization),
        });
    }
    if let Some(proxy) = proxy {
        write!(
            stream,
            "CONNECT {} HTTP/1.1\r\nHost: {}\r\n",
            target.authority(),
            target.authority()
        )?;
        if let Some(authorization) = proxy.authorization {
            write!(stream, "Proxy-Authorization: {authorization}\r\n")?;
        }
        stream.write_all(b"Connection: keep-alive\r\n\r\n")?;
        stream.flush()?;
        let (header, pending) = read_header(&mut stream, MAX_HEADER_BYTES)?;
        if !pending.is_empty() {
            bail!("upstream proxy sent unexpected CONNECT payload");
        }
        let status = String::from_utf8_lossy(&header)
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse::<u16>().ok());
        if status != Some(200) {
            bail!("upstream proxy CONNECT failed");
        }
    }
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let name = ServerName::try_from(target.host.clone()).context("invalid TLS server name")?;
    let connection = ClientConnection::new(Arc::new(config), name)?;
    Ok(UpstreamConnection {
        stream: UpstreamStream::Tls(Box::new(StreamOwned::new(connection, stream))),
        absolute_form: false,
        proxy_authorization: None,
    })
}

fn configured_proxy(target: &ReverseTarget) -> Result<Option<ProxyTarget>> {
    if proxy_bypass(&target.host) {
        return Ok(None);
    }
    let keys: &[&str] = if target.scheme == "https" {
        &["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"]
    } else if std::env::var_os("REQUEST_METHOD").is_some() {
        &["http_proxy", "all_proxy", "ALL_PROXY"]
    } else {
        &["http_proxy", "HTTP_PROXY", "all_proxy", "ALL_PROXY"]
    };
    let value = keys.iter().find_map(|key| {
        std::env::var(key)
            .ok()
            .filter(|value| !value.trim().is_empty())
    });
    if let Some(value) = value {
        return ProxyTarget::parse(value.trim()).map(Some);
    }
    if proxy_environment_present() {
        return Ok(None);
    }
    macos_system_proxy(target)
}

fn proxy_environment_present() -> bool {
    [
        "http_proxy",
        "HTTP_PROXY",
        "https_proxy",
        "HTTPS_PROXY",
        "all_proxy",
        "ALL_PROXY",
        "no_proxy",
        "NO_PROXY",
    ]
    .iter()
    .any(|key| {
        if *key == "HTTP_PROXY" && std::env::var_os("REQUEST_METHOD").is_some() {
            return false;
        }
        std::env::var(key)
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
struct MacOsProxySettings {
    http_enabled: bool,
    http_host: Option<String>,
    http_port: Option<u16>,
    https_enabled: bool,
    https_host: Option<String>,
    https_port: Option<u16>,
    exclude_simple: bool,
    exceptions: Vec<String>,
}

fn parse_scutil_proxy_settings(output: &str) -> MacOsProxySettings {
    let mut settings = MacOsProxySettings::default();
    let mut in_exceptions = false;
    for raw_line in output.lines() {
        let line = raw_line.trim();
        if in_exceptions {
            if line == "}" {
                in_exceptions = false;
                continue;
            }
            if let Some((index, value)) = line.split_once(':') {
                if index
                    .trim()
                    .chars()
                    .all(|character| character.is_ascii_digit())
                {
                    let value = value.trim();
                    if !value.is_empty() {
                        settings.exceptions.push(value.to_owned());
                    }
                }
            }
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "HTTPEnable" => settings.http_enabled = value == "1",
            "HTTPProxy" => settings.http_host = nonempty(value),
            "HTTPPort" => settings.http_port = value.parse().ok(),
            "HTTPSEnable" => settings.https_enabled = value == "1",
            "HTTPSProxy" => settings.https_host = nonempty(value),
            "HTTPSPort" => settings.https_port = value.parse().ok(),
            "ExcludeSimpleHostnames" => settings.exclude_simple = value == "1",
            "ExceptionsList" if value.starts_with("<array>") => in_exceptions = true,
            _ => {}
        }
    }
    settings
}

fn nonempty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

impl MacOsProxySettings {
    fn proxy_for(&self, target: &ReverseTarget) -> Result<Option<ProxyTarget>> {
        if self.bypasses(&target.host) {
            return Ok(None);
        }
        let (enabled, host, port) = if target.scheme == "https" {
            (
                self.https_enabled,
                self.https_host.as_deref(),
                self.https_port,
            )
        } else {
            (self.http_enabled, self.http_host.as_deref(), self.http_port)
        };
        if !enabled {
            return Ok(None);
        }
        let Some(host) = host else {
            return Ok(None);
        };
        if host.is_empty()
            || host.contains(['\r', '\n', '\0'])
            || host.chars().any(char::is_whitespace)
        {
            bail!("invalid macOS system proxy host");
        }
        Ok(Some(ProxyTarget {
            host: host.to_owned(),
            port: port.unwrap_or(80),
            authorization: None,
        }))
    }

    fn bypasses(&self, host: &str) -> bool {
        let host = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if self.exclude_simple && !host.contains(['.', ':']) {
            return true;
        }
        self.exceptions
            .iter()
            .any(|pattern| macos_exception_matches(&host, pattern))
    }
}

fn macos_exception_matches(host: &str, pattern: &str) -> bool {
    let pattern = pattern.trim().trim_end_matches('.').to_ascii_lowercase();
    if pattern.is_empty() {
        return false;
    }
    if let (Ok(host), Some((network, prefix))) = (
        host.parse::<std::net::Ipv4Addr>(),
        parse_ipv4_network(&pattern),
    ) {
        let mask = if prefix == 0 {
            0
        } else {
            u32::MAX << (32 - prefix)
        };
        return u32::from(host) & mask == network & mask;
    }
    wildcard_match(host.as_bytes(), pattern.as_bytes())
}

fn parse_ipv4_network(value: &str) -> Option<(u32, u32)> {
    let (address, explicit_prefix) = value
        .split_once('/')
        .map(|(address, prefix)| (address, Some(prefix)))
        .unwrap_or((value, None));
    let components: Vec<_> = address.split('.').collect();
    if components.is_empty() || components.len() > 4 {
        return None;
    }
    let prefix = explicit_prefix
        .map(str::parse::<u32>)
        .transpose()
        .ok()?
        .unwrap_or((components.len() * 8) as u32);
    if prefix > 32 {
        return None;
    }
    let mut octets = [0u8; 4];
    for (index, component) in components.iter().enumerate() {
        octets[index] = component.parse().ok()?;
    }
    Some((u32::from_be_bytes(octets), prefix))
}

fn wildcard_match(value: &[u8], pattern: &[u8]) -> bool {
    let (mut value_index, mut pattern_index) = (0, 0);
    let (mut star, mut retry) = (None, 0);
    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == b'?' || pattern[pattern_index] == value[value_index])
        {
            value_index += 1;
            pattern_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star = Some(pattern_index);
            pattern_index += 1;
            retry = value_index;
        } else if let Some(star_index) = star {
            retry += 1;
            value_index = retry;
            pattern_index = star_index + 1;
        } else {
            return false;
        }
    }
    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

#[cfg(target_os = "macos")]
fn macos_system_proxy(target: &ReverseTarget) -> Result<Option<ProxyTarget>> {
    let output = std::process::Command::new("scutil")
        .arg("--proxy")
        .output()
        .context("could not read macOS system proxy settings")?;
    if !output.status.success() {
        return Ok(None);
    }
    let settings = parse_scutil_proxy_settings(&String::from_utf8_lossy(&output.stdout));
    settings.proxy_for(target)
}

#[cfg(not(target_os = "macos"))]
fn macos_system_proxy(_target: &ReverseTarget) -> Result<Option<ProxyTarget>> {
    Ok(None)
}

fn debug_proxy_response(method: &str, request_target: &str, status: u16) {
    if std::env::var("CODEX_METER_DEBUG_PROXY").as_deref() == Ok("1") {
        eprintln!(
            "{}",
            safe_proxy_debug_summary(method, request_target, status)
        );
    }
}

fn safe_proxy_debug_summary(method: &str, request_target: &str, status: u16) -> String {
    let method =
        if !method.is_empty() && method.len() <= 32 && method.bytes().all(is_http_token_byte) {
            method
        } else {
            "UNKNOWN"
        };
    let path = request_target.split('?').next().unwrap_or("/");
    let path: String = path
        .bytes()
        .take(256)
        .map(|byte| {
            if byte.is_ascii_graphic() {
                char::from(byte)
            } else {
                '_'
            }
        })
        .collect();
    format!("{method} {path} {status}")
}

fn is_http_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

#[allow(clippy::collapsible_if)] // let-chains would raise the MSRV above Rust 1.85.
fn proxy_bypass(host: &str) -> bool {
    let value = std::env::var("no_proxy")
        .or_else(|_| std::env::var("NO_PROXY"))
        .unwrap_or_default();
    value.split(',').any(|entry| {
        let mut pattern = entry.trim();
        if pattern == "*" {
            return true;
        }
        if pattern.starts_with('[') {
            pattern = pattern
                .strip_prefix('[')
                .and_then(|value| value.split_once(']').map(|(host, _)| host))
                .unwrap_or(pattern);
        } else if pattern.matches(':').count() == 1 {
            if let Some((without_port, port)) = pattern.rsplit_once(':') {
                if port.chars().all(|character| character.is_ascii_digit()) {
                    pattern = without_port;
                }
            }
        }
        let pattern = pattern.trim_start_matches('.');
        !pattern.is_empty()
            && (host.eq_ignore_ascii_case(pattern)
                || host
                    .to_ascii_lowercase()
                    .ends_with(&format!(".{}", pattern.to_ascii_lowercase())))
    })
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                output.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn read_header<R: Read>(stream: &mut R, limit: usize) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            bail!("incomplete HTTP header");
        }
        data.extend_from_slice(&buffer[..count]);
        if let Some(index) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            return Ok((data[..index + 4].to_vec(), data[index + 4..].to_vec()));
        }
        if data.len() > limit {
            bail!("HTTP header too large");
        }
    }
}

fn copy_exact_with_prefix<R: Read, W: Write>(
    source: &mut R,
    target: &mut W,
    prefix: Vec<u8>,
    length: u64,
) -> Result<()> {
    if prefix.len() as u64 > length {
        bail!("pipelined HTTP requests are not supported");
    }
    target.write_all(&prefix)?;
    let mut remaining = length - prefix.len() as u64;
    let copied = std::io::copy(&mut source.take(remaining), target)?;
    remaining -= copied;
    if remaining != 0 {
        bail!("truncated request body");
    }
    Ok(())
}

fn copy_response_exact<R: Read, W: Write>(
    upstream: &mut R,
    client: &mut W,
    prefix: Vec<u8>,
    length: u64,
    scanner: &mut SseTimingScanner,
) -> Result<u64> {
    if prefix.len() as u64 > length {
        bail!("upstream sent more bytes than Content-Length");
    }
    client.write_all(&prefix)?;
    scanner.feed(&prefix);
    let mut total = prefix.len() as u64;
    let mut buffer = [0u8; 16_384];
    while total < length {
        let wanted = (length - total).min(buffer.len() as u64) as usize;
        let count = upstream.read(&mut buffer[..wanted])?;
        if count == 0 {
            bail!("truncated upstream response");
        }
        client.write_all(&buffer[..count])?;
        client.flush()?;
        scanner.feed(&buffer[..count]);
        total += count as u64;
    }
    Ok(total)
}

fn copy_response_to_end<R: Read, W: Write>(
    upstream: &mut R,
    client: &mut W,
    prefix: Vec<u8>,
    scanner: &mut SseTimingScanner,
) -> Result<u64> {
    client.write_all(&prefix)?;
    scanner.feed(&prefix);
    let mut total = prefix.len() as u64;
    let mut buffer = [0u8; 16_384];
    loop {
        let count = upstream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        client.write_all(&buffer[..count])?;
        client.flush()?;
        scanner.feed(&buffer[..count]);
        total += count as u64;
    }
    Ok(total)
}

fn copy_chunked<R: Read, W: Write>(
    upstream: &mut R,
    client: &mut W,
    prefix: Vec<u8>,
    scanner: &mut SseTimingScanner,
) -> Result<u64> {
    let mut source = BufReader::new(Cursor::new(prefix).chain(upstream));
    let mut payload_bytes = 0u64;
    loop {
        let mut size_line = String::new();
        if source.read_line(&mut size_line)? == 0 || size_line.len() > 1024 {
            bail!("invalid chunked upstream response");
        }
        client.write_all(size_line.as_bytes())?;
        let size = u64::from_str_radix(size_line.trim().split(';').next().unwrap_or(""), 16)
            .context("invalid chunk size")?;
        if size == 0 {
            loop {
                let mut trailer = String::new();
                source.read_line(&mut trailer)?;
                client.write_all(trailer.as_bytes())?;
                if trailer == "\r\n" || trailer.is_empty() {
                    break;
                }
                if trailer.len() > MAX_HEADER_BYTES {
                    bail!("chunk trailer too large");
                }
            }
            break;
        }
        let mut remaining = size;
        let mut buffer = [0u8; 16_384];
        while remaining > 0 {
            let wanted = remaining.min(buffer.len() as u64) as usize;
            source.read_exact(&mut buffer[..wanted])?;
            client.write_all(&buffer[..wanted])?;
            scanner.feed(&buffer[..wanted]);
            payload_bytes += wanted as u64;
            remaining -= wanted as u64;
        }
        let mut ending = [0u8; 2];
        source.read_exact(&mut ending)?;
        if ending != *b"\r\n" {
            bail!("invalid chunk ending");
        }
        client.write_all(&ending)?;
        client.flush()?;
    }
    Ok(payload_bytes)
}

fn relay_duplex<A: Read + Write, B: Read + Write>(
    left: &mut A,
    right: &mut B,
) -> Result<(u64, u64)> {
    // WebSocket traffic is opaque; short read timeouts on the underlying sockets
    // allow this single-threaded loop to serve both directions without inspecting frames.
    let mut outgoing = 0;
    let mut incoming = 0;
    let mut left_open = true;
    let mut right_open = true;
    let mut buffer = [0u8; 65_536];
    let mut last_activity = Instant::now();
    while left_open || right_open {
        if left_open {
            match left.read(&mut buffer) {
                Ok(0) => left_open = false,
                Ok(count) => {
                    right.write_all(&buffer[..count])?;
                    outgoing += count as u64;
                    last_activity = Instant::now();
                }
                Err(error) if is_retryable(&error) => {}
                Err(error) => return Err(error.into()),
            }
        }
        if right_open {
            match right.read(&mut buffer) {
                Ok(0) => right_open = false,
                Ok(count) => {
                    left.write_all(&buffer[..count])?;
                    incoming += count as u64;
                    last_activity = Instant::now();
                }
                Err(error) if is_retryable(&error) => {}
                Err(error) => return Err(error.into()),
            }
        }
        if last_activity.elapsed() >= IO_TIMEOUT {
            break;
        }
    }
    Ok((outgoing, incoming))
}

fn is_retryable(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::Interrupted
    )
}

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn is_hop_by_hop(name: &str) -> bool {
    [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailers",
        "transfer-encoding",
        "upgrade",
        "host",
    ]
    .iter()
    .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

#[derive(Debug, Default)]
struct SseTimingScanner {
    started: Option<Instant>,
    buffer: Vec<u8>,
    first_event_ms: Option<f64>,
    first_output_ms: Option<f64>,
}

impl SseTimingScanner {
    fn new(started: Instant) -> Self {
        Self {
            started: Some(started),
            ..Self::default()
        }
    }

    fn feed(&mut self, chunk: &[u8]) {
        if self.first_output_ms.is_some() {
            return;
        }
        self.buffer.extend_from_slice(chunk);
        if self.buffer.len() > MAX_HEADER_BYTES {
            let discard = self.buffer.len() - MAX_HEADER_BYTES;
            self.buffer.drain(..discard);
        }
        while let Some(index) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=index).collect();
            let line = line.strip_suffix(b"\n").unwrap_or(&line);
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let Some(event) = line.strip_prefix(b"event:") else {
                continue;
            };
            let elapsed = self
                .started
                .map(|started| started.elapsed().as_secs_f64() * 1000.0)
                .unwrap_or_default();
            self.first_event_ms.get_or_insert(elapsed);
            if matches!(
                trim_ascii(event),
                b"response.output_text.delta"
                    | b"response.content_part.added"
                    | b"response.output_item.added"
                    | b"response.completed"
            ) {
                self.first_output_ms = Some(elapsed);
            }
        }
    }
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

fn parse_connect_target(value: &str) -> Result<(String, u16)> {
    if value.len() > 1024 || value.contains(['\r', '\n', '\0']) {
        bail!("invalid CONNECT target");
    }
    if let Some(value) = value.strip_prefix('[') {
        let (host, port) = value
            .split_once("]:")
            .context("invalid IPv6 CONNECT target")?;
        if host.is_empty() {
            bail!("empty CONNECT host");
        }
        return Ok((host.into(), port.parse()?));
    }
    if let Some((host, port)) = value.rsplit_once(':') {
        if host.is_empty() {
            bail!("empty CONNECT host");
        }
        return Ok((host.into(), port.parse()?));
    }
    if value.is_empty() {
        bail!("empty CONNECT host");
    }
    Ok((value.into(), 443))
}

fn base_flow(mode: &str, source: &str, started_at: chrono::DateTime<Utc>) -> NetworkFlow {
    NetworkFlow {
        mode: mode.into(),
        data_source: source.into(),
        started_at: Some(started_at.to_rfc3339_opts(SecondsFormat::Millis, true)),
        success: Some(false),
        ..NetworkFlow::default()
    }
}

fn finish_flow(flow: &mut NetworkFlow, started: Instant) {
    flow.ended_at = Some(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
    flow.duration_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
    flow.event_fingerprint.clear();
    flow.event_fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(flow).unwrap_or_default())
    );
}

fn error_kind(error: &anyhow::Error) -> String {
    error
        .chain()
        .last()
        .map(|source| source.to_string())
        .unwrap_or_else(|| "proxy error".into())
        .chars()
        .take(128)
        .collect()
}

#[derive(Debug, Clone)]
pub struct TlsMaterial {
    pub ca_cert: PathBuf,
    pub ca_key: PathBuf,
    pub leaf_cert: PathBuf,
    pub leaf_key: PathBuf,
}

/// Create a short-lived local CA and localhost leaf certificate without overwriting keys.
pub fn initialize_tls_material(directory: &Path) -> Result<TlsMaterial> {
    fs::create_dir_all(directory)?;
    let material = TlsMaterial {
        ca_cert: directory.join("codex-meter-ca.pem"),
        ca_key: directory.join("codex-meter-ca-key.pem"),
        leaf_cert: directory.join("localhost.pem"),
        leaf_key: directory.join("localhost-key.pem"),
    };
    let paths = [
        &material.ca_cert,
        &material.ca_key,
        &material.leaf_cert,
        &material.leaf_key,
    ];
    if paths.iter().all(|path| path.exists()) {
        ensure_certificate_chain(&material.leaf_cert, &material.ca_cert)?;
        return Ok(material);
    }
    if paths.iter().any(|path| path.exists()) {
        bail!("partial TLS material exists; move it aside before regenerating");
    }
    let temp = tempfile::tempdir_in(directory)?;
    let ca_key = temp.path().join("ca-key.pem");
    let ca_cert = temp.path().join("ca.pem");
    let leaf_key = temp.path().join("leaf-key.pem");
    let leaf_csr = temp.path().join("leaf.csr");
    let leaf_cert = temp.path().join("leaf.pem");
    run_openssl(&[
        "req",
        "-x509",
        "-newkey",
        "rsa:3072",
        "-nodes",
        "-days",
        "30",
        "-sha256",
        "-subj",
        "/CN=Codex Meter Local Diagnostic CA",
        "-addext",
        "basicConstraints=critical,CA:TRUE",
        "-addext",
        "keyUsage=critical,keyCertSign,cRLSign",
        "-keyout",
        path(&ca_key),
        "-out",
        path(&ca_cert),
    ])?;
    run_openssl(&[
        "req",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-sha256",
        "-subj",
        "/CN=localhost",
        "-keyout",
        path(&leaf_key),
        "-out",
        path(&leaf_csr),
    ])?;
    let extension = temp.path().join("localhost.ext");
    fs::write(
        &extension,
        "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost,IP:127.0.0.1,IP:::1\n",
    )?;
    run_openssl(&[
        "x509",
        "-req",
        "-in",
        path(&leaf_csr),
        "-CA",
        path(&ca_cert),
        "-CAkey",
        path(&ca_key),
        "-CAcreateserial",
        "-out",
        path(&leaf_cert),
        "-days",
        "30",
        "-sha256",
        "-extfile",
        path(&extension),
    ])?;
    let mut chain = fs::read(&leaf_cert)?;
    chain.extend_from_slice(b"\n");
    chain.extend_from_slice(&fs::read(&ca_cert)?);
    fs::write(&leaf_cert, chain)?;
    for (source, target) in [
        (&ca_cert, &material.ca_cert),
        (&ca_key, &material.ca_key),
        (&leaf_cert, &material.leaf_cert),
        (&leaf_key, &material.leaf_key),
    ] {
        fs::rename(source, target)?;
    }
    set_private_permissions(&material.ca_key);
    set_private_permissions(&material.leaf_key);
    Ok(material)
}

fn ensure_certificate_chain(leaf_cert: &Path, ca_cert: &Path) -> Result<()> {
    let leaf = fs::read(leaf_cert)?;
    let ca = fs::read(ca_cert)?;
    if !leaf.windows(ca.len()).any(|window| window == ca.as_slice()) {
        let mut chain = leaf;
        if !chain.ends_with(b"\n") {
            chain.push(b'\n');
        }
        chain.extend_from_slice(&ca);
        fs::write(leaf_cert, chain)?;
    }
    Ok(())
}

pub fn load_tls_server_config(material: &TlsMaterial) -> Result<Arc<ServerConfig>> {
    let mut certificates = BufReader::new(File::open(&material.leaf_cert)?);
    let certificates: Vec<_> =
        rustls_pemfile::certs(&mut certificates).collect::<std::io::Result<_>>()?;
    let key = rustls_pemfile::private_key(&mut BufReader::new(File::open(&material.leaf_key)?))?
        .context("TLS private key not found")?;
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certificates, key)?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) {}

fn path(value: &Path) -> &str {
    value.to_str().unwrap_or("")
}

fn run_openssl(arguments: &[&str]) -> Result<()> {
    let output = std::process::Command::new("openssl")
        .args(arguments)
        .output()
        .context("openssl not found")?;
    if !output.status.success() {
        bail!(
            "openssl: {}",
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .last()
                .unwrap_or("failed")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MemoryClient {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Read for MemoryClient {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buffer)
        }
    }

    impl Write for MemoryClient {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.output.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl ProxyIo for MemoryClient {
        fn set_relay_timeout(&self, _timeout: Duration) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn parses_connect_targets() {
        assert_eq!(
            parse_connect_target("api.openai.com:443").unwrap(),
            ("api.openai.com".into(), 443)
        );
        assert_eq!(
            parse_connect_target("[2001:db8::1]:8443").unwrap(),
            ("2001:db8::1".into(), 8443)
        );
        assert!(parse_connect_target("bad\r\nHost: secret").is_err());
    }

    #[test]
    fn parses_reverse_target_without_credentials() {
        let target = ReverseTarget::parse("https://api.openai.com/v1/").unwrap();
        assert_eq!(target.host, "api.openai.com");
        assert_eq!(target.port, 443);
        assert_eq!(target.prefix, "/v1");
        assert!(ReverseTarget::parse("https://token@api.openai.com").is_err());
    }

    #[test]
    fn parses_upstream_proxy_credentials_without_exposing_them_in_errors() {
        let proxy = ProxyTarget::parse("http://user:p%40ss@proxy.example:8080").unwrap();
        assert_eq!(proxy.host, "proxy.example");
        assert_eq!(proxy.port, 8080);
        assert_eq!(proxy.authorization.as_deref(), Some("Basic dXNlcjpwQHNz"));
        let error = match ProxyTarget::parse("https://user:SECRET@proxy.example") {
            Ok(_) => panic!("HTTPS proxy should be rejected"),
            Err(error) => error.to_string(),
        };
        assert!(!error.contains("SECRET"));
    }

    #[test]
    fn proxy_debug_summary_contains_only_safe_method_path_and_status() {
        let summary = safe_proxy_debug_summary(
            "POST",
            "/v1/responses?api_key=QUERY_SECRET&prompt=PRIVATE",
            201,
        );
        assert_eq!(summary, "POST /v1/responses 201");
        assert!(!summary.contains("QUERY_SECRET"));
        assert!(!summary.contains("PRIVATE"));
        assert_eq!(
            safe_proxy_debug_summary("GET\u{1b}INJECT", "/safe?secret", 200),
            "UNKNOWN /safe 200"
        );
    }

    #[test]
    fn parses_and_selects_macos_system_proxies() {
        let settings = parse_scutil_proxy_settings(
            r#"<dictionary> {
  ExceptionsList : <array> {
    0 : *.local
    1 : 169.254/16
    2 : 10.1
  }
  ExcludeSimpleHostnames : 1
  HTTPEnable : 1
  HTTPPort : 8080
  HTTPProxy : http-proxy.example
  HTTPSEnable : 1
  HTTPSPort : 8443
  HTTPSProxy : secure-proxy.example
}"#,
        );
        assert!(settings.http_enabled);
        assert!(settings.https_enabled);
        assert!(settings.exclude_simple);
        assert_eq!(settings.exceptions.len(), 3);

        let http = ReverseTarget::parse("http://api.openai.com").unwrap();
        let proxy = settings.proxy_for(&http).unwrap().unwrap();
        assert_eq!(proxy.host, "http-proxy.example");
        assert_eq!(proxy.port, 8080);

        let https = ReverseTarget::parse("https://api.openai.com").unwrap();
        let proxy = settings.proxy_for(&https).unwrap().unwrap();
        assert_eq!(proxy.host, "secure-proxy.example");
        assert_eq!(proxy.port, 8443);

        for upstream in [
            "https://printer.local",
            "https://intranet",
            "https://169.254.2.3",
            "https://10.1.9.8",
        ] {
            assert!(
                settings
                    .proxy_for(&ReverseTarget::parse(upstream).unwrap())
                    .unwrap()
                    .is_none(),
                "{upstream} should bypass the proxy"
            );
        }
        assert!(!settings.bypasses("10.2.9.8"));
    }

    #[test]
    fn scanner_keeps_only_timing_state() {
        let mut scanner = SseTimingScanner::new(Instant::now());
        scanner.feed(b"data: SECRET PROMPT\nevent: response.output_text.delta\n");
        assert!(scanner.first_event_ms.is_some());
        assert!(scanner.first_output_ms.is_some());
        assert!(!format!("{scanner:?}").contains("SECRET"));
    }

    #[test]
    fn header_reader_separates_early_body_bytes() {
        let mut input = Cursor::new(b"POST / HTTP/1.1\r\nContent-Length: 4\r\n\r\nBODY".to_vec());
        let (header, body) = read_header(&mut input, MAX_HEADER_BYTES).unwrap();
        assert!(header.ends_with(b"\r\n\r\n"));
        assert_eq!(body, b"BODY");
    }

    #[test]
    fn reverse_proxy_forwards_content_but_flow_retains_only_metadata() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let upstream = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (header, pending) = read_header(&mut stream, MAX_HEADER_BYTES).unwrap();
            assert!(String::from_utf8_lossy(&header).starts_with("POST /v1/responses HTTP/1.1"));
            let mut body = pending;
            while body.len() < 6 {
                let mut buffer = [0u8; 6];
                let count = stream.read(&mut buffer).unwrap();
                body.extend_from_slice(&buffer[..count]);
            }
            assert_eq!(body, b"SECRET");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK")
                .unwrap();
        });
        let target = ReverseTarget::parse(&format!("http://{address}/v1")).unwrap();
        let mut client = MemoryClient {
            input: Cursor::new(
                b"POST /responses HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer PRIVATE\r\nContent-Length: 6\r\n\r\nSECRET".to_vec(),
            ),
            output: Vec::new(),
        };
        let mut flow = base_flow("reverse_proxy", "local_reverse_proxy", Utc::now());
        forward_http(&mut client, &target, Instant::now(), &mut flow).unwrap();
        upstream.join().unwrap();
        assert!(client.output.ends_with(b"OK"));
        let debug = format!("{flow:?}");
        assert_eq!(flow.request_bytes, 6);
        assert_eq!(flow.response_bytes, 2);
        assert!(!debug.contains("SECRET"));
        assert!(!debug.contains("PRIVATE"));
    }

    #[test]
    fn tls_material_is_complete_reusable_and_loadable_when_openssl_exists() {
        if std::process::Command::new("openssl")
            .arg("version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let material = initialize_tls_material(directory.path()).unwrap();
        assert!(material.ca_cert.is_file());
        assert!(material.ca_key.is_file());
        assert!(material.leaf_cert.is_file());
        assert!(material.leaf_key.is_file());
        load_tls_server_config(&material).unwrap();
        let reused = initialize_tls_material(directory.path()).unwrap();
        assert_eq!(reused.ca_cert, material.ca_cert);
    }
}
