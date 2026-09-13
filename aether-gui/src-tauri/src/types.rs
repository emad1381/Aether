use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelConfig {
    #[serde(default = "default_protocol")]
    pub protocol: String, // "masque", "masque-h2", "wg", "gool", "mim", "tor", "tor-reverse", "tor-only"

    #[serde(default = "default_socks_port")]
    pub socks_port: u16, // default: 1819

    #[serde(default)]
    pub http_port: Option<u16>, // optional: 1820

    #[serde(default = "default_scan_mode")]
    pub scan_mode: String, // "turbo", "balanced", "thorough", "stealth", "ironclad"

    #[serde(default = "default_ip_family")]
    pub ip_family: String, // "v4", "v6", "both"

    #[serde(default = "default_noize")]
    pub noize: String, // "firewall", "balanced", "gfw", "aggressive", "light", "off"

    #[serde(default)]
    pub peer: Option<String>, // manual ip:port override

    #[serde(default)]
    pub wiw_outer: Option<String>,

    #[serde(default)]
    pub wiw_inner: Option<String>,

    #[serde(default)]
    pub mim_outer: Option<String>,

    #[serde(default)]
    pub mim_inner: Option<String>,

    #[serde(default)]
    pub fragment: bool, // TLS ClientHello fragmentation on H2

    #[serde(default)]
    pub fragment_size: Option<String>, // e.g. "16-32"

    #[serde(default)]
    pub fragment_delay: Option<String>, // e.g. "2-10"

    #[serde(default = "default_keepalive")]
    pub keepalive: u16,

    #[serde(default)]
    pub dns: Option<String>, // e.g. "1.1.1.1,1.0.0.1"

    #[serde(default)]
    pub upstream: Option<String>, // e.g. "socks5://127.0.0.1:1080"

    #[serde(default = "default_true")]
    pub auto_system_proxy: bool,

    #[serde(default = "default_bypass_list")]
    pub bypass_list: String,

    #[serde(default)]
    pub route_direct: Option<String>,

    #[serde(default)]
    pub route_block: Option<String>,

    #[serde(default = "default_true")]
    pub auto_connect: bool,

    #[serde(default = "default_tunnel_mode")]
    pub tunnel_mode: String, // "proxy" or "system-wide"

    #[serde(default)]
    pub tor_enabled: bool,

    #[serde(default = "default_tor_mode")]
    pub tor_mode: String, // "carry", "reach", "tor-only"

    #[serde(default)]
    pub tor_bridges: bool,

    #[serde(default)]
    pub tor_country: Option<String>,

    #[serde(default)]
    pub launch_at_startup: bool,

    #[serde(default)]
    pub start_minimized: bool,

    #[serde(default = "default_true")]
    pub close_to_tray: bool,

    #[serde(default)]
    pub team: Option<String>,

    #[serde(default)]
    pub access_email: Option<String>,

    #[serde(default)]
    pub access_token: Option<String>,

    #[serde(default)]
    pub access_id: Option<String>,

    #[serde(default)]
    pub access_secret: Option<String>,
}

fn default_protocol() -> String {
    "masque".to_string()
}
fn default_socks_port() -> u16 {
    1819
}
fn default_scan_mode() -> String {
    "balanced".to_string()
}
fn default_ip_family() -> String {
    "v4".to_string()
}
fn default_noize() -> String {
    "firewall".to_string()
}
fn default_keepalive() -> u16 {
    5
}
fn default_true() -> bool {
    true
}
fn default_tunnel_mode() -> String {
    "proxy".to_string()
}
fn default_tor_mode() -> String {
    "carry".to_string()
}
fn default_bypass_list() -> String {
    "<local>;localhost;127.*;10.*;192.168.*;172.16.*;*.ir".to_string()
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            protocol: default_protocol(),
            socks_port: default_socks_port(),
            http_port: Some(1820),
            scan_mode: default_scan_mode(),
            ip_family: default_ip_family(),
            noize: default_noize(),
            peer: None,
            wiw_outer: None,
            wiw_inner: None,
            mim_outer: None,
            mim_inner: None,
            fragment: false,
            fragment_size: None,
            fragment_delay: None,
            keepalive: default_keepalive(),
            dns: None,
            upstream: None,
            auto_system_proxy: true,
            auto_connect: true,
            bypass_list: default_bypass_list(),
            route_direct: None,
            route_block: None,
            tunnel_mode: default_tunnel_mode(),
            tor_enabled: false,
            tor_mode: default_tor_mode(),
            tor_bridges: false,
            tor_country: None,
            launch_at_startup: false,
            start_minimized: false,
            close_to_tray: true,
            team: None,
            access_email: None,
            access_token: None,
            access_id: None,
            access_secret: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Disconnected,
    Scanning,
    Connecting,
    Connected,
    Reconnecting,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelStatus {
    pub state: State,
    pub latency_ms: Option<u64>,
    pub uptime_secs: u64,
    pub socks_endpoint: String,
    pub system_proxy_active: bool,
    pub exit_ip: Option<String>,
    pub colo: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceInfo {
    pub ip: String,
    pub loc: String,
    pub colo: String,
    pub warp: String,
}
