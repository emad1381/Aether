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

pub struct Supervisor {
    child: Mutex<Option<Child>>,
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
        if self.status.lock().state != State::Disconnected
            && self.status.lock().state != State::Error
        {
            return Err("Tunnel is already running or connecting".to_string());
        }

        let bin_path = find_aether_binary()?;
        self.is_stopping.store(false, Ordering::SeqCst);
        *self.current_config.lock() = Some(cfg.clone());

        // Initialize Status
        {
            let mut st = self.status.lock();
            st.state = State::Connecting;
            st.latency_ms = None;
            st.error_message = None;
            st.socks_endpoint = format!("127.0.0.1:{}", cfg.socks_port);
            st.system_proxy_active = false;
            let _ = app.emit("aether-status", st.clone());
        }

        // Auto Mode is deliberately disabled for an explicit Tor configuration.
        // Tor is an opt-in transport, not a background matrix dimension.
        if cfg.auto_connect && !cfg.tor_enabled {
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

                let mut st = self.status.lock();
                st.state = State::Error;
                st.error_message = Some(
                    "Auto mode exhausted every protocol, obfuscation, and IP-version combination. Check basic network connectivity or enable Tor Integration."
                        .to_string(),
                );
                let _ = app.emit("aether-status", st.clone());
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
        cmd.stdin(std::process::Stdio::null()); // NEVER wait for STDIN input!
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
        });

        Ok(())
    }

    async fn kill_current_child(&self) {
        let child_opt = self.child.lock().take();
        if let Some(mut child) = child_opt {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }

    pub async fn stop_tunnel(&self, app: AppHandle) -> Result<(), String> {
        self.is_stopping.store(true, Ordering::SeqCst);
        let _ = self.kill_tx.send(());

        self.kill_current_child().await;

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

    pub fn set_connected(&self, app: &AppHandle, cfg: &TunnelConfig, custom_proto: Option<&str>) {
        let mut st = self.status.lock();
        st.state = State::Connected;
        if let Some(proto) = custom_proto {
            st.protocol = Some(proto.to_string());
        }
        *self.start_time.lock() = Some(Instant::now());

        if cfg.auto_system_proxy || cfg.tunnel_mode == "system-wide" {
            let socks_addr = format!("127.0.0.1:{}", cfg.socks_port);
            let http_addr = cfg
                .http_port
                .map(|p| format!("127.0.0.1:{p}"))
                .or_else(|| Some("127.0.0.1:1820".to_string()));
            if let Ok(()) =
                set_windows_proxy(true, &socks_addr, http_addr.as_deref(), &cfg.bypass_list)
            {
                st.system_proxy_active = true;
            }
        }

        let _ = app.emit("aether-status", st.clone());
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
    for retry in 0..3_u16 {
        let Some((socks_port, http_port)) = find_free_port_pair(
            socks_port_start + retry,
            http_port_start + retry,
        ) else {
            continue;
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
                emit_auto_log(&app, "WARN", format!("[AUTO #{id}] port conflict; retrying with the next free local port."));
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

fn is_bind_conflict(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("address already in use")
        || lower.contains("only one usage of each socket address")
        || (lower.contains("bind") && lower.contains("address"))
}

fn find_free_port_pair(socks_start: u16, http_start: u16) -> Option<(u16, u16)> {
    for offset in 0..99_u16 {
        let socks = socks_start.checked_add(offset)?;
        let http = http_start.checked_add(offset)?;
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

#[cfg(windows)]
struct KillOnCloseJob(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
unsafe impl Send for KillOnCloseJob {}

#[cfg(windows)]
impl Drop for KillOnCloseJob {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0); }
    }
}

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
        if process.is_null() { return None; }
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() { CloseHandle(process); return None; }
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
        if assigned { Some(KillOnCloseJob(job)) } else { CloseHandle(job); None }
    }
}

#[cfg(not(windows))]
struct KillOnCloseJob;

#[cfg(not(windows))]
fn attach_kill_on_close_job(_child: &Child) -> Option<KillOnCloseJob> { Some(KillOnCloseJob) }

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
                    socks_bound |= line.contains("socks5 server listening");
                    emit_auto_log(app, log_level(&line), format!("[AUTO #{id}] {line}"));
                }, Ok(None) => stderr = None, Err(error) => { emit_auto_log(app, "WARN", format!("[AUTO #{id}] stderr read error: {error}")); stderr = None; } }
            }
            line = async { match stdout.as_mut() { Some(lines) => lines.next_line().await, None => std::future::pending().await } } => {
                match line { Ok(Some(line)) => {
                    port_conflict |= is_bind_conflict(&line);
                    saw_data_plane |= is_data_plane_confirmation(&line);
                    socks_bound |= line.contains("socks5 server listening");
                    emit_auto_log(app, log_level(&line), format!("[AUTO #{id}] {line}"));
                }, Ok(None) => stdout = None, Err(error) => { emit_auto_log(app, "WARN", format!("[AUTO #{id}] stdout read error: {error}")); stdout = None; } }
            }
        }

        if port_conflict {
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

    // State machine extraction
    if line.contains("socks5 server listening")
        || line.contains("exposing socks5")
        || line.contains("wireguard tunnel validated (end-to-end data confirmed)")
    {
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

    let effective_http = cfg.http_port.or(Some(1820));
    if let Some(http) = effective_http {
        args.push("--http-proxy".to_string());
        args.push(format!("127.0.0.1:{http}"));
    }

    // CRITICAL: Prevent STDIN prompt for last connection:
    args.push("--no-quick-reconnect".to_string());

    // Protocol selection
    if cfg.tor_enabled {
        match cfg.tor_mode.as_str() {
            "reach" => args.push("--tor-reverse".to_string()),
            "tor-only" => args.push("--tor-only".to_string()),
            _ => args.push("--tor".to_string()),
        }
        if cfg.tor_bridges {
            args.push("--tor-bridges".to_string());
        }
    } else {
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
                args.push("--h3".to_string());
            }
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
