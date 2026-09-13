#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod ping;
mod proxy;
mod supervisor;
mod types;

use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State as TauriState};

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

            let _tray = TrayIconBuilder::new()
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
                            let _ = sup.stop_tunnel(app_handle).await;
                            let _ = set_windows_proxy(false, "127.0.0.1:1819", None, "");
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
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Minimize to tray instead of killing
                api.prevent_close();
                let _ = window.hide();
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
            close_window
        ])
        .run(tauri::generate_context!())
        .expect("error while running Aether GUI");
}
