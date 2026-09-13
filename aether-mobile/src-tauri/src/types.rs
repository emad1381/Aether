use serde::{Deserialize, Serialize};

/// All of the app's settings, exactly as the mobile UI sends them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "d_protocol")]
    pub protocol: String, // masque | masque-h2 | wg
    #[serde(default = "d_scan")]
    pub scan: String, // fast | balanced | deep
    #[serde(default = "d_obf")]
    pub obfuscation: String, // off | balanced | strong
    #[serde(default)]
    pub server: String, // optional ip:port, empty means auto
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

impl Default for Settings {
    fn default() -> Self {
        Self {
            protocol: d_protocol(),
            scan: d_scan(),
            obfuscation: d_obf(),
            server: String::new(),
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
