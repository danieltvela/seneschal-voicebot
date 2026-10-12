//! Point ALSA's virtual `default` PCM at the configured device.
//!
//! On machines whose first sound card is HDMI-only (no PCM 0), the ALSA
//! `default` PCM cannot open: dmix/dsnoop fail with `unable to open slave`
//! and libasound spams stderr. ALSA honours the `ALSA_PCM_CARD` environment
//! variable when resolving `default`, so we resolve the configured device to
//! its ALSA card id and set that env var before any PCM is opened.
//!
//! Only compiled on Linux (libasound).

use tracing::{info, warn};

/// Resolve the configured audio device name to an ALSA card id and set
/// `ALSA_PCM_CARD` so the `default` PCM routes to that card instead of the
/// (often unusable) card 0.
pub fn set_default_pcm_card(device_name: Option<&str>) {
    let Some(device_name) = device_name else {
        return;
    };
    let Some(card_id) = find_card_id(device_name) else {
        warn!(
            target: "audio",
            "Could not resolve ALSA card id for '{device_name}' — 'default' PCM may fail to open"
        );
        return;
    };
    // SAFETY: process-wide environment change made before any ALSA PCM is
    // opened; only affects the ALSA `default` alias, not named devices.
    unsafe {
        std::env::set_var("ALSA_PCM_CARD", &card_id);
    }
    info!(
        target: "audio",
        "ALSA default PCM card -> '{card_id}' (resolved from '{device_name}')"
    );
}

/// Read `/proc/asound/cards` and return the ALSA id (`[...]`) of the first
/// card whose long name contains `device_name` (case-insensitive).
fn find_card_id(device_name: &str) -> Option<String> {
    let cards = std::fs::read_to_string("/proc/asound/cards").ok()?;
    let needle = device_name.to_lowercase();
    for line in cards.lines() {
        if !line.to_lowercase().contains(&needle) {
            continue;
        }
        let rest = line.split('[').nth(1)?;
        let id = rest.split(']').next()?.trim();
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    None
}
