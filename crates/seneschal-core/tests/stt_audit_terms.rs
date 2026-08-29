//! STT audit — Scenario 3: technical terms & LLM model names.
//!
//! Runs `TERMS_ES` / `TERMS_EN` fixtures (espeak-ng, deliberately slow rate)
//! through `transcribe_complete` with STT_LANGUAGE = es / en / auto and
//! scores per-token recall against `TERM_TOKENS`.
//!
//! Additionally, `stt_audit_terms_initial_prompt` probes the whisper-cpp-plus
//! API directly with `FullParams::initial_prompt()` (the production
//! `transcribe()` in `stt/whisper.rs` does NOT set a prompt — this test
//! quantifies whether an LLM-domain prompt fixes the Qwen/kwin confusion and
//! whether the feature should be wired in).

#[path = "stt_common/mod.rs"]
mod stt_common;

use stt_common::*;
use whisper_cpp_plus::{FullParams, SamplingStrategy, WhisperContext, WhisperState};

/// Run one language config over both terms fixtures via the provider's own
/// code path. Prints + returns per-token recall.
fn terms_for_language(language: &str) -> (usize, usize, String, String, u128, u128) {
    let stt = new_provider(language);
    let es = load_wav_f32(&ensure_fixture_terms_es().unwrap()).unwrap();
    let en = load_wav_f32(&ensure_fixture_terms_en().unwrap()).unwrap();
    let (q_es, ms_es) = run_transcribe_complete(&stt, &es);
    let (q_en, ms_en) = run_transcribe_complete(&stt, &en);
    let dur_es = es.len() as f32 / SR as f32;
    let dur_en = en.len() as f32 / SR as f32;
    println!(
        "\n[terms_{language}] ES input {}s -> {}ms (RTF={:.3})\n[terms_{language}] ES: {:?}",
        dur_es,
        ms_es,
        ms_es as f32 * 1e-3 / dur_es,
        q_es.text
    );
    println!(
        "[terms_{language}] EN input {}s -> {}ms (RTF={:.3})\n[terms_{language}] EN: {:?}",
        dur_en,
        ms_en,
        ms_en as f32 * 1e-3 / dur_en,
        q_en.text
    );
    let hits_es = check_tokens(&format!("terms_{language}_es"), &q_es.text, TERM_TOKENS);
    let hits_en = check_tokens(&format!("terms_{language}_en"), &q_en.text, TERM_TOKENS);
    (hits_es, hits_en, q_es.text, q_en.text, ms_es, ms_en)
}

/// Terms in Spanish-voiced speech, transcribed with language pinned to es.
#[tokio::test]
#[ignore = "STT audit: real Whisper + VAD models (make test-stt)"]
async fn stt_audit_terms_es() {
    skip_if_no_models();
    let (h_es, _h_en, _, _, _, _) = terms_for_language("es");
    // es-pinned must at least catch the Spanish audio's core model name.
    assert!(
        h_es >= 8,
        "es-pinned recall too low: {h_es}/{} tokens on Spanish terms",
        TERM_TOKENS.len()
    );
}

/// Terms in English-voiced speech, transcribed with language pinned to en.
#[tokio::test]
#[ignore = "STT audit: real Whisper + VAD models (make test-stt)"]
async fn stt_audit_terms_en() {
    skip_if_no_models();
    let (_, h_en, _, _, _, _) = terms_for_language("en");
    assert!(
        h_en >= 8,
        "en-pinned recall too low: {h_en}/{} tokens on English terms",
        TERM_TOKENS.len()
    );
}

/// Terms with auto-detect (issue #217 default): must stay close to the
/// pinned-language recall (auto-detect on a 13 s segment should not wreck it).
#[tokio::test]
#[ignore = "STT audit: real Whisper + VAD models (make test-stt)"]
async fn stt_audit_terms_auto() {
    skip_if_no_models();
    let (h_es, h_en, text_es, text_en, _, _) = terms_for_language("auto");
    assert!(
        h_es >= 6 && h_en >= 6,
        "auto-detect recall too low: es {h_es}/{} en {h_en}/{}",
        TERM_TOKENS.len(),
        TERM_TOKENS.len()
    );
    println!(
        "[terms_auto] ES: {text_es}\n[terms_auto] EN: {text_en}"
    );
}

/// Direct whisper-cpp-plus probe: does `initial_prompt` (LLM-domain context)
/// improve model-name recognition? Production `transcribe()` never sets a
/// prompt; this isolates the single variable.
#[tokio::test]
#[ignore = "STT audit: real Whisper + VAD models (make test-stt)"]
async fn stt_audit_terms_initial_prompt() {
    skip_if_no_models();
    let (wm, _) = model_paths();
    let ctx = WhisperContext::new(&wm).expect("load whisper");
    let audio = load_wav_f32(&ensure_fixture_terms_es().unwrap()).unwrap();

    let probe = |lang: &str, prompt: Option<&str>| -> (String, u128) {
        let mut state = WhisperState::new(&ctx).expect("state");
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 })
            .language(if lang == "auto" { "" } else { lang })
            .print_special(false)
            .print_progress(false)
            .print_realtime(false)
            .print_timestamps(false)
            .no_timestamps(true)
            .single_segment(true);
        if let Some(p) = prompt {
            params = params.initial_prompt(p);
        }
        let t = std::time::Instant::now();
        state.full(params, &audio).expect("full()");
        let n = state.full_n_segments();
        let mut text = String::new();
        for i in 0..n {
            if let Ok(seg) = state.full_get_segment_text(i) {
                text.push_str(seg.trim());
                text.push(' ');
            }
        }
        (text.trim().to_string(), t.elapsed().as_millis())
    };

    let (t0, m0) = probe("es", None);
    let prompt = "Transcripción de voz sobre tecnología: nombres de modelos de IA como \
                   Qwen, Qwen3.8, Qwen3.8-27B, Whisper, Silero, SGLang, vLLM, DFlash2. \
                   GPUs: RTX PRO 6000.";
    let (t1, m1) = probe("es", Some(prompt));
    println!(
        "\n[prompt_probe] ES no-prompt ({}ms): {t0:?}",
        m0
    );
    println!("[prompt_probe] ES with-prompt ({}ms): {t1:?}", m1);
    let h0 = check_tokens("prompt_probe_es_noprompt", &t0, TERM_TOKENS);
    let h1 = check_tokens("prompt_probe_es_prompt", &t1, TERM_TOKENS);
    println!(
        "[prompt_probe] recall without prompt: {h0}/{} | with prompt: {h1}/{}",
        TERM_TOKENS.len(),
        TERM_TOKENS.len()
    );
    // Informational, not asserted: record whether the prompt helps.
    assert!(!t0.is_empty() && !t1.is_empty());
}

/// "Qwen" spelling-probe: repeated short utterances of the exact model name,
/// to classify the dominant mis-hearings (kwin / cuen / cwen / 3.8 → 38 etc).
#[tokio::test]
#[ignore = "STT audit: real Whisper + VAD models (make test-stt)"]
async fn stt_audit_qwen_repeated() {
    skip_if_no_models();
    // Synthesize the name 5 times, 250 ms apart, espeak es voice.
    let dir = fixture_dir();
    let out = dir.join("qwen_repeated.wav");
    let mut texts: Vec<String> = Vec::new();
    let mut parts: Vec<std::path::PathBuf> = Vec::new();
    for i in 0..5 {
        let phrases = [
            "Qwen 3.8 27B",
            "Qwen3.8-27B",
            "el modelo Qwen 3.8 27B",
            "Qwen 3.8 27B",
            "Qwen 3.8 27 B",
        ];
        let p = dir.join(format!("qwr_{i}.wav"));
        if !p.exists() {
            let raw = dir.join(format!("qwr_{i}.raw.wav"));
            std::process::Command::new("espeak-ng")
                .args(["-v", "es", "-s", "150", "-w", raw.to_str().unwrap(), phrases[i]])
                .status()
                .expect("espeak-ng");
            std::process::Command::new("ffmpeg")
                .args([
                    "-y",
                    "-loglevel", "error",
                    "-i", raw.to_str().unwrap(),
                    "-ar", "16000", "-ac", "1", "-sample_fmt", "s16", p.to_str().unwrap(),
                ])
                .status()
                .expect("ffmpeg");
            let _ = std::fs::remove_file(raw);
        }
        parts.push(p.clone());
        texts.push(phrases[i].to_string());
    }
    if !out.exists() {
        concat_for_qwen(&parts, &out).expect("concat");
    }
    let audio = load_wav_f32(&out).unwrap();
    println!("[qwen_repeated] expected: {texts:?} | audio {}s", audio.len() as f32 / SR as f32);

    for lang in ["es", "auto"] {
        let stt = new_provider(lang);
        let (q, ms) = run_transcribe_complete(&stt, &audio);
        println!(
            "[qwen_repeated_{lang}] ({}ms): {:?}",
            ms, q.text
        );
        let h = check_tokens(&format!("qwen_repeated_{lang}"), &q.text, &["qwen", "3.8", "27"]);
        println!("[qwen_repeated_{lang}] qwen/3.8/27 recall: {h}/3");
    }
    // Informational only — exact spelling of "Qwen" is the known failure mode.
}

fn concat_for_qwen(parts: &[std::path::PathBuf], out: &std::path::Path) -> anyhow::Result<()> {
    let data: Vec<Vec<i16>> = parts.iter().map(|p| read_wav_pcm16(p)).collect::<anyhow::Result<_>>()?;
    let mut v: Vec<i16> = Vec::new();
    for (i, d) in data.iter().enumerate() {
        if i > 0 {
            v.extend(std::iter::repeat(0i16).take(SR / 1000 * 250));
        }
        v.extend_from_slice(d);
    }
    // write_wav_pcm16 is private to stt_common; re-emit a minimal WAV here.
    let n = v.len() as u32;
    let mut b = Vec::with_capacity(44 + v.len() * 2);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&((36 + n * 2) as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&16000u32.to_le_bytes());
    b.extend_from_slice(&32000u32.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(n * 2).to_le_bytes());
    for s in &v {
        b.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(out, b)?;
    Ok(())
}
