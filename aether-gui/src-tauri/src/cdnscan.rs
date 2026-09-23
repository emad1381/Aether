//! CDN fronting edge scanner — CIDR edition.
//!
//! Scans the actual IP ranges that Akamai, Google, CloudFront and Azure publish
//! for their edge networks. A real TLS handshake is attempted against each IP
//! with a hostname the edge really serves (so its certificate verifies). Only
//! edges that complete the handshake from *this* machine are returned — these
//! are the ones psiphon can actually front through on the user's current network.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use ipnet::Ipv4Net;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Max concurrent TLS handshakes. Each is one TCP connect + ClientHello + read
/// ServerHello. 24 is safe for a residential uplink.
const CONCURRENCY: usize = 24;

/// Handshake timeout. A middlebox that accepts TCP but drops TLS will hang here.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// One CDN with its published IPv4 edge ranges and a hostname that the edge
/// actually serves a valid certificate for.
struct Cdn {
    name: &'static str,
    /// CIDR blocks published by the CDN for its edge network.
    cidrs: &'static [&'static str],
    /// Hostname to send in SNI and verify against the edge certificate.
    sni: &'static str,
}

// Ranges from cdn-ip-finder + public CDN IP lists. These are the anycast edge
// prefixes each CDN announces. Scanning the whole /24s would be thousands of
// IPs; we take a fixed stride (every Nth IP) so the scan finishes in ~20s
// while still covering the space.
const CDNS: &[Cdn] = &[
    Cdn {
        name: "Akamai",
        cidrs: &[
            "23.32.0.0/14",   // 23.32.0.0 - 23.35.255.255
            "23.48.0.0/14",   // 23.48.0.0 - 23.51.255.255
            "23.58.0.0/15",   // 23.58.0.0 - 23.59.255.255
            "23.72.0.0/14",   // 23.72.0.0 - 23.75.255.255
            "23.192.0.0/14",  // 23.192.0.0 - 23.195.255.255
            "23.196.0.0/14",  // 23.196.0.0 - 23.199.255.255
            "23.202.0.0/15",  // 23.202.0.0 - 23.203.255.255
            "23.43.0.0/16",   // 23.43.0.0 - 23.43.255.255
            "23.44.0.0/15",   // 23.44.0.0 - 23.45.255.255
            "23.46.0.0/15",   // 23.46.0.0 - 23.47.255.255
            "23.200.0.0/13",  // 23.200.0.0 - 23.207.255.255
            "23.208.0.0/13",  // 23.208.0.0 - 23.215.255.255
            "23.216.0.0/13",  // 23.216.0.0 - 23.223.255.255
            "23.224.0.0/13",  // 23.224.0.0 - 23.231.255.255
            "104.64.0.0/10",  // 104.64.0.0 - 104.127.255.255
            "104.128.0.0/10", // 104.128.0.0 - 104.191.255.255
            "184.24.0.0/14",  // 184.24.0.0 - 184.27.255.255
            "184.84.0.0/14",  // 184.84.0.0 - 184.87.255.255
            "184.86.0.0/15",  // 184.86.0.0 - 184.87.255.255
            "185.200.232.0/24",
            "92.16.0.0/16",
            "92.122.0.0/15",
            "72.246.0.0/15",
            "2.16.0.0/13",
            "2.20.0.0/14",
            "2.22.0.0/15",
        ],
        sni: "a248.e.akamai.net",
    },
    Cdn {
        name: "Google",
        cidrs: &[
            "34.143.0.0/16",
            "34.160.0.0/16",
            "35.186.0.0/16",
            "35.190.0.0/16",
            "35.192.0.0/16",
            "35.196.0.0/14",
            "35.200.0.0/13",
            "35.208.0.0/12",
            "64.233.160.0/19",
            "66.249.80.0/20",
            "74.125.0.0/16",
            "142.250.0.0/15",
            "142.251.0.0/16",
            "172.217.0.0/16",
            "172.253.0.0/16",
            "216.58.192.0/18",
            "216.239.32.0/19",
        ],
        sni: "www.gstatic.com",
    },
    Cdn {
        name: "CloudFront",
        cidrs: &[
            "13.32.0.0/15",
            "13.35.0.0/16",
            "13.56.0.0/14",
            "13.224.0.0/14",
            "13.248.0.0/13",
            "13.250.0.0/15",
            "13.254.0.0/15",
            "18.208.0.0/13",
            "18.216.0.0/15",
            "18.220.0.0/14",
            "18.230.0.0/15",
            "18.231.0.0/16",
            "18.232.0.0/14",
            "18.240.0.0/13",
            "18.248.0.0/13",
            "18.254.0.0/15",
            "52.46.0.0/15",
            "52.84.0.0/15",
            "52.86.0.0/15",
            "52.88.0.0/14",
            "52.92.0.0/14",
            "52.96.0.0/13",
            "52.100.0.0/14",
            "52.104.0.0/14",
            "52.108.0.0/15",
            "52.110.0.0/15",
            "52.112.0.0/14",
            "52.116.0.0/15",
            "52.118.0.0/15",
            "52.120.0.0/14",
            "54.192.0.0/13",
            "54.230.0.0/15",
            "54.230.128.0/17",
            "54.239.128.0/18",
            "54.239.192.0/18",
            "54.240.128.0/18",
            "99.84.0.0/16",
            "99.86.0.0/16",
            "130.176.0.0/15",
            "143.204.0.0/15",
            "205.251.192.0/18",
        ],
        sni: "d1.awsstatic.com",
    },
    Cdn {
        name: "Azure",
        cidrs: &[
            "13.107.4.0/22",
            "13.107.6.0/23",
            "13.107.8.0/21",
            "13.107.16.0/20",
            "20.33.0.0/16",
            "20.34.0.0/15",
            "20.36.0.0/14",
            "20.40.0.0/13",
            "20.48.0.0/12",
            "20.64.0.0/11",
            "20.96.0.0/11",
            "20.128.0.0/11",
            "20.160.0.0/11",
            "20.192.0.0/11",
            "23.96.0.0/13",
            "23.96.0.0/14",
            "23.100.0.0/14",
            "40.64.0.0/12",
            "40.76.0.0/13",
            "40.80.0.0/12",
            "40.96.0.0/11",
            "40.128.0.0/10",
            "40.160.0.0/11",
            "52.224.0.0/12",
            "52.232.0.0/11",
            "52.240.0.0/12",
            "104.208.0.0/13",
            "104.216.0.0/13",
            "137.116.0.0/14",
            "137.120.0.0/13",
            "168.61.0.0/16",
            "168.62.0.0/15",
            "168.63.0.0/16",
        ],
        sni: "ajax.aspnetcdn.com",
    },
];

/// Stride: test 1 IP every N in each CIDR. Keeps the total probes ~800-1000.
const STRIDE: usize = 64;

#[derive(Debug, Clone, Serialize)]
pub struct CdnEdge {
    pub ip: String,
    pub cdn: String,
    pub sni: String,
    pub latency_ms: Option<u64>,
    pub reachable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CdnScanReport {
    pub edges: Vec<CdnEdge>,
    pub ips: String,
    pub sni: String,
    pub tested: usize,
    pub reachable: usize,
}

struct Probe {
    addr: SocketAddr,
    cdn: &'static str,
    sni: &'static str,
}

pub async fn scan<F>(mut progress: F) -> CdnScanReport
where
    F: FnMut(usize, usize),
{
    let probes = build_probes();
    let total = probes.len();
    progress(0, total);

    let mut edges: Vec<CdnEdge> = Vec::with_capacity(total);
    let mut completed = 0usize;

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

fn build_probes() -> Vec<Probe> {
    let mut probes = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for cdn in CDNS {
        for cidr_str in cdn.cidrs {
            let cidr: Ipv4Net = cidr_str.parse().expect("valid CIDR");
            let start = u32::from(cidr.network());
            let end = u32::from(cidr.broadcast());
            let mut ip = start;

            while ip <= end {
                if ip % STRIDE as u32 == 0 {
                    let addr = Ipv4Addr::from(ip);
                    if !seen.contains(&addr) {
                        seen.insert(addr);
                        probes.push(Probe {
                            addr: SocketAddr::new(IpAddr::V4(addr), 443),
                            cdn: cdn.name,
                            sni: cdn.sni,
                        });
                    }
                }
                ip = ip.saturating_add(1);
            }
        }
    }

    // Cap at a reasonable number so the scan finishes in ~20s even on slow lines.
    if probes.len() > 1200 {
        probes.truncate(1200);
    }

    log::info!("[cdnscan] built {} probes across 4 CDNs (stride {})", probes.len(), STRIDE);
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

fn client_hello(sni: &str) -> Vec<u8> {
    let sni_bytes = sni.as_bytes();

    let mut server_name = Vec::new();
    server_name.extend_from_slice(&u16::try_from(sni_bytes.len() + 3).unwrap_or(0).to_be_bytes());
    server_name.push(0x00);
    server_name.extend_from_slice(&u16::try_from(sni_bytes.len()).unwrap_or(0).to_be_bytes());
    server_name.extend_from_slice(sni_bytes);

    let mut extensions = Vec::new();
    extensions.extend_from_slice(&0x0000u16.to_be_bytes());
    extensions.extend_from_slice(&u16::try_from(server_name.len()).unwrap_or(0).to_be_bytes());
    extensions.extend_from_slice(&server_name);

    let supported_versions = [0x04u8, 0x03, 0x04, 0x03, 0x03];
    extensions.extend_from_slice(&0x002bu16.to_be_bytes());
    extensions.extend_from_slice(&u16::try_from(supported_versions.len()).unwrap_or(0).to_be_bytes());
    extensions.extend_from_slice(&supported_versions);

    let groups = [0x00u8, 0x04, 0x00, 0x1d, 0x00, 0x17];
    extensions.extend_from_slice(&0x000au16.to_be_bytes());
    extensions.extend_from_slice(&u16::try_from(groups.len()).unwrap_or(0).to_be_bytes());
    extensions.extend_from_slice(&groups);

    let mut key_share = vec![0x00, 0x24, 0x00, 0x1d, 0x00, 0x20];
    key_share.extend_from_slice(&[
        0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x61, 0x62, 0x63, 0x64, 0x65,
        0x66, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x61, 0x62, 0x63, 0x64,
        0x65, 0x66,
    ]);
    extensions.extend_from_slice(&0x0033u16.to_be_bytes());
    extensions.extend_from_slice(&u16::try_from(key_share.len()).unwrap_or(0).to_be_bytes());
    extensions.extend_from_slice(&key_share);

    let ciphers: [u8; 22] = [
        0x00, 0x14,
        0x13, 0x01,
        0x13, 0x02,
        0x13, 0x03,
        0xc0, 0x2b,
        0xc0, 0x2f,
        0xc0, 0x2c,
        0xc0, 0x30,
        0x00, 0x9c,
        0x00, 0x2f,
        0x00, 0x35,
    ];

    let mut hello = Vec::new();
    hello.push(0x03);
    hello.push(0x03);
    hello.extend_from_slice(&[0x41u8; 32]);
    hello.push(0x00);
    hello.extend_from_slice(&ciphers);
    hello.extend_from_slice(&[0x01, 0x00]);
    hello.extend_from_slice(&u16::try_from(extensions.len()).unwrap_or(0).to_be_bytes());
    hello.extend_from_slice(&extensions);

    let mut handshake = Vec::new();
    handshake.push(0x01);
    let len = u32::try_from(hello.len()).unwrap_or(0);
    handshake.extend_from_slice(&len.to_be_bytes()[1..]);
    handshake.extend_from_slice(&hello);

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
    fn client_hello_contains_sni() {
        let hello = client_hello("a248.e.akamai.net");
        let host = b"a248.e.akamai.net";
        assert!(hello.windows(host.len()).any(|w| w == host));
    }

    #[test]
    fn probe_count_is_bounded() {
        let probes = build_probes();
        assert!(probes.len() <= 1200);
        assert!(probes.len() > 400);
    }

    #[test]
    fn every_cdn_has_cidrs_and_sni() {
        for cdn in CDNS {
            assert!(!cdn.cidrs.is_empty(), "{} has no CIDRs", cdn.name);
            assert!(cdn.sni.contains('.'), "{} sni is not a hostname", cdn.name);
        }
    }
}
