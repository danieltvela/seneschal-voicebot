use cpal::traits::{DeviceTrait, HostTrait};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::i18n;
use crate::pipeline::PipelineFrame;

/// Spawn a background task that polls for the configured input device.
///
/// When the device transitions from unavailable → available, the startup
/// greeting notification is sent to the LLM via `transcript_tx`.
pub fn spawn(
    device_name: String,
    transcript_tx: mpsc::Sender<PipelineFrame>,
    language: String,
    poll_interval_secs: u64,
    shutdown: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        monitor_loop(
            &device_name,
            &transcript_tx,
            &language,
            poll_interval_secs,
            &shutdown,
        )
        .await;
    })
}

async fn monitor_loop(
    device_name: &str,
    transcript_tx: &mpsc::Sender<PipelineFrame>,
    language: &str,
    poll_interval_secs: u64,
    shutdown: &AtomicBool,
) {
    info!(
        target: "audio",
        "Device monitor started for '{}' (poll every {}s)",
        device_name,
        poll_interval_secs,
    );

    let mut was_available = is_device_available(device_name);
    info!(
        target: "audio",
        "Initial device '{}' state: available={}",
        device_name,
        was_available,
    );

    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(poll_interval_secs));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;

        if shutdown.load(Ordering::SeqCst) {
            info!(target: "audio", "Device monitor shutting down");
            break;
        }

        let is_available = is_device_available(device_name);

        if is_available && !was_available {
            info!(
                target: "audio",
                "Device '{}' connected — sending startup greeting",
                device_name,
            );

            let now = chrono::Local::now();
            let time_str = now.format("%H:%M").to_string();
            let date_str = now.format("%d/%m/%Y").to_string();
            let notification = i18n::get_notification("startup", language)
                .replace("{time_str}", &time_str)
                .replace("{date_str}", &date_str);

            if transcript_tx
                .send(PipelineFrame::SystemNotification { text: notification })
                .await
                .is_err()
            {
                warn!(target: "audio", "Device monitor transcript channel closed, exiting");
                break;
            }
        }

        was_available = is_available;
    }
}

/// Returns `true` if a connected input device matches the given name filter.
fn is_device_available(device_name: &str) -> bool {
    let host = cpal::default_host();
    let name_lower = device_name.to_lowercase();

    match host.input_devices() {
        Ok(devices) => {
            for device in devices {
                if let Ok(desc) = device.description() {
                    let device_name_str = desc.name();
                    if device_name_str.to_lowercase().contains(&name_lower)
                        && device.default_input_config().is_ok()
                    {
                        return true;
                    }
                }
            }
            false
        }
        Err(e) => {
            warn!(target: "audio", "Failed to enumerate input devices: {}", e);
            false
        }
    }
}
