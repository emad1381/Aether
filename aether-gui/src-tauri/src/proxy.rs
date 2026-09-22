#[cfg(windows)]
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_BINARY};
#[cfg(windows)]
use winreg::{RegKey, RegValue};

#[cfg(windows)]
use windows_sys::Win32::Networking::WinInet::{
    InternetSetOptionW, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED,
};

#[cfg(windows)]
fn update_connection_settings(enabled: bool, server: &str, bypass: &str) {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let connections_key = match hkcu.create_subkey(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\\Connections",
    ) {
        Ok((k, _)) => k,
        Err(_) => return,
    };

    let mut counter: u32 = 1;
    if let Ok(existing) = connections_key.get_raw_value("DefaultConnectionSettings") {
        if existing.bytes.len() >= 8 {
            let existing_counter = u32::from_le_bytes([
                existing.bytes[4],
                existing.bytes[5],
                existing.bytes[6],
                existing.bytes[7],
            ]);
            counter = existing_counter.wrapping_add(1);
        }
    }

    let mut data: Vec<u8> = Vec::with_capacity(128);
    // Header 0x46 (70 in decimal)
    data.extend_from_slice(&70u32.to_le_bytes());
    // Counter
    data.extend_from_slice(&counter.to_le_bytes());
    // Flags: 0x03 (manual proxy enabled), 0x01 (proxy disabled)
    let flags: u32 = if enabled { 3 } else { 1 };
    data.extend_from_slice(&flags.to_le_bytes());

    // Server string length & ASCII bytes
    let server_bytes = server.as_bytes();
    data.extend_from_slice(&(server_bytes.len() as u32).to_le_bytes());
    data.extend_from_slice(server_bytes);

    // Bypass string length & ASCII bytes
    let bypass_bytes = bypass.as_bytes();
    data.extend_from_slice(&(bypass_bytes.len() as u32).to_le_bytes());
    data.extend_from_slice(bypass_bytes);

    // 36-byte zero padding
    data.extend_from_slice(&[0u8; 36]);

    let reg_val = RegValue {
        vtype: REG_BINARY,
        bytes: data.into(),
    };

    let _ = connections_key.set_raw_value("DefaultConnectionSettings", &reg_val);
    let _ = connections_key.set_raw_value("SavedLegacySettings", &reg_val);
}

pub fn set_windows_proxy(
    enabled: bool,
    socks_addr: &str,
    http_addr: Option<&str>,
    bypass_list: &str,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let settings = hkcu
            .open_subkey_with_flags(
                "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings",
                KEY_WRITE | KEY_READ,
            )
            .map_err(|e| format!("Failed to open internet settings registry key: {e}"))?;

        let enable_val: u32 = if enabled { 1 } else { 0 };
        settings
            .set_value("ProxyEnable", &enable_val)
            .map_err(|e| format!("Failed to set ProxyEnable: {e}"))?;

        // Modern Windows 10 & 11 Settings (ms-settings:network-proxy) requires a clean
        // "host:port" format (e.g. "127.0.0.1:1820"). Multi-protocol strings with prefixes
        // like "socks=..." are misparsed by modern Windows Settings: it takes "socks=127.0.0.1"
        // as the hostname, prepends "http://", and displays "http://socks=127.0.0.1" in the IP box.
        // Therefore, we always use a clean "host:port", stripped of any "socks=" prefix.
        let server_str = if let Some(http) = http_addr {
            http.to_string()
        } else {
            socks_addr.trim_start_matches("socks=").to_string()
        };

        if enabled {
            settings
                .set_value("ProxyServer", &server_str)
                .map_err(|e| format!("Failed to set ProxyServer: {e}"))?;
            settings
                .set_value("ProxyOverride", &bypass_list)
                .map_err(|e| format!("Failed to set ProxyOverride: {e}"))?;
        }

        // Synchronize with modern Windows 10/11 Settings app binary registry keys
        update_connection_settings(enabled, &server_str, bypass_list);

        unsafe {
            InternetSetOptionW(
                std::ptr::null(),
                INTERNET_OPTION_SETTINGS_CHANGED,
                std::ptr::null_mut(),
                0,
            );
            InternetSetOptionW(
                std::ptr::null(),
                INTERNET_OPTION_REFRESH,
                std::ptr::null_mut(),
                0,
            );
        }

        Ok(())
    }

    #[cfg(not(windows))]
    {
        let _ = (enabled, socks_addr, http_addr, bypass_list);
        Ok(())
    }
}

pub fn is_windows_proxy_enabled() -> bool {
    #[cfg(windows)]
    {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(settings) =
            hkcu.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings")
        {
            if let Ok(val) = settings.get_value::<u32, _>("ProxyEnable") {
                return val == 1;
            }
        }
        false
    }

    #[cfg(not(windows))]
    false
}
