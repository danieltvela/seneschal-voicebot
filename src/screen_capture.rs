//! Shared screen capture utility used by the `take_screenshot` tool and
//! `EyesDaemon`.  On macOS, delegates to `screencapture(1)`.
//!
//! When the command fails — which is common when running over SSH or without
//! Screen Recording permission — the error includes diagnostic hints to help
//! the user resolve the problem.

use std::env;

/// Captures the current screen to a temporary file and returns the raw PNG
/// bytes.
pub async fn capture_screen() -> Result<Vec<u8>, String> {
    capture_screen_to("/tmp/seneschal_screenshot.png").await
}

/// Open the Screen Recording privacy pane in System Settings.
///
/// Returns `true` if the URL was accepted by the OS.
pub fn open_screen_recording_settings() -> bool {
    std::process::Command::new("open")
        .args([
            "-g",
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ── internal helpers ────────────────────────────────────────────────────

async fn capture_screen_to(path: &str) -> Result<Vec<u8>, String> {
    // -x: no shutter sound; -t png: force PNG format
    let output = tokio::process::Command::new("screencapture")
        .args(["-x", "-t", "png", path])
        .output()
        .await
        .map_err(|e| format!("screencapture failed to launch: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let base = format!("screencapture error: {stderr}");
        return Err(diagnose(&base, &stderr));
    }

    tokio::fs::read(path)
        .await
        .map_err(|e| format!("failed to read screenshot file: {e}"))
}

fn diagnose(err_msg: &str, stderr: &str) -> String {
    let is_ssh = env::var("SSH_TTY").is_ok() || env::var("SSH_CONNECTION").is_ok();
    let is_permission_err = stderr.contains("could not create image from display");

    if !is_permission_err {
        return err_msg.to_string();
    }

    let mut msg = err_msg.to_string();
    msg.push_str("\n\n");
    msg.push_str("This usually means Screen Recording permission has not been granted.\n\n");

    if is_ssh {
        msg.push_str(
            "The process is running over SSH.  `screencapture` cannot access the display\n\
             without explicit permission.  To fix:\n\n\
             1. On the remote machine, open\n\
                System Settings → Privacy & Security → Screen Recording\n\
             2. Grant the permission to your terminal emulator\n\
                (Terminal, iTerm 2, Warp, etc.)\n\
             3. Disconnect and reconnect your SSH session so the new\n\
                permissions take effect\n\
             4. Try the screenshot again\n\n\
             Alternatively, run Voicebot directly from a local terminal on\n\
             the machine where the display is attached.",
        );
    } else {
        msg.push_str(
            "Open System Settings → Privacy & Security → Screen Recording\n\
             and enable it for your terminal app.  Restart Voicebot afterwards.",
        );
    }

    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnose_permission_error_adds_hint() {
        let msg = diagnose(
            "screencapture error: could not create image from display",
            "could not create image from display",
        );
        assert!(msg.contains("Screen Recording permission"));
    }

    #[test]
    fn diagnose_other_errors_passthrough() {
        let msg = diagnose(
            "screencapture error: No such file or directory",
            "No such file or directory",
        );
        assert!(!msg.contains("Screen Recording"));
        assert!(msg.contains("No such file or directory"));
    }
}
