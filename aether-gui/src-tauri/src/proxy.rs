#[cfg(windows)]
use std::ptr::null_mut;
#[cfg(windows)]
use windows_sys::Win32::Networking::WinInet::{
    InternetSetOptionW, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED,
};
#[cfg(windows)]
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY_CURRENT_USER, KEY_READ,
    KEY_WRITE, REG_DWORD, REG_SZ,
};

const REG_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings";

#[cfg(windows)]
fn to_wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub fn set_windows_proxy(
    enabled: bool,
    socks_addr: &str,
    http_addr: Option<&str>,
    bypass_list: &str,
) -> Result<(), String> {
    #[cfg(windows)]
    unsafe {
        let path_w = to_wide(REG_PATH);
        let mut hkey = 0;
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            path_w.as_ptr(),
            0,
            KEY_WRITE | KEY_READ,
            &mut hkey,
        );
        if status != 0 {
            return Err(format!("Failed to open internet settings registry key: error code {status}"));
        }

        let enable_val: u32 = if enabled { 1 } else { 0 };
        let name_enable = to_wide("ProxyEnable");
        RegSetValueExW(
            hkey,
            name_enable.as_ptr(),
            0,
            REG_DWORD,
            &enable_val as *const u32 as *const u8,
            std::mem::size_of::<u32>() as u32,
        );

        if enabled {
            // Format ProxyServer: if HTTP port provided, specify both; else socks-only
            let server_str = if let Some(http) = http_addr {
                format!("http={http};https={http};socks={socks_addr}")
            } else {
                format!("socks={socks_addr}")
            };
            let name_server = to_wide("ProxyServer");
            let server_w = to_wide(&server_str);
            RegSetValueExW(
                hkey,
                name_server.as_ptr(),
                0,
                REG_SZ,
                server_w.as_ptr() as *const u8,
                (server_w.len() * 2) as u32,
            );

            let name_override = to_wide("ProxyOverride");
            let override_w = to_wide(bypass_list);
            RegSetValueExW(
                hkey,
                name_override.as_ptr(),
                0,
                REG_SZ,
                override_w.as_ptr() as *const u8,
                (override_w.len() * 2) as u32,
            );
        }

        RegCloseKey(hkey);

        // Notify WinINet that settings changed so Edge/Chrome/services update immediately
        InternetSetOptionW(0, INTERNET_OPTION_SETTINGS_CHANGED, null_mut(), 0);
        InternetSetOptionW(0, INTERNET_OPTION_REFRESH, null_mut(), 0);

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
    unsafe {
        let path_w = to_wide(REG_PATH);
        let mut hkey = 0;
        let status = RegOpenKeyExW(HKEY_CURRENT_USER, path_w.as_ptr(), 0, KEY_READ, &mut hkey);
        if status != 0 {
            return false;
        }

        let name_enable = to_wide("ProxyEnable");
        let mut data_type = 0;
        let mut data: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;

        let res = RegQueryValueExW(
            hkey,
            name_enable.as_ptr(),
            null_mut(),
            &mut data_type,
            &mut data as *mut u32 as *mut u8,
            &mut size,
        );

        RegCloseKey(hkey);
        res == 0 && data == 1
    }

    #[cfg(not(windows))]
    false
}
