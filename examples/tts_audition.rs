//! Audición de voces Kokoro: renderiza una frase de prueba a ficheros WAV,
//! uno por (voz, velocidad), para poder compararlas sin arrancar el pipeline.
//!
//! Uso:
//!   cargo run --release --features kokoro --example tts_audition
//!   cargo run --release --features kokoro --example tts_audition -- --voices ef_dora,em_alex
//!   cargo run --release --features kokoro --example tts_audition -- --speed 1.0,1.43
//!   cargo run --release --features kokoro --example tts_audition -- \
//!       --voices bm_fable,af_heart --lang en-us --text "Good morning, sir."
//!
//! Modelos: KOKORO_MODEL / KOKORO_VOICES del .env (dotenvy los carga aquí
//! también), con fallback a models/kokoro-v1.0.onnx + models/voices-v1.0.bin.
//!
//! Salida: tts_samples/<voz>__s<velocidad>.wav (PCM16 mono 24 kHz).
//! Reproducir: aplay tts_samples/ef_dora__s1.0.wav  (o paplay / ffplay).
//!
//! La voz acepta la misma sintaxis que KOKORO_VOICE, incluidas mezclas
//! (`em_alex.4+bm_fable.6`). `--lang` es el código BCP-47 para la fonemización
//! de espeak-ng (es, en-us, en-gb...); debe casar con el idioma del texto.

use anyhow::{Result, bail};
use seneschal_core::tts::KokoroTts;
use std::path::PathBuf;

const DEFAULT_VOICES: &str = "ef_dora,em_alex,em_santa,em_alex.4+bm_fable.6";
const DEFAULT_SPEEDS: &str = "1.0,1.43";
const DEFAULT_LANG: &str = "es";
const DEFAULT_TEXT: &str = "Señor, buenos días. Son las nueve y cuarto y tengo 3 reuniones: \
                            una a las 10:30 y dos por la tarde. La temperatura es de 21 grados.";
const SAMPLE_RATE: u32 = 24_000;

struct Args {
    voices: Vec<String>,
    speeds: Vec<f32>,
    lang: String,
    text: String,
    out_dir: PathBuf,
}

fn parse_args() -> Result<Args> {
    let mut voices = DEFAULT_VOICES.to_string();
    let mut speeds = DEFAULT_SPEEDS.to_string();
    let mut lang = DEFAULT_LANG.to_string();
    let mut text = DEFAULT_TEXT.to_string();
    let mut out_dir = PathBuf::from("tts_samples");

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = |flag: &str| -> Result<String> {
            it.next()
                .ok_or_else(|| anyhow::anyhow!("{flag} requiere un valor"))
        };
        match arg.as_str() {
            "--voices" => voices = val("--voices")?,
            "--speed" => speeds = val("--speed")?,
            "--lang" => lang = val("--lang")?,
            "--text" => text = val("--text")?,
            "--out" => out_dir = PathBuf::from(val("--out")?),
            "--help" | "-h" => {
                println!(
                    "Uso: cargo run --features kokoro --example tts_audition -- \
                     [--voices ef_dora,em_alex] [--speed 1.0,1.43] [--lang es] \
                     [--text \"...\"] [--out tts_samples]"
                );
                std::process::exit(0);
            }
            other => bail!("Argumento desconocido: {other} (usa --help)"),
        }
    }

    let speeds = speeds
        .split(',')
        .map(|s| {
            s.trim()
                .parse::<f32>()
                .map_err(|e| anyhow::anyhow!("--speed '{s}' inválido: {e}"))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(Args {
        voices: voices.split(',').map(str::trim).map(String::from).collect(),
        speeds,
        lang,
        text,
        out_dir,
    })
}

/// Nombre de fichero seguro para una spec de voz (`em_alex.4+bm_fable.6`).
fn sanitize(voice: &str) -> String {
    voice
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// WAV canónico de 44 bytes: PCM16 mono.
fn write_wav(path: &PathBuf, samples: &[f32]) -> Result<()> {
    use std::io::Write;
    let data_len = (samples.len() * 2) as u32;
    let mut buf = Vec::with_capacity(44 + samples.len() * 2);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_len).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // tamaño del chunk fmt
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&1u16.to_le_bytes()); // mono
    buf.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    buf.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // byte rate
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32_767.0) as i16;
        buf.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::File::create(path)?.write_all(&buf)?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let args = parse_args()?;

    let model = std::env::var("KOKORO_MODEL").unwrap_or_else(|_| "models/kokoro-v1.0.onnx".into());
    let voices_bin =
        std::env::var("KOKORO_VOICES").unwrap_or_else(|_| "models/voices-v1.0.bin".into());
    if !std::path::Path::new(&model).exists() || !std::path::Path::new(&voices_bin).exists() {
        bail!(
            "Faltan {model} o {voices_bin} (descarga: https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX)"
        );
    }
    std::fs::create_dir_all(&args.out_dir)?;

    let tts = KokoroTts::new(&model, &voices_bin, "ef_dora", &args.lang, 1.0, None).await?;

    println!(
        "Texto: {}\nLang: {} | Modelos: {model} + {voices_bin}\nSalida: {}\n",
        args.text,
        args.lang,
        args.out_dir.display()
    );

    for voice in &args.voices {
        for speed in &args.speeds {
            let t0 = std::time::Instant::now();
            match tts.synthesize_with(voice, &args.lang, *speed, &args.text) {
                Ok(pcm) => {
                    let secs = pcm.len() as f32 / SAMPLE_RATE as f32;
                    let name = format!("{}__s{}.wav", sanitize(voice), format_args!("{speed:.2}"));
                    let path = args.out_dir.join(&name);
                    write_wav(&path, &pcm)?;
                    println!(
                        "  ✓ {voice} @ {speed:.2} -> {} ({secs:.1}s audio, {}ms render)",
                        path.display(),
                        t0.elapsed().as_millis()
                    );
                }
                Err(e) => println!("  ✗ {voice} @ {speed:.2}: {e:#}"),
            }
        }
    }
    Ok(())
}
