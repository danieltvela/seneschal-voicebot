//! Route whisper.cpp / ggml native stderr logging into `tracing`.
//!
//! whisper.cpp prints `whisper_*` and `ggml_*` diagnostics (VAD init, per-chunk
//! `whisper_vad_detect_speech`, backend info, ...) straight to stderr via
//! `fprintf`. On the TUI screen that shows up as stray text. whisper.cpp exposes
//! `whisper_log_set` to install a global callback, so we forward those messages
//! to `tracing` instead — INFO/DEBUG go to trace (hidden at default levels),
//! WARN/ERROR stay visible.

use std::ffi::{CStr, c_char, c_int, c_void};

use tracing::{error, trace, warn};

// ggml_log_level values (see ggml.h). INFO/DEBUG are routed to trace.
const GGML_LOG_LEVEL_WARN: c_int = 3;
const GGML_LOG_LEVEL_ERROR: c_int = 4;

type GgmlLogCallback = unsafe extern "C" fn(c_int, *const c_char, *mut c_void);

unsafe extern "C" {
    /// `WHISPER_API void whisper_log_set(ggml_log_callback, void*)`.
    fn whisper_log_set(callback: Option<GgmlLogCallback>, user_data: *mut c_void);
}

/// Install the whisper.cpp log hook so its native diagnostics go to `tracing`
/// instead of stderr. Call once at startup, before any STT/VAD context exists.
pub fn install() {
    // SAFETY: installs a static C callback that only forwards to `tracing`.
    unsafe {
        whisper_log_set(Some(route), std::ptr::null_mut());
    }
}

unsafe extern "C" fn route(level: c_int, text: *const c_char, _user_data: *mut c_void) {
    if text.is_null() {
        return;
    }
    let s = unsafe { CStr::from_ptr(text) }.to_string_lossy();
    let s = s.trim_end_matches(['\r', '\n']);
    if s.is_empty() {
        return;
    }
    match level {
        GGML_LOG_LEVEL_WARN => warn!(target: "whisper", "{s}"),
        GGML_LOG_LEVEL_ERROR => error!(target: "whisper", "{s}"),
        // INFO/DEBUG (VAD init, per-chunk detection, backend info) → trace,
        // hidden from the default log levels and from the TUI screen.
        _ => trace!(target: "whisper", "{s}"),
    }
}
