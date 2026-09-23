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
}

/// Real update check: compare against the latest GitHub release.
#[tauri::command]
async fn check_for_updates() -> Result<UpdateInfo, String> {
    let client = reqwest::Client::builder()
        .user_agent(format!("aether-gui/{}", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
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
    let current = env!("CARGO_PKG_VERSION").to_string();
    Ok(UpdateInfo {
        update_available: latest != current,
        current,
        latest,
        url,
    })
}

#[cfg(windows)]
fn apply_launch_at_startup(enable: bool) -> Result<(), String> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_WRITE};
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
                            let _ = sup.stop_tunnel(app_handle).await;
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
            if let Ok(icon) = tauri::image::Image::from_bytes(
                include_bytes!("../icons/128x128.png"),
                tauri::image::ImageFormat::Png,
            ) {
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
            start_dragging,
            team_otp_submit,
            check_for_updates,
            set_launch_at_startup,
            scan_cdn_edges
        ])
        .run(tauri::generate_context!())
        .expect("error while running Aether GUI");
}
