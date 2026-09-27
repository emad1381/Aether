use std::net::IpAddr;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::Mutex;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x08000000;

static TUN_PROCESS: Mutex<Option<Child>> = Mutex::new(None);
static CONFIGURED_ROUTES: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn is_tun_active() -> bool {
    TUN_PROCESS.lock().unwrap().is_some()
}

fn find_executable(name: &str) -> Option<PathBuf> {
    // 1. Current executable's directory
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join(name);
            if candidate.exists() {
                return Some(candidate);
            }
            let pt_candidate = parent.join("pt").join(name);
            if pt_candidate.exists() {
                return Some(pt_candidate);
            }
        }
    }
    // 2. Current working directory
    let cwd_candidate = PathBuf::from(name);
    if cwd_candidate.exists() {
        return Some(cwd_candidate);
    }
    let pt_candidate = PathBuf::from("pt").join(name);
    if pt_candidate.exists() {
        return Some(pt_candidate);
    }
    None
}

/// Detect the default IPv4 gateway IP address on Windows
fn get_default_gateway() -> Option<String> {
    let mut cmd = Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-Command",
        "(Get-NetRoute -DestinationPrefix '0.0.0.0/0' -ErrorAction SilentlyContinue | Sort-Object RouteMetric | Select-Object -First 1).NextHop",
    ]);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = cmd.output().ok()?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.parse::<IpAddr>().is_ok() {
            return Some(text);
        }
    }
    None
}

/// Run a system command without creating a console window
fn run_cmd(cmd: &str, args: &[&str]) -> Result<(), String> {
    let mut command = Command::new(cmd);
    command.args(args);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    match command.output() {
        Ok(out) => {
            if out.status.success() {
                Ok(())
            } else {
                let err = String::from_utf8_lossy(&out.stderr);
                Err(format!("Command '{cmd} {}' failed: {err}", args.join(" ")))
            }
        }
        Err(e) => Err(format!("Failed to execute '{cmd}': {e}")),
    }
}

pub fn start_tun(socks_port: u16, bypass_ips: &[IpAddr]) -> Result<(), String> {
    stop_tun(bypass_ips);

    let tun2socks_bin = find_executable("tun2socks.exe")
        .ok_or_else(|| "tun2socks.exe not found in application directory".to_string())?;

    let wintun_dll = find_executable("wintun.dll")
        .ok_or_else(|| "wintun.dll not found in application directory".to_string())?;

    log::info!("[tun] using tun2socks at {}", tun2socks_bin.display());
    log::info!("[tun] using wintun at {}", wintun_dll.display());

    // Ensure wintun.dll is available in the current directory or tun2socks directory
    if let Some(t_parent) = tun2socks_bin.parent() {
        let target_dll = t_parent.join("wintun.dll");
        if !target_dll.exists() && wintun_dll.exists() {
            let _ = std::fs::copy(&wintun_dll, &target_dll);
        }
    }

    let gateway = get_default_gateway();
    log::info!("[tun] detected physical default gateway: {:?}", gateway);

    // 1. Launch tun2socks with wintun device
    let mut cmd = Command::new(&tun2socks_bin);
    cmd.args([
        "-device",
        "wintun://AetherTun",
        "-proxy",
        &format!("socks5://127.0.0.1:{socks_port}"),
        "-loglevel",
        "info",
    ]);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn tun2socks: {e}"))?;

    *TUN_PROCESS.lock().unwrap() = Some(child);

    // 2. Wait 600ms for Wintun virtual adapter to initialize in Windows
    std::thread::sleep(Duration::from_millis(600));

    // 3. Configure IP address on AetherTun adapter
    if let Err(e) = run_cmd(
        "netsh",
        &[
            "interface",
            "ipv4",
            "set",
            "address",
            "name=AetherTun",
            "source=static",
            "addr=198.18.0.1",
            "mask=255.255.0.0",
        ],
    ) {
        log::warn!("[tun] netsh set address warning: {e}");
    }

    // 4. Set DNS on AetherTun adapter
    let _ = run_cmd(
        "netsh",
        &[
            "interface",
            "ipv4",
            "set",
            "dnsservers",
            "name=AetherTun",
            "static",
            "address=1.1.1.1",
            "register=none",
            "validate=no",
        ],
    );

    let mut configured = Vec::new();

    // 5. Add bypass routes for remote endpoints and known Cloudflare Anycast blocks
    // so tunnel transport traffic never loops back into the TUN adapter
    if let Some(gw) = &gateway {
        for ip in bypass_ips {
            let ip_str = ip.to_string();
            let _ = run_cmd(
                "route",
                &["add", &ip_str, "mask", "255.255.255.255", gw, "metric", "1"],
            );
            configured.push(ip_str);
        }

        let cf_blocks = [
            ("162.159.192.0", "255.255.255.0"),
            ("162.159.193.0", "255.255.255.0"),
            ("162.159.195.0", "255.255.255.0"),
            ("188.114.96.0", "255.255.252.0"),
        ];
        for (subnet, mask) in &cf_blocks {
            let _ = run_cmd("route", &["add", subnet, "mask", mask, gw, "metric", "1"]);
            configured.push(subnet.to_string());
        }
    }

    // 6. Add split default routes (0.0.0.0/1 and 128.0.0.0/1) directing all system traffic to AetherTun
    let _ = run_cmd(
        "route",
        &["add", "0.0.0.0", "mask", "128.0.0.0", "198.18.0.1", "metric", "1"],
    );
    let _ = run_cmd(
        "route",
        &["add", "128.0.0.0", "mask", "128.0.0.0", "198.18.0.1", "metric", "1"],
    );

    *CONFIGURED_ROUTES.lock().unwrap() = configured;
    log::info!("[tun] AetherTun virtual adapter active; full system routed via 198.18.0.1");

    Ok(())
}

pub fn stop_tun(bypass_ips: &[IpAddr]) {
    // 1. Remove split default routes
    let _ = run_cmd("route", &["delete", "0.0.0.0", "mask", "128.0.0.0"]);
    let _ = run_cmd("route", &["delete", "128.0.0.0", "mask", "128.0.0.0"]);

    // 2. Remove bypass routes
    let mut configured = CONFIGURED_ROUTES.lock().unwrap();
    for ip_str in configured.drain(..) {
        let _ = run_cmd("route", &["delete", &ip_str]);
    }
    for ip in bypass_ips {
        let _ = run_cmd("route", &["delete", &ip.to_string()]);
    }

    let cf_subnets = [
        "162.159.192.0",
        "162.159.193.0",
        "162.159.195.0",
        "188.114.96.0",
    ];
    for subnet in &cf_subnets {
        let _ = run_cmd("route", &["delete", subnet]);
    }

    // 3. Terminate tun2socks process
    let mut guard = TUN_PROCESS.lock().unwrap();
    if let Some(mut child) = guard.take() {
        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(windows)]
    {
        let mut kill_cmd = Command::new("taskkill");
        kill_cmd.args(["/F", "/IM", "tun2socks.exe"]);
        kill_cmd.creation_flags(CREATE_NO_WINDOW);
        let _ = kill_cmd.output();
    }

    log::info!("[tun] TUN adapter deactivated and routes restored.");
}

pub fn cleanup_leftovers() {
    let _ = run_cmd("route", &["delete", "0.0.0.0", "mask", "128.0.0.0"]);
    let _ = run_cmd("route", &["delete", "128.0.0.0", "mask", "128.0.0.0"]);
    #[cfg(windows)]
    {
        let mut kill_cmd = Command::new("taskkill");
        kill_cmd.args(["/F", "/IM", "tun2socks.exe"]);
        kill_cmd.creation_flags(CREATE_NO_WINDOW);
        let _ = kill_cmd.output();
    }
}
