use serde::{Deserialize, Serialize};

/// All of the app's settings, exactly as the mobile UI sends them.
///
/// The nested protocols (`gool`, `mim`) need a second hop; the UI keeps it in
/// `inner_server` and leaves it blank to let the scan choose. The Zero Trust
/// and routing fields are the same ones the CLI reads from the environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "d_protocol")]
    pub protocol: String, // masque | masque-h2 | wg | gool | mim
    #[serde(default = "d_scan")]
    pub scan: String, // turbo | balanced | thorough | stealth | ironclad
    #[serde(default = "d_obf")]
    pub obfuscation: String, // off | light | firewall | balanced | gfw | aggressive
    #[serde(default)]
    pub server: String, // optional ip:port, empty means auto
    /// The inner hop of a nested protocol. Empty means the scan picks it.
    #[serde(default)]
    pub inner_server: String,
    /// v4 | v6 | both
    #[serde(default = "d_ip_family")]
    pub ip_family: String,
    /// Resolvers the tunnel uses, comma separated.
    #[serde(default = "d_dns")]
    pub dns: String,
    /// Domains and networks that must never reach the tunnel.
    #[serde(default)]
    pub route_block: String,
    /// Domains and networks that bypass the tunnel entirely.
    #[serde(default)]
    pub route_direct: String,
    /// Zero Trust organization slug, blank for personal WARP.
    #[serde(default)]
    pub team: String,
    #[serde(default)]
    pub access_email: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub access_id: String,
    #[serde(default)]
    pub access_secret: String,
}

fn d_protocol() -> String {
    "masque".into()
}
fn d_scan() -> String {
    "balanced".into()
}
fn d_obf() -> String {
    "balanced".into()
}
fn d_ip_family() -> String {
    "v4".into()
}
fn d_dns() -> String {
    "1.1.1.1,1.0.0.1".into()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            protocol: d_protocol(),
            scan: d_scan(),
            obfuscation: d_obf(),
            server: String::new(),
            inner_server: String::new(),
            ip_family: d_ip_family(),
            dns: d_dns(),
            route_block: String::new(),
            route_direct: String::new(),
            team: String::new(),
            access_email: String::new(),
            access_token: String::new(),
            access_id: String::new(),
            access_secret: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Disconnected,
    Provisioning,
    Scanning,
    Connecting,
    Connected,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub phase: Phase,
    pub detail: String,
    pub protocol: String,
    pub latency_ms: Option<u64>,
    pub exit_ip: Option<String>,
    pub colo: Option<String>,
    pub loc: Option<String>,
    pub warp: Option<String>,
    pub socks: String,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            phase: Phase::Disconnected,
            detail: "Disconnected".into(),
            protocol: "MASQUE / HTTP/3".into(),
            latency_ms: None,
            exit_ip: None,
            colo: None,
            loc: None,
            warp: None,
            socks: "127.0.0.1:1819".into(),
        }
    }
}
