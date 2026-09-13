#[cfg(windows)]
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
#[cfg(windows)]
use winreg::RegKey;

#[cfg(windows)]
use windows_sys::Win32::Networking::WinInet::{
    InternetSetOptionW, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED,
};

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

        if enabled {
            let server_str = if let Some(http) = http_addr {
                format!("http={http};https={http};socks={socks_addr}")
            } else {
                format!("socks={socks_addr}")
            };
            settings
                .set_value("ProxyServer", &server_str)
                .map_err(|e| format!("Failed to set ProxyServer: {e}"))?;
            settings
                .set_value("ProxyOverride", &bypass_list)
                .map_err(|e| format!("Failed to set ProxyOverride: {e}"))?;
        }

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
