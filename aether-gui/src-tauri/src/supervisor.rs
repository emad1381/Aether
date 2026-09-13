use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::sync::broadcast;

use crate::proxy::set_windows_proxy;
use crate::types::{LogEntry, State, TunnelConfig, TunnelStatus};

pub struct Supervisor {
    child: Mutex<Option<Child>>,
    current_config: Mutex<Option<TunnelConfig>>,
    status: Mutex<TunnelStatus>,
    start_time: Mutex<Option<Instant>>,
    kill_tx: broadcast::Sender<()>,
    is_stopping: AtomicBool,
}

impl Supervisor {
    pub fn new() -> Self {
        let (kill_tx, _) = broadcast::channel(4);
        Self {
            child: Mutex::new(None),
            current_config: Mutex::new(None),
            status: Mutex::new(TunnelStatus {
                state: State::Disconnected,
                latency_ms: None,
                uptime_secs: 0,
                socks_endpoint: "127.0.0.1:1819".to_string(),
                system_proxy_active: false,
                exit_ip: None,
                colo: None,
                error_message: None,
            }),
            start_time: Mutex::new(None),
            kill_tx,
            is_stopping: AtomicBool::new(false),
        }
    }

    pub fn get_status(&self) -> TunnelStatus {
        let mut st = self.status.lock().clone();
        if st.state == State::Connected {
            if let Some(start) = *self.start_time.lock() {
                st.uptime_secs = start.elapsed().as_secs();
            }
        } else {
            st.uptime_secs = 0;
        }
        st
    }

    pub async fn start_tunnel(&self, app: AppHandle, cfg: TunnelConfig) -> Result<(), String> {
        if self.status.lock().state != State::Disconnected
            && self.status.lock().state != State::Error
        {
            return Err("Tunnel is already running or connecting".to_string());
        }

        let bin_path = find_aether_binary()?;
        let args = build_cli_args(&cfg);

        self.is_stopping.store(false, Ordering::SeqCst);
        *self.current_config.lock() = Some(cfg.clone());

        {
            let mut st = self.status.lock();
            st.state = State::Connecting;
            st.latency_ms = None;
            st.error_message = None;
            st.socks_endpoint = format!("127.0.0.1:{}", cfg.socks_port);
            st.system_proxy_active = false;
            let _ = app.emit("aether-status", st.clone());
        }

        let mut cmd = tokio::process::Command::new(&bin_path);
        cmd.args(&args);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn aether executable at '{}': {e}", bin_path.display()))?;

        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::Foundation::CloseHandle;
            use windows_sys::Win32::System::JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
                JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            };
            use windows_sys::Win32::System::Threading::{
                OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
            };

            if let Some(pid) = child.id() {
                let proc_handle = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if !proc_handle.is_null() {
                    let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
                    if !job.is_null() {
                        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                        SetInformationJobObject(
                            job,
                            JobObjectExtendedLimitInformation,
                            &info as *const _ as _,
                            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                        );
                        AssignProcessToJobObject(job, proc_handle);
                    }
                    CloseHandle(proc_handle);
                }
            }
        }

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        *self.child.lock() = Some(child);

        // Spawn log readers
        let app_clone = app.clone();
        let mut kill_rx = self.kill_tx.subscribe();

        let cfg_clone = cfg.clone();
        tokio::spawn(async move {
            let mut lines_out = stdout.map(|s| BufReader::new(s).lines());
            let mut lines_err = stderr.map(|s| BufReader::new(s).lines());

            loop {
                tokio::select! {
                    _ = kill_rx.recv() => {
                        break;
                    }
                    line = async {
                        if let Some(ref mut l) = lines_err {
                            l.next_line().await
                        } else {
                            std::future::pending().await
                        }
                    } => {
                        match line {
                            Ok(Some(msg)) => {
                                handle_log_line(&app_clone, &msg, &cfg_clone);
                            }
                            Ok(None) => break,
                            Err(_) => break,
                        }
                    }
                    line = async {
                        if let Some(ref mut l) = lines_out {
                            l.next_line().await
                        } else {
                            std::future::pending().await
                        }
                    } => {
                        match line {
                            Ok(Some(msg)) => {
                                handle_log_line(&app_clone, &msg, &cfg_clone);
                            }
                            Ok(None) => break,
                            Err(_) => break,
                        }
                    }
                }
            }
        });

        Ok(())
    }

    pub async fn stop_tunnel(&self, app: AppHandle) -> Result<(), String> {
        self.is_stopping.store(true, Ordering::SeqCst);
        let _ = self.kill_tx.send(());

        let child_opt = self.child.lock().take();
        if let Some(mut child) = child_opt {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }

        // Deactivate system proxy if enabled
        let _ = set_windows_proxy(false, "127.0.0.1:1819", None, "");

        {
            let mut st = self.status.lock();
            st.state = State::Disconnected;
            st.system_proxy_active = false;
            st.latency_ms = None;
            st.uptime_secs = 0;
            let _ = app.emit("aether-status", st.clone());
        }
        *self.start_time.lock() = None;

        let entry = LogEntry {
            timestamp: chrono_now(),
            level: "INFO".to_string(),
            message: "Tunnel stopped.".to_string(),
        };
        let _ = app.emit("aether-log", entry);

        Ok(())
    }

    pub fn set_connected(&self, app: &AppHandle, cfg: &TunnelConfig) {
        let mut st = self.status.lock();
        st.state = State::Connected;
        *self.start_time.lock() = Some(Instant::now());

        if cfg.auto_system_proxy {
            let socks_addr = format!("127.0.0.1:{}", cfg.socks_port);
            let http_addr = cfg.http_port.map(|p| format!("127.0.0.1:{p}"));
            if let Ok(()) = set_windows_proxy(true, &socks_addr, http_addr.as_deref(), &cfg.bypass_list) {
                st.system_proxy_active = true;
            }
        }

        let _ = app.emit("aether-status", st.clone());
    }

    pub fn set_scanning(&self, app: &AppHandle) {
        let mut st = self.status.lock();
        st.state = State::Scanning;
        let _ = app.emit("aether-status", st.clone());
    }

    pub fn set_reconnecting(&self, app: &AppHandle) {
        let mut st = self.status.lock();
        st.state = State::Reconnecting;
        let _ = app.emit("aether-status", st.clone());
    }

    pub fn set_latency(&self, app: &AppHandle, ms: u64) {
        let mut st = self.status.lock();
        st.latency_ms = Some(ms);
        let _ = app.emit("aether-status", st.clone());
    }
}

fn chrono_now() -> String {
    let now = std::time::SystemTime::now();
    let dt = chrono::DateTime::<chrono::Local>::from(now);
    dt.format("%H:%M:%S").to_string()
}

fn handle_log_line(app: &AppHandle, line: &str, cfg: &TunnelConfig) {
    let level = if line.contains("ERROR") || line.contains("[-] ") || line.starts_with("error:") {
        "ERROR"
    } else if line.contains("WARN") || line.contains("[!] ") {
        "WARN"
    } else if line.contains("DEBUG") {
        "DEBUG"
    } else {
        "INFO"
    };

    let entry = LogEntry {
        timestamp: chrono_now(),
        level: level.to_string(),
        message: line.to_string(),
    };
    let _ = app.emit("aether-log", entry);

    // State machine extraction
    if line.contains("socks5 server listening") || line.contains("exposing socks5") || line.contains("wireguard tunnel validated (end-to-end data confirmed)") {
        if let Some(state) = app.try_state::<Arc<Supervisor>>() {
            state.set_connected(app, cfg);
        }
    } else if line.contains("hunting for a working") || line.contains("hunting for") {
        if let Some(state) = app.try_state::<Arc<Supervisor>>() {
            state.set_scanning(app);
        }
    } else if line.contains("reconnecting") || line.contains("tunnel closed; reconnecting") {
        if let Some(state) = app.try_state::<Arc<Supervisor>>() {
            state.set_reconnecting(app);
        }
    } else if let Some(idx) = line.find("(rtt ") {
        let rest = &line[idx + 5..];
        if let Some(end) = rest.find(')') {
            let raw_rtt = &rest[..end];
            // Format can be "84ms" or "84.2ms" or "Duration { ... }"
            let num: String = raw_rtt.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            if let Ok(ms_f) = num.parse::<f64>() {
                if let Some(state) = app.try_state::<Arc<Supervisor>>() {
                    state.set_latency(app, ms_f as u64);
                }
            }
        }
    }
}

fn find_aether_binary() -> Result<PathBuf, String> {
    // 1. Check directory of current executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join("aether.exe");
            if candidate.is_file() {
                return Ok(candidate);
            }
            let candidate_unix = parent.join("aether");
            if candidate_unix.is_file() {
                return Ok(candidate_unix);
            }
        }
    }

    // 2. Relative development paths
    let dev_candidates = [
        "target/release/aether.exe",
        "target/debug/aether.exe",
        "../target/release/aether.exe",
        "../target/debug/aether.exe",
        "../../target/release/aether.exe",
        "aether/target/release/aether.exe",
        "../aether/target/release/aether.exe",
        "target/release/aether",
        "target/debug/aether",
        "../target/release/aether",
        "aether/target/release/aether",
    ];

    for rel in &dev_candidates {
        let p = Path::new(rel);
        if p.is_file() {
            return Ok(p.to_path_buf());
        }
    }

    // 3. Check system PATH
    if let Ok(output) = std::process::Command::new("where").arg("aether").output() {
        if output.status.success() {
            let out = String::from_utf8_lossy(&output.stdout);
            if let Some(first) = out.lines().next() {
                let p = PathBuf::from(first.trim());
                if p.is_file() {
                    return Ok(p);
                }
            }
        }
    }

    Err("Could not find 'aether.exe'. Please ensure aether.exe is located in the same folder as aether-gui.exe.".to_string())
}

fn build_cli_args(cfg: &TunnelConfig) -> Vec<String> {
    let mut args = Vec::new();

    // SOCKS5 and optional HTTP proxy
    args.push("--bind".to_string());
    args.push(format!("127.0.0.1:{}", cfg.socks_port));

    if let Some(http) = cfg.http_port {
        args.push("--http-proxy".to_string());
        args.push(format!("127.0.0.1:{http}"));
    }

    // Protocol
    match cfg.protocol.as_str() {
        "masque" => {
            args.push("--masque".to_string());
        }
        "masque-h2" => {
            args.push("--masque".to_string());
            args.push("--h2".to_string());
            if cfg.fragment {
                args.push("--fragment".to_string());
                if let Some(ref sz) = cfg.fragment_size {
                    args.push("--fragment-size".to_string());
                    args.push(sz.clone());
                }
                if let Some(ref dl) = cfg.fragment_delay {
                    args.push("--fragment-delay".to_string());
                    args.push(dl.clone());
                }
            }
        }
        "wg" | "wireguard" => {
            args.push("--wg".to_string());
        }
        "gool" | "wiw" => {
            args.push("--gool".to_string());
            if let Some(ref out) = cfg.wiw_outer {
                args.push("--wiw-outer".to_string());
                args.push(out.clone());
            }
            if let Some(ref inn) = cfg.wiw_inner {
                args.push("--wiw-inner".to_string());
                args.push(inn.clone());
            }
        }
        "mim" => {
            args.push("--mim".to_string());
            if let Some(ref out) = cfg.mim_outer {
                args.push("--mim-outer".to_string());
                args.push(out.clone());
            }
            if let Some(ref inn) = cfg.mim_inner {
                args.push("--mim-inner".to_string());
                args.push(inn.clone());
            }
        }
        "tor" => {
            args.push("--tor".to_string());
        }
        "tor-reverse" => {
            args.push("--tor-reverse".to_string());
        }
        "tor-only" => {
            args.push("--tor-only".to_string());
        }
        _ => {
            args.push("--masque".to_string());
        }
    }

    // Scan mode
    args.push("--scan".to_string());
    args.push(cfg.scan_mode.clone());

    // IP Family
    match cfg.ip_family.as_str() {
        "v6" | "6" => args.push("-6".to_string()),
        "both" | "dual" => args.push("--dual".to_string()),
        _ => args.push("-4".to_string()),
    }

    // Obfuscation Noise
    args.push("--noize".to_string());
    args.push(cfg.noize.clone());

    // Manual peer
    if let Some(ref p) = cfg.peer {
        if !p.trim().is_empty() {
            args.push("--peer".to_string());
            args.push(p.trim().to_string());
        }
    }

    // Keepalive
    args.push("--keepalive".to_string());
    args.push(cfg.keepalive.to_string());

    // DNS
    if let Some(ref d) = cfg.dns {
        if !d.trim().is_empty() {
            args.push("--dns".to_string());
            args.push(d.trim().to_string());
        }
    }

    // Upstream proxy
    if let Some(ref u) = cfg.upstream {
        if !u.trim().is_empty() {
            args.push("--upstream".to_string());
            args.push(u.trim().to_string());
        }
    }

    // Routing rules
    if let Some(ref rd) = cfg.route_direct {
        if !rd.trim().is_empty() {
            args.push("--route-direct".to_string());
            args.push(rd.trim().to_string());
        }
    }
    if let Some(ref rb) = cfg.route_block {
        if !rb.trim().is_empty() {
            args.push("--route-block".to_string());
            args.push(rb.trim().to_string());
        }
    }

    // Zero Trust
    if let Some(ref t) = cfg.team {
        if !t.trim().is_empty() {
            args.push("--team".to_string());
            args.push(t.trim().to_string());
            if let Some(ref em) = cfg.access_email {
                args.push("--access-email".to_string());
                args.push(em.trim().to_string());
            }
            if let Some(ref tk) = cfg.access_token {
                args.push("--access-token".to_string());
                args.push(tk.trim().to_string());
            }
            if let Some(ref id) = cfg.access_id {
                args.push("--access-id".to_string());
                args.push(id.trim().to_string());
            }
            if let Some(ref sec) = cfg.access_secret {
                args.push("--access-secret".to_string());
                args.push(sec.trim().to_string());
            }
        }
    }

    args
}
