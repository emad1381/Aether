use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast;

/// One line of the core engine's log, forwarded to the UI.
#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    pub level: String,
    pub message: String,
}

static LOG_TX: std::sync::OnceLock<broadcast::Sender<LogLine>> = std::sync::OnceLock::new();

/// The core engine reports through the `log` crate. The CLI installs
/// env_logger; the mobile app installs this instead so every line reaches the
/// activity view rather than a stdout nobody reads.
pub struct BroadcastLogger;

impl log::Log for BroadcastLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        if let Some(tx) = LOG_TX.get() {
            let _ = tx.send(LogLine {
                level: record.level().as_str().to_lowercase(),
                message: record.args().to_string(),
            });
        }
    }

    fn flush(&self) {}
}

/// Install the logger and return the receiving end. Safe to call twice: the
/// second call returns a receiver that simply never fires.
pub fn install() -> broadcast::Receiver<LogLine> {
    let (tx, rx) = broadcast::channel::<LogLine>(512);
    if LOG_TX.set(tx).is_err() {
        return broadcast::channel::<LogLine>(1).1;
    }
    if log::set_boxed_logger(Box::new(BroadcastLogger)).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    rx
}

/// Forward log lines to the webview as `aether-log` events.
pub fn spawn_pump(app: AppHandle, mut rx: broadcast::Receiver<LogLine>) {
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(line) => {
                    let _ = app.emit("aether-log", line);
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}
