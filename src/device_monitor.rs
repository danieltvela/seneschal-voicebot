use cpal::traits::{DeviceTrait, HostTrait};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::agents::ProactiveEvent;

/// Spawn a background task that polls for the configured audio device.
///
/// When the device transitions from unavailable → available, sends a
/// `ProactiveEvent::DeviceConnected` so the main loop can start capture,
/// swap the output device, and send the startup greeting.
///
/// When `device_name` is `None`, monitors the default input device.
pub fn spawn(
    device_name: Option<String>,
    proactive_tx: mpsc::Sender<ProactiveEvent>,
    poll_interval_secs: u64,
    shutdown: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        monitor_loop(
            device_name.as_deref(),
            &proactive_tx,
            poll_interval_secs,
            &shutdown,
        )
        .await;
    })
}

async fn monitor_loop(
    device_name: Option<&str>,
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

    let mut was_available = is_device_available(device_name);
    info!(
        target: "audio",
        "Initial device '{}' state: available={}",
        label,
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
                "Device '{}' connected — signalling DeviceConnected",
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

        was_available = is_available;
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

/// Returns `true` if a connected audio device matches the given name filter.
///
/// Checks **both** input and output devices so a Bluetooth headset reconnect
/// is detected regardless of which side becomes available first.
///
/// When `device_name` is `None`, checks whether _any_ input device is available
/// (i.e. the default input device exists).
fn is_device_available(device_name: Option<&str>) -> bool {
    let host = cpal::default_host();

    // No specific device — check whether any input device exists
    if device_name.is_none() {
        return match host.input_devices() {
            Ok(devices) => {
                let devices: Vec<_> = devices.collect();
                let found = devices.iter().any(|d| d.default_input_config().is_ok());
                if found {
                    debug!(target: "audio", "Default input device is available");
                } else {
                    debug!(target: "audio", "No input device available");
                }
                found
            }
            Err(e) => {
                warn!(target: "audio", "Failed to enumerate input devices: {}", e);
                false
            }
        };
    }

    let device_name = device_name.unwrap();
    let (name_filter, index) = parse_device_name(device_name);
    let name_lower = name_filter.to_lowercase();

    // Check input devices
    if let Ok(devices) = host.input_devices() {
        let collected: Vec<_> = devices.collect();
        let matches: Vec<String> = collected
            .iter()
            .filter(|d| {
                d.description()
                    .map(|desc| {
                        desc.name().to_lowercase().contains(&name_lower)
                            && d.default_input_config().is_ok()
                    })
                    .unwrap_or(false)
            })
            .filter_map(|d| d.description().ok().map(|desc| desc.name().to_string()))
            .collect();

        if let Some(idx) = index {
            if idx < matches.len() {
                debug!(
                    target: "audio",
                    "Input device '{}' found at index {} ({} matches)",
                    matches[idx],
                    idx,
                    matches.len(),
                );
                return true;
            }
        } else if !matches.is_empty() {
            debug!(
                target: "audio",
                "Input device '{}' found ({} matches)",
                matches[0],
                matches.len(),
            );
            return true;
        }
    }

    // Check output devices
    if let Ok(devices) = host.output_devices() {
        let collected: Vec<_> = devices.collect();
        let matches: Vec<String> = collected
            .iter()
            .filter(|d| {
                d.description()
                    .map(|desc| {
                        desc.name().to_lowercase().contains(&name_lower)
                            && d.default_output_config().is_ok()
                    })
                    .unwrap_or(false)
            })
            .filter_map(|d| d.description().ok().map(|desc| desc.name().to_string()))
            .collect();

        if let Some(idx) = index {
            if idx < matches.len() {
                debug!(
                    target: "audio",
                    "Output device '{}' found at index {} ({} matches)",
                    matches[idx],
                    idx,
                    matches.len(),
                );
                return true;
            }
        } else if !matches.is_empty() {
            debug!(
                target: "audio",
                "Output device '{}' found ({} matches)",
                matches[0],
                matches.len(),
            );
            return true;
        }
    }

    debug!(
        target: "audio",
        "Device '{}' not found in input or output devices",
        name_filter,
    );
    false
}
