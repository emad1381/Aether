//! CDN fronting edge scanner.
//!
//! Finds edge addresses of the big CDNs (Akamai, Google, CloudFront, Azure) that
//! this machine can actually complete a TLS handshake with, so they can be handed
//! to psiphon as `FrontedMeekCDNScanSpec` candidates. The test is a real TLS
//! handshake against one of the CDN's own hostnames: a TCP connect alone proves
//! nothing, because a middlebox can accept the socket and then reset the handshake,
//! and because psiphon verifies the edge certificate against the name it presents.
//! An address that survives here is one psiphon can front through.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// How many addresses are probed at once. Each probe is one TLS handshake, so
/// this is bounded by what a residential uplink tolerates, not by CPU.
const CONCURRENCY: usize = 24;

/// A handshake that has not finished in this long is treated as filtered.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// DNS lookups for the ranges go through the system resolver and can stall on a
/// network that sinks blocked names, so each one is capped.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(4);

/// One CDN the scanner knows how to find edges for.
struct Cdn {
    name: &'static str,
    /// Hostnames that resolve into the CDN's edge space. Resolved at scan time
    /// rather than hardcoded, because edge addresses are anycast and move.
    domains: &'static [&'static str],
    /// The name presented in the TLS handshake. It has to be a name the edge
    /// actually holds a certificate for, or the handshake fails even though the
    /// address is reachable.
    sni: &'static str,
}

const CDNS: &[Cdn] = &[
    Cdn {
        name: "Akamai",
        domains: &[
            "www.apple.com",
            "www.adobe.com",
            "www.paypal.com",
            "www.bbc.com",
            "a248.e.akamai.net",
        ],
        sni: "a248.e.akamai.net",
    },
    Cdn {
        name: "Google",
        domains: &[
            "www.gstatic.com",
            "fonts.googleapis.com",
            "ajax.googleapis.com",
            "storage.googleapis.com",
            "accounts.google.com",
        ],
        sni: "www.gstatic.com",
    },
    Cdn {
        name: "CloudFront",
        domains: &[
            "d1.awsstatic.com",
            "aws.amazon.com",
            "d36cz9buwru1tt.cloudfront.net",
            "images-na.ssl-images-amazon.com",
        ],
        sni: "d1.awsstatic.com",
    },
    Cdn {
        name: "Azure",
        domains: &[
            "ajax.aspnetcdn.com",
            "az416426.vo.msecnd.net",
            "az784690.vo.msecnd.net",
            "cdn.office.net",
        ],
        sni: "ajax.aspnetcdn.com",
    },
];

/// One address the scanner tried.
#[derive(Debug, Clone, Serialize)]
pub struct CdnEdge {
    pub ip: String,
    pub cdn: String,
    pub sni: String,
    /// Round trip of the TLS handshake, when it completed.
    pub latency_ms: Option<u64>,
    pub reachable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CdnScanReport {
    pub edges: Vec<CdnEdge>,
    /// Reachable addresses, comma separated, ready for the psiphon CDN field.
    pub ips: String,
    /// The server names those addresses were validated against.
    pub sni: String,
    pub tested: usize,
    pub reachable: usize,
}

struct Probe {
    addr: SocketAddr,
    cdn: &'static str,
    sni: &'static str,
}

/// Scan every known CDN and report which edge addresses complete a TLS handshake
/// from this machine. `progress` is called with (completed, total) as probes
/// finish, so the caller can surface a live count.
pub async fn scan<F>(mut progress: F) -> CdnScanReport
where
    F: FnMut(usize, usize),
{
    let probes = gather_candidates().await;
    let total = probes.len();
    progress(0, total);

    let mut edges: Vec<CdnEdge> = Vec::with_capacity(total);
    let mut completed = 0usize;

    // buffer_unordered keeps `CONCURRENCY` handshakes in flight and yields each
    // as it settles, so a stalled probe never blocks the ones behind it.
    use futures::stream::{self, StreamExt};
    let mut inflight = stream::iter(probes.into_iter().map(probe_edge)).buffer_unordered(CONCURRENCY);

    while let Some(edge) = inflight.next().await {
        completed += 1;
        progress(completed, total);
        edges.push(edge);
    }

    edges.sort_by(|a, b| {
        b.reachable
            .cmp(&a.reachable)
            .then_with(|| a.cdn.cmp(&b.cdn))
            .then_with(|| a.latency_ms.unwrap_or(u64::MAX).cmp(&b.latency_ms.unwrap_or(u64::MAX)))
    });

    let reachable_edges: Vec<&CdnEdge> = edges.iter().filter(|e| e.reachable).collect();

    let mut ips: Vec<&str> = Vec::new();
    let mut names: Vec<&str> = Vec::new();
    for edge in &reachable_edges {
        if !ips.contains(&edge.ip.as_str()) {
            ips.push(&edge.ip);
        }
        if !names.contains(&edge.sni.as_str()) {
            names.push(&edge.sni);
        }
    }

    let reachable = reachable_edges.len();
    CdnScanReport {
        ips: ips.join(", "),
        sni: names.join(", "),
        tested: edges.len(),
        reachable,
        edges,
    }
}

async fn gather_candidates() -> Vec<Probe> {
    let mut probes = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    for cdn in CDNS {
        for domain in cdn.domains {
            let resolved = match tokio::time::timeout(RESOLVE_TIMEOUT, tokio::net::lookup_host((*domain, 443))).await {
                Ok(Ok(addrs)) => addrs,
                _ => continue,
            };
            for addr in resolved {
                // Fronting over IPv6 buys nothing here: the psiphon fronting spec
                // takes the addresses as dial targets and the CDNs answer both,
                // but an IPv6 failure mode is invisible in the result list.
                if !addr.is_ipv4() {
                    continue;
                }
                let ip = addr.ip().to_string();
                if seen.contains(&ip) {
                    continue;
                }
                seen.push(ip);
                probes.push(Probe {
                    addr,
                    cdn: cdn.name,
                    sni: cdn.sni,
                });
            }
        }
    }

    probes
}

async fn probe_edge(probe: Probe) -> CdnEdge {
    let started = Instant::now();
    let reachable = handshake(probe.addr, probe.sni).await;
    CdnEdge {
        ip: probe.addr.ip().to_string(),
        cdn: probe.cdn.to_string(),
        sni: probe.sni.to_string(),
        latency_ms: reachable.then(|| started.elapsed().as_millis() as u64),
        reachable,
    }
}

/// Complete a TLS 1.2+ handshake and read the ServerHello back. The certificate
/// is not verified — verification is the tunnel core's job, and it verifies
/// against the name it presents — but the ServerHello has to arrive and parse,
/// which is exactly what a filtered or sunk address fails to do.
async fn handshake(addr: SocketAddr, sni: &str) -> bool {
    let result = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake_inner(addr, sni)).await;
    matches!(result, Ok(Ok(())))
}

async fn handshake_inner(addr: SocketAddr, sni: &str) -> Result<(), ()> {
    let mut tcp = tokio::time::timeout(HANDSHAKE_TIMEOUT, tokio::net::TcpStream::connect(addr))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
    let _ = tcp.set_nodelay(true);

    tcp.write_all(&client_hello(sni)).await.map_err(|_| ())?;

    let mut buf = [0u8; 2048];
    let read = tokio::time::timeout(HANDSHAKE_TIMEOUT, tcp.read(&mut buf))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;

    // A TLS record: content type 0x16 (handshake), a supported version, and a
    // ServerHello (handshake type 0x02) inside it.
    if read < 6 || buf[0] != 0x16 {
        return Err(());
    }
    let major = buf[1];
    let minor = buf[2];
    let version_ok = major == 0x03 && (minor == 0x01 || minor == 0x02 || minor == 0x03 || minor == 0x04);
    if !version_ok || buf[5] != 0x02 {
        return Err(());
    }
    Ok(())
}

/// A minimal TLS 1.2 ClientHello advertising TLS 1.3, with the server name
/// extension set. Only the extensions a CDN edge needs in order to answer are
/// included; this is a reachability probe, not a fingerprint.
fn client_hello(sni: &str) -> Vec<u8> {
    let sni_bytes = sni.as_bytes();

    // server_name extension: type 0x0000, then the host as a DNS name.
    let mut server_name = Vec::new();
    server_name.extend_from_slice(&u16::try_from(sni_bytes.len() + 3).unwrap_or(0).to_be_bytes());
    server_name.push(0x00); // host_name name type
    server_name.extend_from_slice(&u16::try_from(sni_bytes.len()).unwrap_or(0).to_be_bytes());
    server_name.extend_from_slice(sni_bytes);

    let mut extensions = Vec::new();
    extensions.extend_from_slice(&0x0000u16.to_be_bytes());
    extensions.extend_from_slice(&u16::try_from(server_name.len()).unwrap_or(0).to_be_bytes());
    extensions.extend_from_slice(&server_name);

    // supported_versions: offer TLS 1.3 and 1.2.
    let supported_versions = [0x04u8, 0x03, 0x04, 0x03, 0x03];
    extensions.extend_from_slice(&0x002bu16.to_be_bytes());
    extensions.extend_from_slice(&u16::try_from(supported_versions.len()).unwrap_or(0).to_be_bytes());
    extensions.extend_from_slice(&supported_versions);

    // supported_groups (x25519, secp256r1) and a key share, because a TLS 1.3
    // edge will not answer a ClientHello that offers 1.3 without one.
    let groups = [0x00u8, 0x04, 0x00, 0x1d, 0x00, 0x17];
    extensions.extend_from_slice(&0x000au16.to_be_bytes());
    extensions.extend_from_slice(&u16::try_from(groups.len()).unwrap_or(0).to_be_bytes());
    extensions.extend_from_slice(&groups);

    // key_share: x25519 with a fixed public key. The probe never sends
    // application data, so the key only has to be well formed.
    let mut key_share = vec![0x00, 0x24, 0x00, 0x1d, 0x00, 0x20];
    key_share.extend_from_slice(&[
        0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x61, 0x62, 0x63, 0x64, 0x65,
        0x66, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x61, 0x62, 0x63, 0x64,
        0x65, 0x66,
    ]);
    extensions.extend_from_slice(&0x0033u16.to_be_bytes());
    extensions.extend_from_slice(&u16::try_from(key_share.len()).unwrap_or(0).to_be_bytes());
    extensions.extend_from_slice(&key_share);

    // A short cipher list covering what the four CDNs negotiate.
    let ciphers: [u8; 22] = [
        0x00, 0x14, // 10 cipher suites follow
        0x13, 0x01, // TLS_AES_128_GCM_SHA256
        0x13, 0x02, // TLS_AES_256_GCM_SHA384
        0x13, 0x03, // TLS_CHACHA20_POLY1305_SHA256
        0xc0, 0x2b, // TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256
        0xc0, 0x2f, // TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256
        0xc0, 0x2c, // TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384
        0xc0, 0x30, // TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384
        0x00, 0x9c, // TLS_RSA_WITH_AES_128_GCM_SHA256
        0x00, 0x2f, // TLS_RSA_WITH_AES_128_CBC_SHA
        0x00, 0x35, // TLS_RSA_WITH_AES_256_CBC_SHA
    ];

    let mut hello = Vec::new();
    hello.push(0x03);
    hello.push(0x03); // legacy_version: TLS 1.2
    hello.extend_from_slice(&[0x41u8; 32]); // random
    hello.push(0x00); // no session id
    hello.extend_from_slice(&ciphers);
    hello.extend_from_slice(&[0x01, 0x00]); // null compression
    hello.extend_from_slice(&u16::try_from(extensions.len()).unwrap_or(0).to_be_bytes());
    hello.extend_from_slice(&extensions);

    // Handshake header: type 0x01 (ClientHello), 24-bit length.
    let mut handshake = Vec::new();
    handshake.push(0x01);
    let len = u32::try_from(hello.len()).unwrap_or(0);
    handshake.extend_from_slice(&len.to_be_bytes()[1..]);
    handshake.extend_from_slice(&hello);

    // Record header: handshake, legacy version, 16-bit length.
    let mut record = Vec::new();
    record.extend_from_slice(&[0x16, 0x03, 0x01]);
    record.extend_from_slice(&u16::try_from(handshake.len()).unwrap_or(0).to_be_bytes());
    record.extend_from_slice(&handshake);
    record
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_hello_is_a_well_formed_tls_record() {
        let hello = client_hello("a248.e.akamai.net");

        assert_eq!(hello[0], 0x16, "content type is handshake");
        assert_eq!(&hello[1..3], &[0x03, 0x01], "record version is TLS 1.0");

        let record_len = u16::from_be_bytes([hello[3], hello[4]]) as usize;
        assert_eq!(hello.len(), 5 + record_len, "record length matches the body");
        assert_eq!(hello[5], 0x01, "handshake type is ClientHello");

        // The server name has to be in there verbatim, or the edge has nothing
        // to select a certificate against.
        let host = b"a248.e.akamai.net";
        assert!(
            hello.windows(host.len()).any(|window| window == host),
            "SNI host is embedded in the ClientHello"
        );
    }

    #[test]
    fn every_cdn_has_a_probe_target_and_a_name() {
        for cdn in CDNS {
            assert!(!cdn.domains.is_empty(), "{} has no domains", cdn.name);
            assert!(cdn.sni.contains('.'), "{} sni is not a hostname", cdn.name);
        }
    }
}
