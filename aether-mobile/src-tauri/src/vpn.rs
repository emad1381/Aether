//! System-wide VPN mode.
//!
//! Android owns the tun device (only a VpnService may create one), so the
//! Kotlin side owns the permission dialog and the descriptor, and we own the
//! packet pump: a userspace tun2socks whose upstream is the in-process core's
//! SOCKS5 listener on 127.0.0.1:1819. The app is excluded from its own
//! VpnService, so the engine's traffic to the Cloudflare edge never loops back
//! into the tun.
//!
//! The bridge is deliberately one directional: Kotlin calls the three exported
//! natives below (poll for work, hand over the descriptor, report an outcome),
//! and Rust only flips flags. No JNI environment juggling, no class lookups.

use parking_lot::Mutex;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// The core's SOCKS listener, and the transport the relay runs behind it.
pub const SOCKS_UPSTREAM: &str = "socks5://127.0.0.1:1819";
pub const TUN_MTU: u16 = 1500;
pub const UPSTREAM_DNS: &str = "1.1.1.1";

static WANT_START: AtomicBool = AtomicBool::new(false);
static WANT_STOP: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, Serialize)]
pub struct VpnState {
    pub state: String,
    pub detail: String,
}

impl Default for VpnState {
    fn default() -> Self {
        Self {
            state: "off".into(),
            detail: "not requested".into(),
        }
    }
}

fn cell() -> &'static Mutex<VpnState> {
    static STATE: OnceLock<Mutex<VpnState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(VpnState::default()))
}

pub fn state() -> VpnState {
    cell().lock().clone()
}

pub(crate) fn set_state(state: &str, detail: impl Into<String>) {
    let mut guard = cell().lock();
    guard.state = state.to_string();
    guard.detail = detail.into();
}

/// Asks the Kotlin bridge to request consent and start the VpnService. The
/// answer arrives later, on the JNI callbacks further down.
pub fn start() -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        WANT_STOP.store(false, Ordering::SeqCst);
        WANT_START.store(true, Ordering::SeqCst);
        set_state("starting", "waiting for the Android VPN permission");
        Ok(())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = &WANT_START;
        Err("VPN mode is an Android VpnService feature".into())
    }
}

/// Tears the relay down and asks Kotlin to stop the service.
pub fn stop() -> Result<(), String> {
    WANT_START.store(false, Ordering::SeqCst);
    WANT_STOP.store(true, Ordering::SeqCst);
    #[cfg(target_os = "android")]
    relay::cancel();
    set_state("off", "stopped");
    Ok(())
}

// ---------------------------------------------------------------------------
// Exported natives: com.aether.mobile.AetherVpn
// ---------------------------------------------------------------------------

/// Polled by the Kotlin bridge: bit 0 = start, bit 1 = stop.
#[no_mangle]
pub extern "system" fn Java_com_aether_mobile_AetherVpn_nativePoll(
    _env: *mut std::ffi::c_void,
    _class: *mut std::ffi::c_void,
) -> i32 {
    let mut bits = 0;
    if WANT_START.swap(false, Ordering::SeqCst) {
        bits |= 1;
    }
    if WANT_STOP.swap(false, Ordering::SeqCst) {
        bits |= 2;
    }
    bits
}

/// The VpnService built the tun and detached the descriptor for us.
#[no_mangle]
pub extern "system" fn Java_com_aether_mobile_AetherVpn_nativeOnTunReady(
    _env: *mut std::ffi::c_void,
    _class: *mut std::ffi::c_void,
    fd: i32,
) -> i32 {
    #[cfg(target_os = "android")]
    {
        match relay::start(fd) {
            Ok(()) => 0,
            Err(message) => {
                log::error!("[vpn] {message}");
                set_state("error", message);
                1
            }
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = fd;
        1
    }
}

/// Outcome codes from the Kotlin side: 0 ok, 1 consent refused, 2 establish
/// failed, 3 service stopped.
#[no_mangle]
pub extern "system" fn Java_com_aether_mobile_AetherVpn_nativeReport(
    _env: *mut std::ffi::c_void,
    _class: *mut std::ffi::c_void,
    code: i32,
) {
    match code {
        0 => set_state("starting", "permission granted, bringing the tun up"),
        1 => set_state("error", "the VPN permission was refused on the phone"),
        2 => set_state("error", "Android could not establish the tun device"),
        3 => {
            #[cfg(target_os = "android")]
            relay::cancel();
            set_state("off", "stopped");
        }
        other => set_state("error", format!("the VPN bridge reported code {other}")),
    }
}

#[cfg(target_os = "android")]
mod relay {
    use super::{set_state, SOCKS_UPSTREAM, TUN_MTU, UPSTREAM_DNS};
    use parking_lot::Mutex;
    use std::io;
    use std::os::fd::AsRawFd;
    use std::pin::Pin;
    use std::task::{ready, Context, Poll};
    use tokio::io::unix::AsyncFd;
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tun2proxy::{ArgDns, ArgProxy, Args, CancellationToken};

    static CANCEL: Mutex<Option<CancellationToken>> = Mutex::new(None);

    pub fn cancel() {
        if let Some(token) = CANCEL.lock().take() {
            token.cancel();
        }
    }

    pub fn start(fd: i32) -> Result<(), String> {
        if fd < 0 {
            return Err("the VPN service did not return a usable tun descriptor".into());
        }
        cancel();

        let device = TunFd::new(fd)?;
        let proxy = ArgProxy::try_from(SOCKS_UPSTREAM)
            .map_err(|error| format!("the SOCKS upstream {SOCKS_UPSTREAM} was rejected: {error}"))?;
        let dns_addr = UPSTREAM_DNS
            .parse()
            .map_err(|_| format!("{UPSTREAM_DNS} is not an address"))?;

        let args = Args {
            proxy,
            setup: false,
            ipv6_enabled: true,
            dns: ArgDns::OverTcp,
            dns_addr,
            mtu: TUN_MTU,
            ..Default::default()
        };

        let token = CancellationToken::new();
        *CANCEL.lock() = Some(token.clone());
        set_state("on", "capturing every app on the phone");

        tauri::async_runtime::spawn(async move {
            let outcome = tun2proxy::run(device, TUN_MTU, args, token).await;
            let superseded = CANCEL.lock().is_some();
            if !superseded {
                match outcome {
                    Ok(_) => set_state("off", "stopped"),
                    Err(error) => {
                        log::error!("[vpn] relay ended: {error}");
                        set_state("error", error.to_string());
                    }
                }
            }
        });

        Ok(())
    }

    /// A tun descriptor presented as an async packet device: one read yields one
    /// IP packet, one write sends one.
    struct TunFd {
        fd: AsyncFd<OwnedFd>,
    }

    struct OwnedFd(i32);

    impl AsRawFd for OwnedFd {
        fn as_raw_fd(&self) -> i32 {
            self.0
        }
    }

    impl Drop for OwnedFd {
        fn drop(&mut self) {
            unsafe { libc::close(self.0) };
        }
    }

    impl TunFd {
        fn new(fd: i32) -> Result<Self, String> {
            unsafe {
                let flags = libc::fcntl(fd, libc::F_GETFL);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                    libc::close(fd);
                    return Err("the tun descriptor could not be made non blocking".into());
                }
            }
            AsyncFd::new(OwnedFd(fd))
                .map(|fd| Self { fd })
                .map_err(|error| format!("the tun descriptor was rejected by tokio: {error}"))
        }
    }

    impl AsyncRead for TunFd {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            loop {
                let mut guard = ready!(self.fd.poll_read_ready(cx))?;
                let result = guard.try_io(|inner| {
                    let raw = inner.get_ref().as_raw_fd();
                    let slice = buf.initialize_unfilled();
                    let read =
                        unsafe { libc::read(raw, slice.as_mut_ptr() as *mut std::ffi::c_void, slice.len()) };
                    if read < 0 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(read as usize)
                    }
                });
                match result {
                    Ok(Ok(read)) => {
                        buf.advance(read);
                        return Poll::Ready(Ok(()));
                    }
                    Ok(Err(error)) => return Poll::Ready(Err(error)),
                    Err(_would_block) => continue,
                }
            }
        }
    }

    impl AsyncWrite for TunFd {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            loop {
                let mut guard = ready!(self.fd.poll_write_ready(cx))?;
                let result = guard.try_io(|inner| {
                    let raw = inner.get_ref().as_raw_fd();
                    let written =
                        unsafe { libc::write(raw, buf.as_ptr() as *const std::ffi::c_void, buf.len()) };
                    if written < 0 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(written as usize)
                    }
                });
                match result {
                    Ok(Ok(written)) => return Poll::Ready(Ok(written)),
                    Ok(Err(error)) => return Poll::Ready(Err(error)),
                    Err(_would_block) => continue,
                }
            }
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
}
