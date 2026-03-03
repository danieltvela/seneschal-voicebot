use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, StreamConfig};
use std::sync::{Arc, Mutex};

/// Audio output handler for playing synthesized speech
pub struct AudioOutput {
    device: Device,
    config: StreamConfig,
    buffer: Arc<Mutex<Vec<f32>>>,
}

impl AudioOutput {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .context("No output device available")?;

        let config = device
            .default_output_config()
            .context("Failed to get default output config")?;

        let config = StreamConfig {
            channels: config.channels(),
            sample_rate: config.sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        };

        Ok(Self {
            device,
            config,
            buffer: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Play audio samples through the speaker
    pub fn play(&self, samples: Vec<f32>) -> Result<Stream> {
        let buffer = self.buffer.clone();
        {
            let mut buf = buffer.lock().unwrap();
            buf.clear();
            buf.extend_from_slice(&samples);
        }

        let mut position = 0;
        let buffer_clone = buffer.clone();

        let stream = self.device.build_output_stream(
            &self.config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let buf = buffer_clone.lock().unwrap();
                for sample in data.iter_mut() {
                    if position < buf.len() {
                        *sample = buf[position];
                        position += 1;
                    } else {
                        *sample = 0.0;
                    }
                }
            },
            |err| eprintln!("Audio output error: {}", err),
            None,
        )?;

        stream.play()?;
        Ok(stream)
    }

    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.config.channels
    }
}
