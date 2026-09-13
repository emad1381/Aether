use std::fs;
use std::path::PathBuf;

use crate::types::TunnelConfig;

fn config_path() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        let p = PathBuf::from(appdata).join("Aether");
        let _ = fs::create_dir_all(&p);
        p.join("gui_config.json")
    } else {
        PathBuf::from("gui_config.json")
    }
}

pub fn load_config() -> TunnelConfig {
    let path = config_path();
    if path.is_file() {
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<TunnelConfig>(&content) {
                return cfg;
            }
        }
    }
    TunnelConfig::default()
}

pub fn save_config(cfg: &TunnelConfig) -> Result<(), String> {
    let path = config_path();
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(())
}
