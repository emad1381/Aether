mod engine;
mod logbridge;
mod types;
mod vpn;

use std::sync::Arc;

use tauri::{Manager, State};

use engine::Engine;
use types::{Settings, Status};

#[tauri::command]
fn vpn_state() -> vpn::VpnState {
    vpn::state()
}

#[tauri::command]
fn vpn_start() -> Result<(), String> {
    vpn::start()
}

#[tauri::command]
fn vpn_stop() -> Result<(), String> {
    vpn::stop()
}

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

/// Sign in to a Zero Trust organization with a token the UI already holds.
#[tauri::command]
async fn team_sign_in(
    team: String,
    access_token: Option<String>,
    access_id: Option<String>,
    access_secret: Option<String>,
) -> Result<Option<String>, String> {
    let mut credentials = aether::api::TeamCredentials::new(&team)
        .map_err(|e| format!("{e}"))?;
    credentials.token = access_token.filter(|t| !t.trim().is_empty());
    credentials.client_id = access_id.filter(|t| !t.trim().is_empty());
    credentials.client_secret = access_secret.filter(|t| !t.trim().is_empty());
    if credentials.token.is_none() && credentials.client_id.is_none() {
        return Err("an access token or a service key is required".into());
    }
    aether::api::team_sign_in(&credentials)
        .await
        .map(Some)
        .map_err(|e| format!("{e}"))
}

/// Ask the organization to email a one-time code. Returns a session handle the
/// UI submits that code against.
#[tauri::command]
async fn team_code_request(team: String, email: String) -> Result<u64, String> {
    let credentials = aether::api::TeamCredentials::new(&team).map_err(|e| format!("{e}"))?;
    let session = aether::api::team_email_code_request(&credentials, &email)
        .await
        .map_err(|e| format!("{e}"))?;
    Ok(engine::keep_sign_in_session(session))
}

/// Submit the code the organization emailed. Returns a token when it accepted.
#[tauri::command]
async fn team_code_submit(session: u64, code: String) -> Result<Option<String>, String> {
    let session = engine::take_sign_in_session(session)?;
    let token = aether::api::team_email_code_submit(&session, &code)
        .await
        .map_err(|e| format!("{e}"))?;
    Ok(token)
}

/// Forget the stored Zero Trust token and drop back to personal WARP.
#[tauri::command]
async fn team_sign_out() -> Result<(), String> {
    aether::api::team_forget_token().await;
    Ok(())
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
            disconnect,
            team_sign_in,
            team_code_request,
            team_code_submit,
            team_sign_out,
            vpn_state,
            vpn_start,
            vpn_stop
        ])
        .run(tauri::generate_context!())
        .expect("error while running Aether");
}
