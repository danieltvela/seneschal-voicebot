use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tracing::debug;

use super::output::AudioOutput;

/// Generates and plays a subtle "processing" sound while background tools execute.
///
/// The sound is a low-volume sine wave (440 Hz, amplitude 0.05) played in short
/// bursts with gaps. Provides auditory feedback without interfering with TTS speech.
///
/// Uses a generation counter to avoid duplicate filler loops on stop/start races.
pub struct FillerController {
    audio_output: Arc<AudioOutput>,
    /// Monotonically increasing generation counter. Each `start()` increments it.
    /// Spawned tasks capture their generation and exit if it no longer matches.
    generation: Arc<AtomicU64>,
    active: Arc<AtomicBool>,
    sample_rate: u32,
}

impl FillerController {
    pub fn new(audio_output: Arc<AudioOutput>, sample_rate: u32) -> Self {
        Self {
            audio_output,
            generation: Arc::new(AtomicU64::new(0)),
            active: Arc::new(AtomicBool::new(false)),
            sample_rate,
        }
    }

    /// Start playing the filler sound. Stops any previous filler loop first.
    pub fn start(&self) {
        if self.active.swap(true, Ordering::SeqCst) {
            return;
        }
        let gen_id = self.generation.fetch_add(1, Ordering::SeqCst) + 1;

        let audio = Arc::clone(&self.audio_output);
        let generation = Arc::clone(&self.generation);
        let active = Arc::clone(&self.active);
        let sample_rate = self.sample_rate;

        std::thread::spawn(move || {
            loop {
                // Check if we've been superseded by a newer generation
                if generation.load(Ordering::SeqCst) != gen_id {
                    active.store(false, Ordering::SeqCst);
                    break;
                }

                // Generate one burst: 440 Hz sine, 200ms, amplitude 0.05
                let burst = generate_sine_burst(sample_rate, 440.0, 0.2, 0.05);

                // Play with cancel flag that checks generation
                let cancel = Arc::new(AtomicBool::new(false));
                let cancel_check = Arc::clone(&cancel);
                let gen_check = Arc::clone(&generation);
                let gen_val = gen_id;

                // Spawn a checker that sets cancel when generation changes
                std::thread::spawn(move || {
                    while gen_check.load(Ordering::SeqCst) == gen_val {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    cancel_check.store(true, Ordering::SeqCst);
                });

                if let Err(e) = audio.play_blocking(&burst, sample_rate, &cancel) {
                    debug!(target: "audio", "Filler playback error: {}", e);
                }

                // Check generation again after playback
                if generation.load(Ordering::SeqCst) != gen_id {
                    active.store(false, Ordering::SeqCst);
                    break;
                }

                // Gap between bursts: 800ms with generation check
                for _ in 0..16 {
                    if generation.load(Ordering::SeqCst) != gen_id {
                        active.store(false, Ordering::SeqCst);
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        });
    }

    /// Stop playing the filler sound by incrementing the generation counter.
    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.active.store(false, Ordering::SeqCst);
    }

    /// Returns true if the filler is currently playing.
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }
}

/// Generate a sine wave burst: `duration_secs` seconds at `frequency` Hz.
fn generate_sine_burst(
    sample_rate: u32,
    frequency: f64,
    duration_secs: f64,
    amplitude: f32,
) -> Vec<f32> {
    let num_samples = (sample_rate as f64 * duration_secs) as usize;
    let mut samples = Vec::with_capacity(num_samples);

    // Fade in/out to avoid clicks (10ms each)
    let fade_samples = (sample_rate as f64 * 0.010) as usize;

    for i in 0..num_samples {
        let t = i as f64 / sample_rate as f64;
        let sine = (2.0 * std::f64::consts::PI * frequency * t).sin() as f32;

        // Apply fade envelope
        let envelope = if i < fade_samples {
            i as f32 / fade_samples as f32
        } else if i >= num_samples - fade_samples {
            (num_samples - i) as f32 / fade_samples as f32
        } else {
            1.0
        };

        samples.push(sine * amplitude * envelope);
    }

    samples
}
