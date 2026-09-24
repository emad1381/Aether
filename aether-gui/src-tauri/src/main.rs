#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cdnscan;
mod config;
mod matrix;
mod ping;
mod proxy;
mod supervisor;
mod types;

use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State as TauriState};

use crate::config::{load_config, save_config};
use crate::ping::{fetch_trace, measure_latency};
use crate::proxy::{is_windows_proxy_enabled, set_windows_proxy};
use crate::supervisor::Supervisor;
use crate::types::{State, TraceInfo, TunnelConfig, TunnelStatus};

#[tauri::command]
async fn start_tunnel(
    cfg: TunnelConfig,
    supervisor: TauriState<'_, Arc<Supervisor>>,
    app: AppHandle,
) -> Result<(), String> {
    let _ = save_config(&cfg);
    supervisor.start_tunnel(app, cfg).await
}

#[tauri::command]
async fn stop_tunnel(
    supervisor: TauriState<'_, Arc<Supervisor>>,
    app: AppHandle,
) -> Result<(), String> {
    supervisor.stop_tunnel(app).await
}

#[tauri::command]
fn get_status(supervisor: TauriState<'_, Arc<Supervisor>>) -> TunnelStatus {
    supervisor.get_status()
}

#[tauri::command]
fn toggle_proxy(enable: bool, cfg: TunnelConfig) -> Result<bool, String> {
    let socks_addr = format!("127.0.0.1:{}", cfg.socks_port);
    let http_addr = cfg.http_port.map(|p| format!("127.0.0.1:{p}"));
    set_windows_proxy(enable, &socks_addr, http_addr.as_deref(), &cfg.bypass_list)?;
    Ok(is_windows_proxy_enabled())
}

#[tauri::command]
async fn test_latency(socks_port: u16) -> Result<u64, String> {
    measure_latency(socks_port).await
}

#[tauri::command]
async fn fetch_trace_info(socks_port: u16) -> Result<TraceInfo, String> {
    fetch_trace(socks_port).await
}

#[tauri::command]
fn get_saved_config() -> TunnelConfig {
    load_config()
}

/// Scan CDN edge addresses from this machine and report which ones complete a
/// TLS handshake, so the fronting fields can be filled with edges that work on
/// the network the user is actually on. `progress` carries (done, total) while
/// the scan runs.
#[tauri::command]
async fn scan_cdn_edges(app: AppHandle) -> Result<cdnscan::CdnScanReport, String> {
    let report = cdnscan::scan(|done, total| {
        let _ = app.emit(
            "aether-cdn-scan",
            serde_json::json!({ "done": done, "total": total }),
        );
    })
    .await;
    Ok(report)
}

#[tauri::command]
fn save_gui_config(cfg: TunnelConfig) -> Result<(), String> {
    save_config(&cfg)
}

#[tauri::command]
fn minimize_window(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.minimize();
    }
}

#[tauri::command]
fn maximize_window(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if let Ok(is_max) = w.is_maximized() {
            if is_max {
                let _ = w.unmaximize();
            } else {
                let _ = w.maximize();
            }
        }
    }
}

#[tauri::command]
fn close_window(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.close();
    }
}

/// Exit hard so the already-armed updater helper can apply the downloaded zip.
/// Two things otherwise keep the process alive forever here: window.close() is
/// intercepted by the close-to-tray handler (the window just hides), and
/// stop_tunnel can block on a wedged child — so stop gets a 3-second budget,
/// the system proxy is cleared best-effort, and then the process exits
/// directly without ever raising CloseRequested.
#[tauri::command]
async fn quit_for_update(app: AppHandle, supervisor: TauriState<'_, Arc<Supervisor>>) {
    let cfg = load_config();
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        supervisor.stop_tunnel(app.clone()),
    )
    .await;
    let socks = format!("127.0.0.1:{}", cfg.socks_port);
    let http = cfg.http_port.map(|p| format!("127.0.0.1:{p}"));
    let _ = set_windows_proxy(false, &socks, http.as_deref(), "");
    std::process::exit(0);
}

#[tauri::command]
fn start_dragging(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.start_dragging();
    }
}

/// Answer the engine's Zero Trust email-code prompt over the child stdin.
#[tauri::command]
async fn team_otp_submit(
    code: String,
    supervisor: TauriState<'_, Arc<Supervisor>>,
) -> Result<(), String> {
    supervisor.submit_team_code(code).await
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub update_available: bool,
    pub url: String,
    /// Direct browser_download_url of the Windows zip in that release, empty
    /// when the release carries no asset yet (a build still running).
    pub asset_url: String,
}

const UPDATE_ASSET_NAME: &str = "aether-windows-x86_64-gui.zip";

async fn fetch_latest_release(
    client: &reqwest::Client,
) -> Result<(String, String, String), String> {
    let release: serde_json::Value = client
        .get("https://api.github.com/repos/emad1381/Aether/releases/latest")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let latest = release
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim_start_matches('v')
        .to_string();
    if latest.is_empty() {
        return Err("no release tag found".to_string());
    }
    let url = release
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://github.com/emad1381/Aether/releases")
        .to_string();
    let asset_url = release
        .get("assets")
        .and_then(|v| v.as_array())
        .and_then(|assets| {
            assets.iter().find(|a| {
                a.get("name").and_then(|n| n.as_str()) == Some(UPDATE_ASSET_NAME)
            })
        })
        .and_then(|a| a.get("browser_download_url"))
        .and_then(|u| u.as_str())
        .unwrap_or_default()
        .to_string();
    Ok((latest, url, asset_url))
}

/// True when `candidate` is a strictly newer version than `current`. A plain
/// != check would offer the user an older release whenever the tag moves
/// backwards or the build under test is ahead of the published tag.
fn is_newer_version(candidate: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim_start_matches('v')
            .split('.')
            .map(|part| part.trim().parse::<u64>().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parse(candidate), parse(current));
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false
}

/// Real update check: compare against the latest GitHub release.
#[tauri::command]
async fn check_for_updates() -> Result<UpdateInfo, String> {
    let client = reqwest::Client::builder()
        .user_agent(format!("aether-gui/{}", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let (latest, url, asset_url) = fetch_latest_release(&client).await?;
    let current = env!("CARGO_PKG_VERSION").to_string();
    Ok(UpdateInfo {
        update_available: is_newer_version(&latest, &current),
        current,
        latest,
        url,
        asset_url,
    })
}

/// Download the release zip, stage it, and arm a detached helper that waits for
/// this process to exit, unpacks over the install folder and relaunches. The
/// in-app files cannot be replaced while the exe is running, hence the helper.
#[tauri::command]
async fn download_update(app: AppHandle) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent(format!("aether-gui/{}", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    let (_latest, _url, asset_url) = fetch_latest_release(&client).await?;
    if asset_url.is_empty() {
        return Err("the latest release has no Windows zip yet; the build may still be running".into());
    }

    let stage = std::env::temp_dir().join("AetherUpdate");
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    let zip_path = stage.join(UPDATE_ASSET_NAME);

    let response = client
        .get(&asset_url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let total = response.content_length().unwrap_or(0);

    let mut downloaded: u64 = 0;
    let mut file = std::fs::File::create(&zip_path).map_err(|e| e.to_string())?;
    use std::io::Write;
    let mut stream = response;
    while let Some(chunk) = stream
        .chunk()
        .await
        .map_err(|e| e.to_string())?
    {
        file.write_all(&chunk).map_err(|e| e.to_string())?;
        downloaded += chunk.len() as u64;
        let _ = app.emit(
            "aether-update-progress",
            serde_json::json!({ "downloaded": downloaded, "total": total }),
        );
    }
    drop(file);

    // Helper: wait for our pid, unpack over the install dir, relaunch, self-delete.
    let install_dir = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .ok_or("exe has no parent dir")?
        .to_path_buf();
    let pid = std::process::id().to_string();
    let script = stage.join("apply-update.ps1");
    let script_body = format!(
        r#"param([string]$OwnerPid, [string]$Zip, [string]$InstallDir, [string]$ExeName)
while (Get-Process -Id $OwnerPid -ErrorAction SilentlyContinue) {{ Start-Sleep -Milliseconds 800 }}
$stage = Join-Path $env:TEMP ("AetherApply-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
try {{
  Expand-Archive -Path $Zip -DestinationPath $stage -Force
  Get-ChildItem -Path $stage -Force | ForEach-Object {{
    $dest = Join-Path $InstallDir $_.Name
    if ($_.PSIsContainer) {{ Copy-Item -LiteralPath $_.FullName -Destination $InstallDir -Recurse -Force }}
    else {{ Copy-Item -LiteralPath $_.FullName -Destination $dest -Force }}
  }}
}} finally {{
  Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}}
Start-Process -FilePath (Join-Path $InstallDir $ExeName)
Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $Zip -Force -ErrorAction SilentlyContinue
"#,
    );
    std::fs::write(&script, script_body).map_err(|e| e.to_string())?;

    let mut helper = std::process::Command::new("powershell.exe");
    helper
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script)
        .arg(&pid)
        .arg(&zip_path)
        .arg(&install_dir)
        .arg("aether-gui.exe");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        const DETACHED_PROCESS: u32 = 0x00000008;
        helper.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    helper
        .spawn()
        .map_err(|e| format!("could not start the updater helper: {e}"))?;

    Ok(())
}

#[cfg(windows)]
fn apply_launch_at_startup(enable: bool) -> Result<(), String> {
    use winreg::enums::HKEY_CURRENT_USER;
    let hkcu = winreg::RegKey::predef(HKEY_CURRENT_USER);
    let (run, _) = hkcu
        .create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
        .map_err(|e| format!("run key: {e}"))?;
    if enable {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let command = format!("\"{}\"", exe.display());
        run.set_value("Aether", &command)
            .map_err(|e| format!("autostart: {e}"))
    } else {
        let _ = run.delete_value("Aether");
        Ok(())
    }
}

#[cfg(not(windows))]
fn apply_launch_at_startup(_enable: bool) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
fn set_launch_at_startup(enable: bool, mut cfg: TunnelConfig) -> Result<(), String> {
    apply_launch_at_startup(enable)?;
    cfg.launch_at_startup = enable;
    save_config(&cfg)
}

fn main() {
    let supervisor = Arc::new(Supervisor::new());

    tauri::Builder::default()
        .manage(supervisor.clone())
        .setup(move |app| {
            // Build system tray menu
            let show_item = MenuItem::with_id(app, "show", "Show Aether", true, None::<&str>)?;
            let toggle_item = MenuItem::with_id(app, "toggle", "Connect / Disconnect", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Exit", true, None::<&str>)?;

            let menu = Menu::with_items(app, &[&show_item, &toggle_item, &quit_item])?;

            let mut tray_builder = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("Aether")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "toggle" => {
                        let supervisor = app.state::<Arc<Supervisor>>();
                        let current = supervisor.get_status();
                        let app_handle = app.clone();
                        let sup = supervisor.inner().clone();
                        tauri::async_runtime::spawn(async move {
                            if current.state == State::Connected
                                || current.state == State::Connecting
                                || current.state == State::Scanning
                            {
                                let _ = sup.stop_tunnel(app_handle).await;
                            } else {
                                let cfg = load_config();
                                let _ = sup.start_tunnel(app_handle, cfg).await;
                            }
                        });
                    }
                    "quit" => {
                        let supervisor = app.state::<Arc<Supervisor>>();
                        let app_handle = app.clone();
                        let sup = supervisor.inner().clone();
                        tauri::async_runtime::spawn(async move {
                            let cfg = load_config();
                            let socks = format!("127.0.0.1:{}", cfg.socks_port);
                            let http = cfg.http_port.map(|p| format!("127.0.0.1:{p}"));
                            // Bounded: a wedged stop_tunnel must never keep the
                            // process alive — an armed updater helper waits on
                            // this pid, and a user clicking Exit expects an exit.
                            let _ = tokio::time::timeout(
                                std::time::Duration::from_secs(3),
                                sup.stop_tunnel(app_handle),
                            )
                            .await;
                            let _ = set_windows_proxy(false, &socks, http.as_deref(), "");
                            std::process::exit(0);
                        });
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: tauri::tray::MouseButton::Left, .. } = event {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                });
            // Exactly one tray icon. Tauri used to also create a second one from
            // the config's `app.trayIcon` — that was the colored icon with no
            // menu, while this code-built tray had the menu but no icon, so
            // Windows showed two entries and only the blank one was clickable.
            // The config tray is gone; this one carries both menu and icon.
            if let Ok(icon) =
                tauri::image::Image::from_bytes(include_bytes!("../icons/128x128.png"))
            {
                tray_builder = tray_builder.icon(icon);
            }
            let _tray = tray_builder.build(app)?;

            // Honor the saved startup behavior: keep the run-at-login registry
            // entry in sync, and open hidden when the user asked for tray-only.
            {
                let cfg = load_config();
                let _ = apply_launch_at_startup(cfg.launch_at_startup);
                if cfg.start_minimized {
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.hide();
                    }
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if load_config().close_to_tray {
                    // Minimize to tray instead of killing
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            start_tunnel,
            stop_tunnel,
            get_status,
            toggle_proxy,
            test_latency,
            fetch_trace_info,
            get_saved_config,
            save_gui_config,
            minimize_window,
            maximize_window,
            close_window,
            quit_for_update,
            start_dragging,
            team_otp_submit,
            check_for_updates,
            download_update,
            set_launch_at_startup,
            scan_cdn_edges
        ])
        .run(tauri::generate_context!())
        .expect("error while running Aether GUI");
}
