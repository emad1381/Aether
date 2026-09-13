mod engine;
mod logbridge;
mod types;

use std::sync::Arc;

use tauri::{Manager, State};

use engine::Engine;
use types::{Settings, Status};

#[tauri::command]
fn get_status(state: State<'_, Arc<Engine>>) -> Status {
    state.status()
}

#[tauri::command]
fn get_settings() -> Settings {
    engine::load_settings()
}

#[tauri::command]
fn save_settings(settings: Settings) {
    engine::save_settings(&settings);
}

#[tauri::command]
fn connect(
    state: State<'_, Arc<Engine>>,
    app: tauri::AppHandle,
    settings: Settings,
) -> Result<(), String> {
    state.connect(app, settings)
}

#[tauri::command]
fn disconnect(state: State<'_, Arc<Engine>>, app: tauri::AppHandle) {
    state.disconnect(app);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let engine = Engine::new();

    tauri::Builder::default()
        .manage(engine.clone())
        .setup(move |app| {
            // The identity file and settings need a directory the app may
            // actually write to; on Android that is the app's data dir, which
            // only the host can hand us.
            if let Ok(dir) = app.path().app_data_dir() {
                let _ = std::fs::create_dir_all(&dir);
                std::env::set_var("AETHER_APP_DIR", &dir);
            }
            engine::data_dir(); // prime the cached path now

            let rx = logbridge::install();
            logbridge::spawn_pump(app.handle().clone(), rx);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            get_settings,
            save_settings,
            connect,
            disconnect
        ])
        .run(tauri::generate_context!())
        .expect("error while running Aether");
}
