//! Microphone permission status for the desktop audio panel (issue #34).
//!
//! On macOS the capturing process is the daemon sidecar, so the TCC flow must
//! be driven (or at least verified) there rather than in the GUI shell. Linux
//! does not have an equivalent user prompt.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MicrophonePermission {
    /// No user prompt is needed on this platform.
    NotRequired,
    NotDetermined,
    Restricted,
    Denied,
    Authorized,
}

#[cfg(target_os = "macos")]
pub fn microphone_permission() -> MicrophonePermission {
    use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeAudio};

    // SAFETY: AVFoundation class method with a static media-type constant.
    let status = unsafe {
        AVCaptureDevice::authorizationStatusForMediaType(
            AVMediaTypeAudio.expect("AVMediaTypeAudio"),
        )
    };
    match status {
        objc2_av_foundation::AVAuthorizationStatus::NotDetermined => {
            MicrophonePermission::NotDetermined
        }
        objc2_av_foundation::AVAuthorizationStatus::Restricted => MicrophonePermission::Restricted,
        objc2_av_foundation::AVAuthorizationStatus::Denied => MicrophonePermission::Denied,
        objc2_av_foundation::AVAuthorizationStatus::Authorized => MicrophonePermission::Authorized,
        _ => MicrophonePermission::NotDetermined,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn microphone_permission() -> MicrophonePermission {
    MicrophonePermission::NotRequired
}

/// Ask macOS for microphone access and wait for the TCC answer.
#[cfg(target_os = "macos")]
pub async fn request_microphone_permission() -> MicrophonePermission {
    tokio::task::spawn_blocking(request_microphone_permission_blocking)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(target: "audio", "Microphone permission task failed: {error}");
            microphone_permission()
        })
}

#[cfg(not(target_os = "macos"))]
pub async fn request_microphone_permission() -> MicrophonePermission {
    microphone_permission()
}

#[cfg(target_os = "macos")]
fn request_microphone_permission_blocking() -> MicrophonePermission {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeAudio};
    use std::sync::mpsc;

    if microphone_permission() == MicrophonePermission::Authorized {
        return MicrophonePermission::Authorized;
    }

    let (tx, rx) = mpsc::channel::<bool>();
    let block = RcBlock::new(move |granted: Bool| {
        let _ = tx.send(granted.as_bool());
    });
    let block_ptr = RcBlock::as_ptr(&block) as *mut block2::DynBlock<dyn Fn(Bool)>;

    // SAFETY: the AVFoundation completion block is kept alive by `block` until
    // the blocking channel receive below returns.
    unsafe {
        AVCaptureDevice::requestAccessForMediaType_completionHandler(
            AVMediaTypeAudio.expect("AVMediaTypeAudio"),
            &*block_ptr,
        );
    }

    match rx.recv() {
        Ok(true) => MicrophonePermission::Authorized,
        Ok(false) => MicrophonePermission::Denied,
        Err(error) => {
            tracing::warn!(target: "audio", "Microphone permission callback failed: {error}");
            microphone_permission()
        }
    }
}
