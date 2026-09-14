use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use aether::api::{self, Cancel, ScanRequest, Transport, TunnelSpec};
use aether::prober::IpScan;
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter};

use crate::types::{Phase, Settings, Status};

pub const SOCKS_PORT: u16 = 1819;

/// Where the device identity and app settings live. Android gives the app a
/// writable private directory; an embedder can point us at it explicitly.
pub fn data_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        if let Ok(dir) = std::env::var("AETHER_APP_DIR") {
            let p = PathBuf::from(dir);
            if p.is_dir() {
                return p;
            }
        }
        std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(std::env::temp_dir)
    })
    .clone()
}

pub fn load_settings() -> Settings {
    std::fs::read_to_string(data_dir().join("settings.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_settings(s: &Settings) {
    if let Ok(json) = serde_json::to_string(s) {
        let _ = std::fs::write(data_dir().join("settings.json"), json);
    }
}

/// A Zero Trust email sign-in in flight, held for the UI to submit its code.
type SignInSession = aether::zerotrust::EmailSignIn;

fn sign_in_sessions(
) -> &'static std::sync::Mutex<std::collections::HashMap<u64, SignInSession>> {
    static SESSIONS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<u64, SignInSession>>,
    > = std::sync::OnceLock::new();
    SESSIONS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Keep a sign-in session the UI is about to ask a code for, and hand back the
/// handle it submits against.
pub fn keep_sign_in_session(session: SignInSession) -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    sign_in_sessions().lock().insert(id, session);
    id
}

/// Take the session back so the code can be submitted against it. Each session
/// is single use: the UI has to request a new code if this one is consumed.
pub fn take_sign_in_session(id: u64) -> Result<SignInSession, String> {
    sign_in_sessions()
        .lock()
        .remove(&id)
        .ok_or_else(|| format!("there is no sign-in session {id}"))
}

/// The engine runs one tunnel session at a time, in-process, through the
/// core's own public API. Each session owns a `Cancel`; `disconnect` signals it
/// and the task tears the tunnel down. A session counter guards against an old
/// task writing status after a newer one has started.
pub struct Engine {
    status: Mutex<Status>,
    cancel: Mutex<Option<Cancel>>,
    started_at: Mutex<Option<Instant>>,
    session: AtomicU64,
}

impl Engine {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            status: Mutex::new(Status::default()),
            cancel: Mutex::new(None),
            started_at: Mutex::new(None),
            session: AtomicU64::new(0),
        })
    }

    pub fn status(&self) -> Status {
        let mut st = self.status.lock().clone();
        if st.phase == Phase::Connected {
            if let Some(t) = *self.started_at.lock() {
                st.detail = format!("Connected · {}", fmt_duration(t.elapsed()));
            }
        }
        st
    }

    fn set(&self, app: &AppHandle, f: impl FnOnce(&mut Status)) {
        f(&mut self.status.lock());
        let st = self.status();
        let _ = app.emit("aether-status", st);
    }

    pub fn connect(self: &Arc<Self>, app: AppHandle, settings: Settings) -> Result<(), String> {
        if self.cancel.lock().is_some() {
            return Err("A connection is already running.".into());
        }
        save_settings(&settings);

        let (transport, protocol_label, noize, scan_mode) = plan(&settings);
        let peer = parse_peer(&settings.server);

        let cancel = Cancel::new();
        *self.cancel.lock() = Some(cancel.clone());
        let my_session = self.session.fetch_add(1, Ordering::SeqCst) + 1;

        self.set(&app, |st| {
            st.phase = Phase::Provisioning;
            st.detail = "Preparing device identity…".into();
            st.protocol = protocol_label.clone();
            st.latency_ms = None;
            st.exit_ip = None;
            st.colo = None;
            st.loc = None;
            st.warp = None;
            st.socks = format!("127.0.0.1:{SOCKS_PORT}");
        });

        let engine = self.clone();
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let outcome = engine
                .run_session(
                    app2.clone(),
                    transport,
                    protocol_label,
                    noize,
                    scan_mode,
                    peer,
                    cancel,
                    settings,
                )
                .await;
            // A superseded session keeps its hands off the status.
            if engine.session.load(Ordering::SeqCst) == my_session {
                *engine.cancel.lock() = None;
                *engine.started_at.lock() = None;
                match outcome {
                    Ok(()) => engine.set(&app2, |st| {
                        st.phase = Phase::Disconnected;
                        st.detail = "Disconnected".into();
                    }),
                    Err(message) => engine.set(&app2, |st| {
                        st.phase = Phase::Error;
                        st.detail = message;
                    }),
                }
            }
        });

        Ok(())
    }

    pub fn disconnect(self: &Arc<Self>, app: AppHandle) {
        self.session.fetch_add(1, Ordering::SeqCst);
        if let Some(cancel) = self.cancel.lock().take() {
            cancel.cancel();
        }
        *self.started_at.lock() = None;
        self.set(&app, |st| {
            st.phase = Phase::Disconnected;
            st.detail = "Disconnected".into();
            st.latency_ms = None;
        });
    }

    async fn run_session(
        self: &Arc<Self>,
        app: AppHandle,
        transport: Transport,
        protocol_label: String,
        noize: String,
        scan_mode: String,
        peer: Option<SocketAddr>,
        cancel: Cancel,
        settings: Settings,
    ) -> Result<(), String> {
        // 1. Identity: reuse the stored device or register one on first use.
        // A nested tunnel keeps two, so it opens the outer one and then a
        // second one the tunnel itself dials through the first.
        let base = data_dir().join("aether.toml").to_string_lossy().into_owned();
        let (outer_path, inner_path) = match transport.needs_inner_identity() {
            true => api::nested_identity_paths(&base, transport, None),
            false => (api::identity_path(&base, transport, None), String::new()),
        };
        let request = api::ProvisionRequest::for_transport(transport);
        let identity = api::open_identity(&outer_path, &request)
            .await
            .map_err(|e| format!("Could not register a device: {e}"))?;
        if cancel.is_cancelled() {
            return Ok(());
        }

        let inner_identity = if transport.needs_inner_identity() {
            let inner = api::open_identity(&inner_path, &request)
                .await
                .map_err(|e| format!("Could not register the second device: {e}"))?;
            if cancel.is_cancelled() {
                return Ok(());
            }
            Some(inner)
        } else {
            None
        };

        // 2. Endpoint: a hand-typed server is used as-is; otherwise sweep.
        let endpoint = match peer {
            Some(p) => p,
            None => {
                self.set(&app, |st| {
                    st.phase = Phase::Scanning;
                    st.detail = "Hunting for a clean gateway…".into();
                });
                let scan = ScanRequest::for_transport(transport)
                    .with_mode(&scan_mode)
                    .with_ip(ip_scan_of(&settings))
                    .with_profile(&noize);
                match api::scan(&identity, &scan, &cancel).await {
                    Ok(found) => found.socket(),
                    Err(e) => {
                        if cancel.is_cancelled() {
                            return Ok(());
                        }
                        return Err(format!(
                            "No gateway passed its data check ({e}). Try Deep scan, \
                             stronger obfuscation, or MASQUE / HTTP/2."
                        ));
                    }
                }
            }
        };
        if cancel.is_cancelled() {
            return Ok(());
        }

        // A nested tunnel pins its second hop when the user wrote one down; the
        // empty value means the core picks one on its own.
        let inner_peer = inner_identity.as_ref().and_then(|_| {
            settings
                .inner_server
                .split([',', ';', ' '])
                .find(|part| !part.trim().is_empty())
                .and_then(|part| part.trim().parse::<SocketAddr>().ok())
        });

        self.set(&app, |st| {
            st.phase = Phase::Connecting;
            st.detail = format!("Opening the tunnel to {endpoint}…");
        });

        // 3. Tunnel. `connect` blocks for the session's lifetime; the core
        // refuses to open the local proxy until real traffic has passed, so
        // the port answering *is* the proof the tunnel works.
        let mut spec = TunnelSpec::for_transport(transport)
            .with_socks(SocketAddr::new(IpAddr::V4([127, 0, 0, 1].into()), SOCKS_PORT))
            .with_profile(&noize);
        spec.keepalive = 15;
        spec.verify_timeout = Duration::from_secs(20);
        if let Some(inner) = inner_peer {
            spec = spec.with_inner_peer(inner);
        }

        let opener = {
            let engine = self.clone();
            let app = app.clone();
            let cancel = cancel.clone();
            let protocol_label = protocol_label.clone();
            tauri::async_runtime::spawn(async move {
                watch_proxy_open(engine, app, cancel, protocol_label).await;
            })
        };
        let monitor = {
            let engine = self.clone();
            let app = app.clone();
            let cancel = cancel.clone();
            tauri::async_runtime::spawn(async move {
                latency_monitor(engine, app, cancel).await;
            })
        };

        let outcome = match inner_identity {
            Some(inner) => api::connect_nested(&identity, &inner, endpoint, &spec, &cancel).await,
            None => api::connect(&identity, endpoint, &spec, &cancel).await,
        };
        opener.abort();
        monitor.abort();

        match outcome {
            Ok(()) => Ok(()),
            Err(e) => {
                let text = e.to_string();
                if cancel.is_cancelled() || text.to_lowercase().contains("cancel") {
                    Ok(())
                } else {
                    Err(text)
                }
            }
        }
    }
}

/// The core reads its IP-version choice from a setting the UI owns.
fn ip_scan_of(settings: &Settings) -> IpScan {
    match settings.ip_family.as_str() {
        "v6" | "6" => IpScan::V6,
        "both" | "dual" => IpScan::Both,
        _ => IpScan::V4,
    }
}

/// Poll the SOCKS port until it accepts a connection, then report Connected.
async fn watch_proxy_open(
    engine: Arc<Engine>,
    app: AppHandle,
    cancel: Cancel,
    protocol_label: String,
) {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if cancel.is_cancelled() || Instant::now() > deadline {
            return;
        }
        let opened = tokio::time::timeout(
            Duration::from_millis(500),
            tokio::net::TcpStream::connect(("127.0.0.1", SOCKS_PORT)),
        )
        .await;
        if matches!(opened, Ok(Ok(_))) {
            engine.started_at.lock().replace(Instant::now());
            engine.set(&app, |st| {
                if st.protocol != protocol_label {
                    st.protocol = protocol_label.clone();
                }
                st.phase = Phase::Connected;
                st.detail = "Connected".into();
            });
            return;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(600)) => {}
            _ = cancel.wait() => return,
        }
    }
}

/// Keep latency and exit details fresh while the tunnel is up.
async fn latency_monitor(engine: Arc<Engine>, app: AppHandle, cancel: Cancel) {
    let mut round = 0u32;
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(20)) => {}
            _ = cancel.wait() => return,
        }
        if cancel.is_cancelled() {
            return;
        }
        round += 1;

        match tokio::time::timeout(Duration::from_secs(8), ping_latency()).await {
            Ok(Ok(ms)) => engine.set(&app, |st| {
                st.latency_ms = Some(ms);
                st.phase = Phase::Connected;
            }),
            Ok(Err(_)) | Err(_) => {
                // Only flag staleness once we believed we were connected.
                if engine.status().phase == Phase::Connected {
                    engine.set(&app, |st| {
                        st.phase = Phase::Error;
                        st.detail = "Traffic stopped flowing. Reconnecting…".into();
                        st.latency_ms = None;
                    });
                }
            }
        }

        // Exit details are cheap but not urgent: fetch every third round.
        if round % 3 == 1 {
            if let Ok(trace) = fetch_trace().await {
                engine.set(&app, |st| {
                    st.exit_ip = Some(trace.ip);
                    st.colo = Some(trace.colo);
                    st.loc = Some(trace.loc);
                    st.warp = Some(trace.warp);
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// probes through the local proxy
// ---------------------------------------------------------------------------

fn proxy_client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .proxy(
            reqwest::Proxy::all(format!("socks5h://127.0.0.1:{SOCKS_PORT}"))
                .map_err(|e| e.to_string())?,
        )
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())
}

async fn ping_latency() -> Result<u64, String> {
    let client = proxy_client(Duration::from_secs(6))?;
    let started = Instant::now();
    client
        .get("http://www.gstatic.com/generate_204")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    Ok(started.elapsed().as_millis() as u64)
}

pub struct TraceInfo {
    pub ip: String,
    pub loc: String,
    pub colo: String,
    pub warp: String,
}

async fn fetch_trace() -> Result<TraceInfo, String> {
    let client = proxy_client(Duration::from_secs(8))?;
    let text = client
        .get("https://www.cloudflare.com/cdn-cgi/trace")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;

    let mut trace = TraceInfo {
        ip: "?".into(),
        loc: "?".into(),
        colo: "?".into(),
        warp: "?".into(),
    };
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "ip" => trace.ip = v.trim().into(),
                "loc" => trace.loc = v.trim().into(),
                "colo" => trace.colo = v.trim().into(),
                "warp" => trace.warp = v.trim().into(),
                _ => {}
            }
        }
    }
    Ok(trace)
}

// ---------------------------------------------------------------------------
// UI vocabulary -> core vocabulary
// ---------------------------------------------------------------------------

fn plan(s: &Settings) -> (Transport, String, String, String) {
    let transport = match s.protocol.as_str() {
        "wg" | "wireguard" => Transport::WireGuard,
        "gool" | "wiw" | "warp-in-warp" => Transport::Gool,
        "mim" | "m2" | "masque-in-masque" => Transport::Mim,
        _ => Transport::Masque,
    };
    let label = match s.protocol.as_str() {
        "wg" | "wireguard" => "WireGuard",
        "gool" | "wiw" | "warp-in-warp" => "WARP-in-WARP",
        "mim" | "m2" | "masque-in-masque" => "MASQUE-in-MASQUE",
        "masque-h2" => "MASQUE / HTTP/2",
        _ => "MASQUE / HTTP/3",
    };

    // The core's MASQUE carrier is chosen from this environment switch. The
    // h2 carrier only applies to masque and mim, the two masque protocols.
    match s.protocol.as_str() {
        "masque-h2" | "mim-h2" => std::env::set_var("AETHER_MASQUE_HTTP2", "1"),
        _ => std::env::remove_var("AETHER_MASQUE_HTTP2"),
    }
    match s.ip_family.as_str() {
        "v6" | "6" => std::env::set_var("AETHER_IP", "v6"),
        "both" | "dual" => std::env::set_var("AETHER_IP", "both"),
        _ => std::env::set_var("AETHER_IP", "v4"),
    }

    // The routing rules are read by the core straight out of the environment,
    // the same way the CLI reads them, so they apply before the tunnel comes
    // up rather than after.
    set_or_remove("AETHER_DNS", &s.dns);
    set_or_remove("AETHER_ROUTE_BLOCK", &s.route_block);
    set_or_remove("AETHER_ROUTE_DIRECT", &s.route_direct);
    set_or_remove("AETHER_TEAM", &s.team);
    set_or_remove("AETHER_ACCESS_EMAIL", &s.access_email);
    set_or_remove("AETHER_ACCESS_TOKEN", &s.access_token);
    set_or_remove("AETHER_ACCESS_CLIENT_ID", &s.access_id);
    set_or_remove("AETHER_ACCESS_CLIENT_SECRET", &s.access_secret);
    std::env::remove_var("AETHER_UPSTREAM");
    std::env::remove_var("AETHER_MARK");

    let scan_mode = match s.scan.as_str() {
        "turbo" | "fast" => "turbo",
        "thorough" | "deep" => "thorough",
        "stealth" => "stealth",
        "ironclad" => "ironclad",
        _ => "balanced",
    };
    let noize = match (s.obfuscation.as_str(), transport.carrier()) {
        ("off" | "none", _) => "off",
        ("light", _) => "light",
        ("strong" | "aggressive", aether::api::Carrier::WireGuard) => "aggressive",
        ("strong" | "aggressive", _) => "gfw",
        ("balanced", aether::api::Carrier::WireGuard) => "balanced",
        ("balanced", _) => "firewall",
        ("gfw", _) => "gfw",
        _ => "firewall",
    };
    std::env::set_var("AETHER_NOIZE", noize);

    (transport, label.into(), noize.into(), scan_mode.into())
}

fn set_or_remove(key: &str, value: &str) {
    if value.trim().is_empty() {
        std::env::remove_var(key);
    } else {
        std::env::set_var(key, value.trim());
    }
}

fn parse_peer(raw: &str) -> Option<SocketAddr> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    text.parse().ok()
}

fn fmt_duration(d: Duration) -> String {
    let total = d.as_secs();
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}h {m:02}m {s:02}s")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}
