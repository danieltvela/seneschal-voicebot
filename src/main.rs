mod audio;
mod config;
mod websocket_client;

use anyhow::Result;
use async_channel::{bounded, Sender};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use crate::audio::audio_capture::{AudioCapture, AudioChunk};
use crate::audio::audio_transform::{AudioTransformer, TransformedAudio};
use config::Config;
use websocket_client::WebSocketClient;

const AUDIO_CHANNEL_CAPACITY: usize = 100;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    info!("Starting microphone streamer...");

    // Load configuration from environment
    let config = Config::from_env()?;
    
    // Handle list devices flag
    if config.list_devices {
        AudioCapture::print_devices()?;
        return Ok(());
    }
    
    info!("Configuration loaded: {:?}", config);

    // Initialize audio capture with optional device selection
    let audio_capture = AudioCapture::new(config.audio_device.as_deref())?;
    let source_sample_rate = audio_capture.sample_rate();
    let source_channels = audio_capture.channels();

    info!(
        "Audio source: {} Hz, {} channels",
        source_sample_rate, source_channels
    );

    // TODO: Initialize and run S2S process.

    // Handle shutdown signals
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Received shutdown signal");
        }
    }

    info!("Shutting down...");
    Ok(())
}
