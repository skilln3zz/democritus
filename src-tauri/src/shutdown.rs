//! Leaving the app cleanly, even when Tauri misses the exit.
//!
//! Twice, after long sessions, the window closed but democritus.exe kept
//! running: every thread idle, tao's event loop waiting for messages, and the
//! `RunEvent::Exit` handler never reached (the Ollama server the app started
//! was still alive). tauri-runtime-wry only asks to exit when it handles the
//! window's `Destroyed` event and finds no window left, so a missed or
//! unhandled `Destroyed` leaves the process behind. The watchdog here notices
//! that the main window is gone and finishes the exit itself.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::json;
use tauri::{AppHandle, Manager};

use crate::{security, AppState};

/// A normal exit takes well under a second after the window is gone.
const GRACE: Duration = Duration::from_secs(3);
/// After asking Tauri to exit, how long before stopping the process directly.
const FORCE_AFTER: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(500);

static CLEANED_UP: AtomicBool = AtomicBool::new(false);

/// What has to happen before the process ends: Democritus's firewall rule goes
/// back on if a download had it switched off (the elevated helper also watches
/// for this process to end), and a model server the app started is stopped.
/// Safe to call more than once; only the first call does anything.
pub fn cleanup(app: &AppHandle, how: &str) {
    if CLEANED_UP.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(state) = app.try_state::<AppState>() {
        state.log("security", "app exiting", json!({ "how": how }));
        if let Some((_, gate)) = state.firewall_gate.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            security::signal_gate_done(gate);
        }
        state.server.shutdown();
        // A training worker checkpoints and stops (the build resumes later).
        crate::build::on_exit(&state);
    }
}

#[derive(Debug, PartialEq)]
enum Step {
    Wait,
    RequestExit,
    ForceExit,
}

/// Pure decision, separated for testing: `gone_for` is how long the main window
/// has been gone, `requested_for` how long ago the watchdog asked Tauri to exit.
fn next_step(gone_for: Option<Duration>, requested_for: Option<Duration>) -> Step {
    match (gone_for, requested_for) {
        (_, Some(r)) if r >= FORCE_AFTER => Step::ForceExit,
        (_, Some(_)) => Step::Wait,
        (Some(g), None) if g >= GRACE => Step::RequestExit,
        _ => Step::Wait,
    }
}

/// Starts the watchdog for the main window (Windows only; elsewhere Tauri's own
/// exit is trusted).
pub fn start_watchdog(app: &AppHandle) {
    #[cfg(windows)]
    {
        let Some(hwnd) = app.get_webview_window("main").and_then(|w| w.hwnd().ok()).map(|h| h.0 as isize) else {
            return;
        };
        let app = app.clone();
        std::thread::spawn(move || watch(app, hwnd));
    }
    #[cfg(not(windows))]
    let _ = app;
}

#[cfg(windows)]
fn watch(app: AppHandle, hwnd: isize) {
    #[link(name = "user32")]
    extern "system" {
        fn IsWindow(hwnd: isize) -> i32;
    }
    let mut gone_since = None;
    let mut requested_at: Option<std::time::Instant> = None;
    loop {
        std::thread::sleep(POLL);
        // SAFETY: IsWindow only checks whether the handle still names a window.
        if gone_since.is_none() && unsafe { IsWindow(hwnd) } == 0 {
            gone_since = Some(std::time::Instant::now());
        }
        match next_step(gone_since.map(|t| t.elapsed()), requested_at.map(|t| t.elapsed())) {
            Step::Wait => {}
            Step::RequestExit => {
                if let Some(state) = app.try_state::<AppState>() {
                    state.log("security", "window closed but the app didn't exit; exiting now", json!({}));
                }
                requested_at = Some(std::time::Instant::now());
                app.exit(0); // the normal path: ExitRequested, then RunEvent::Exit runs cleanup
            }
            Step::ForceExit => {
                cleanup(&app, "stopped by the exit watchdog");
                std::process::exit(0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_while_the_window_exists_then_asks_then_forces() {
        let s = Duration::from_secs;
        assert_eq!(next_step(None, None), Step::Wait, "window still open");
        assert_eq!(next_step(Some(s(1)), None), Step::Wait, "Tauri gets time to exit by itself");
        assert_eq!(next_step(Some(GRACE), None), Step::RequestExit);
        assert_eq!(next_step(Some(s(10)), Some(s(1))), Step::Wait, "asked; give the normal exit time");
        assert_eq!(next_step(Some(s(10)), Some(FORCE_AFTER)), Step::ForceExit);
    }
}
