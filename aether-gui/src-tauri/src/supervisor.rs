use std::path::{Path, PathBuf};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::sync::{broadcast, mpsc, Notify, Semaphore};
use tokio::task::JoinSet;

use crate::config::save_config;
use crate::matrix::{self, Candidate, SuccessResult};
use crate::ping::measure_latency;
use crate::proxy::set_windows_proxy;
use crate::types::{LogEntry, State, TunnelConfig, TunnelStatus};

#[cfg(windows)]
pub struct KillOnCloseJob(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
unsafe impl Send for KillOnCloseJob {}
#[cfg(windows)]
unsafe impl Sync for KillOnCloseJob {}

#[cfg(windows)]
impl Drop for KillOnCloseJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(not(windows))]
pub struct KillOnCloseJob;

#[cfg(windows)]
fn attach_kill_on_close_job(child: &Child) -> Option<KillOnCloseJob> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
        JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

    let pid = child.id()?;
    unsafe {
        let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
        if process.is_null() {
            return None;
        }
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            CloseHandle(process);
            return None;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) != 0;
        let assigned = configured && AssignProcessToJobObject(job, process) != 0;
        CloseHandle(process);
        if assigned {
            Some(KillOnCloseJob(job))
        } else {
            CloseHandle(job);
            None
        }
    }
}

#[cfg(not(windows))]
fn attach_kill_on_close_job(_child: &Child) -> Option<KillOnCloseJob> {
    Some(KillOnCloseJob)
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) {
    if let Err(_) = std::fs::create_dir_all(dst) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(src) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &target);
        } else {
            let _ = std::fs::copy(&path, &target);
        }
    }
}

/// Automatically preserve all identity files in %APPDATA%\Aether so that
/// downloading/unzipping a fresh build in a new directory never wipes out
/// existing Cloudflare WARP/MASQUE credentials or Psiphon state.
fn sync_appdata_identities() {
    let Ok(appdata) = std::env::var("APPDATA") else {
        return;
    };
    let appdata_dir = std::path::PathBuf::from(appdata).join("Aether");
    let _ = std::fs::create_dir_all(&appdata_dir);

    let current_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let toml_names = [
        "aether.toml",
        "aether-secondary.toml",
        "aether-masque.toml",
        "aether-masque-secondary.toml",
        "aether-lastconn.toml",
        "aether-masque-lastconn.toml",
    ];

    for name in &toml_names {
        let appdata_file = appdata_dir.join(name);
        let current_file = current_dir.join(name);
        if !current_file.exists() && appdata_file.exists() {
            let _ = std::fs::copy(&appdata_file, &current_file);
        } else if current_file.exists() && !appdata_file.exists() {
            let _ = std::fs::copy(&current_file, &appdata_file);
        }
    }

    let appdata_psiphon = appdata_dir.join("aether.toml-psiphon");
    let current_psiphon = current_dir.join("aether.toml-psiphon");
    if !current_psiphon.exists() && appdata_psiphon.exists() {
        copy_dir_recursive(&appdata_psiphon, &current_psiphon);
    }
}

fn backup_identities_to_appdata() {
    let Ok(appdata) = std::env::var("APPDATA") else {
        return;
    };
    let appdata_dir = std::path::PathBuf::from(appdata).join("Aether");
    let _ = std::fs::create_dir_all(&appdata_dir);

    let current_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let toml_names = [
        "aether.toml",
        "aether-secondary.toml",
        "aether-masque.toml",
        "aether-masque-secondary.toml",
        "aether-lastconn.toml",
        "aether-masque-lastconn.toml",
    ];

    for name in &toml_names {
        let current_file = current_dir.join(name);
        let appdata_file = appdata_dir.join(name);
        if current_file.exists() {
            let _ = std::fs::copy(&current_file, &appdata_file);
        }
    }

    let current_psiphon = current_dir.join("aether.toml-psiphon");
    let appdata_psiphon = appdata_dir.join("aether.toml-psiphon");
    if current_psiphon.exists() {
        copy_dir_recursive(&current_psiphon, &appdata_psiphon);
    }
}

pub struct Supervisor {
    child: Mutex<Option<Child>>,
    child_stdin: Mutex<Option<tokio::process::ChildStdin>>,
    current_job: Mutex<Option<KillOnCloseJob>>,
    current_config: Mutex<Option<TunnelConfig>>,
    status: Mutex<TunnelStatus>,
    start_time: Mutex<Option<Instant>>,
    kill_tx: broadcast::Sender<()>,
    connected_notify: Arc<Notify>,
    is_stopping: AtomicBool,
}

impl Supervisor {
    pub fn new() -> Self {
        let (kill_tx, _) = broadcast::channel(8);
        Self {
            child: Mutex::new(None),
            child_stdin: Mutex::new(None),
            current_job: Mutex::new(None),
            current_config: Mutex::new(None),
            status: Mutex::new(TunnelStatus {
                state: State::Disconnected,
                latency_ms: None,
                uptime_secs: 0,
                socks_endpoint: "127.0.0.1:1819".to_string(),
                system_proxy_active: false,
                protocol: None,
                exit_ip: None,
                colo: None,
                error_message: None,
            }),
            start_time: Mutex::new(None),
            kill_tx,
            connected_notify: Arc::new(Notify::new()),
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
        self.kill_current_child().await;

        if self.status.lock().state != State::Disconnected
            && self.status.lock().state != State::Error
        {
            return Err("Tunnel is already running or connecting".to_string());
        }

        let bin_path = find_aether_binary()?;
        sync_appdata_identities();
        self.is_stopping.store(false, Ordering::SeqCst);
        *self.current_config.lock() = Some(cfg.clone());

        // Initialize Status
        {
            let mut st = self.status.lock();
            st.state = State::Connecting;
            st.latency_ms = None;
            st.protocol = None;
            st.error_message = None;
            st.socks_endpoint = format!("127.0.0.1:{}", cfg.socks_port);
            st.system_proxy_active = false;
            let _ = app.emit("aether-status", st.clone());
        }
        let _ = app.emit(
            "aether-progress",
            serde_json::json!({ "percent": 10, "stage": "Initializing Engine..." }),
        );

        // Auto Mode is deliberately disabled for an explicit Tor or Psiphon
        // configuration. Both are opt-in transports, not background matrix
        // dimensions.
        if cfg.auto_connect && !cfg.tor_enabled && !cfg.psiphon_enabled {
            return self.run_auto_matrix(&bin_path, &app, cfg).await;
        }

        // ===================================================================
        // MANUAL MODE (Uses the exact protocol chosen in Settings)
        // ===================================================================
        let args = build_cli_args(&cfg);
        self.spawn_process(&bin_path, &args, &app, &cfg, None).await
    }

    /// Race the remembered route and strong fallbacks first. Only if that tier
    /// is exhausted do we release the full 48-combination matrix, three engine
    /// processes at a time. Probe instances are bound to isolated local ports;
    /// the selected configuration is restarted on the user's real proxy port.
    async fn run_auto_matrix(
        &self,
        bin_path: &Path,
        app: &AppHandle,
        cfg: TunnelConfig,
    ) -> Result<(), String> {
        const MAX_CONCURRENT: usize = 3;
        const SOCKS_PORT_BASE: u16 = 18_200;
        const HTTP_PORT_BASE: u16 = 19_200;
        const GRACE_WINDOW: Duration = Duration::from_secs(4);

        let scan_mode = if cfg.scan_mode.trim().is_empty() {
            "balanced".to_string()
        } else {
            cfg.scan_mode.clone()
        };
        let candidate_timeout = Duration::from_secs(matrix::timeout_for_scan_mode(&scan_mode));
        let priority = matrix::prioritized_candidates(&cfg);
        let priority_keys: HashSet<Candidate> = priority.iter().cloned().collect();
        let fallback = matrix::full_matrix()
            .into_iter()
            .filter(|candidate| !priority_keys.contains(candidate))
            .collect::<Vec<_>>();

        emit_auto_log(
            app,
            "INFO",
            format!(
                "[AUTO] Starting priority race ({} route(s), {}, {}s per candidate, max {} parallel).",
                priority.len(), scan_mode, candidate_timeout.as_secs(), MAX_CONCURRENT
            ),
        );

        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT));
        let (event_tx, mut event_rx) = mpsc::channel::<CandidateEvent>(32);
        let (cancel_tx, _) = broadcast::channel::<()>(MAX_CONCURRENT + 1);
        let mut external_stop_rx = self.kill_tx.subscribe();
        let mut workers = JoinSet::new();
        let mut running = HashSet::<u64>::new();
        let mut next_id = 0_u64;
        let mut queue = VecDeque::from(priority);
        let mut fallback_pending = Some(fallback);
        let mut successes = Vec::<SuccessResult>::new();
        let mut grace_deadline: Option<tokio::time::Instant> = None;

        loop {
            if self.is_stopping.load(Ordering::SeqCst) {
                let _ = cancel_tx.send(());
                while workers.join_next().await.is_some() {}
                return Ok(());
            }

            // A successful candidate freezes the queue. We only allow the
            // already-running candidates to finish during the exact 4s grace.
            while grace_deadline.is_none() && running.len() < MAX_CONCURRENT {
                let Some(candidate) = queue.pop_front() else { break };
                let id = next_id;
                next_id += 1;
                // Each worker owns a 100-port lane. It chooses a free pair
                // inside that lane before spawning and can retry on bind
                // failure without colliding with another active worker.
                let socks_port = SOCKS_PORT_BASE + (id as u16 * 100);
                let http_port = HTTP_PORT_BASE + (id as u16 * 100);
                running.insert(id);

                emit_auto_log(
                    app,
                    "INFO",
                    format!("[AUTO] Testing {}...", candidate.display_name()),
                );
                {
                    let mut st = self.status.lock();
                    st.protocol = Some(format!("AUTO: Testing {}", candidate.display_name()));
                    let _ = app.emit("aether-status", st.clone());
                }

                let permit = semaphore.clone();
                let event_tx = event_tx.clone();
                let app = app.clone();
                let bin_path = bin_path.to_path_buf();
                let base_cfg = cfg.clone();
                let cancel_rx = cancel_tx.subscribe();
                let stop_rx = self.kill_tx.subscribe();
                workers.spawn(async move {
                    let _permit = permit.acquire_owned().await.expect("matrix semaphore closed");
                    run_candidate(
                        id,
                        candidate,
                        base_cfg,
                        bin_path,
                        socks_port,
                        http_port,
                        candidate_timeout,
                        app,
                        event_tx,
                        cancel_rx,
                        stop_rx,
                    )
                    .await;
                });
            }

            // The priority tier failed: only now is the larger 48-item
            // matrix released. It continues to use the same three-slot cap.
            if running.is_empty() && queue.is_empty() && grace_deadline.is_none() {
                if let Some(fallback) = fallback_pending.take() {
                    emit_auto_log(
                        app,
                        "WARN",
                        format!(
                            "[AUTO] Priority routes failed; expanding to the full {}-candidate matrix.",
                            fallback.len()
                        ),
                    );
                    queue = VecDeque::from(fallback);
                    continue;
                }

                {
                    let mut st = self.status.lock();
                    st.state = State::Error;
                    st.error_message = Some(
                        "Auto mode exhausted every protocol, obfuscation, and IP-version combination. Check basic network connectivity or enable Tor Integration."
                            .to_string(),
                    );
                    let _ = app.emit("aether-status", st.clone());
                }
                emit_auto_log(
                    app,
                    "ERROR",
                    "[AUTO] All protocol / noize / IP-version combinations were exhausted. Check connectivity or try Tor Integration.".to_string(),
                );
                let _ = cancel_tx.send(());
                while workers.join_next().await.is_some() {}
                return Err("Auto connection exhausted all candidates.".to_string());
            }

            if let Some(deadline) = grace_deadline {
                tokio::select! {
                    _ = external_stop_rx.recv() => {
                        self.is_stopping.store(true, Ordering::SeqCst);
                        let _ = cancel_tx.send(());
                        while workers.join_next().await.is_some() {}
                        return Ok(());
                    }
                    _ = tokio::time::sleep_until(deadline) => {
                        let winner = successes.iter().min_by_key(|success| success.latency_ms)
                            .cloned().expect("grace window only starts after a success");
                        let beat = successes.len().saturating_sub(1);
                        let _ = cancel_tx.send(());
                        while workers.join_next().await.is_some() {}

                        emit_auto_log(
                            app,
                            "INFO",
                            format!(
                                "[AUTO] Connected via {} — {}ms, beat {} other candidate(s). Restarting on your configured local proxy port...",
                                winner.candidate.display_name(), winner.latency_ms, beat
                            ),
                        );

                        let mut selected_cfg = cfg.clone();
                        selected_cfg.protocol = winner.candidate.protocol.clone();
                        selected_cfg.noize = winner.candidate.noize.clone();
                        selected_cfg.ip_family = winner.candidate.ip_family.clone();
                        selected_cfg.scan_mode = scan_mode.clone();
                        selected_cfg.last_success_proto = Some(selected_cfg.protocol.clone());
                        selected_cfg.last_success_noize = Some(selected_cfg.noize.clone());
                        selected_cfg.last_success_ip = Some(selected_cfg.ip_family.clone());
                        *self.current_config.lock() = Some(selected_cfg.clone());
                        let _ = save_config(&selected_cfg);

                        {
                            let mut st = self.status.lock();
                            st.state = State::Connecting;
                            st.protocol = Some("AUTO: Switching to final port…".to_string());
                            let _ = app.emit("aether-status", st.clone());
                        }
                        emit_auto_log(
                            app,
                            "INFO",
                            format!("[AUTO] Switching to final port 127.0.0.1:{}…", selected_cfg.socks_port),
                        );

                        let custom_proto = format!("AUTO: {}", winner.candidate.display_name());
                        let args = build_cli_args(&selected_cfg);
                        return self.spawn_process(bin_path, &args, app, &selected_cfg, Some(&custom_proto)).await;
                    }
                    Some(event) = event_rx.recv() => {
                        match event {
                            CandidateEvent::Success { id, result } => {
                                if running.contains(&id) {
                                    emit_auto_log(app, "INFO", format!("[AUTO] {} ready in {}ms; comparing active routes...", result.candidate.display_name(), result.latency_ms));
                                    successes.push(result);
                                }
                            }
                            CandidateEvent::Finished { id, message } => {
                                running.remove(&id);
                                emit_auto_log(app, "WARN", format!("[AUTO] Candidate {id} finished during grace: {message}"));
                            }
                        }
                    }
                }
            } else {
                tokio::select! {
                    _ = external_stop_rx.recv() => {
                        self.is_stopping.store(true, Ordering::SeqCst);
                        let _ = cancel_tx.send(());
                        while workers.join_next().await.is_some() {}
                        return Ok(());
                    }
                    Some(event) = event_rx.recv() => match event {
                        CandidateEvent::Success { id, result } => {
                            if running.contains(&id) {
                                emit_auto_log(app, "INFO", format!("[AUTO] {} ready in {}ms; collecting results for 4 seconds...", result.candidate.display_name(), result.latency_ms));
                                successes.push(result);
                                grace_deadline = Some(tokio::time::Instant::now() + GRACE_WINDOW);
                            }
                        }
                        CandidateEvent::Finished { id, message } => {
                            if running.remove(&id) {
                                emit_auto_log(app, "WARN", format!("[AUTO] Candidate {id} failed: {message}"));
                            }
                        }
                    },
                    else => return Err("Auto candidate event channel closed unexpectedly.".to_string()),
                }
            }
        }
    }

    async fn spawn_process(
        &self,
        bin_path: &Path,
        args: &[String],
        app: &AppHandle,
        cfg: &TunnelConfig,
        custom_proto: Option<&str>,
    ) -> Result<(), String> {
        let mut cmd = tokio::process::Command::new(bin_path);
        cmd.args(args);
        // stdin stays piped instead of null: the engine never blocks on it
        // except when a Zero Trust email login code is due, and the GUI answers
        // that over stdin from the OTP dialog. Nothing else is ever written.
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        // There is no --tor-country flag; the engine only reads the env var.
        if cfg.tor_bridges {
            if let Some(country) = cfg
                .tor_country
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty() && *c != "auto")
            {
                cmd.env("AETHER_TOR_COUNTRY", country);
            }
        }

        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn aether executable at '{}': {e}", bin_path.display()))?;

        *self.current_job.lock() = attach_kill_on_close_job(&child);

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdin = child.stdin.take();

        *self.child_stdin.lock() = stdin;
        *self.child.lock() = Some(child);

        // Spawn log readers
        let app_clone = app.clone();
        let mut kill_rx = self.kill_tx.subscribe();
        let cfg_clone = cfg.clone();
        let custom_proto_owned = custom_proto.map(|s| s.to_string());

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
                                handle_log_line(&app_clone, &msg, &cfg_clone, custom_proto_owned.as_deref());
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
                                handle_log_line(&app_clone, &msg, &cfg_clone, custom_proto_owned.as_deref());
                            }
                            Ok(None) => break,
                            Err(_) => break,
                        }
                    }
                }
            }

            if let Some(state) = app_clone.try_state::<Arc<Supervisor>>() {
                if !state.is_stopping.load(Ordering::SeqCst) {
                    let mut st = state.status.lock();
                    if st.state == State::Connecting {
                        st.state = State::Error;
                        if st.error_message.is_none() {
                            st.error_message = Some("Tunnel process exited unexpectedly".to_string());
                        }
                        let _ = app_clone.emit("aether-status", st.clone());
                    }
                }
            }
        });

        Ok(())
    }

    async fn kill_current_child(&self) {
        *self.child_stdin.lock() = None;
        *self.current_job.lock() = None;
        let child_opt = self.child.lock().take();
        if let Some(mut child) = child_opt {
            #[cfg(windows)]
            if let Some(pid) = child.id() {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x08000000;
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .creation_flags(CREATE_NO_WINDOW)
                    .output();
            }
            let _ = child.kill().await;
            let _ = child.wait().await;
        }

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/IM", "psiphon-tunnel-core.exe"])
                .creation_flags(CREATE_NO_WINDOW)
                .output();
        }
    }

    /// Answer the engine's Zero Trust email-code prompt. The child announces
    /// the prompt on a log line and reads the answer from stdin, one line.
    /// The stdin handle is taken out of the lock before any await so the
    /// future stays Send, and is put back for a later prompt (a rejected code
    /// makes the engine ask again).
    pub async fn submit_team_code(&self, code: String) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;
        let mut taken = self.child_stdin.lock().take();
        let Some(ref mut writer) = taken else {
            return Err("no tunnel process is waiting for a code".to_string());
        };
        writer
            .write_all(code.trim().as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        writer
            .write_all(b"\n")
            .await
            .map_err(|e| e.to_string())?;
        writer.flush().await.map_err(|e| e.to_string())?;
        *self.child_stdin.lock() = taken;
        Ok(())
    }

    pub async fn stop_tunnel(&self, app: AppHandle) -> Result<(), String> {
        self.is_stopping.store(true, Ordering::SeqCst);
        let _ = self.kill_tx.send(());

        self.kill_current_child().await;

        // Deactivate system proxy if enabled, pointing at the port that was
        // actually set, not a hardcoded default.
        {
            let saved = self.current_config.lock().clone().unwrap_or_default();
            let socks_addr = format!("127.0.0.1:{}", saved.socks_port);
            let tor_only = saved.tor_enabled && saved.tor_mode == "tor-only";
            let psiphon_only = saved.psiphon_enabled && saved.psiphon_mode == "psiphon-only";
            let http_addr = if tor_only {
                None
            } else if psiphon_only {
                saved
                    .psiphon_http
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .or_else(|| saved.http_port.map(|p| format!("127.0.0.1:{p}")))
                    .or_else(|| Some("127.0.0.1:1820".to_string()))
            } else {
                let p = saved.http_port.unwrap_or(1820);
                Some(format!("127.0.0.1:{p}"))
            };
            let _ = set_windows_proxy(false, &socks_addr, http_addr.as_deref(), "");
        }

        {
            let mut st = self.status.lock();
            st.state = State::Disconnected;
            st.system_proxy_active = false;
            st.latency_ms = None;
            st.uptime_secs = 0;
            let _ = app.emit("aether-status", st.clone());
        }
        *self.start_time.lock() = None;
        let _ = app.emit(
            "aether-progress",
            serde_json::json!({ "percent": 0, "stage": "Disconnected" }),
        );

        let entry = LogEntry {
            timestamp: chrono_now(),
            level: "INFO".to_string(),
            message: "Tunnel stopped.".to_string(),
        };
        let _ = app.emit("aether-log", entry);

        backup_identities_to_appdata();

        Ok(())
    }

    pub fn set_connected(&self, app: &AppHandle, cfg: &TunnelConfig, custom_proto: Option<&str>) {
        let mut st = self.status.lock();
        st.state = State::Connected;
        if let Some(proto) = custom_proto {
            st.protocol = Some(proto.to_string());
        }
        *self.start_time.lock() = Some(Instant::now());

        if cfg.auto_system_proxy || cfg.tunnel_mode == "system-wide" {
            let socks_addr = format!("127.0.0.1:{}", cfg.socks_port);
            let tor_only = cfg.tor_enabled && cfg.tor_mode == "tor-only";
            let psiphon_only = cfg.psiphon_enabled && cfg.psiphon_mode == "psiphon-only";
            let http_addr = if tor_only {
                None
            } else if psiphon_only {
                cfg.psiphon_http
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .or_else(|| cfg.http_port.map(|p| format!("127.0.0.1:{p}")))
                    .or_else(|| Some("127.0.0.1:1820".to_string()))
            } else {
                let p = cfg.http_port.unwrap_or(1820);
                Some(format!("127.0.0.1:{p}"))
            };
            if let Ok(()) =
                set_windows_proxy(true, &socks_addr, http_addr.as_deref(), &cfg.bypass_list)
            {
                st.system_proxy_active = true;
            }
        }

        let _ = app.emit("aether-status", st.clone());
        backup_identities_to_appdata();
        let _ = app.emit(
            "aether-progress",
            serde_json::json!({ "percent": 100, "stage": "Connected" }),
        );
        self.connected_notify.notify_waiters();
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

    pub fn set_error(&self, app: &AppHandle, msg: String) {
        let mut st = self.status.lock();
        st.state = State::Error;
        st.error_message = Some(msg);
        let _ = app.emit("aether-status", st.clone());
        let _ = app.emit(
            "aether-progress",
            serde_json::json!({ "percent": 0, "stage": "Connection Error" }),
        );
    }
}

enum CandidateEvent {
    Success { id: u64, result: SuccessResult },
    Finished { id: u64, message: String },
}

#[allow(clippy::too_many_arguments)]
async fn run_candidate(
    id: u64,
    candidate: Candidate,
    mut cfg: TunnelConfig,
    bin_path: PathBuf,
    socks_port_start: u16,
    http_port_start: u16,
    timeout: Duration,
    app: AppHandle,
    event_tx: mpsc::Sender<CandidateEvent>,
    mut cancel_rx: broadcast::Receiver<()>,
    mut stop_rx: broadcast::Receiver<()>,
) {
    cfg.protocol = candidate.protocol.clone();
    cfg.noize = candidate.noize.clone();
    cfg.ip_family = candidate.ip_family.clone();
    let mut lane_offset = 0_u16;
    for _attempt in 0..MAX_PORT_ATTEMPTS {
        let Some((socks_port, http_port)) =
            find_free_port_pair(socks_port_start, http_port_start, lane_offset)
        else {
            break;
        };
        cfg.socks_port = socks_port;
        cfg.http_port = Some(http_port);

        match run_candidate_once(
            id,
            candidate.clone(),
            cfg.clone(),
            &bin_path,
            socks_port,
            timeout,
            &app,
            &event_tx,
            &mut cancel_rx,
            &mut stop_rx,
        ).await {
            CandidateRunResult::PortConflict => {
                emit_auto_log(
                    &app,
                    "WARN",
                    format!(
                        "[AUTO #{id}] local ports {socks_port}/{http_port} conflicted; retrying at the next free pair in this lane."
                    ),
                );
                // The scan returns the first free pair, so without stepping
                // past the pair that just failed the retry would probe the
                // same two ports again and waste the remaining attempts.
                lane_offset = socks_port.saturating_sub(socks_port_start) + 1;
            }
            CandidateRunResult::Complete => return,
        }
    }

    let _ = event_tx.send(CandidateEvent::Finished {
        id,
        message: "could not reserve an isolated SOCKS/HTTP port pair".to_string(),
    }).await;
}

enum CandidateRunResult { PortConflict, Complete }

fn is_data_plane_confirmation(line: &str) -> bool {
    // QUIC MASQUE, H2 MASQUE, direct WG, WARP-in-WARP and MASQUE-in-MASQUE
    // all emit this stable success suffix only after their end-to-end probe.
    line.contains("tunnel validated (end-to-end data confirmed)")
}

fn parse_exit_line(line: &str) -> Option<(String, Option<String>, Option<String>, Option<u64>)> {
    let marker = if let Some(idx) = line.find("exit: ") {
        &line[idx + 6..]
    } else if let Some(idx) = line.find("egress: ") {
        &line[idx + 8..]
    } else {
        return None;
    };

    let parts: Vec<&str> = marker.split(',').map(str::trim).collect();
    if parts.is_empty() {
        return None;
    }

    let ip = parts[0].to_string();
    let mut loc: Option<String> = None;
    let mut colo: Option<String> = None;
    let mut latency: Option<u64> = None;

    for part in &parts[1..] {
        if part.contains("via") {
            let sub: Vec<&str> = part.split("via").map(str::trim).collect();
            if sub.len() == 2 {
                loc = Some(sub[0].to_string());
                colo = Some(sub[1].to_string());
            }
        } else if part.ends_with("ms to cloudflare") || part.ends_with("ms") {
            let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(ms) = digits.parse::<u64>() {
                latency = Some(ms);
            }
        } else if part.len() == 2 && part.chars().all(|c| c.is_ascii_uppercase()) {
            loc = Some(part.to_string());
        } else if part.len() == 3 && part.chars().all(|c| c.is_ascii_uppercase()) {
            colo = Some(part.to_string());
        }
    }

    Some((ip, colo, loc, latency))
}

fn extract_progress(line: &str) -> Option<(u8, String)> {
    if line.contains("hunting for a working") || line.contains("hunting for") {
        Some((20, "Hunting for optimal endpoint...".to_string()))
    } else if line.contains("[AUTO #") {
        Some((35, "Testing route candidates...".to_string()))
    } else if line.contains("starting psiphon") {
        Some((30, "Starting Psiphon engine...".to_string()))
    } else if line.contains("psiphon: Config migration") {
        Some((45, "Initializing Psiphon config...".to_string()))
    } else if line.contains("psiphon reached a server") {
        Some((75, "Psiphon server connected".to_string()))
    } else if line.contains("bootstrapping tor") {
        Some((25, "Bootstrapping Tor network...".to_string()))
    } else if line.contains("tor bootstrap:") {
        if let Some(idx) = line.find("tor bootstrap: ") {
            let rest = &line[idx + 15..];
            if let Some(pct_end) = rest.find('%') {
                if let Ok(pct) = rest[..pct_end].trim().parse::<u8>() {
                    let scaled = 30 + ((pct as f32 / 100.0) * 60.0) as u8;
                    let msg = rest[pct_end + 1..].trim_start_matches(':').trim();
                    let stage = if !msg.is_empty() {
                        format!("Tor: {msg} ({pct}%)")
                    } else {
                        format!("Tor Bootstrap {pct}%")
                    };
                    return Some((scaled.min(92), stage));
                }
            }
        }
        Some((50, "Tor bootstrapping...".to_string()))
    } else if line.contains("psiphon is ready;") || line.contains("tor is ready;") {
        Some((95, "Tunnel established, finalizing proxy...".to_string()))
    } else if line.contains("tunnel validated") || line.contains("socks5 server listening") || line.contains("socks5 listening on") {
        Some((98, "Tunnel validated, setting system proxy...".to_string()))
    } else if line.contains("exit:") || line.contains("egress:") {
        Some((100, "Connected".to_string()))
    } else {
        None
    }
}

/// The engine binds its SOCKS listener only once a route is genuinely usable.
/// The MASQUE family prints "...; exposing socks5" for the outer hop of
/// MASQUE-in-MASQUE before any inner hop answers, so that line alone must
/// never be read as a connection.
fn is_socks_listener_ready(line: &str) -> bool {
    line.contains("socks5 server listening")
        || line.contains("socks5 listening on")
        || line.contains("psiphon is ready;")
        || line.contains("tor is ready;")
}

fn is_bind_conflict(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("address already in use")
        || lower.contains("only one usage of each socket address")
        || (lower.contains("bind") && lower.contains("address"))
}

/// Ports each concurrent worker owns exclusively, so two temporary engines
/// can never negotiate the same local pair and a retry stays in its own lane.
const LANE_WIDTH: u16 = 100;

/// Local port pairs one candidate may try before it is reported unusable.
const MAX_PORT_ATTEMPTS: u16 = 3;

fn find_free_port_pair(socks_base: u16, http_base: u16, start_offset: u16) -> Option<(u16, u16)> {
    for offset in start_offset..LANE_WIDTH {
        let Some(socks) = socks_base.checked_add(offset) else { break };
        let Some(http) = http_base.checked_add(offset) else { break };
        let socks_listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, socks));
        let http_listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, http));
        if let (Ok(socks_listener), Ok(http_listener)) = (socks_listener, http_listener) {
            drop(socks_listener);
            drop(http_listener);
            return Some((socks, http));
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
async fn run_candidate_once(
    id: u64,
    candidate: Candidate,
    cfg: TunnelConfig,
    bin_path: &Path,
    socks_port: u16,
    timeout: Duration,
    app: &AppHandle,
    event_tx: &mpsc::Sender<CandidateEvent>,
    cancel_rx: &mut broadcast::Receiver<()>,
    stop_rx: &mut broadcast::Receiver<()>,
) -> CandidateRunResult {
    let mut command = tokio::process::Command::new(bin_path);
    command.args(build_cli_args(&cfg));
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    { command.creation_flags(0x08000000); }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = event_tx.send(CandidateEvent::Finished { id, message: format!("could not start engine: {error}") }).await;
            return CandidateRunResult::Complete;
        }
    };
    // Keep this guard alive for the entire temporary candidate lifetime. If
    // the GUI or task dies, closing the job handle terminates the child too.
    let _job = attach_kill_on_close_job(&child);
    let mut stdout = child.stdout.take().map(|pipe| BufReader::new(pipe).lines());
    let mut stderr = child.stderr.take().map(|pipe| BufReader::new(pipe).lines());
    let deadline = tokio::time::Instant::now() + timeout;
    let mut saw_data_plane = false;
    let mut socks_bound = false;
    let mut reported_success = false;
    let mut port_conflict = false;

    loop {
        tokio::select! {
            _ = cancel_rx.recv() => { let _ = child.kill().await; let _ = child.wait().await; return CandidateRunResult::Complete; }
            _ = stop_rx.recv() => { let _ = child.kill().await; let _ = child.wait().await; return CandidateRunResult::Complete; }
            _ = tokio::time::sleep_until(deadline) => {
                let _ = child.kill().await; let _ = child.wait().await;
                if !reported_success { let _ = event_tx.send(CandidateEvent::Finished { id, message: format!("timed out after {}s", timeout.as_secs()) }).await; }
                return CandidateRunResult::Complete;
            }
            result = child.wait() => {
                if port_conflict { return CandidateRunResult::PortConflict; }
                if !reported_success {
                    let message = match result { Ok(status) => format!("engine exited ({status})"), Err(error) => format!("engine wait failed: {error}") };
                    let _ = event_tx.send(CandidateEvent::Finished { id, message }).await;
                }
                return CandidateRunResult::Complete;
            }
            line = async { match stderr.as_mut() { Some(lines) => lines.next_line().await, None => std::future::pending().await } } => {
                match line { Ok(Some(line)) => {
                    port_conflict |= is_bind_conflict(&line);
                    saw_data_plane |= is_data_plane_confirmation(&line);
                    socks_bound |= is_socks_listener_ready(&line);
                    emit_auto_log(app, log_level(&line), format!("[AUTO #{id}] {line}"));
                }, Ok(None) => stderr = None, Err(error) => { emit_auto_log(app, "WARN", format!("[AUTO #{id}] stderr read error: {error}")); stderr = None; } }
            }
            line = async { match stdout.as_mut() { Some(lines) => lines.next_line().await, None => std::future::pending().await } } => {
                match line { Ok(Some(line)) => {
                    port_conflict |= is_bind_conflict(&line);
                    saw_data_plane |= is_data_plane_confirmation(&line);
                    socks_bound |= is_socks_listener_ready(&line);
                    emit_auto_log(app, log_level(&line), format!("[AUTO #{id}] {line}"));
                }, Ok(None) => stdout = None, Err(error) => { emit_auto_log(app, "WARN", format!("[AUTO #{id}] stdout read error: {error}")); stdout = None; } }
            }
        }

        if port_conflict && !reported_success {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return CandidateRunResult::PortConflict;
        }
        if saw_data_plane && socks_bound && !reported_success {
            match tokio::time::timeout(Duration::from_secs(4), measure_latency(socks_port)).await {
                Ok(Ok(latency_ms)) => {
                    reported_success = true;
                    let _ = event_tx.send(CandidateEvent::Success { id, result: SuccessResult { candidate: candidate.clone(), latency_ms } }).await;
                }
                Ok(Err(error)) => {
                    let _ = child.kill().await; let _ = child.wait().await;
                    let _ = event_tx.send(CandidateEvent::Finished { id, message: format!("SOCKS traffic probe failed: {error}") }).await;
                    return CandidateRunResult::Complete;
                }
                Err(_) => {
                    let _ = child.kill().await; let _ = child.wait().await;
                    let _ = event_tx.send(CandidateEvent::Finished { id, message: "SOCKS traffic probe timed out".to_string() }).await;
                    return CandidateRunResult::Complete;
                }
            }
        }
    }
}

fn log_level(line: &str) -> &'static str {
    if line.contains("ERROR") || line.contains("[-] ") || line.starts_with("error:") {
        "ERROR"
    } else if line.contains("WARN") || line.contains("[!] ") {
        "WARN"
    } else {
        "INFO"
    }
}

fn emit_auto_log(app: &AppHandle, level: &str, message: String) {
    let _ = app.emit("aether-log", LogEntry {
        timestamp: chrono_now(),
        level: level.to_string(),
        message,
    });
}

fn chrono_now() -> String {
    let now = std::time::SystemTime::now();
    let dt = chrono::DateTime::<chrono::Local>::from(now);
    dt.format("%H:%M:%S").to_string()
}

/// The engine prints this marker when it is blocked waiting for a Zero Trust
/// email login code on stdin. It must match zerotrust::CODE_PROMPT_MARKER in
/// the core: "[zerotrust] login-code-needed attempt=N email=E".
const OTP_MARKER: &str = "[zerotrust] login-code-needed";

fn parse_otp_request(line: &str) -> Option<(String, u32)> {
    if !line.contains(OTP_MARKER) {
        return None;
    }
    let tail = line.split(OTP_MARKER).nth(1)?;
    let mut email: Option<String> = None;
    let mut attempt: u32 = 1;
    for token in tail.split_whitespace() {
        if let Some(v) = token.strip_prefix("email=") {
            email = Some(v.to_string());
        } else if let Some(v) = token.strip_prefix("attempt=") {
            attempt = v.parse().unwrap_or(1);
        }
    }
    email.map(|e| (e, attempt))
}

/// The engine announces the tor listener once it is genuinely carrying
/// traffic: "[+] tor is ready; 127.0.0.1:1820 leaves through tor, carried by
/// the tunnel" (the plain forms drop the trailing clause). The dashboard
/// shows its TOR badge from that moment on.
fn parse_tor_addr(line: &str) -> Option<String> {
    let rest = line.split("tor is ready;").nth(1)?;
    for word in rest.split_whitespace() {
        if let Some((host, port)) = word.rsplit_once(':') {
            if !host.is_empty() && port.parse::<u16>().is_ok() {
                return Some(word.to_string());
            }
        }
    }
    None
}

/// Same protocol as parse_tor_addr for the psiphon carrier:
/// "[+] psiphon is ready; 127.0.0.1:1821 leaves through psiphon" or
/// "[+] psiphon is ready; the tunnel goes out through 127.0.0.1:1821". The
/// dashboard shows its PSIPHON badge once the engine announces it.
fn parse_psiphon_addr(line: &str) -> Option<String> {
    let rest = line.split("psiphon is ready;").nth(1)?;
    for word in rest.split_whitespace() {
        if let Some((host, port)) = word.rsplit_once(':') {
            if !host.is_empty() && port.parse::<u16>().is_ok() {
                return Some(word.to_string());
            }
        }
    }
    None
}

fn handle_log_line(app: &AppHandle, line: &str, cfg: &TunnelConfig, custom_proto: Option<&str>) {
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

    if let Some((email, attempt)) = parse_otp_request(line) {
        let _ = app.emit(
            "aether-otp-request",
            serde_json::json!({ "email": email, "attempt": attempt }),
        );
        return;
    }

    if let Some(addr) = parse_tor_addr(line) {
        let _ = app.emit("aether-tor-addr", serde_json::json!({ "addr": addr }));
    }

    if let Some(addr) = parse_psiphon_addr(line) {
        let _ = app.emit("aether-psiphon-addr", serde_json::json!({ "addr": addr }));
    }

    // State machine extraction
    if is_socks_listener_ready(line) {
        if let Some(state) = app.try_state::<Arc<Supervisor>>() {
            state.set_connected(app, cfg, custom_proto);
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
            let num: String = raw_rtt
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(ms_f) = num.parse::<f64>() {
                if let Some(state) = app.try_state::<Arc<Supervisor>>() {
                    state.set_latency(app, ms_f as u64);
                }
            }
        }
    }

    if let Some((exit_ip, colo, loc, latency)) = parse_exit_line(line) {
        if let Some(state) = app.try_state::<Arc<Supervisor>>() {
            let mut st = state.status.lock();
            st.exit_ip = Some(exit_ip.clone());
            if let Some(c) = colo.as_ref() {
                st.colo = Some(c.clone());
            }
            if let Some(l) = latency {
                st.latency_ms = Some(l);
            }
            let _ = app.emit("aether-status", st.clone());
        }
        let _ = app.emit(
            "aether-exit-info",
            serde_json::json!({
                "ip": exit_ip,
                "colo": colo,
                "loc": loc,
                "latency_ms": latency
            }),
        );
    }

    if let Some((percent, stage)) = extract_progress(line) {
        let _ = app.emit(
            "aether-progress",
            serde_json::json!({ "percent": percent, "stage": stage }),
        );
    }

    if line.contains("the socks5 listener cannot use")
        || line.contains("Only one usage of each socket address")
    {
        if let Some(state) = app.try_state::<Arc<Supervisor>>() {
            state.set_error(app, line.to_string());
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

/// The bridge lines typed into the GUI, one per row. The engine splits
/// AETHER_TOR_BRIDGES on newlines and semicolons (see tor::bridges), so the
/// same split happens here and each line goes out as its own --tor-bridge.
fn manual_bridge_lines(cfg: &TunnelConfig) -> Vec<String> {
    cfg.tor_bridge_lines
        .as_deref()
        .unwrap_or_default()
        .split(['\n', ';'])
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn build_cli_args(cfg: &TunnelConfig) -> Vec<String> {
    let mut args = Vec::new();

    let tor_only = cfg.tor_enabled && cfg.tor_mode == "tor-only";
    let psiphon_only = cfg.psiphon_enabled && cfg.psiphon_mode == "psiphon-only";

    // SOCKS5 and optional HTTP proxy
    args.push("--bind".to_string());
    args.push(format!("127.0.0.1:{}", cfg.socks_port));

    // Only pass --http-proxy when the user asked for one. In tor-only mode the
    // --bind listener itself IS the tor exit, so an extra HTTP proxy on the
    // same port would collide with it.
    if !tor_only && !psiphon_only {
        let http = cfg.http_port.unwrap_or(1820);
        args.push("--http-proxy".to_string());
        args.push(format!("127.0.0.1:{http}"));
    }

    // CRITICAL: Prevent STDIN prompt for last connection:
    args.push("--no-quick-reconnect".to_string());

    // Protocol selection. Tor chain/reverse still rides a real transport, so
    // the protocol flags go out together with the tor flag — this is what
    // makes tor work on every protocol from the GUI. Tor-only has no tunnel
    // at all, so it gets no protocol flag.
    if !tor_only && !psiphon_only {
        match cfg.protocol.as_str() {
            "masque" => {
                args.push("--masque".to_string());
                args.push("--h3".to_string()); // CRITICAL: Sets AETHER_MASQUE_HTTP2="0" so it NEVER prompts on STDIN!
            }
            "masque-h2" => {
                args.push("--masque".to_string());
                args.push("--h2".to_string()); // Sets AETHER_MASQUE_HTTP2="1" so it NEVER prompts on STDIN!
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
            _ => {
                args.push("--masque".to_string());
                args.push("--h3".to_string());
            }
        }
    }

    if cfg.tor_enabled {
        match cfg.tor_mode.as_str() {
            "reach" => args.push("--tor-reverse".to_string()),
            "tor-only" => args.push("--tor-only".to_string()),
            _ => args.push("--tor".to_string()),
        }
        // Hand-typed bridge lines win over the automatic bridgedb fetch: the
        // engine reads them from AETHER_TOR_BRIDGES, and --tor-bridges would
        // overwrite them with "auto".
        let manual = manual_bridge_lines(cfg);
        if manual.is_empty() {
            if cfg.tor_bridges {
                args.push("--tor-bridges".to_string());
            }
        } else {
            for line in manual {
                args.push("--tor-bridge".to_string());
                args.push(line);
            }
        }
        if let Some(ref bind) = cfg.tor_bind {
            let bind = bind.trim();
            if !bind.is_empty() {
                args.push("--tor-bind".to_string());
                args.push(bind.to_string());
            }
        }
    }

    if cfg.psiphon_enabled {
        match cfg.psiphon_mode.as_str() {
            // The engine forces the http/2 carrier itself in reverse mode, so
            // the GUI keeps the user's protocol choice; a wireguard choice that
            // cannot cross psiphon is caught and reported by the engine.
            "reach" => args.push("--psiphon-reverse".to_string()),
            "psiphon-only" => args.push("--psiphon-only".to_string()),
            _ => args.push("--psiphon".to_string()),
        }

        match cfg.psiphon_shape.trim().to_ascii_lowercase().as_str() {
            "cdn" | "direct" => {
                args.push("--psiphon-mode".to_string());
                args.push(cfg.psiphon_shape.trim().to_ascii_lowercase());
            }
            _ => {}
        }
        if let Some(ref region) = cfg.psiphon_region {
            let region = region.trim().to_uppercase();
            if region.len() == 2 && region.chars().all(|c| c.is_ascii_alphabetic()) {
                args.push("--psiphon-region".to_string());
                args.push(region);
            }
        }
        let ip = cfg.psiphon_cdn_ips.trim();
        if !ip.is_empty() {
            args.push("--psiphon-cdn-ips".to_string());
            args.push(ip.to_string());
        }
        let sni = cfg.psiphon_cdn_sni.trim();
        if !sni.is_empty() {
            args.push("--psiphon-cdn-sni".to_string());
            args.push(sni.to_string());
        }
        if let Some(ref bin) = cfg.psiphon_bin {
            let bin = bin.trim();
            if !bin.is_empty() {
                args.push("--psiphon-bin".to_string());
                args.push(bin.to_string());
            }
        }
        let psiphon_http = cfg
            .psiphon_http
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .or_else(|| cfg.http_port.map(|p| format!("127.0.0.1:{p}")))
            .or_else(|| Some("127.0.0.1:1820".to_string()));

        if let Some(ref http) = psiphon_http {
            args.push("--psiphon-http".to_string());
            args.push(http.clone());
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

    // Manual peer override
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_bridges(lines: Option<&str>, bridges: bool) -> TunnelConfig {
        TunnelConfig {
            tor_enabled: true,
            tor_bridges: bridges,
            tor_bridge_lines: lines.map(str::to_string),
            ..TunnelConfig::default()
        }
    }

    fn bridge_args(cfg: &TunnelConfig) -> Vec<String> {
        let args = build_cli_args(cfg);
        let mut values = Vec::new();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            if arg.as_str() == "--tor-bridge" {
                if let Some(value) = iter.next() {
                    values.push(value.clone());
                }
            }
        }
        values
    }

    #[test]
    fn manual_bridge_lines_split_like_the_engine_does() {
        let cfg = cfg_with_bridges(
            Some(
                "obfs4 192.0.2.55:38114 316E64 cert=abc iat-mode=0 \n\n; webtunnel 10.0.0.1:443 ABCD url=https://example.org/abc\n",
            ),
            false,
        );
        let lines = manual_bridge_lines(&cfg);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("obfs4 192.0.2.55"));
        assert!(lines[1].starts_with("webtunnel 10.0.0.1"));
        assert_eq!(bridge_args(&cfg).len(), 2);
    }

    #[test]
    fn manual_lines_replace_the_automatic_fetch_flag() {
        let cfg = cfg_with_bridges(Some("obfs4 192.0.2.55:38114 316E64 cert=abc"), true);
        let args = build_cli_args(&cfg);
        assert!(!args.iter().any(|arg| arg.as_str() == "--tor-bridges"));
        assert_eq!(bridge_args(&cfg).len(), 1);
    }

    #[test]
    fn the_automatic_fetch_flag_survives_when_nothing_is_typed() {
        let cfg = cfg_with_bridges(None, true);
        assert!(build_cli_args(&cfg).iter().any(|arg| arg.as_str() == "--tor-bridges"));

        let blank = cfg_with_bridges(Some("   \n ;  "), true);
        assert!(manual_bridge_lines(&blank).is_empty());
        assert!(build_cli_args(&blank).iter().any(|arg| arg.as_str() == "--tor-bridges"));
    }

    #[test]
    fn tor_addr_comes_only_from_a_live_listener_line() {
        assert_eq!(
            parse_tor_addr(
                "[+] tor is ready; 127.0.0.1:1820 leaves through tor, carried by the tunnel"
            ),
            Some("127.0.0.1:1820".to_string())
        );
        assert_eq!(
            parse_tor_addr("[+] tor is ready; 127.0.0.1:1820 leaves through tor"),
            Some("127.0.0.1:1820".to_string())
        );
        assert_eq!(
            parse_tor_addr(
                "[+] tor is ready; the tunnel goes out through 127.0.0.1:1820"
            ),
            Some("127.0.0.1:1820".to_string())
        );
        assert_eq!(
            parse_tor_addr("[*] bootstrapping tor through the tunnel at 127.0.0.1:1819"),
            None
        );
        assert_eq!(parse_tor_addr("[+] tor is ready; soon"), None);
    }

    #[test]
    fn psiphon_mode_flags_map_to_the_engine_spellings() {
        let cfg = TunnelConfig {
            psiphon_enabled: true,
            protocol: "masque".to_string(),
            ..TunnelConfig::default()
        };
        assert!(build_cli_args(&cfg).iter().any(|a| a == "--psiphon"));

        let reach = TunnelConfig {
            psiphon_enabled: true,
            psiphon_mode: "reach".to_string(),
            ..TunnelConfig::default()
        };
        assert!(build_cli_args(&reach).iter().any(|a| a == "--psiphon-reverse"));

        let only = TunnelConfig {
            psiphon_enabled: true,
            psiphon_mode: "psiphon-only".to_string(),
            ..TunnelConfig::default()
        };
        assert!(build_cli_args(&only).iter().any(|a| a == "--psiphon-only"));
    }

    #[test]
    fn psiphon_only_suppresses_the_http_proxy_and_the_tunnel_protocol() {
        let cfg = TunnelConfig {
            psiphon_enabled: true,
            psiphon_mode: "psiphon-only".to_string(),
            http_port: Some(1820),
            protocol: "masque".to_string(),
            ..TunnelConfig::default()
        };
        let args = build_cli_args(&cfg);
        assert!(!args.iter().any(|a| a == "--http-proxy"));
        assert!(!args.iter().any(|a| a == "--masque"));
    }

    #[test]
    fn psiphon_region_shape_and_bin_follow_the_user() {
        let cfg = TunnelConfig {
            psiphon_enabled: true,
            psiphon_mode: "carry".to_string(),
            psiphon_region: Some("de".to_string()),
            psiphon_shape: "cdn".to_string(),
            psiphon_cdn_ips: "1.2.3.4, 5.6.7.8".to_string(),
            psiphon_cdn_sni: "cdn.example.com".to_string(),
            psiphon_bin: Some("C:\\tools\\psiphon-tunnel-core.exe".to_string()),
            psiphon_http: Some("127.0.0.1:1822".to_string()),
            ..TunnelConfig::default()
        };
        let args = build_cli_args(&cfg);
        assert!(args.iter().any(|a| a == "--psiphon-region"));
        assert!(args.windows(2).any(|w| w[0] == "--psiphon-region" && w[1] == "DE"));
        assert!(args.iter().any(|a| a == "--psiphon-mode"));
        assert!(args.windows(2).any(|w| w[0] == "--psiphon-mode" && w[1] == "cdn"));
        assert!(args.windows(2).any(|w| w[0] == "--psiphon-cdn-ips" && w[1] == "1.2.3.4, 5.6.7.8"));
        assert!(args.windows(2).any(|w| w[0] == "--psiphon-cdn-sni" && w[1] == "cdn.example.com"));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--psiphon-bin" && w[1] == "C:\\tools\\psiphon-tunnel-core.exe"));
        assert!(args.windows(2).any(|w| w[0] == "--psiphon-http" && w[1] == "127.0.0.1:1822"));
    }

    #[test]
    fn psiphon_addr_comes_only_from_a_live_listener_line() {
        assert_eq!(
            parse_psiphon_addr("[+] psiphon is ready; 127.0.0.1:1821 leaves through psiphon"),
            Some("127.0.0.1:1821".to_string())
        );
        assert_eq!(
            parse_psiphon_addr(
                "[+] psiphon is ready; 127.0.0.1:1821 leaves through psiphon, carried by the tunnel"
            ),
            Some("127.0.0.1:1821".to_string())
        );
        assert_eq!(
            parse_psiphon_addr(
                "[+] psiphon is ready; the tunnel goes out through 127.0.0.1:1821"
            ),
            Some("127.0.0.1:1821".to_string())
        );
        assert_eq!(
            parse_psiphon_addr("[+] tor is ready; 127.0.0.1:1820 leaves through tor"),
            None
        );
        assert_eq!(parse_psiphon_addr("[+] psiphon is ready; soon"), None);
    }

    #[test]
    fn socks_listener_ready_recognizes_all_modes() {
        assert!(is_socks_listener_ready("socks5 server listening on 127.0.0.1:1819"));
        assert!(is_socks_listener_ready("[+] psiphon is ready; 127.0.0.1:1819 leaves through psiphon"));
        assert!(is_socks_listener_ready("[+] psiphon is ready; the tunnel goes out through 127.0.0.1:1821"));
        assert!(is_socks_listener_ready("[+] tor is ready; 127.0.0.1:1819 leaves through tor"));
        assert!(!is_socks_listener_ready("[*] starting psiphon with no tunnel underneath it"));
    }

    #[test]
    fn exit_line_parser_extracts_telemetry() {
        let (ip, colo, loc, lat) = parse_exit_line(
            "[+] psiphon exit: 212.227.6.72, DE via FRA, 335ms to cloudflare"
        ).unwrap();
        assert_eq!(ip, "212.227.6.72");
        assert_eq!(loc.as_deref(), Some("DE"));
        assert_eq!(colo.as_deref(), Some("FRA"));
        assert_eq!(lat, Some(335));
    }

    #[test]
    fn progress_extractor_scales_stages() {
        assert_eq!(extract_progress("hunting for a working endpoint").map(|(p, _)| p), Some(20));
        assert_eq!(extract_progress("starting psiphon").map(|(p, _)| p), Some(30));
        assert_eq!(extract_progress("psiphon reached a server at 1.2.3.4").map(|(p, _)| p), Some(75));
        assert_eq!(extract_progress("[+] psiphon is ready; 127.0.0.1:1819 leaves through psiphon").map(|(p, _)| p), Some(95));
        assert_eq!(extract_progress("[+] psiphon exit: 212.227.6.72, DE via FRA, 335ms to cloudflare").map(|(p, _)| p), Some(100));
    }
}
