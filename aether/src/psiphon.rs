use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::error::{AetherError, Result};

const DEFAULT_BIND: &str = "127.0.0.1:1821";
const READY_TIMEOUT_SECS: u64 = 180;
const BINARY_NAMES: &[&str] = &["psiphon-tunnel-core", "psiphon"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    Chain,
    Reverse,
    Only,
}

pub fn mode() -> Mode {
    let raw = std::env::var("AETHER_PSIPHON")
        .unwrap_or_default()
        .trim()
        .to_lowercase();

    match raw.as_str() {
        "" | "0" | "off" | "false" | "no" => Mode::Off,
        "reverse" | "rev" | "warp-over-psiphon" => Mode::Reverse,
        "only" | "alone" | "psiphon-only" | "direct" => Mode::Only,
        _ => Mode::Chain,
    }
}

pub fn listen_address() -> SocketAddr {
    std::env::var("AETHER_PSIPHON_BIND")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or_else(|| DEFAULT_BIND.parse().expect("a literal address"))
}

pub fn http_listen_address() -> Option<SocketAddr> {
    std::env::var("AETHER_PSIPHON_HTTP")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && value != "off")
        .and_then(|value| value.parse().ok())
}

pub fn state_dir(base_config: &str) -> PathBuf {
    if let Some(dir) = std::env::var("AETHER_PSIPHON_DIR")
        .ok()
        .filter(|d| !d.trim().is_empty())
    {
        return PathBuf::from(dir.trim());
    }

    let stem = base_config.trim_end_matches(['/', '\\']);
    PathBuf::from(format!("{stem}-psiphon"))
}

fn region() -> Option<String> {
    std::env::var("AETHER_PSIPHON_REGION")
        .ok()
        .map(|value| value.trim().to_uppercase())
        .filter(|value| value.len() == 2 && value.chars().all(|c| c.is_ascii_alphabetic()))
}

fn ready_timeout() -> Duration {
    let secs = std::env::var("AETHER_PSIPHON_READY_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&v| v > 0)
        .map(|v| v.min(86_400))
        .unwrap_or(READY_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

fn exe_suffix() -> &'static str {
    if cfg!(windows) {
        ".exe"
    } else {
        ""
    }
}

fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(here) = exe.parent() {
            dirs.push(here.to_path_buf());
            dirs.push(here.join("pt"));
        }
    }

    dirs.push(PathBuf::from("."));
    dirs.push(PathBuf::from("pt"));

    if let Ok(path) = std::env::var("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }

    dirs
}

pub fn binary() -> Option<PathBuf> {
    if let Some(given) = std::env::var("AETHER_PSIPHON_BIN")
        .ok()
        .map(|v| PathBuf::from(v.trim()))
        .filter(|p| p.is_file())
    {
        return Some(given);
    }

    let suffix = exe_suffix();
    for dir in search_dirs() {
        for name in BINARY_NAMES {
            let candidate = dir.join(format!("{name}{suffix}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

pub fn install_hint() -> String {
    let goos = match std::env::consts::OS {
        "android" => "linux",
        other => other,
    };
    let goarch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        "arm" => "arm",
        other => other,
    };

    let looked = search_dirs()
        .iter()
        .take(4)
        .map(|dir| dir.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "psiphon needs the psiphon-tunnel-core console client, and it was not in {looked} or on \
         PATH. every release archive ships it in the pt folder beside the binary, so keep that \
         folder next to aether instead of moving the binary out on its own. from source run \
         'bash psiphon-build.sh {goos} {goarch} pt'. point AETHER_PSIPHON_BIN at one you already \
         have"
    )
}

const PROPAGATION_CHANNEL_ID: &str = "FFFFFFFFFFFFFFFF";
const SPONSOR_ID: &str = "FFFFFFFFFFFFFFFF";
const SERVER_LIST_URL: &str =
    "https://s3.amazonaws.com//psiphon/web/mjr4-p23r-puwl/server_list_compressed";
const SERVER_LIST_SIGNATURE_KEY: &str = concat!(
    "MIICIDANBgkqhkiG9w0BAQEFAAOCAg0AMIICCAKCAgEAt7Ls+/39r+T6zNW7GiVpJfzq/xvL9SBH",
    "5rIFnk0RXYEYavax3WS6HOD35eTAqn8AniOwiH+DOkvgSKF2caqk/y1dfq47Pdymtwzp9ikpB1C5",
    "OfAysXzBiwVJlCdajBKvBZDerV1cMvRzCKvKwRmvDmHgphQQ7WfXIGbRbmmk6opMBh3roE42Kcot",
    "LFtqp0RRwLtcBRNtCdsrVsjiI1Lqz/lH+T61sGjSjQ3CHMuZYSQJZo/KrvzgQXpkaCTdbObxHqb6",
    "/+i1qaVOfEsvjoiyzTxJADvSytVtcTjijhPEV6XskJVHE1Zgl+7rATr/pDQkw6DPCNBS1+Y6fy7G",
    "stZALQXwEDN/qhQI9kWkHijT8ns+i1vGg00Mk/6J75arLhqcodWsdeG/M/moWgqQAnlZAGVtJI1O",
    "geF5fsPpXu4kctOfuZlGjVZXQNW34aOzm8r8S0eVZitPlbhcPiR4gT/aSMz/wd8lZlzZYsje/Jr8",
    "u/YtlwjjreZrGRmG8KMOzukV3lLmMppXFMvl4bxv6YFEmIuTsOhbLTwFgh7KYNjodLj/LsqRVfwz",
    "31PgWQFTEPICV7GCvgVlPRxnofqKSjgTWI4mxDhBpVcATvaoBl1L/6WLbFvBsoAUBItWwctO2xal",
    "KxF5szhGm8lccoc5MZr8kfE0uxMgsxz4er68iCID+rsCAQM=",
);

const CDN_PROTOCOLS: &[&str] = &[
    "FRONTED-MEEK-CDN-OSSH",
    "FRONTED-MEEK-CDN-HTTP-OSSH",
    "FRONTED-MEEK-CDN-QUIC-OSSH",
];

const NON_INPROXY_PROTOCOLS: &[&str] = &[
    "SSH",
    "OSSH",
    "TLS-OSSH",
    "UNFRONTED-MEEK-OSSH",
    "UNFRONTED-MEEK-HTTPS-OSSH",
    "UNFRONTED-MEEK-SESSION-TICKET-OSSH",
    "QUIC-OSSH",
    "SHADOWSOCKS-OSSH",
    "FRONTED-MEEK-OSSH",
    "FRONTED-MEEK-CDN-OSSH",
    "FRONTED-MEEK-HTTP-OSSH",
    "FRONTED-MEEK-CDN-HTTP-OSSH",
    "FRONTED-MEEK-QUIC-OSSH",
    "FRONTED-MEEK-CDN-QUIC-OSSH",
];

const CHAINED_PROTOCOLS: &[&str] = &[
    "SSH",
    "OSSH",
    "TLS-OSSH",
    "UNFRONTED-MEEK-OSSH",
    "UNFRONTED-MEEK-HTTPS-OSSH",
    "UNFRONTED-MEEK-SESSION-TICKET-OSSH",
    "SHADOWSOCKS-OSSH",
    "FRONTED-MEEK-OSSH",
    "FRONTED-MEEK-CDN-OSSH",
    "FRONTED-MEEK-HTTP-OSSH",
    "FRONTED-MEEK-CDN-HTTP-OSSH",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Auto,
    Cdn,
    Direct,
}

pub fn shape() -> Shape {
    match std::env::var("AETHER_PSIPHON_MODE")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "cdn" | "fronted" => Shape::Cdn,
        "direct" | "plain" => Shape::Direct,
        _ => Shape::Auto,
    }
}

pub fn chained_protocols(shape: Shape) -> Vec<&'static str> {
    let carries_udp = |name: &&str| name.contains("QUIC") || name.starts_with("INPROXY");

    match shape {
        Shape::Cdn => CDN_PROTOCOLS
            .iter()
            .copied()
            .filter(|name| !carries_udp(name))
            .collect(),
        Shape::Direct => CHAINED_PROTOCOLS
            .iter()
            .copied()
            .filter(|name| !name.starts_with("FRONTED"))
            .collect(),
        Shape::Auto => CHAINED_PROTOCOLS.to_vec(),
    }
}

fn client_platform() -> String {
    let system = if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "android") {
        "Android"
    } else {
        "Linux"
    };
    format!("{system}_aether")
}

fn env_or(name: &str, fallback: &str) -> String {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn cdn_candidates(raw: &str) -> Vec<String> {
    raw.split([' ', '\t', '\n', '\r', ',', ';'])
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

/// The server names the in-app CDN scanner validates edges against, and the
/// names psiphon presents to those edges when the user has not typed their own.
/// Every one of these is a hostname that is actually served from the matching
/// CDN's edge network, which is what makes the fronting connection's TLS
/// certificate verify; a guessed name (www.akamai.com, d1.cloudfront.net) is
/// rejected by the edge with a certificate mismatch and the tunnel never comes
/// up.
pub const CDN_FRONTING_SNI: &[(&str, &[&str])] = &[
    (
        "Akamai",
        &[
            "a248.e.akamai.net",
            "a77.net.akamai.net",
            "a104.net.akamai.net",
            "a184.net.akamai.net",
            "a.akamaized.net",
            // The SNI that actually connected in the reference client's log on
            // this very network: "CDN Fronting edge: 185.200.232.49 via
            // a.akamaized-staging.net".
            "a.akamaized-staging.net",
            "ds-aksb.akamaized.net",
            "ak.net.akamaized.net",
        ],
    ),
    (
        "Google",
        &[
            "fonts.googleapis.com",
            "ajax.googleapis.com",
            "storage.googleapis.com",
            "www.gstatic.com",
            "ssl.gstatic.com",
            "accounts.google.com",
        ],
    ),
    (
        "CloudFront",
        &[
            "d1.awsstatic.com",
            "aws.amazon.com",
            "images-na.ssl-images-amazon.com",
            "d36cz9buwru1tt.cloudfront.net",
        ],
    ),
    (
        "Azure",
        &[
            "ajax.aspnetcdn.com",
            "az416426.vo.msecnd.net",
            "az784690.vo.msecnd.net",
            "cdn.office.net",
            "static.azureedge.net",
        ],
    ),
];

/// Every fronting server name, in the order the scanner presents them.
pub fn cdn_fronting_sni() -> Vec<&'static str> {
    CDN_FRONTING_SNI
        .iter()
        .flat_map(|(_, names)| names.iter().copied())
        .collect()
}

/// Edge addresses that are reachable from inside Iran and front Psiphon
/// correctly, collected from cdn-ip-finder's operator-tested lists and the one
/// pairing that connected live (23.48.23.151 with a248.e.akamai.net). They are
/// offered first so a scan that only found generic Azure/CloudFront edges —
/// which answer the TLS handshake but return 403 to a fronting Host — cannot
/// crowd out the edges that actually carry the tunnel.
const IR_PROVEN_CDN_IPS: &[&str] = &[
    "184.24.77.42", "184.24.77.32", "184.24.77.5", "184.24.77.7", "184.24.77.21",
    "184.24.77.11", "184.24.77.16", "184.24.77.36", "185.200.232.49", "185.200.232.50",
    "185.200.232.42", "185.200.232.41", "185.200.232.43", "185.200.232.8", "23.48.23.151",
    "23.48.23.186", "23.48.23.133", "23.48.23.195", "104.112.146.82", "23.58.193.140",
    "2.22.250.149", "92.16.53.11", "72.246.28.3",
];

/// Akamai edge addresses the reference client (ShirOKhorshid's
/// TunnelManager.java) forces every fronted dial onto via
/// `FrontedMeekDialOverrides`. Ported verbatim: they are the addresses that
/// pair with the Akamai certificate names in the override's verify list, which
/// is what turns "some reachable IP" into "a dial that verifies and routes".
const REFERENCE_DIAL_EDGES: &[&str] = &[
    "23.215.0.206",
    "23.215.0.203",
    "23.212.250.91",
    "23.212.250.78",
    "23.12.147.13",
    "23.12.147.29",
    "23.73.207.8",
    "23.73.207.15",
    "92.123.102.43",
];

/// One dial override: force this server's fronted dial onto `ip` with an
/// explicit SNI and a verify list the edge's certificate actually matches.
/// Mirrors makeEdgeCdnFrontingOverride — match everything, http/1.1, the
/// Chrome-83 TLS profile, and the Akamai hostname set the reference client
/// accepts. The Host header is deliberately not touched: it stays the
/// server's own fronting domain, which is what makes the edge route onward.
fn fronting_edge_override(id: &str, ip: &str, sni: &str) -> serde_json::Value {
    let mut verify: Vec<String> = vec![sni.to_string(), ip.to_string()];
    for name in [
        "a248.e.akamai.net",
        "a.akamaized.net",
        "a.akamaized-staging.net",
        "a.akamaihd.net",
        "a.akamaihd-staging.net",
        "www.akamai.com",
    ] {
        let owned = name.to_string();
        if !verify.contains(&owned) {
            verify.push(owned);
        }
    }

    serde_json::json!({
        "OverrideID": id,
        "MatchDialAddressRegexes": [".*"],
        "DialAddresses": [ip],
        "SNIServerName": sni,
        "VerifyServerNames": verify,
        "ALPNProtocols": ["http/1.1"],
        "TLSProfile": "Chrome-83",
    })
}

/// True for the certificate names the psiphon Akamai fronting setup actually
/// uses (the server entries carry `meekFrontingDomain` values like
/// a248.e.akamai.net and *.akamaized.net). Azure/CloudFront/Google scan SNIs
/// must never be forced onto an Akamai edge: TLS then terminates as the wrong
/// property and the fronting Host gets 403.
fn is_akamai_fronting_name(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    name.ends_with("akamai.net")
        || name.ends_with("akamaihd.net")
        || name.ends_with("akamaized.net")
        || name == "www.akamai.com"
}

fn put_cdn_fronting(map: &mut serde_json::Map<String, serde_json::Value>) {
    let user_ips = cdn_candidates(&std::env::var("AETHER_PSIPHON_CDN_IPS").unwrap_or_default());
    let user_sni = cdn_candidates(&std::env::var("AETHER_PSIPHON_CDN_SNI").unwrap_or_default());

    // The SNI pool for forced edge dials. Only Akamai-family names qualify:
    // the dial addresses below are Akamai edges and the Host header is the
    // server's own *.psiphon3.com domain — a scan result like
    // ajax.aspnetcdn.com terminates TLS as a different property and Akamai
    // answers 403 (verified live). Within the family the reference client
    // rotates candidates and its winning pair on this network was
    // a.akamaized-staging.net, not a248 — so we rotate too, one name per
    // edge, instead of forcing a single SNI onto every edge.
    let mut edge_sni_pool: Vec<String> = user_sni
        .iter()
        .filter(|name| is_akamai_fronting_name(name))
        .cloned()
        .collect();
    for name in cdn_fronting_sni() {
        if is_akamai_fronting_name(name) {
            let owned = name.to_string();
            if !edge_sni_pool.contains(&owned) {
                edge_sni_pool.push(owned);
            }
        }
    }
    if edge_sni_pool.is_empty() {
        edge_sni_pool.push("a248.e.akamai.net".to_string());
    }
    let edge_sni_for = |index: usize| edge_sni_pool[index % edge_sni_pool.len()].clone();

    // Curated edges first, then whatever the in-app scanner found, deduped.
    let mut addresses: Vec<String> = IR_PROVEN_CDN_IPS
        .iter()
        .map(|s| s.to_string())
        .collect();
    for ip in user_ips {
        if !addresses.contains(&ip) {
            addresses.push(ip);
        }
    }

    // Names the edges actually hold certificates for, user's own first when
    // they gave any, then the known-good set.
    let mut names = user_sni;
    for name in cdn_fronting_sni() {
        let owned = name.to_string();
        if !names.contains(&owned) {
            names.push(owned);
        }
    }

    // Psiphon's own maintained edge spec stays enabled alongside ours — the
    // reference client never suppresses it, and its tested pairs are exactly
    // what a custom candidate list should complement rather than replace.
    map.insert(
        "FrontedMeekCDNScanUseBuiltInSpec".into(),
        serde_json::Value::from(true),
    );

    let mut spec = serde_json::Map::new();
    spec.insert("IPCandidates".into(), serde_json::json!(addresses));
    if !names.is_empty() {
        spec.insert("SNIServerNames".into(), serde_json::json!(names));
    }
    map.insert(
        "FrontedMeekCDNScanSpec".into(),
        serde_json::Value::Object(spec),
    );

    // The mechanism that actually connects in the reference client: force the
    // dial onto known edges with a verifiable SNI. Two Fastly paths (psiphon
    // fronts some servers through Fastly, reached as the pypi.org anycast)
    // plus the Akamai edge set — the reference list, then our Iran-proven
    // Akamai addresses. Azure/CloudFront scan results stay in the scan spec
    // only: as forced overrides they would guarantee 403s, because no Akamai
    // certificate verifies on them.
    let mut overrides: Vec<serde_json::Value> = Vec::new();

    let fastly_verify = serde_json::json!([
        "www.python.org",
        "pypi.org",
        "fastly.com",
        "www.fastly.com",
        "developer.fastly.com",
        "githubassets.com",
        "github.com",
        "github.io",
        "githubusercontent.com"
    ]);
    overrides.push(serde_json::json!({
        "OverrideID": "fastly-provider",
        "MatchFrontingProviderIDRegexes": ["(?i)fastly"],
        "DialAddresses": ["pypi.org"],
        "SNIServerName": "pypi.org",
        "VerifyServerNames": fastly_verify,
        "ALPNProtocols": ["h2", "http/1.1"],
        "TLSProfile": "Chrome-83",
    }));
    overrides.push(serde_json::json!({
        "OverrideID": "fastly-address",
        "MatchDialAddressRegexes": ["(?i)(fastly|pypi|python|github)"],
        "DialAddresses": ["pypi.org"],
        "SNIServerName": "pypi.org",
        "VerifyServerNames": fastly_verify,
        "ALPNProtocols": ["h2", "http/1.1"],
        "TLSProfile": "Chrome-83",
    }));

    let mut forced: Vec<String> = Vec::new();
    let mut sni_index = 0usize;
    for (i, ip) in REFERENCE_DIAL_EDGES.iter().enumerate() {
        forced.push(ip.to_string());
        overrides.push(fronting_edge_override(
            &format!("edge-ref-{i}"),
            ip,
            &edge_sni_for(sni_index),
        ));
        sni_index += 1;
    }
    for (i, ip) in IR_PROVEN_CDN_IPS.iter().enumerate() {
        if forced.contains(&ip.to_string()) {
            continue;
        }
        forced.push(ip.to_string());
        overrides.push(fronting_edge_override(
            &format!("edge-cur-{i}"),
            ip,
            &edge_sni_for(sni_index),
        ));
        sni_index += 1;
    }

    map.insert(
        "FrontedMeekDialOverrides".into(),
        serde_json::json!(overrides),
    );
    map.insert(
        "FrontedMeekDialOverridesProbability".into(),
        serde_json::json!(1.0),
    );
}

fn read_base_config() -> Result<Option<serde_json::Value>> {
    let path = match std::env::var("AETHER_PSIPHON_CONFIG")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        Some(path) => path,
        None => return Ok(None),
    };

    let text = std::fs::read_to_string(&path)
        .map_err(|e| AetherError::Other(format!("psiphon config {path}: {e}")))?;

    let parsed: serde_json::Value =
        json5_ish(&text).map_err(|e| AetherError::Other(format!("psiphon config {path}: {e}")))?;

    if !parsed.is_object() {
        return Err(AetherError::Other(format!(
            "psiphon config {path} is not a json object"
        )));
    }

    log::info!("[*] psiphon settings read from {path}");
    Ok(Some(parsed))
}

fn json5_ish(text: &str) -> std::result::Result<serde_json::Value, serde_json::Error> {
    let mut cleaned = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(c) = chars.next() {
        if in_string {
            cleaned.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }

        match c {
            '"' => {
                in_string = true;
                cleaned.push(c);
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut last = '\0';
                for n in chars.by_ref() {
                    if last == '*' && n == '/' {
                        break;
                    }
                    last = n;
                }
                cleaned.push(' ');
            }
            '/' if chars.peek() == Some(&'/') => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        break;
                    }
                }
                cleaned.push('\n');
            }
            _ => cleaned.push(c),
        }
    }

    serde_json::from_str(&cleaned)
}

fn listen_interface(socks: SocketAddr) -> Option<String> {
    if let Some(name) = std::env::var("AETHER_PSIPHON_INTERFACE")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        return Some(name);
    }

    let ip = socks.ip();
    if ip.is_loopback() {
        return None;
    }
    if ip.is_unspecified() {
        return Some("any".to_string());
    }

    log::warn!(
        "[-] psiphon names an interface, not an address, so it cannot be told to listen on {ip} \
         alone; it will listen on loopback. set AETHER_PSIPHON_INTERFACE to the interface name, \
         or bind to 0.0.0.0 to have it listen everywhere"
    );
    None
}

fn effective_ip(socks: SocketAddr) -> std::net::IpAddr {
    match listen_interface(socks) {
        Some(name) if name == "any" => std::net::Ipv4Addr::UNSPECIFIED.into(),
        Some(_) => socks.ip(),
        None => std::net::Ipv4Addr::LOCALHOST.into(),
    }
}

fn build_config(
    state: &Path,
    socks: SocketAddr,
    http: Option<SocketAddr>,
    upstream: Option<SocketAddr>,
    chosen: Shape,
) -> Result<String> {
    let mut map = serde_json::Map::new();

    map.insert(
        "PropagationChannelId".into(),
        serde_json::Value::from(env_or(
            "AETHER_PSIPHON_PROPAGATION_CHANNEL_ID",
            PROPAGATION_CHANNEL_ID,
        )),
    );
    map.insert(
        "SponsorId".into(),
        serde_json::Value::from(env_or("AETHER_PSIPHON_SPONSOR_ID", SPONSOR_ID)),
    );
    map.insert(
        "ClientPlatform".into(),
        serde_json::Value::from(client_platform()),
    );
    map.insert("ClientVersion".into(), serde_json::Value::from("1"));

    let list_url = env_or("AETHER_PSIPHON_SERVER_LIST", SERVER_LIST_URL);
    map.insert(
        "RemoteServerListURLs".into(),
        serde_json::json!([{ "URL": BASE64.encode(list_url.as_bytes()) }]),
    );
    map.insert(
        "RemoteServerListSignaturePublicKey".into(),
        serde_json::Value::from(SERVER_LIST_SIGNATURE_KEY),
    );

    // Indistinguishable TLS for direct dials, and the datastore migration the
    // reference clients set — without it every start logs boltdb migration
    // warnings against an already-migrated store.
    map.insert("UseIndistinguishableTLS".into(), serde_json::Value::from(true));
    map.insert(
        "MigrateDataStoreDirectory".into(),
        serde_json::Value::from(state.to_string_lossy().to_string()),
    );

    put_cdn_fronting(&mut map);

    match (upstream.is_some(), chosen) {
        (true, _) => {
            // Chained over a socks upstream: in-proxy's WebRTC/UDP cannot cross
            // it, so pin the probabilities to zero here — and only here. The
            // reference clients never zero them unconditionally; on this
            // network the in-proxy (WebRTC/Conduit) path is what connects
            // first without a VPN, and zeroing it globally was blocking that
            // route entirely.
            map.insert(
                "InproxyTunnelProtocolPreferProbability".into(),
                serde_json::json!(0.0),
            );
            map.insert(
                "InproxyTunnelProtocolSelectionProbability".into(),
                serde_json::json!(0.0),
            );
            map.insert(
                "LimitTunnelProtocols".into(),
                serde_json::json!(chained_protocols(chosen)),
            );
            if chosen != Shape::Auto {
                map.insert("DisableTactics".into(), serde_json::Value::from(true));
            }
        }
        (false, Shape::Auto) => {
            // No protocol limit at all: psiphon picks, including the in-proxy
            // WebRTC protocols and tactics-sourced broker specs — the exact
            // default path Se7en/ShirOKhorshid connect with first. Our old
            // NON_INPROXY pin here made "auto" a direct-only mode on a
            // network where direct server IPs are blocked.
        }
        (false, Shape::Cdn) => {
            map.insert(
                "LimitTunnelProtocols".into(),
                serde_json::json!(CDN_PROTOCOLS),
            );
            map.insert("DisableTactics".into(), serde_json::Value::from(true));
        }
        (false, Shape::Direct) => {
            map.insert(
                "LimitTunnelProtocols".into(),
                serde_json::json!(NON_INPROXY_PROTOCOLS),
            );
            map.insert("DisableTactics".into(), serde_json::Value::from(true));
        }
    }

    // Standalone (no socks upstream): hammer establishment like the reference
    // client's beast mode — 30+ workers racing servers instead of politely
    // walking the list.
    if upstream.is_none() {
        map.insert("AggressiveEstablishment".into(), serde_json::Value::from(true));
    }

    if let Some(given) = read_base_config()? {
        for (key, value) in given.as_object().expect("checked to be an object") {
            map.insert(key.clone(), value.clone());
        }
    }

    map.insert(
        "DataRootDirectory".into(),
        serde_json::Value::from(state.to_string_lossy().to_string()),
    );
    map.insert(
        "LocalSocksProxyPort".into(),
        serde_json::Value::from(socks.port()),
    );
    map.insert(
        "EmitDiagnosticNotices".into(),
        serde_json::Value::from(true),
    );
    map.insert(
        "EmitDiagnosticNetworkParameters".into(),
        serde_json::Value::from(true),
    );
    map.insert("EmitServerAlerts".into(), serde_json::Value::from(true));
    map.insert(
        "EmitBytesTransferred".into(),
        serde_json::Value::from(false),
    );

    if let Some(name) = listen_interface(socks) {
        map.insert("ListenInterface".into(), serde_json::Value::from(name));
    }

    match http {
        Some(address) => {
            map.insert(
                "LocalHttpProxyPort".into(),
                serde_json::Value::from(address.port()),
            );
            map.insert(
                "DisableLocalHTTPProxy".into(),
                serde_json::Value::from(false),
            );
        }
        None => {
            map.insert(
                "DisableLocalHTTPProxy".into(),
                serde_json::Value::from(true),
            );
        }
    }

    if let Some(code) = region() {
        log::info!("[*] psiphon is asked to leave from {code}");
        map.insert("EgressRegion".into(), serde_json::Value::from(code));
    }

    if let Some(through) = upstream {
        map.insert(
            "UpstreamProxyURL".into(),
            serde_json::Value::from(format!("socks5://{through}")),
        );
    }

    log::info!(
        "[*] psiphon shape: {chosen:?}{}",
        if upstream.is_some() {
            ", carried by the tunnel so only tcp protocols are offered"
        } else {
            ""
        }
    );

    serde_json::to_string(&serde_json::Value::Object(map))
        .map_err(|e| AetherError::Other(format!("psiphon config could not be written: {e}")))
}

#[derive(Deserialize)]
struct Notice {
    #[serde(rename = "noticeType", default)]
    kind: String,
    #[serde(default)]
    data: serde_json::Value,
}

pub struct Running {
    child: Child,
    pub socks: SocketAddr,
    pub http: Option<SocketAddr>,
}

impl Running {
    pub async fn wait(&mut self) -> Result<()> {
        let status = self
            .child
            .wait()
            .await
            .map_err(|e| AetherError::Other(format!("psiphon: {e}")))?;
        Err(AetherError::Other(format!("psiphon stopped: {status}")))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

fn note(kind: &str, data: &serde_json::Value) {
    let message = data
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or_default();

    match kind {
        "ConnectedServer" => {
            if let Some(address) = data.get("ipAddress").and_then(|v| v.as_str()) {
                log::info!("[+] psiphon reached a server at {address}");
            }
        }
        "AvailableEgressRegions" => {
            if let Some(regions) = data.get("regions").and_then(|v| v.as_array()) {
                let named: Vec<&str> = regions.iter().filter_map(|r| r.as_str()).collect();
                if !named.is_empty() {
                    log::info!("[*] psiphon can leave from: {}", named.join(" "));
                }
            }
        }
        "ConnectingServer" | "ActiveTunnel" | "Homepage" => {
            log::debug!("[psiphon] {kind} {data}");
        }
        "Warning" | "Alert" | "Error" | "InternalError" => {
            let text = pick(message, data);
            if expected_without_inproxy(&text) {
                log::debug!("[psiphon] {text}");
            } else if kind == "Error" || kind == "InternalError" {
                log::error!("[-] psiphon: {text}");
            } else {
                log::warn!("[-] psiphon: {text}");
            }
        }
        _ => log::debug!("[psiphon] {kind} {data}"),
    }
}

/// Materialize the bundled server list into hex server-entry lines and return a
/// path psiphon can be started with `-serverList` on.
///
/// The Psiphon distribution channel serves `server_list_compressed`: a zlib
/// stream of a signed JSON object whose `data` field is newline-separated hex
/// server entries. That wrapper is not what the tunnel core imports — feeding it
/// the wrapper fails with `encoding/hex: invalid byte: U+0078 'x'` and the client
/// is left with zero servers, unable to connect on any network that also blocks
/// the S3 download. The `data` lines themselves are exactly what `-serverList`
/// wants (verified: 428 entries import), so whichever form the file arrives in,
/// the lines are unwrapped here and written into the state directory for the
/// child process. Decoding the hex into plaintext would be wrong: it fuses the
/// lines together and psiphon then sees one giant malformed entry.
fn server_list_file(state: &Path) -> Option<PathBuf> {
    // The file is searched in three places because they drift apart in
    // practice: the state directory the engine was pointed at, the folder
    // beside the engine binary (where the release zip seeds it), and the
    // AppData identity mirror that survives re-extractions. A list found
    // outside the state directory is copied in, so psiphon's own datastore
    // reader finds it too.
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(given) = server_list_path(state) {
        candidates.push(given);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(
                dir.join("aether.toml-psiphon")
                    .join(SERVER_LIST_SUBDIR)
                    .join("remote_server_list"),
            );
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        candidates.push(
            PathBuf::from(appdata)
                .join("Aether")
                .join("aether.toml-psiphon")
                .join(SERVER_LIST_SUBDIR)
                .join("remote_server_list"),
        );
    }

    let Some(path) = candidates.into_iter().find(|p| p.is_file()) else {
        log::warn!(
            "[-] psiphon bundled server list not found (searched state dir, beside the engine binary, and AppData); without it psiphon depends on the blocked S3 download"
        );
        return None;
    };

    let bytes = std::fs::read(&path).ok()?;
    let entries = decode_server_list(&bytes)?;

    // Let psiphon's own importer see the same file.
    let native = state.join(SERVER_LIST_SUBDIR).join("remote_server_list");
    if path != native {
        let _ = std::fs::create_dir_all(native.parent().unwrap_or(state));
        if !native.exists() {
            let _ = std::fs::copy(&path, &native);
        }
    }

    let out = state.join("server-entries.txt");
    std::fs::write(&out, &entries).ok()?;

    let count = entries.lines().filter(|line| !line.trim().is_empty()).count();
    log::info!(
        "[+] psiphon bundled server list: {count} entries from {}",
        path.display()
    );
    Some(out)
}

const SERVER_LIST_SUBDIR: &str = "ca.psiphon.PsiphonTunnel.tunnel-core";

#[cfg(test)]
mod server_list_tests {
    use super::decode_server_list;

    #[test]
    fn compressed_signed_wrapper_keeps_its_hex_lines_separate() {
        // The distribution channel serves exactly this shape: a zlib stream of a
        // signed JSON object whose `data` is newline-separated hex entries. The
        // lines are what `-serverList` imports (428 of them in the real file).
        // Decoding the hex or stripping its newlines fuses every entry into one
        // line, which the parser then rejects as a single malformed entry — the
        // exact "1 entries" failure this must not regress to.
        let line_one = "30203020302030207b7d";
        let line_two = "30203020302030207b7d";
        let data = format!("{line_one}\n{line_two}\n{line_one}");
        let wrapper = format!(
            "{{\"data\":{},\"signature\":\"AAAA\",\"signingPublicKeyDigest\":\"BBBB\"}}",
            serde_json::to_string(&data).unwrap()
        );

        let mut compressed = Vec::new();
        {
            let mut encoder =
                flate2::write::ZlibEncoder::new(&mut compressed, flate2::Compression::default());
            std::io::Write::write_all(&mut encoder, wrapper.as_bytes()).unwrap();
            encoder.finish().unwrap();
        }
        assert_eq!(compressed[0], 0x78, "the channel compresses with a zlib header");

        let decoded = decode_server_list(&compressed).expect("wrapper should decode");
        assert_eq!(decoded, data, "hex lines pass through verbatim");
        assert_eq!(decoded.lines().count(), 3, "lines stay separate");
        assert!(!decoded.contains("signingPublicKeyDigest"));
    }

    #[test]
    fn raw_entries_pass_through_unchanged() {
        let raw = b"30203020302030207b7d\n30203020302030207b7d\n";
        let decoded = decode_server_list(raw).expect("raw entries should decode");
        assert_eq!(decoded, "30203020302030207b7d\n30203020302030207b7d");
    }

    #[test]
    fn wrapper_without_a_data_field_is_rejected() {
        let wrapper = br#"{"signature":"AAAA"}"#;
        assert!(decode_server_list(wrapper).is_none());
    }
}

fn server_list_path(state: &Path) -> Option<PathBuf> {
    let given = std::env::var("AETHER_PSIPHON_SERVER_LIST_FILE")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);

    let bundled = state.join(SERVER_LIST_SUBDIR).join("remote_server_list");

    [given, Some(bundled)]
        .into_iter()
        .flatten()
        .find(|path| path.is_file())
}

fn decode_server_list(bytes: &[u8]) -> Option<String> {
    let text = inflate_if_compressed(bytes);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        log::warn!("[-] psiphon server list: empty after inflate");
        return None;
    }

    // The signed wrapper is one JSON object; the raw entries are hex lines. A
    // file that is already raw entries passes through untouched.
    let wrapper: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(value) => value,
        Err(_) => {
            let count = trimmed.lines().filter(|l| !l.trim().is_empty()).count();
            log::info!("[*] psiphon server list: raw entries format ({count} lines)");
            return Some(trimmed.to_string());
        }
    };

    let Some(data) = wrapper.get("data").and_then(|v| v.as_str()) else {
        log::warn!("[-] psiphon server list wrapper has no 'data' field");
        return None;
    };

    // The data field is already the newline-separated hex lines `-serverList`
    // consumes. It must not be hex-decoded into plaintext: the newlines are
    // separators between hex lines, not encoded data, and stripping them fuses
    // all 428 entries into one line the parser rejects.
    let entries = data.trim();
    let count = entries.lines().filter(|line| !line.trim().is_empty()).count();
    if count == 0 {
        log::warn!("[-] psiphon server list: wrapper 'data' field is empty");
        return None;
    }
    log::info!("[+] psiphon server list: unwrap OK, {count} entries from signed wrapper");
    Some(entries.to_string())
}

fn inflate_if_compressed(bytes: &[u8]) -> String {
    // 0x78 0x9C is the zlib header the distribution channel compresses with.
    if bytes.len() > 2 && bytes[0] == 0x78 && bytes[1] == 0x9c {
        let mut decoder = flate2::read::ZlibDecoder::new(bytes);
        let mut inflated = String::new();
        if std::io::Read::read_to_string(&mut decoder, &mut inflated).is_ok() && !inflated.is_empty()
        {
            return inflated;
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn expected_without_inproxy(text: &str) -> bool {
    text.contains("DSL fetch failed")
        || text.contains("no broker specs")
        || text.contains("tactics request aborted: no capable servers")
}

fn pick<'a>(message: &'a str, data: &'a serde_json::Value) -> String {
    if message.is_empty() {
        data.to_string()
    } else {
        message.to_string()
    }
}

pub async fn start(
    state: &Path,
    socks: SocketAddr,
    http: Option<SocketAddr>,
    upstream: Option<SocketAddr>,
) -> Result<Running> {
    let chosen = shape();

    // CDN fronting only works when a reachable edge of the *same* CDN that
    // hosts each server's fronting domain answers: the dial is an IP, so the
    // SNI comes out empty and the Host header carries the server's Akamai (or
    // CloudFront) domain. On a network where those edges are filtered, every
    // attempt dies — silently as dial timeouts, or with 403s from whatever
    // other CDN edges do answer. Give the fronting attempt a shorter budget,
    // then fall back to the auto shape once so unfronted protocols (OSSH,
    // TLS-OSSH, ...) still get their turn instead of failing the whole tunnel.
    if chosen == Shape::Cdn {
        let cdn_budget = ready_timeout().min(Duration::from_secs(80));
        match start_inner(state, socks, http, upstream, chosen, cdn_budget).await {
            Ok(running) => return Ok(running),
            Err(e) => {
                log::warn!(
                    "[-] cdn fronting did not come up on this network ({e}); retrying once with the auto shape so direct protocols get a chance"
                );
            }
        }
        return start_inner(state, socks, http, upstream, Shape::Auto, ready_timeout()).await;
    }

    start_inner(state, socks, http, upstream, chosen, ready_timeout()).await
}

async fn start_inner(
    state: &Path,
    socks: SocketAddr,
    http: Option<SocketAddr>,
    upstream: Option<SocketAddr>,
    chosen: Shape,
    timeout: Duration,
) -> Result<Running> {
    let exe = binary().ok_or_else(|| AetherError::Other(install_hint()))?;

    std::fs::create_dir_all(state)
        .map_err(|e| AetherError::Other(format!("psiphon state dir {}: {e}", state.display())))?;

    let config = build_config(state, socks, http, upstream, chosen)?;
    let config_path = state.join("aether-psiphon.json");
    std::fs::write(&config_path, config)
        .map_err(|e| AetherError::Other(format!("psiphon config could not be saved: {e}")))?;

    log::info!("[*] starting psiphon from {}", exe.display());

    let mut command = Command::new(&exe);
    command
        .arg("-config")
        .arg(&config_path)
        .arg("-dataRootDirectory")
        .arg(state);
    // Ship the server list in through the flag that is known to import it, so
    // psiphon comes up with real servers even when the S3 distribution channel
    // is blocked and it can never refresh the list itself.
    if let Some(list) = server_list_file(state) {
        command.arg("-serverList").arg(list);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| AetherError::Other(format!("psiphon would not start: {e}")))?;

    let notices = child
        .stderr
        .take()
        .ok_or_else(|| AetherError::Other("psiphon gave no notice stream".into()))?;

    if let Some(out) = child.stdout.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                log::debug!("[psiphon] {line}");
            }
        });
    }

    let mut lines = BufReader::new(notices).lines();
    let mut socks_port: Option<u16> = None;
    let mut http_port: Option<u16> = None;
    let mut tunnelled = false;

    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let line = match tokio::time::timeout_at(deadline, lines.next_line()).await {
            Err(_) => {
                return Err(AetherError::Other(
                    "psiphon did not come up in time; it may be blocked on this network".into(),
                ))
            }
            Ok(Err(e)) => return Err(AetherError::Other(format!("psiphon: {e}"))),
            Ok(Ok(None)) => {
                return Err(AetherError::Other(
                    "psiphon stopped before it was ready".into(),
                ))
            }
            Ok(Ok(Some(line))) => line,
        };

        let notice: Notice = match serde_json::from_str(&line) {
            Ok(notice) => notice,
            Err(_) => {
                log::debug!("[psiphon] {line}");
                continue;
            }
        };

        match notice.kind.as_str() {
            "ListeningSocksProxyPort" => {
                socks_port = notice
                    .data
                    .get("port")
                    .and_then(|p| p.as_u64())
                    .map(|p| p as u16);
            }
            "ListeningHttpProxyPort" => {
                http_port = notice
                    .data
                    .get("port")
                    .and_then(|p| p.as_u64())
                    .map(|p| p as u16);
            }
            "Tunnels" => {
                let count = notice
                    .data
                    .get("count")
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0);
                if count > 0 {
                    tunnelled = true;
                } else if tunnelled {
                    log::warn!("[-] psiphon lost its tunnel; it is reconnecting");
                    tunnelled = false;
                }
            }
            other => note(other, &notice.data),
        }

        if tunnelled && socks_port.is_some() {
            break;
        }
    }

    let bound = effective_ip(socks);
    let socks = SocketAddr::new(bound, socks_port.unwrap_or(socks.port()));
    let http = http.map(|address| SocketAddr::new(bound, http_port.unwrap_or(address.port())));

    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            match serde_json::from_str::<Notice>(&line) {
                Ok(notice) => note(&notice.kind, &notice.data),
                Err(_) => log::debug!("[psiphon] {line}"),
            }
        }
    });

    Ok(Running { child, socks, http })
}

fn announce(proxy: SocketAddr, what: &'static str) {
    tokio::spawn(async move {
        crate::exitloc::report_through_socks(proxy, what).await;
    });
}

pub async fn run_only(listen: SocketAddr, state: PathBuf) -> Result<()> {
    let http = http_listen_address();
    log::info!("[*] starting psiphon with no tunnel underneath it");

    let mut running = start(&state, listen, http, None).await?;
    log::info!(
        "[+] psiphon is ready; {} leaves through psiphon",
        running.socks
    );
    if let Some(address) = running.http {
        log::info!("[+] psiphon http proxy on {address}");
    }
    announce(running.socks, "psiphon");

    running.wait().await
}

pub async fn run_chain(through: SocketAddr, state: PathBuf) -> Result<()> {
    let listen = listen_address();
    let http = http_listen_address();

    wait_for_proxy(through).await;
    log::info!("[*] starting psiphon through the tunnel at {through}");

    let mut running = start(&state, listen, http, Some(through)).await?;
    log::info!(
        "[+] psiphon is ready; {} leaves through psiphon, carried by the tunnel",
        running.socks
    );
    if let Some(address) = running.http {
        log::info!("[+] psiphon http proxy on {address}");
    }
    announce(running.socks, "psiphon through the tunnel");

    running.wait().await
}

pub async fn start_reverse(state: PathBuf) -> Result<SocketAddr> {
    let listen = listen_address();
    let http = http_listen_address();
    log::info!("[*] starting psiphon; the tunnel will be dialled through it");

    let mut running = start(&state, listen, http, None).await?;
    let socks = running.socks;
    log::info!("[+] psiphon is ready; the tunnel goes out through {socks}");
    announce(socks, "psiphon");

    tokio::spawn(async move {
        if let Err(e) = running.wait().await {
            log::error!("[-] psiphon: {e}");
        }
    });

    Ok(socks)
}

async fn wait_for_proxy(through: SocketAddr) {
    let mut announced = false;
    loop {
        if tokio::net::TcpStream::connect(through).await.is_ok() {
            return;
        }
        if !announced {
            log::info!("[*] psiphon is waiting for the tunnel on {through}");
            announced = true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn hold() -> std::sync::MutexGuard<'static, ()> {
        ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn clear() {
        for name in [
            "AETHER_PSIPHON",
            "AETHER_PSIPHON_BIND",
            "AETHER_PSIPHON_HTTP",
            "AETHER_PSIPHON_REGION",
            "AETHER_PSIPHON_DIR",
            "AETHER_PSIPHON_INTERFACE",
            "AETHER_PSIPHON_MODE",
            "AETHER_PSIPHON_CDN_IPS",
            "AETHER_PSIPHON_CDN_SNI",
        ] {
            std::env::remove_var(name);
        }
    }

    #[test]
    fn every_mode_has_a_spelling() {
        let _held = hold();

        clear();
        assert_eq!(mode(), Mode::Off);
        for written in ["1", "on", "chain", "yes"] {
            std::env::set_var("AETHER_PSIPHON", written);
            assert_eq!(mode(), Mode::Chain, "{written}");
        }
        for written in ["reverse", "rev", "warp-over-psiphon"] {
            std::env::set_var("AETHER_PSIPHON", written);
            assert_eq!(mode(), Mode::Reverse, "{written}");
        }
        for written in ["only", "alone", "psiphon-only", "direct"] {
            std::env::set_var("AETHER_PSIPHON", written);
            assert_eq!(mode(), Mode::Only, "{written}");
        }
        for written in ["off", "0", "no", "false"] {
            std::env::set_var("AETHER_PSIPHON", written);
            assert_eq!(mode(), Mode::Off, "{written}");
        }

        clear();
    }

    #[test]
    fn the_state_dir_sits_beside_the_identity_file() {
        let _held = hold();

        clear();
        assert_eq!(state_dir("/tmp/id"), PathBuf::from("/tmp/id-psiphon"));
        std::env::set_var("AETHER_PSIPHON_DIR", "/var/psi");
        assert_eq!(state_dir("/tmp/id"), PathBuf::from("/var/psi"));

        clear();
    }

    #[test]
    fn only_a_real_country_code_is_passed_on() {
        let _held = hold();

        clear();
        assert!(region().is_none());
        std::env::set_var("AETHER_PSIPHON_REGION", "de");
        assert_eq!(region().as_deref(), Some("DE"));
        std::env::set_var("AETHER_PSIPHON_REGION", "germany");
        assert!(region().is_none());

        clear();
    }

    #[test]
    fn a_config_with_comments_still_parses() {
        let _held = hold();
        let text = r#"
        /* psiphon ships these with comments */
        {
            "PropagationChannelId": "AAAA", // inline
            "SponsorId": "BBBB"
        }
        "#;
        let parsed = json5_ish(text).expect("it should parse");
        assert_eq!(parsed["PropagationChannelId"], "AAAA");
        assert_eq!(parsed["SponsorId"], "BBBB");
    }

    #[test]
    fn a_url_inside_a_string_is_not_taken_for_a_comment() {
        let _held = hold();
        let text = r#"{"RemoteServerListURLs": "https://example.com/list"}"#;
        let parsed = json5_ish(text).expect("it should parse");
        assert_eq!(parsed["RemoteServerListURLs"], "https://example.com/list");
    }

    #[test]
    fn the_built_in_config_can_stand_on_its_own() {
        let _held = hold();

        clear();
        let dir = std::env::temp_dir();
        let text = build_config(
            &dir,
            "127.0.0.1:1821".parse().expect("an address"),
            None,
            None,
            shape(),
        )
        .expect("a config");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");

        assert_eq!(parsed["PropagationChannelId"], PROPAGATION_CHANNEL_ID);
        assert_eq!(parsed["SponsorId"], SPONSOR_ID);
        assert!(parsed["RemoteServerListSignaturePublicKey"]
            .as_str()
            .is_some_and(|key| key.len() > 100));
        assert_eq!(parsed["LocalSocksProxyPort"], 1821);
        assert_eq!(parsed["DisableLocalHTTPProxy"], true);

        clear();
    }

    #[test]
    fn the_server_list_url_travels_base64_encoded() {
        let _held = hold();

        clear();
        let dir = std::env::temp_dir();
        let text = build_config(
            &dir,
            "127.0.0.1:1821".parse().expect("an address"),
            None,
            None,
            shape(),
        )
        .expect("a config");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        let encoded = parsed["RemoteServerListURLs"][0]["URL"]
            .as_str()
            .expect("a url");
        let decoded = BASE64.decode(encoded).expect("it decodes");
        assert_eq!(String::from_utf8(decoded).expect("utf8"), SERVER_LIST_URL);

        clear();
    }

    #[test]
    fn a_carried_psiphon_is_offered_no_udp_protocol() {
        let _held = hold();

        clear();
        let dir = std::env::temp_dir();
        let text = build_config(
            &dir,
            "127.0.0.1:1821".parse().expect("an address"),
            None,
            Some("127.0.0.1:1819".parse().expect("an address")),
            shape(),
        )
        .expect("a config");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");

        assert_eq!(parsed["UpstreamProxyURL"], "socks5://127.0.0.1:1819");
        let offered = parsed["LimitTunnelProtocols"]
            .as_array()
            .expect("a protocol list");
        assert!(!offered.is_empty());
        for name in offered {
            let name = name.as_str().expect("a name");
            assert!(
                !name.contains("QUIC") && !name.starts_with("INPROXY"),
                "{name} carries udp, which cannot cross a socks5 hop"
            );
        }

        clear();
    }

    #[test]
    fn a_config_of_your_own_is_laid_over_the_built_in_one() {
        let _held = hold();

        clear();
        let dir = std::env::temp_dir();
        let path = dir.join(format!("aether-psi-{}.json", std::process::id()));
        std::fs::write(&path, r#"{"SponsorId": "MINE"}"#).expect("write");
        std::env::set_var("AETHER_PSIPHON_CONFIG", &path);

        let text = build_config(
            &dir,
            "127.0.0.1:1821".parse().expect("an address"),
            None,
            None,
            shape(),
        )
        .expect("a config");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(parsed["SponsorId"], "MINE");
        assert_eq!(parsed["PropagationChannelId"], PROPAGATION_CHANNEL_ID);

        std::env::remove_var("AETHER_PSIPHON_CONFIG");
        let _ = std::fs::remove_file(&path);

        clear();
    }

    #[test]
    fn cdn_fronting_carries_the_reference_client_dial_overrides() {
        let _held = hold();

        clear();
        std::env::set_var("AETHER_PSIPHON_MODE", "cdn");
        std::env::set_var("AETHER_PSIPHON_CDN_IPS", "9.9.9.9");
        // An Azure scan SNI saved by the user: the forced Akamai edge dials
        // must NOT inherit it — that pairing is what produced 403 Forbidden
        // on every attempt (verified against a live Akamai edge).
        std::env::set_var("AETHER_PSIPHON_CDN_SNI", "ajax.aspnetcdn.com");

        let dir = std::env::temp_dir();
        let text = build_config(
            &dir,
            "127.0.0.1:1821".parse().expect("an address"),
            None,
            None,
            Shape::Cdn,
        )
        .expect("a config");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");

        // The reference client (ShirOKhorshid) never suppresses psiphon's own
        // edge spec, forces every fronted dial through overrides at full
        // probability, and includes a Fastly path — all three must be present
        // or custom edges degrade back to bare IPs that 403.
        assert_eq!(parsed["FrontedMeekCDNScanUseBuiltInSpec"], true);
        assert_eq!(parsed["FrontedMeekDialOverridesProbability"], 1.0);

        let overrides = parsed["FrontedMeekDialOverrides"]
            .as_array()
            .expect("overrides array");
        assert!(overrides.len() >= 10, "reference edges plus ours");

        let ids: Vec<&str> = overrides
            .iter()
            .filter_map(|o| o["OverrideID"].as_str())
            .collect();
        assert!(ids.contains(&"fastly-provider"), "Fastly path present");
        assert!(ids.contains(&"fastly-address"), "Fastly path present");

        let first_edge = overrides
            .iter()
            .find(|o| o["OverrideID"].as_str() == Some("edge-ref-0"))
            .expect("first reference edge");
        assert_eq!(first_edge["DialAddresses"][0], "23.215.0.206");
        // The user's Azure SNI must be ignored for edge dials; the pool starts
        // at the Akamai fronting name the psiphon Host routes under.
        assert_eq!(first_edge["SNIServerName"], "a248.e.akamai.net");

        // And the pool rotates: the second edge must present a *different*
        // Akamai-family name, because the reference client's winning pair on
        // this network was a.akamaized-staging.net — a single SNI forced onto
        // every edge leaves whichever name is broken blocking all of them.
        let second_edge = overrides
            .iter()
            .find(|o| o["OverrideID"].as_str() == Some("edge-ref-1"))
            .expect("second reference edge");
        assert_eq!(second_edge["SNIServerName"], "a77.net.akamai.net");
        assert_ne!(
            first_edge["SNIServerName"], second_edge["SNIServerName"],
            "edge SNIs must rotate across the Akamai pool"
        );
        assert!(first_edge["VerifyServerNames"]
            .as_array()
            .expect("verify list")
            .iter()
            .any(|v| v == "www.akamai.com"));

        // The user's scanned IPs reach the scan spec; the forced overrides
        // stay on CDN edges whose certificates can actually verify.
        assert_eq!(parsed["FrontedMeekCDNScanSpec"]["IPCandidates"][0], "9.9.9.9");

        std::env::remove_var("AETHER_PSIPHON_MODE");
        std::env::remove_var("AETHER_PSIPHON_CDN_IPS");
        std::env::remove_var("AETHER_PSIPHON_CDN_SNI");
        clear();
    }

    #[test]
    fn standalone_auto_leaves_the_protocol_choice_to_psiphon() {
        let _held = hold();

        clear();
        let dir = std::env::temp_dir();
        let text = build_config(
            &dir,
            "127.0.0.1:1821".parse().expect("an address"),
            None,
            None,
            Shape::Auto,
        )
        .expect("a config");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");

        // The path the reference clients connect with first on this network:
        // no LimitTunnelProtocols pin, in-proxy probabilities left at psiphon's
        // defaults (so WebRTC/Conduit and tactics broker specs are in play),
        // and aggressive establishment.
        assert!(
            parsed.get("LimitTunnelProtocols").is_none(),
            "standalone auto must not pin the protocol list"
        );
        assert!(
            parsed.get("InproxyTunnelProtocolSelectionProbability").is_none(),
            "standalone auto must not zero in-proxy"
        );
        assert_eq!(parsed.get("DisableTactics"), None);
        assert_eq!(parsed["AggressiveEstablishment"], true);

        clear();
    }

    #[test]
    fn chained_over_upstream_still_zeroes_in_proxy() {
        let _held = hold();

        clear();
        let dir = std::env::temp_dir();
        let text = build_config(
            &dir,
            "127.0.0.1:1821".parse().expect("an address"),
            None,
            Some("127.0.0.1:1819".parse().expect("an address")),
            Shape::Auto,
        )
        .expect("a config");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");

        // WebRTC/UDP cannot cross a socks5 hop, so the chained case keeps the
        // zero — it is the only case that gets it.
        assert_eq!(parsed["InproxyTunnelProtocolSelectionProbability"], 0.0);
        assert!(parsed.get("LimitTunnelProtocols").is_some());

        clear();
    }

    #[test]
    fn the_cdn_shape_only_offers_fronted_meek() {
        let _held = hold();

        clear();
        std::env::set_var("AETHER_PSIPHON_MODE", "cdn");
        assert_eq!(shape(), Shape::Cdn);
        for name in chained_protocols(Shape::Cdn) {
            assert!(name.starts_with("FRONTED-MEEK-CDN"));
            assert!(!name.contains("QUIC"));
        }
        std::env::remove_var("AETHER_PSIPHON_MODE");

        clear();
    }

    #[test]
    fn loopback_needs_no_interface_name() {
        let _held = hold();

        clear();
        assert!(listen_interface("127.0.0.1:1821".parse().expect("an address")).is_none());
        assert_eq!(
            effective_ip("127.0.0.1:1821".parse().expect("an address")),
            std::net::Ipv4Addr::LOCALHOST
        );

        clear();
    }

    #[test]
    fn binding_everywhere_is_psiphons_word_any() {
        let _held = hold();

        clear();
        assert_eq!(
            listen_interface("0.0.0.0:1821".parse().expect("an address")).as_deref(),
            Some("any")
        );
        assert!(effective_ip("0.0.0.0:1821".parse().expect("an address")).is_unspecified());

        clear();
    }

    #[test]
    fn a_named_interface_wins_over_the_address() {
        let _held = hold();

        clear();
        std::env::set_var("AETHER_PSIPHON_INTERFACE", "eth0");
        assert_eq!(
            listen_interface("127.0.0.1:1821".parse().expect("an address")).as_deref(),
            Some("eth0")
        );
        std::env::remove_var("AETHER_PSIPHON_INTERFACE");

        clear();
    }

    #[test]
    fn the_default_listener_is_its_own_port() {
        let _held = hold();

        clear();
        assert_eq!(listen_address().port(), 1821);
        assert!(http_listen_address().is_none());

        clear();
    }
}
