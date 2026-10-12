use cpal::traits::{DeviceTrait, HostTrait};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;
use tracing::{info, warn};

use seneschal_common::events::ProactiveEvent;

/// Spawn a background task that watches for the configured audio device and
/// starts the audio pipeline when it becomes usable.
///
/// The monitor is **presence-based**: it checks whether the configured device is
/// enumerated (never opens it). Opening the device here collides with the app's
/// own capture/output streams (the device is busy → probe fails → availability
/// "flaps"), which used to re-trigger the startup greeting over and over.
///
/// When the device is present but capture is not active, it sends
/// `ProactiveEvent::DeviceConnected` so the main loop (re)starts capture. The
/// main loop de-duplicates the greeting, so repeated signals just become retry
/// attempts, never duplicate greetings.
pub fn spawn(
    device_name: Option<String>,
    capture_active: Arc<AtomicBool>,
    proactive_tx: mpsc::Sender<ProactiveEvent>,
    poll_interval_secs: u64,
    shutdown: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        monitor_loop(
            device_name.as_deref(),
            &capture_active,
            &proactive_tx,
            poll_interval_secs,
            &shutdown,
        )
        .await;
    })
}

async fn monitor_loop(
    device_name: Option<&str>,
    capture_active: &AtomicBool,
    proactive_tx: &mpsc::Sender<ProactiveEvent>,
    poll_interval_secs: u64,
    shutdown: &AtomicBool,
) {
    let label = device_name
        .map(|n| n.to_string())
        .unwrap_or_else(|| "default input device".to_string());
    info!(
        target: "audio",
        "Device monitor started for '{}' (poll every {}s)",
        label,
        poll_interval_secs,
    );

    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(poll_interval_secs));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;

        if shutdown.load(Ordering::SeqCst) {
            info!(target: "audio", "Device monitor shutting down");
            break;
        }

        // Present but not capturing → (re)start the audio pipeline.
        let present = device_present(device_name);
        let active = capture_active.load(Ordering::SeqCst);
        if present && !active {
            info!(
                target: "audio",
                "Device '{}' present and capture idle — signalling DeviceConnected",
                label,
            );

            if proactive_tx
                .send(ProactiveEvent::DeviceConnected)
                .await
                .is_err()
            {
                warn!(target: "audio", "Device monitor proactive channel closed, exiting");
                break;
            }
        }
    }
}

/// Strip an optional `#N` index suffix from a device name string.
///
/// `"AirPods Pro#0"` → `("AirPods Pro", Some(0))`
/// `"AirPods Pro"`   → `("AirPods Pro", None)`
fn parse_device_name(name: &str) -> (&str, Option<usize>) {
    if let Some(pos) = name.rfind('#')
        && let Ok(idx) = name[pos + 1..].parse::<usize>()
    {
        return (&name[..pos], Some(idx));
    }
    (name, None)
}

/// Returns `true` if the configured device is currently enumerated, checking
/// both input and output sides so a headset reconnect is detected regardless of
/// which side becomes available first.
///
/// This never opens the device: opening it while the app holds a capture/output
/// stream fails with "busy" and makes the availability flap (re-triggering the
/// startup greeting). Presence is what matters here; openability is the main
/// loop's job.
fn device_present(device_name: Option<&str>) -> bool {
    let host = cpal::default_host();

    let Some(device_name) = device_name else {
        // No specific device — report available when any input device exists.
        return match host.input_devices() {
            Ok(mut devices) => devices.find(|d| d.default_input_config().is_ok()).is_some(),
            Err(e) => {
                warn!(target: "audio", "Failed to enumerate input devices: {}", e);
                false
            }
        };
    };

    let (name_filter, _index) = parse_device_name(device_name);
    let name_lower = name_filter.to_lowercase();

    let matched = |dev: cpal::Device| {
        dev.description()
            .map(|desc| desc.name().to_lowercase().contains(&name_lower))
            .unwrap_or(false)
    };

    let in_inputs = match host.input_devices() {
        Ok(devices) => {
            let mut devices = devices;
            devices.any(matched)
        }
        Err(e) => {
            warn!(target: "audio", "Failed to enumerate input devices: {}", e);
            false
        }
    };
    if in_inputs {
        return true;
    }

    match host.output_devices() {
        Ok(devices) => {
            let mut devices = devices;
            devices.any(matched)
        }
        Err(e) => {
            warn!(target: "audio", "Failed to enumerate output devices: {}", e);
            false
        }
    }
}
