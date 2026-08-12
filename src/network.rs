//! Content-free network timing and packet metadata.

use crate::models::{NetworkFlowRecord, Quality};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, SecondsFormat, Utc};
use regex::Regex;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Write;
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct NetworkFlow {
    pub event_fingerprint: String,
    pub mode: String,
    pub data_source: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub destination_host: Option<String>,
    pub destination_ip: Option<String>,
    pub destination_port: Option<u16>,
    pub protocol: Option<String>,
    pub tls_version: Option<String>,
    pub alpn: Option<String>,
    pub http_status: Option<u16>,
    pub request_bytes: u64,
    pub response_bytes: u64,
    pub packets_out: u64,
    pub packets_in: u64,
    pub dns_ms: Option<f64>,
    pub tcp_ms: Option<f64>,
    pub tls_ms: Option<f64>,
    pub ttfb_ms: Option<f64>,
    pub first_event_ms: Option<f64>,
    pub first_output_ms: Option<f64>,
    pub duration_ms: Option<f64>,
    pub success: Option<bool>,
    pub error_type: Option<String>,
}

impl From<NetworkFlow> for NetworkFlowRecord {
    fn from(flow: NetworkFlow) -> Self {
        Self {
            event_fingerprint: flow.event_fingerprint,
            mode: flow.mode,
            data_source: flow.data_source.clone(),
            started_at: flow.started_at,
            ended_at: flow.ended_at,
            destination_host: flow.destination_host,
            destination_ip: flow.destination_ip,
            destination_port: flow.destination_port.map(i64::from),
            protocol: flow.protocol,
            tls_version: flow.tls_version,
            alpn: flow.alpn,
            http_status: flow.http_status.map(i64::from),
            request_bytes: bounded_i64(flow.request_bytes),
            response_bytes: bounded_i64(flow.response_bytes),
            packets_out: bounded_i64(flow.packets_out),
            packets_in: bounded_i64(flow.packets_in),
            dns_ms: flow.dns_ms,
            tcp_ms: flow.tcp_ms,
            tls_ms: flow.tls_ms,
            ttfb_ms: flow.ttfb_ms,
            first_event_ms: flow.first_event_ms,
            first_output_ms: flow.first_output_ms,
            duration_ms: flow.duration_ms,
            success: flow.success,
            error_type: flow.error_type,
            thread_id: None,
            turn_id: None,
            response_id: None,
            quality: Quality::exact(flow.data_source),
        }
    }
}

fn bounded_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

pub fn probe_endpoint(host: &str, port: u16, timeout: Duration) -> NetworkFlow {
    let wall_start = Utc::now();
    let started = Instant::now();
    let mut flow = NetworkFlow {
        mode: "probe".into(),
        data_source: "socket_probe".into(),
        started_at: Some(iso(wall_start)),
        destination_host: Some(host.into()),
        destination_port: Some(port),
        protocol: Some("tls".into()),
        success: Some(false),
        ..NetworkFlow::default()
    };
    let result = (|| -> Result<()> {
        let mark = Instant::now();
        let addresses: Vec<_> = (host, port).to_socket_addrs()?.collect();
        flow.dns_ms = Some(mark.elapsed().as_secs_f64() * 1000.0);
        let address = *addresses.first().context("no addresses")?;
        flow.destination_ip = Some(address.ip().to_string());
        let mark = Instant::now();
        let stream = TcpStream::connect_timeout(&address, timeout)?;
        flow.tcp_ms = Some(mark.elapsed().as_secs_f64() * 1000.0);
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;

        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let name = ServerName::try_from(host.to_owned()).context("invalid TLS server name")?;
        let connection = ClientConnection::new(Arc::new(config), name)?;
        let mark = Instant::now();
        let mut tls = StreamOwned::new(connection, stream);
        tls.flush()?;
        while tls.conn.is_handshaking() {
            tls.conn.complete_io(&mut tls.sock)?;
        }
        flow.tls_ms = Some(mark.elapsed().as_secs_f64() * 1000.0);
        flow.tls_version = tls
            .conn
            .protocol_version()
            .map(|version| format!("{version:?}"));
        flow.alpn = tls
            .conn
            .alpn_protocol()
            .map(|value| String::from_utf8_lossy(value).into_owned());
        flow.success = Some(true);
        Ok(())
    })();
    if let Err(error) = result {
        flow.error_type = Some(error.to_string().chars().take(256).collect());
    }
    flow.ended_at = Some(iso(Utc::now()));
    flow.duration_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
    fingerprint(&mut flow);
    flow
}

pub fn capture_metadata(
    hosts: &[String],
    interface: Option<&str>,
    port: u16,
    duration: Duration,
    packet_limit: usize,
) -> Result<Vec<NetworkFlow>> {
    let tcpdump = find_command("tcpdump").context("tcpdump not found")?;
    let interface = match interface {
        Some(interface) => interface.to_owned(),
        None => default_capture_interface(&tcpdump)
            .context("tcpdump did not report a usable capture interface")?,
    };
    let mut resolved = HashMap::new();
    for host in hosts {
        if let Ok(addresses) = (host.as_str(), port).to_socket_addrs() {
            for address in addresses {
                resolved.insert(address.ip(), host.clone());
            }
        }
    }
    if resolved.is_empty() {
        bail!("no capture host resolved");
    }
    let filter = format!(
        "tcp port {port} and ({})",
        resolved
            .keys()
            .map(|ip| format!("host {ip}"))
            .collect::<Vec<_>>()
            .join(" or ")
    );
    let mut child = Command::new(&tcpdump)
        .args([
            "-i",
            &interface,
            "-nn",
            "-tt",
            "-l",
            "-q",
            "-c",
            &packet_limit.max(1).to_string(),
            &filter,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + duration.max(Duration::from_millis(100));
    let mut timed_out = false;
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    if child.try_wait()?.is_none() {
        timed_out = true;
        let _ = child.kill();
    }
    let output = child.wait_with_output()?;
    if !timed_out && !output.status.success() && output.stdout.is_empty() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail_lower = detail.to_ascii_lowercase();
        if detail_lower.contains("permission") || detail_lower.contains("operation not permitted") {
            bail!(
                "tcpdump permission denied; grant capture capability or run this command with appropriate privileges"
            );
        }
        bail!("{}", detail.lines().last().unwrap_or("tcpdump failed"));
    }
    Ok(parse_tcpdump(
        &String::from_utf8_lossy(&output.stdout),
        &resolved,
        port,
    ))
}

pub fn parse_tcpdump(
    text: &str,
    remote_ips: &HashMap<IpAddr, String>,
    port: u16,
) -> Vec<NetworkFlow> {
    let expression = Regex::new(
        r"^\s*(?P<ts>\d+(?:\.\d+)?)\s+IP(?:6)?\s+(?P<src>\S+)\s+>\s+(?P<dst>\S+):.*?length\s+(?P<len>\d+)",
    ).expect("static packet regex");
    #[derive(Default)]
    struct Bucket {
        first: f64,
        last: f64,
        out_packets: u64,
        in_packets: u64,
        out_bytes: u64,
        in_bytes: u64,
    }
    let mut buckets: HashMap<IpAddr, Bucket> = HashMap::new();
    for line in text.lines() {
        let Some(found) = expression.captures(line) else {
            continue;
        };
        let Some((src_ip, src_port)) = split_endpoint(&found["src"]) else {
            continue;
        };
        let Some((dst_ip, dst_port)) = split_endpoint(&found["dst"]) else {
            continue;
        };
        let remote = if remote_ips.contains_key(&src_ip) {
            src_ip
        } else if remote_ips.contains_key(&dst_ip) {
            dst_ip
        } else {
            continue;
        };
        let timestamp = found["ts"].parse::<f64>().unwrap_or_default();
        let length = found["len"].parse::<u64>().unwrap_or_default();
        let bucket = buckets.entry(remote).or_insert_with(|| Bucket {
            first: timestamp,
            last: timestamp,
            ..Bucket::default()
        });
        bucket.first = bucket.first.min(timestamp);
        bucket.last = bucket.last.max(timestamp);
        if dst_ip == remote && (dst_port == Some(port) || src_port != Some(port)) {
            bucket.out_packets += 1;
            bucket.out_bytes += length;
        } else {
            bucket.in_packets += 1;
            bucket.in_bytes += length;
        }
    }
    buckets
        .into_iter()
        .map(|(ip, bucket)| {
            let mut flow = NetworkFlow {
                mode: "passive".into(),
                data_source: "tcpdump_metadata".into(),
                destination_host: remote_ips.get(&ip).cloned(),
                destination_ip: Some(ip.to_string()),
                destination_port: Some(port),
                protocol: Some("tcp/tls-opaque".into()),
                started_at: DateTime::from_timestamp_millis((bucket.first * 1000.0) as i64)
                    .map(iso),
                ended_at: DateTime::from_timestamp_millis((bucket.last * 1000.0) as i64).map(iso),
                request_bytes: bucket.out_bytes,
                response_bytes: bucket.in_bytes,
                packets_out: bucket.out_packets,
                packets_in: bucket.in_packets,
                duration_ms: Some((bucket.last - bucket.first) * 1000.0),
                success: Some(true),
                ..NetworkFlow::default()
            };
            fingerprint(&mut flow);
            flow
        })
        .collect()
}

fn default_capture_interface(tcpdump: &str) -> Option<String> {
    let output = Command::new(tcpdump).arg("-D").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let names: Vec<_> = text
        .lines()
        .filter_map(|line| {
            line.trim()
                .split_once('.')
                .map(|(_, name)| name.split_whitespace().next().unwrap_or(name).to_owned())
        })
        .collect();
    ["any", "pktap", "en0"]
        .iter()
        .find(|preferred| names.iter().any(|name| name == **preferred))
        .map(|value| (*value).into())
        .or_else(|| names.first().cloned())
}

fn split_endpoint(value: &str) -> Option<(IpAddr, Option<u16>)> {
    let value = value.trim_end_matches(':');
    if let Ok(ip) = value.parse() {
        return Some((ip, None));
    }
    if let Some(value) = value.strip_prefix('[') {
        let (host, port) = value.rsplit_once("].")?;
        return Some((host.parse().ok()?, port.parse().ok()));
    }
    let (host, port) = value.rsplit_once('.')?;
    Some((host.parse().ok()?, port.parse().ok()))
}

fn fingerprint(flow: &mut NetworkFlow) {
    flow.event_fingerprint.clear();
    let encoded = serde_json::to_vec(flow).unwrap_or_default();
    flow.event_fingerprint = format!("{:x}", Sha256::digest(encoded));
}

fn iso(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn find_command(command: &str) -> Option<String> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|directory| {
            directory.join(if cfg!(windows) {
                format!("{command}.exe")
            } else {
                command.into()
            })
        })
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_parser_keeps_only_metadata() {
        let remotes = HashMap::from([("1.2.3.4".parse().unwrap(), "api.openai.com".into())]);
        let flows = parse_tcpdump(
            "1786492800.1 IP 10.0.0.2.50000 > 1.2.3.4.443: tcp 0 length 123\n1786492800.2 IP 1.2.3.4.443 > 10.0.0.2.50000: tcp 0 length 456",
            &remotes,
            443,
        );
        assert_eq!(flows.len(), 1);
        assert_eq!(flows[0].request_bytes, 123);
        assert_eq!(flows[0].response_bytes, 456);
    }

    #[test]
    fn packet_parser_accepts_ip6_and_ipv6_endpoints() {
        let remotes = HashMap::from([("2001:db8::1".parse().unwrap(), "api.openai.com".into())]);
        let flows = parse_tcpdump(
            "1786492800.1 IP6 [fd00::2].50000 > [2001:db8::1].443: tcp 0 length 12\n1786492800.2 IP6 [2001:db8::1].443 > [fd00::2].50000: tcp 0 length 34",
            &remotes,
            443,
        );
        assert_eq!(flows.len(), 1);
        assert_eq!(flows[0].request_bytes, 12);
        assert_eq!(flows[0].response_bytes, 34);
        let record: NetworkFlowRecord = flows.into_iter().next().unwrap().into();
        assert_eq!(record.request_bytes, 12);
        assert_eq!(record.quality.source, "tcpdump_metadata");
    }
}
