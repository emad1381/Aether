//! Candidate generation and ordering for Smart Auto Mode.
//!
//! The runner intentionally owns process lifetime; this module is pure so the
//! complete 48-entry fallback matrix can be tested without touching the network.

use std::collections::HashSet;

use crate::types::TunnelConfig;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Candidate {
    pub protocol: String,
    pub noize: String,
    pub ip_family: String,
}

impl Candidate {
    pub fn new(protocol: &str, noize: &str, ip_family: &str) -> Self {
        Self {
            protocol: protocol.to_string(),
            noize: noize.to_string(),
            ip_family: ip_family.to_string(),
        }
    }

    pub fn display_name(&self) -> String {
        let protocol = match self.protocol.as_str() {
            "masque" => "MASQUE / HTTP/3",
            "masque-h2" => "MASQUE / HTTP/2",
            "wg" => "WireGuard",
            "gool" => "WARP-in-WARP",
            "mim" => "MASQUE-in-MASQUE",
            other => other,
        };
        format!("{protocol} / {} / {}", self.noize, self.ip_family)
    }
}

#[derive(Debug, Clone)]
pub struct SuccessResult {
    pub candidate: Candidate,
    pub latency_ms: u64,
}

/// Time budget given to one engine process. These match the GUI's documented
/// scan-mode policy and deliberately do not undercut Aether's own probes.
pub fn timeout_for_scan_mode(scan_mode: &str) -> u64 {
    match scan_mode.trim().to_ascii_lowercase().as_str() {
        "turbo" => 30,
        "balanced" | "" => 45,
        "thorough" | "deep" | "ironclad" | "stealth" | "quiet" => 60,
        _ => 45,
    }
}

/// A small set that is likely to work on a reconnect. The last known-good
/// candidate comes first; HTTP/2 is a deliberate additional fallback because
/// it survives networks where UDP/QUIC is filtered. The complete 48-item
/// protocol × profile × IP matrix follows only after these candidates fail.
pub fn prioritized_candidates(cfg: &TunnelConfig) -> Vec<Candidate> {
    let mut ordered = Vec::new();

    if let (Some(protocol), Some(noize), Some(ip_family)) = (
        cfg.last_success_proto.as_deref(),
        cfg.last_success_noize.as_deref(),
        cfg.last_success_ip.as_deref(),
    ) {
        ordered.push(Candidate::new(protocol, noize, ip_family));
    }

    ordered.extend([
        Candidate::new("gool", "firewall", "v4"),
        Candidate::new("masque-h2", "firewall", "v4"),
        Candidate::new("wg", "balanced", "v4"),
        Candidate::new("masque", "firewall", "v4"),
    ]);

    let mut seen = HashSet::new();
    ordered.retain(|candidate| seen.insert(candidate.clone()));
    ordered
}

/// Exactly 48 standard candidates: four transport modes × their four valid
/// obfuscation profiles × IPv4/IPv6/dual-stack. Tor is intentionally absent:
/// it remains an explicit user opt-in rather than an Auto Mode dimension.
pub fn full_matrix() -> Vec<Candidate> {
    let mut candidates = Vec::with_capacity(48);
    let ip_versions = ["v4", "v6", "both"];

    for protocol in ["masque", "gool", "mim"] {
        for noize in ["off", "light", "firewall", "gfw"] {
            for ip_family in ip_versions {
                candidates.push(Candidate::new(protocol, noize, ip_family));
            }
        }
    }

    for noize in ["off", "light", "balanced", "aggressive"] {
        for ip_family in ip_versions {
            candidates.push(Candidate::new("wg", noize, ip_family));
        }
    }

    debug_assert_eq!(candidates.len(), 48);
    candidates
}

/// Gives reconnect candidates first, then adds the full matrix without
/// duplicates. `masque-h2` is a high-value priority candidate but not part of
/// the defined 48-combination matrix.
pub fn auto_candidates(cfg: &TunnelConfig) -> Vec<Candidate> {
    let mut candidates = prioritized_candidates(cfg);
    let mut seen: HashSet<Candidate> = candidates.iter().cloned().collect();
    candidates.extend(full_matrix().into_iter().filter(|candidate| seen.insert(candidate.clone())));
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_has_all_48_requested_combinations() {
        assert_eq!(full_matrix().len(), 48);
    }

    #[test]
    fn remembered_route_is_first() {
        let cfg = TunnelConfig {
            last_success_proto: Some("mim".into()),
            last_success_noize: Some("gfw".into()),
            last_success_ip: Some("both".into()),
            ..TunnelConfig::default()
        };
        assert_eq!(prioritized_candidates(&cfg)[0], Candidate::new("mim", "gfw", "both"));
    }
}
