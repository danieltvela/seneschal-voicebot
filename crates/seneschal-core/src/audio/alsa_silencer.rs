//! Redirect libasound's stderr diagnostics into `tracing`.
//!
//! ALSA prints errors such as `pcm_dsnoop ... unable to open slave` or
//! `Cannot open shared library libasound_module_*` straight to stderr via its
//! global error handler. That noise shows up on the terminal/TUI even though
//! the app handles the underlying conditions gracefully. The C shim in
//! `native/alsa_silencer.c` installs a global `snd_lib_error_set_handler` that
//! forwards every message here (debug level), keeping the screen clean.
//! Only compiled on Linux (libasound).

use std::ffi::{CStr, c_char};

use tracing::trace;

unsafe extern "C" {
    /// Defined in `native/alsa_silencer.c`.
    fn seneschal_alsa_set_message_sink(sink: Option<unsafe extern "C" fn(*const c_char)>);
    fn seneschal_alsa_silence();
}

/// Install the libasound error handler that routes its diagnostics to
/// `tracing` instead of stderr. Idempotent; call once at startup.
pub fn install() {
    // SAFETY: installs a static C callback that only forwards to `tracing`.
    unsafe {
        seneschal_alsa_set_message_sink(Some(route));
        seneschal_alsa_silence();
    }
}

unsafe extern "C" fn route(msg: *const c_char) {
    if msg.is_null() {
        return;
    }
    let s = unsafe { CStr::from_ptr(msg) }.to_string_lossy();
    let s = s.trim_end_matches(['\r', '\n']);
    if !s.is_empty() {
        trace!(target: "alsa", "{s}");
    }
}
