//! Versioned, data-driven API-equivalent pricing.

use std::{
    fs,
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    path::Path,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, FixedOffset};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned, pki_types::ServerName};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::models::TokenUsage;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Price {
    pub model: String,
    pub provider: String,
    pub effective_from: String,
    pub version: String,
    pub input_per_million: f64,
    pub cached_input_per_million: f64,
    pub cache_write_per_million: f64,
    pub output_per_million: f64,
    pub long_context_threshold: Option<i64>,
    #[serde(default = "one")]
    pub long_context_input_multiplier: f64,
    #[serde(default = "one")]
    pub long_context_output_multiplier: f64,
}

const fn one() -> f64 {
    1.0
}

#[derive(Debug, Clone, PartialEq)]
pub struct CostBreakdown {
    pub regular_input_usd: f64,
    pub cached_input_usd: f64,
    pub cache_write_usd: f64,
    pub output_usd: f64,
    pub total_usd: f64,
    pub without_cache_usd: f64,
    pub savings_usd: f64,
    pub pricing_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedPrice<'a> {
    pub price: &'a Price,
    /// The usage predates the first price we know for this model. The price is
    /// useful for an API-equivalent estimate, but must not be presented as the
    /// model's exact historical list price.
    pub historical_estimate: bool,
}

impl ResolvedPrice<'_> {
    pub fn version(&self) -> String {
        if self.historical_estimate {
            format!("{}:historical-estimate", self.price.version)
        } else {
            self.price.version.clone()
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PricingCatalog {
    pub entries: Vec<Price>,
    pub catalog_version: String,
    pub currency: String,
    pub source: String,
}

#[derive(Deserialize)]
struct PriceFile {
    #[serde(default)]
    catalog_version: String,
    #[serde(default = "default_currency")]
    currency: String,
    #[serde(default)]
    source: String,
    prices: Vec<Price>,
}

const UPDATE_HOST: &str = "raw.githubusercontent.com";
const UPDATE_PATH: &str = "/DelicateNorman/codex-meter/main/codex_meter/data/pricing.json";
const CHECKSUM_PATH: &str = "/DelicateNorman/codex-meter/main/codex_meter/data/pricing.sha256";

fn default_currency() -> String {
    "USD".into()
}

impl PricingCatalog {
    pub fn new(entries: Vec<Price>) -> Self {
        Self {
            entries,
            catalog_version: "custom".into(),
            currency: default_currency(),
            source: String::new(),
        }
    }

    /// Load the catalog compiled into the standalone binary.
    pub fn bundled() -> Result<Self> {
        Self::from_json(include_str!("../codex_meter/data/pricing.json"))
            .context("parse bundled pricing catalog")
    }

    pub fn from_path(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("read pricing catalog {}", path.display()))?;
        Self::from_json(&text).with_context(|| format!("parse pricing catalog {}", path.display()))
    }

    /// Load user updates while always retaining newer entries compiled into
    /// the binary. A user entry replaces only the same model/provider/effective
    /// tuple, so ordinary application upgrades cannot be masked by a stale
    /// pricing.json copied during the first install.
    pub fn load_with_bundled(path: &Path) -> Result<Self> {
        let mut bundled = Self::bundled()?;
        let Ok(local) = Self::from_path(path) else {
            return Ok(bundled);
        };
        let bundled_latest = latest_effective(&bundled.entries);
        let local_latest = latest_effective(&local.entries);
        if local_latest < bundled_latest {
            return Ok(bundled);
        }
        for entry in local.entries {
            bundled.entries.retain(|existing| {
                !(existing.model == entry.model
                    && existing.provider == entry.provider
                    && existing.effective_from == entry.effective_from)
            });
            bundled.entries.push(entry);
        }
        if !local.catalog_version.is_empty() {
            bundled.catalog_version = local.catalog_version;
            bundled.currency = local.currency;
            bundled.source = local.source;
        }
        Ok(bundled)
    }

    pub fn from_json(value: &str) -> Result<Self> {
        let parsed: PriceFile = serde_json::from_str(value)?;
        let catalog = Self {
            entries: parsed.prices,
            catalog_version: parsed.catalog_version,
            currency: parsed.currency,
            source: parsed.source,
        };
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn validate(&self) -> Result<()> {
        if self.entries.is_empty() {
            bail!("pricing catalog has no entries");
        }
        if self.currency != "USD" {
            bail!("pricing catalog currency must be USD");
        }
        for entry in &self.entries {
            if entry.model.trim().is_empty()
                || entry.provider.trim().is_empty()
                || entry.model.contains(['\r', '\n', '\0'])
                || entry.provider.contains(['\r', '\n', '\0'])
                || parse_time(&entry.effective_from).is_none()
                || [
                    entry.input_per_million,
                    entry.cached_input_per_million,
                    entry.cache_write_per_million,
                    entry.output_per_million,
                    entry.long_context_input_multiplier,
                    entry.long_context_output_multiplier,
                ]
                .into_iter()
                .any(|value| !value.is_finite() || value < 0.0)
            {
                bail!("pricing catalog contains an invalid entry");
            }
        }
        Ok(())
    }

    pub fn resolve<'a>(
        &'a self,
        model: Option<&str>,
        provider: Option<&str>,
        at: Option<&str>,
    ) -> Option<&'a Price> {
        let model = match model? {
            "gpt-5.6" => "gpt-5.6-sol",
            value => value,
        };
        let provider = provider.unwrap_or("openai");
        let at = at.and_then(parse_time);
        self.entries
            .iter()
            .filter(|entry| {
                if entry.model != model || entry.provider != provider {
                    return false;
                }
                match (parse_time(&entry.effective_from), at) {
                    (Some(effective), Some(at)) => effective <= at,
                    (Some(_), None) => true,
                    (None, _) => false,
                }
            })
            .max_by(|left, right| left.effective_from.cmp(&right.effective_from))
    }

    /// Resolve a price for an API-equivalent estimate. Calls before the first
    /// known price for an otherwise known model use that earliest price and are
    /// explicitly marked as historical estimates. Unknown models remain
    /// unpriced instead of silently borrowing another model's rate.
    pub fn resolve_for_estimate<'a>(
        &'a self,
        model: Option<&str>,
        provider: Option<&str>,
        at: Option<&str>,
    ) -> Option<ResolvedPrice<'a>> {
        if let Some(price) = self.resolve(model, provider, at) {
            return Some(ResolvedPrice {
                price,
                historical_estimate: false,
            });
        }

        let model = normalize_model(model?);
        let provider = provider.unwrap_or("openai");
        let at = parse_time(at?)?;
        let price = self
            .entries
            .iter()
            .filter(|entry| entry.model == model && entry.provider == provider)
            .filter_map(|entry| parse_time(&entry.effective_from).map(|time| (entry, time)))
            .min_by_key(|(_, effective)| *effective)?;
        (at < price.1).then_some(ResolvedPrice {
            price: price.0,
            historical_estimate: true,
        })
    }

    pub fn calculate(&self, usage: TokenUsage, price: &Price) -> CostBreakdown {
        let (input_multiplier, output_multiplier) = match price.long_context_threshold {
            Some(threshold) if usage.input_tokens > threshold => (
                price.long_context_input_multiplier,
                price.long_context_output_multiplier,
            ),
            _ => (1.0, 1.0),
        };
        let scale = 1_000_000.0;
        let regular = usage.billable_regular_input_tokens() as f64
            * price.input_per_million
            * input_multiplier
            / scale;
        let cached =
            usage.cached_input_tokens as f64 * price.cached_input_per_million * input_multiplier
                / scale;
        let cache_write =
            usage.cache_write_tokens as f64 * price.cache_write_per_million * input_multiplier
                / scale;
        // Reasoning tokens are already part of output_tokens.
        let output =
            usage.output_tokens as f64 * price.output_per_million * output_multiplier / scale;
        let total = regular + cached + cache_write + output;
        let without_cache =
            usage.input_tokens as f64 * price.input_per_million * input_multiplier / scale + output;
        CostBreakdown {
            regular_input_usd: regular,
            cached_input_usd: cached,
            cache_write_usd: cache_write,
            output_usd: output,
            total_usd: total,
            without_cache_usd: without_cache,
            savings_usd: without_cache - total,
            pricing_version: price.version.clone(),
        }
    }
}

fn normalize_model(model: &str) -> &str {
    match model {
        "gpt-5.6" => "gpt-5.6-sol",
        value => value,
    }
}

fn parse_time(value: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(value).ok()
}

fn latest_effective(entries: &[Price]) -> Option<DateTime<FixedOffset>> {
    entries
        .iter()
        .filter_map(|entry| parse_time(&entry.effective_from))
        .max()
}

pub fn update_catalog(path: &Path, timeout: Duration) -> Result<PricingCatalog> {
    let body = https_get(UPDATE_HOST, UPDATE_PATH, timeout).context("download pricing catalog")?;
    let checksum =
        https_get(UPDATE_HOST, CHECKSUM_PATH, timeout).context("download pricing checksum")?;
    verify_checksum(&body, &checksum)?;
    let text = std::str::from_utf8(&body).context("pricing catalog is not UTF-8")?;
    let catalog = PricingCatalog::from_json(text)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let staged = path.with_extension("json.next");
    fs::write(&staged, &body)
        .with_context(|| format!("stage pricing catalog {}", staged.display()))?;
    fs::rename(&staged, path).or_else(|_| {
        fs::copy(&staged, path)?;
        fs::remove_file(&staged)
    })?;
    Ok(catalog)
}

fn verify_checksum(body: &[u8], checksum: &[u8]) -> Result<()> {
    let expected = std::str::from_utf8(checksum)?
        .split_ascii_whitespace()
        .next()
        .context("pricing checksum is empty")?;
    if expected.len() != 64 || !expected.bytes().all(|value| value.is_ascii_hexdigit()) {
        bail!("pricing checksum is invalid");
    }
    let actual = format!("{:x}", Sha256::digest(body));
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("pricing checksum mismatch; existing catalog was preserved");
    }
    Ok(())
}

fn https_get(host: &str, path: &str, timeout: Duration) -> Result<Vec<u8>> {
    let timeout = timeout.max(Duration::from_millis(100));
    let address = (host, 443)
        .to_socket_addrs()?
        .find_map(|address| TcpStream::connect_timeout(&address, timeout).ok())
        .context("could not connect to pricing host")?;
    address.set_read_timeout(Some(timeout))?;
    address.set_write_timeout(Some(timeout))?;
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let name = ServerName::try_from(host.to_owned()).context("invalid pricing TLS host")?;
    let connection = ClientConnection::new(Arc::new(config), name)?;
    let mut stream = StreamOwned::new(connection, address);
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: codex-meter/{}\r\nAccept: application/json,text/plain\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
        env!("CARGO_PKG_VERSION")
    )?;
    stream.flush()?;
    let mut response = Vec::new();
    stream
        .take(1_048_577)
        .read_to_end(&mut response)
        .context("read pricing response")?;
    if response.len() > 1_048_576 {
        bail!("pricing response exceeded 1 MiB");
    }
    parse_http_response(&response)
}

fn parse_http_response(response: &[u8]) -> Result<Vec<u8>> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .context("pricing server returned an invalid HTTP response")?;
    let headers = std::str::from_utf8(&response[..header_end])?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .context("pricing server returned an invalid HTTP status")?;
    if status != 200 {
        bail!("pricing server returned HTTP {status}");
    }
    let body = &response[header_end + 4..];
    if headers
        .lines()
        .any(|line| line.eq_ignore_ascii_case("transfer-encoding: chunked"))
    {
        decode_chunked(body)
    } else {
        Ok(body.to_vec())
    }
}

fn decode_chunked(mut body: &[u8]) -> Result<Vec<u8>> {
    let mut decoded = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|window| window == b"\r\n")
            .context("invalid chunked pricing response")?;
        let size_text = std::str::from_utf8(&body[..line_end])?
            .split(';')
            .next()
            .unwrap_or_default();
        let size = usize::from_str_radix(size_text.trim(), 16)
            .context("invalid chunk size in pricing response")?;
        body = &body[line_end + 2..];
        if size == 0 {
            break;
        }
        if body.len() < size + 2 || &body[size..size + 2] != b"\r\n" {
            bail!("truncated chunked pricing response");
        }
        decoded.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_alias_and_calculates_cache_savings() {
        let catalog = PricingCatalog::bundled().unwrap();
        let price = catalog
            .resolve(Some("gpt-5.6"), None, Some("2026-08-12T00:00:00Z"))
            .unwrap();
        assert_eq!(price.model, "gpt-5.6-sol");
        let usage = TokenUsage {
            input_tokens: 200_000,
            cached_input_tokens: 160_000,
            output_tokens: 20_000,
            reasoning_tokens: 10_000,
            total_tokens: 220_000,
            ..Default::default()
        };
        let cost = catalog.calculate(usage, price);
        assert!((cost.total_usd - 0.88).abs() < 1e-9);
        assert!((cost.savings_usd - 0.72).abs() < 1e-9);
    }

    #[test]
    fn long_context_multipliers_match_python() {
        let catalog = PricingCatalog::bundled().unwrap();
        let price = catalog
            .resolve(Some("gpt-5.6-sol"), Some("openai"), None)
            .unwrap();
        let usage = TokenUsage {
            input_tokens: 300_000,
            output_tokens: 100_000,
            ..Default::default()
        };
        let cost = catalog.calculate(usage, price);
        assert!((cost.total_usd - 7.5).abs() < 1e-9);
    }

    #[test]
    fn historical_estimates_use_earliest_known_rate_without_pricing_unknown_models() {
        let catalog = PricingCatalog::bundled().unwrap();
        assert!(
            catalog
                .resolve(
                    Some("gpt-5.6-sol"),
                    Some("openai"),
                    Some("2026-07-20T00:00:00Z")
                )
                .is_none()
        );
        let resolved = catalog
            .resolve_for_estimate(
                Some("gpt-5.6-sol"),
                Some("openai"),
                Some("2026-07-20T00:00:00Z"),
            )
            .unwrap();
        assert!(resolved.historical_estimate);
        assert!(resolved.version().ends_with(":historical-estimate"));
        assert!(
            catalog
                .resolve_for_estimate(
                    Some("codex-auto-review"),
                    Some("openai"),
                    Some("2026-08-10T00:00:00Z")
                )
                .is_none()
        );
    }

    #[test]
    fn checksum_and_chunked_http_parsers_reject_tampering() {
        let body = br#"{"prices":[]}"#;
        let checksum = format!("{:x}  pricing.json\n", Sha256::digest(body));
        verify_checksum(body, checksum.as_bytes()).unwrap();
        assert!(verify_checksum(b"tampered", checksum.as_bytes()).is_err());

        let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\ntest\r\n3\r\n123\r\n0\r\n\r\n";
        assert_eq!(parse_http_response(response).unwrap(), b"test123");
        assert!(parse_http_response(b"HTTP/1.1 500 Nope\r\n\r\n").is_err());
    }

    #[test]
    fn stale_user_catalog_does_not_hide_newer_bundled_prices() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pricing.json");
        fs::write(
            &path,
            r#"{
              "catalog_version":"old",
              "currency":"USD",
              "source":"custom",
              "prices":[{
                "model":"gpt-5.6-luna","provider":"openai",
                "effective_from":"2026-08-01T00:00:00Z","version":"custom-old",
                "input_per_million":1.0,"cached_input_per_million":0.1,
                "cache_write_per_million":1.25,"output_per_million":6.0,
                "long_context_threshold":272000,
                "long_context_input_multiplier":2.0,"long_context_output_multiplier":1.5
              }]
            }"#,
        )
        .unwrap();
        let catalog = PricingCatalog::load_with_bundled(&path).unwrap();
        let current = catalog
            .resolve(
                Some("gpt-5.6-luna"),
                Some("openai"),
                Some("2026-08-14T12:00:00Z"),
            )
            .unwrap();
        assert_eq!(current.input_per_million, 1.0);
        assert_eq!(current.version, "openai-2026-08-12");
        assert_eq!(catalog.catalog_version, "openai-2026-08-14");
        assert_ne!(catalog.source, "custom");
    }
}
