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
    read_config(&path).unwrap_or_default()
}

pub fn save_config(cfg: &TunnelConfig) -> Result<(), String> {
    let path = config_path();
    write_config(&path, cfg)
}

fn read_config(path: &std::path::Path) -> Option<TunnelConfig> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str::<TunnelConfig>(&content).ok()
}

fn write_config(path: &std::path::Path, cfg: &TunnelConfig) -> Result<(), String> {
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_success_fields_survive_save_load_cycle() {
        let path = std::env::temp_dir().join(format!(
            "aether-gui-config-roundtrip-{}.json",
            std::process::id()
        ));
        let cfg = TunnelConfig {
            last_success_proto: Some("gool".to_string()),
            last_success_noize: Some("firewall".to_string()),
            last_success_ip: Some("both".to_string()),
            ..TunnelConfig::default()
        };

        write_config(&path, &cfg).expect("config should save");
        let loaded = read_config(&path).expect("config should load");
        let _ = fs::remove_file(&path);

        assert_eq!(loaded.last_success_proto.as_deref(), Some("gool"));
        assert_eq!(loaded.last_success_noize.as_deref(), Some("firewall"));
        assert_eq!(loaded.last_success_ip.as_deref(), Some("both"));
    }
}
