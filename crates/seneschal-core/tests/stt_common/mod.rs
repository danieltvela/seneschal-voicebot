//! Shared helpers for the STT audit integration tests (2026-08-25).
//!
//! - WAV fixture generation (espeak-ng + ffmpeg -> 16 kHz mono s16le),
//!   cached under `<workspace>/target/stt-fixtures/`.
//! - WAV loading / onset / offset energy detection.
//! - Streaming VAD+Whisper pipeline runner that measures, per segment:
//!   * `t_start_stream` -- stream time (s) at which SpeechStart fired
//!   * `t_close_stream` -- stream time (s) at which the segment was closed
//!   * `infer_wall_ms` -- wall-clock time of the `process_audio` call that
//!     contained the blocking Whisper inference
//!   * transcript + quality metrics
//! - Text normalization + token-presence helpers for accuracy verdicts.

#![allow(dead_code)]

use anyhow::{bail, Context, Result};
use seneschal_core::stt::{
    SpeechEvent, TranscriptionQuality, WhisperSttProvider, WhisperSTTVADConfig,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use tokio::sync::mpsc;

/// Pipeline sample rate (matches production: `sample_rate = 16000`).
pub const SR: usize = 16_000;
/// Chunk size fed to the provider, matching production `chunk_ms = 100`.
pub const CHUNK: usize = SR / 10;

// -- Model paths ------------------------------------------------------------

/// Workspace root (.../voicebot) as seen from crates/seneschal-core.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf()
}

/// Resolve a possibly-relative model path against the workspace root (the
/// test cwd is the crate dir, production cwd is the workspace root).
fn resolve_model_path(p: &str) -> String {
    let path = std::path::Path::new(p);
    if path.is_absolute() {
        p.to_string()
    } else {
        let abs = workspace_root().join(path);
        if abs.exists() {
            // Keep it relative to the workspace root so the log line matches
            // production naming; whisper-cpp resolves relative to process cwd,
            // so return the absolute path for safety.
            abs.to_string_lossy().to_string()
        } else {
            p.to_string()
        }
    }
}

pub fn model_paths() -> (String, String) {
    let whisper = std::env::var("WHISPER_MODEL")
        .unwrap_or_else(|_| "models/ggml-large-v3-turbo.bin".to_string());
    let vad = std::env::var("VAD_MODEL").unwrap_or_else(|_| {
        // NOTE: the embedded default in seneschal.pro.toml points to the old
        // name `ggml-silero-vad.bin`; the file actually on disk (and in .env)
        // is `ggml-silero-v5.1.2.bin`. Try both.
        "models/ggml-silero-v5.1.2.bin".to_string()
    });
    (resolve_model_path(&whisper), resolve_model_path(&vad))
}

pub fn models_available() -> bool {
    let (wm, vm) = model_paths();
    std::path::Path::new(&wm).exists() && std::path::Path::new(&vm).exists()
}

pub fn skip_if_no_models() {
    if !models_available() {
        let (wm, vm) = model_paths();
        eprintln!(
            "SKIP: STT models not found (whisper={wm}, vad={vm}). Set WHISPER_MODEL / VAD_MODEL."
        );
        std::process::exit(0);
    }
}

/// Default provider config mirroring production (seneschal.pro.toml + .env):
/// silence 300 ms, start 0.65 / end 0.45, confirm_probes 2.
pub fn default_config(language: &str) -> WhisperSTTVADConfig {
    let (wm, vm) = model_paths();
    WhisperSTTVADConfig {
        whisper_model: wm,
        vad_model: vm,
        language: language.to_string(),
        silence_ms: 300,
        vad_start_threshold: 0.65,
        vad_end_threshold: 0.45,
        vad_confirm_probes: 2,
    }
}

/// Create a fresh provider per test. Deliberately NOT cached: the VAD state
/// machine is stateful, so every test owns its own instance to avoid
/// cross-test interference when cargo runs tests in parallel within a binary.
/// Model load cost (~2-5 s each) is acceptable for a manually-run audit suite.
pub fn new_provider(language: &str) -> WhisperSttProvider {
    WhisperSttProvider::new(default_config(language))
        .expect("failed to load Whisper/VAD models")
}

// -- Fixture generation ------------------------------------------------------

pub fn fixture_dir() -> PathBuf {
    let dir = workspace_root().join("target").join("stt-fixtures");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn run(cmd: &str, args: &[&str], _out: &Path) -> Result<()> {
    let status = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("failed to spawn {cmd}"))?;
    if !status.status.success() {
        bail!(
            "{cmd} failed: {}",
            String::from_utf8_lossy(&status.stderr).trim()
        );
    }
    Ok(())
}

/// espeak-ng -> wav -> ffmpeg -> 16 kHz mono s16le.
fn synth(text: &str, voice: &str, rate: u32, out: &Path) -> Result<()> {
    let tmp_raw = out.with_extension("raw.wav");
    run(
        "espeak-ng",
        &["-v", voice, "-s", &rate.to_string(), "-w", tmp_raw.to_str().unwrap(), text],
        &tmp_raw,
    )?;
    run(
        "ffmpeg",
        &[
            "-y",
            "-loglevel",
            "error",
            "-i",
            tmp_raw.to_str().unwrap(),
            "-ar",
            "16000",
            "-ac",
            "1",
            "-sample_fmt",
            "s16",
            out.to_str().unwrap(),
        ],
        out,
    )?;
    let _ = std::fs::remove_file(&tmp_raw);
    Ok(())
}

/// Concatenate 16 kHz mono s16le WAVs with `silence_ms` of silence between.
fn concat_wavs(inputs: &[PathBuf], silence_ms: u32, out: &Path) -> Result<()> {
    let data = inputs
        .iter()
        .map(|p| read_wav_pcm16(p))
        .collect::<Result<Vec<Vec<i16>>>>()?;
    let mut out_data: Vec<i16> = Vec::new();
    for (i, d) in data.iter().enumerate() {
        if i > 0 {
            out_data.extend(std::iter::repeat(0i16).take(SR / 1000 * silence_ms as usize));
        }
        out_data.extend_from_slice(d);
    }
    write_wav_pcm16(out, &out_data)?;
    Ok(())
}

/// Generate (or load from cache) a named fixture.
pub fn fixture(name: &str, make: impl FnOnce() -> Result<()>) -> Result<PathBuf> {
    let force = std::env::var("STT_FORCE_REGEN").is_ok();
    let path = fixture_dir().join(name);
    if !force && path.exists() {
        return Ok(path);
    }
    make()?;
    if !path.exists() {
        bail!("generator for {name} did not produce {}", path.display());
    }
    Ok(path)
}

// -- WAV I/O -----------------------------------------------------------------

pub fn read_wav_pcm16(path: &Path) -> Result<Vec<i16>> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("cannot read WAV {}", path.display()))?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a RIFF/WAV file: {}", path.display());
    }
    let mut pos = 12usize;
    let mut data: Option<Vec<u8>> = None;
    while pos + 8 <= bytes.len() {
        let cid = String::from_utf8_lossy(&bytes[pos..pos + 4]).to_string();
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        if cid == "data" {
            data = Some(bytes[body..body + size.min(bytes.len() - body)].to_vec());
            break;
        }
        pos = body + size + (size % 2);
    }
    let data = data.ok_or_else(|| anyhow::anyhow!("no data chunk in {}", path.display()))?;
    Ok(data
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect())
}

fn write_wav_pcm16(path: &Path, data: &[i16]) -> Result<()> {
    let n = data.len() as u32;
    let mut v = Vec::with_capacity(44 + data.len() * 2);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&((36 + n * 2) as u32).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); // PCM
    v.extend_from_slice(&1u16.to_le_bytes()); // mono
    v.extend_from_slice(&(SR as u32).to_le_bytes());
    v.extend_from_slice(&(SR as u32 * 2).to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(n * 2).to_le_bytes());
    for s in data {
        v.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, v)?;
    Ok(())
}

pub fn load_wav_f32(path: &Path) -> Result<Vec<f32>> {
    Ok(read_wav_pcm16(path)?
        .iter()
        .map(|s| *s as f32 / 32768.0)
        .collect())
}

/// First stream second where 10 ms RMS exceeds threshold.
pub fn speech_onset(samples: &[f32], threshold: f32) -> f32 {
    let win = SR / 100;
    let mut i = 0;
    while i + win <= samples.len() {
        let rms =
            (samples[i..i + win].iter().map(|s| s * s).sum::<f32>() / win as f32).sqrt();
        if rms > threshold {
            return i as f32 / SR as f32;
        }
        i += win / 4;
    }
    0.0
}

/// Last stream second where speech energy still exists.
pub fn speech_offset(samples: &[f32], threshold: f32) -> f32 {
    let win = SR / 100;
    let mut i = samples.len().saturating_sub(win);
    while i > 0 {
        let rms =
            (samples[i..i + win].iter().map(|s| s * s).sum::<f32>() / win as f32).sqrt();
        if rms > threshold {
            return (i + win) as f32 / SR as f32;
        }
        i -= win / 4;
    }
    samples.len() as f32 / SR as f32
}

// -- Streaming pipeline runner -----------------------------------------------

pub struct SegmentTiming {
    /// Stream time (s) when SpeechStart fired (VAD commit).
    pub t_start_stream: f32,
    /// Stream time (s) when the segment was closed (silence threshold / cap).
    pub t_close_stream: f32,
    /// Wall clock ms of the `process_audio` call that contained the
    /// blocking Whisper inference (approx inference time).
    pub infer_wall_ms: u128,
    pub quality: TranscriptionQuality,
}

pub struct RunReport {
    pub segments: Vec<SegmentTiming>,
    pub stream_duration_s: f32,
}

impl RunReport {
    /// Print a compact, report-ready table line for one run.
    pub fn print(&self, label: &str) {
        println!("\n=== {label} (stream {:.2}s) ===", self.stream_duration_s);
        for (i, seg) in self.segments.iter().enumerate() {
            let seg_dur = seg.t_close_stream - seg.t_start_stream;
            let rtf = if seg_dur > 0.0 {
                seg.infer_wall_ms as f32 * 1e-3 / seg_dur
            } else {
                0.0
            };
            println!(
                "[{label}] seg{}: start@{:.3}s close@{:.3}s dur={:.2}s infer={}ms RTF={:.3}",
                i,
                seg.t_start_stream,
                seg.t_close_stream,
                seg_dur,
                seg.infer_wall_ms,
                rtf
            );
            println!("[{label}] seg{} transcript: {:?}", i, seg.quality.text);
            println!(
                "[{label}] seg{} quality: logprob={:.3} compression={:.2} no_speech_prob={:.3}",
                i,
                seg.quality.avg_logprob,
                seg.quality.compression_ratio,
                seg.quality.no_speech_prob
            );
        }
        if self.segments.is_empty() {
            println!("[{label}] NO segments produced");
        }
    }
}

/// Feed `audio` to the provider in production-sized chunks (100 ms) and
/// collect events with stream-time + wall-clock instrumentation.
pub async fn run_stream(stt: &mut WhisperSttProvider, audio: &[f32]) -> RunReport {
    let (tx, mut rx) = mpsc::channel(256);
    let mut segments: Vec<SegmentTiming> = Vec::new();
    let mut start_at: Option<f32> = None;
    let mut fed = 0usize;

    'feed: loop {
        if fed >= audio.len() {
            break 'feed;
        }
        let chunk_end = fed + CHUNK.min(audio.len() - fed);
        let chunk = &audio[fed..chunk_end];
        fed = chunk_end;
        let t_call = Instant::now();
        stt.process_audio(chunk, &tx).await.unwrap();
        let stream_now = fed as f32 / SR as f32;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                SpeechEvent::SpeechStart => {
                    start_at = Some(stream_now);
                }
                SpeechEvent::SpeechEnd(quality) => {
                    segments.push(SegmentTiming {
                        t_start_stream: start_at.take().unwrap_or(0.0),
                        t_close_stream: stream_now,
                        infer_wall_ms: t_call.elapsed().as_millis(),
                        quality,
                    });
                }
                SpeechEvent::Speech(p) => {
                    println!("[partial] {p}");
                }
            }
        }
    }

    // Flush: 4 s of silence guarantees the final segment closes
    // (silence_ms=300 + probe quantization).
    let silence = vec![0.0f32; SR * 4];
    for chunk in silence.chunks(CHUNK) {
        let t_call = Instant::now();
        stt.process_audio(chunk, &tx).await.unwrap();
        fed += chunk.len();
        let stream_now = fed as f32 / SR as f32;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                SpeechEvent::SpeechStart => {
                    start_at = Some(stream_now);
                }
                SpeechEvent::SpeechEnd(quality) => {
                    segments.push(SegmentTiming {
                        t_start_stream: start_at.take().unwrap_or(0.0),
                        t_close_stream: stream_now,
                        infer_wall_ms: t_call.elapsed().as_millis(),
                        quality,
                    });
                }
                SpeechEvent::Speech(p) => {
                    println!("[partial] {p}");
                }
            }
        }
    }

    RunReport {
        segments,
        stream_duration_s: (audio.len() / SR) as f32,
    }
}

/// One-shot transcription through the same code path used by the provider
/// (`transcribe_complete` -> internal `transcribe()`), with wall timing.
pub fn run_transcribe_complete(
    stt: &WhisperSttProvider,
    audio: &[f32],
) -> (TranscriptionQuality, u128) {
    let t = Instant::now();
    let q = stt.transcribe_complete(audio).expect("transcribe_complete failed");
    (q, t.elapsed().as_millis())
}

// -- Text normalization / accuracy helpers ------------------------------------

pub fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == ' ')
        .collect()
}

/// Whitespace-insensitive substring test, e.g. "Qwen 3.8 27B" matches
/// "qwen3.827b" even if whisper returned "Qwen 3.8 27 B".
pub fn has_token(text: &str, token: &str) -> bool {
    let t: String = norm(text).chars().filter(|c| !c.is_whitespace()).collect();
    let k: String = norm(token).chars().filter(|c| !c.is_whitespace()).collect();
    !k.is_empty() && t.contains(&k)
}

/// Check a list of expected tokens; print hits/misses. Returns hit count.
pub fn check_tokens(label: &str, text: &str, tokens: &[&str]) -> usize {
    let mut hits = 0;
    for tok in tokens {
        let ok = has_token(text, tok);
        if ok {
            hits += 1;
        }
        println!("[{label}] token {tok:?}: {}", if ok { "HIT" } else { "MISS" });
    }
    println!("[{label}] tokens: {hits}/{} hit", tokens.len());
    hits
}

// -- Audio fixtures used across the audit -------------------------------------

/// Continuous Spanish speech, no pauses, ~25-30 s (must exceed the 20 s cap).
pub const CONT_ES: &str = "Estoy auditando el sistema de reconocimiento de voz porque la latencia \
resulta alta cuando hablo en continuo y además las transcripciones de los nombres de modelos de \
inteligencia artificial salen mal, por lo que voy a medir el tiempo de detección del habla, la \
precisión del modelo en español e inglés y el rendimiento real de la inferencia en esta máquina, \
y luego comparar los resultados con las expectativas para identificar cada fallo y proponer \
mejoras concretas";

/// Continuous English speech, no pauses, ~25-30 s.
pub const CONT_EN: &str = "I am auditing the speech recognition system because the latency is too \
high when I speak continuously and also the transcriptions of machine learning model names keep \
coming out wrong, so I will measure the voice activity detection timing, the transcription \
accuracy in both Spanish and English, and the real inference performance on this machine, then \
I will compare the measured numbers against the expected values to identify every failure and \
propose concrete improvements for the pipeline";

/// Mid-phrase code-switching: Spanish voice reading a sentence that mixes in
/// English technical phrases (Spanish phonemes -- realistic code-switching).
pub const CODESWITCH_MIX: &str = "Hoy voy a probar el Qwen 3.8 27B porque the model performs \
really well en mi opinión y el SGLang lo sirve sin problemas";

/// Language boundary: Spanish sentence, then English sentence (different voice).
pub const BOUNDARY_ES: &str = "El modelo Qwen 3.8 funciona muy bien en mi opinión";
pub const BOUNDARY_EN: &str = "and it runs fast on this machine so the latency is acceptable";

/// Technical terms in Spanish.
pub const TERMS_ES: &str = "Estoy usando el modelo Qwen 3.8 27B, también Whisper Large V3 Turbo, \
Silero VAD, SGLang, vLLM, la tarjeta RTX PRO 6000 y DFlash2";

/// Technical terms in English.
pub const TERMS_EN: &str = "I am using the model Qwen 3.8 27B, also Whisper Large V3 Turbo, \
Silero VAD, SGLang, vLLM, the RTX PRO 6000 card and DFlash2";

pub const SHORT_ES: &str = "Hola, ¿qué tal está el sistema de audio hoy?";
pub const SHORT_EN: &str = "Hello, the audio system works fine today.";

/// Expected key tokens for the terms fixtures (whitespace-insensitive).
pub const TERM_TOKENS: &[&str] = &[
    "qwen", "3.8", "27", "whisper", "large", "v3", "turbo", "silero", "vad", "sglang", "vllm",
    "rtx", "pro", "6000", "dflash2",
];

pub fn ensure_fixture_cont_es() -> Result<PathBuf> {
    fixture("cont_es.wav", || synth(CONT_ES, "es", 165, &fixture_dir().join("cont_es.wav")))
}

pub fn ensure_fixture_cont_en() -> Result<PathBuf> {
    fixture("cont_en.wav", || synth(CONT_EN, "en-us", 175, &fixture_dir().join("cont_en.wav")))
}

pub fn ensure_fixture_codeswitch_mix() -> Result<PathBuf> {
    fixture("codeswitch_mix.wav", || {
        synth(CODESWITCH_MIX, "es", 160, &fixture_dir().join("codeswitch_mix.wav"))
    })
}

pub fn ensure_fixture_codeswitch_boundary() -> Result<PathBuf> {
    fixture("codeswitch_boundary.wav", || {
        let es = fixture_dir().join("csb_es.wav");
        let en = fixture_dir().join("csb_en.wav");
        synth(BOUNDARY_ES, "es", 160, &es)?;
        synth(BOUNDARY_EN, "en-us", 175, &en)?;
        concat_wavs(&[es.clone(), en], 400, &fixture_dir().join("codeswitch_boundary.wav"))?;
        Ok(())
    })
}

pub fn ensure_fixture_terms_es() -> Result<PathBuf> {
    fixture("terms_es.wav", || {
        synth(TERMS_ES, "es", 150, &fixture_dir().join("terms_es.wav"))
    })
}

pub fn ensure_fixture_terms_en() -> Result<PathBuf> {
    fixture("terms_en.wav", || {
        synth(TERMS_EN, "en-us", 170, &fixture_dir().join("terms_en.wav"))
    })
}

pub fn ensure_fixture_short_es() -> Result<PathBuf> {
    fixture("short_es.wav", || {
        synth(SHORT_ES, "es", 150, &fixture_dir().join("short_es.wav"))
    })
}

pub fn ensure_fixture_short_en() -> Result<PathBuf> {
    fixture("short_en.wav", || {
        synth(SHORT_EN, "en-us", 175, &fixture_dir().join("short_en.wav"))
    })
}
