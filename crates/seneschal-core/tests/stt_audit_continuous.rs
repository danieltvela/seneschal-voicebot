//! STT audit — Scenario 1: continuous speech (no pauses) + latency baseline.
//!
//! Feeds >25 s of continuous synthetic speech through the real
//! `WhisperSttProvider::process_audio` pipeline (Silero VAD probes +
//! batched Whisper inference on SpeechEnd) and measures:
//!   * when SpeechStart fires (VAD commit latency vs audio onset),
//!   * that no segment closes while the user is still speaking (contract P5:
//!     the VAD silence timeout closes an utterance, never a fixed-length cut),
//!   * when SpeechEnd + transcript arrives after speech offset (VAD silence
//!     tail + inference time),
//!   * RTF = inference wall time / segment audio duration.
//!
//! Run: `make test-stt` (or `cargo test -- --ignored stt --test-threads=1`).

#[path = "stt_common/mod.rs"]
mod stt_common;

use stt_common::*;

/// Continuous Spanish speech (~27 s, max internal pause 240 ms) longer than
/// any fixed segment limit must stay ONE segment: the VAD silence timeout
/// closes it only after the user stops speaking, and the transcript covers the
/// whole utterance. This test FAILs if a segment closes while speech is still
/// being fed, or if the transcript is incomplete.
#[tokio::test]
#[ignore = "STT audit: real Whisper + VAD models (make test-stt)"]
async fn stt_audit_continuous_es() {
    skip_if_no_models();
    let audio = load_wav_f32(&ensure_fixture_cont_es().unwrap()).unwrap();
    let onset = speech_onset(&audio, 0.02);
    let offset = speech_offset(&audio, 0.02);
    println!(
        "[cont_es] fixture: {}s, onset={:.3}s, offset={:.3}s",
        audio.len() as f32 / SR as f32,
        onset,
        offset
    );

    let mut stt = new_provider("es");
    let rep = run_stream(&mut stt, &audio).await;
    rep.print("cont_es");

    // One utterance: continuous speech must not be cut mid-stream, however
    // long it runs (this is where the old 20 s hard cap used to fire).
    assert_eq!(
        rep.segments.len(),
        1,
        "continuous speech must stay one segment until the VAD sees silence, got {}",
        rep.segments.len()
    );
    let seg = &rep.segments[0];
    assert!(
        seg.t_close_stream >= offset,
        "segment closed at {:.2}s, before the speech ended at {:.2}s — a \
         segment must never be finalized while the user is still speaking",
        seg.t_close_stream,
        offset
    );
    // Every spoken word must be transcribed, first and last included.
    assert!(
        !seg.quality.text.trim().is_empty(),
        "segment produced an empty transcript"
    );
    assert!(
        has_token(&seg.quality.text, "auditando") && has_token(&seg.quality.text, "concretas"),
        "transcript must cover the whole utterance, got {:?}",
        seg.quality.text
    );
    // VAD commit latency: SpeechStart must fire within 600 ms of audio onset
    // (100 ms probes + 2-probe confirm ≈ 200-300 ms; Silero's own reaction
    // lag on synthetic audio adds a little).
    let start_lat = seg.t_start_stream - onset;
    assert!(
        start_lat < 0.6,
        "SpeechStart fired {:.2}s after onset (budget 600 ms)",
        start_lat
    );
    println!(
        "[cont_es] VAD commit latency = {:.3}s (start@{:.3}s vs onset@{:.3}s)",
        start_lat, seg.t_start_stream, onset
    );
    println!(
        "[cont_es] final segment: close@{:.3}s, speech offset@{:.3}s, infer={}ms",
        seg.t_close_stream, offset, seg.infer_wall_ms
    );
}

/// Same scenario in English (continuous ~26 s).
#[tokio::test]
#[ignore = "STT audit: real Whisper + VAD models (make test-stt)"]
async fn stt_audit_continuous_en() {
    skip_if_no_models();
    let audio = load_wav_f32(&ensure_fixture_cont_en().unwrap()).unwrap();
    let onset = speech_onset(&audio, 0.02);
    let offset = speech_offset(&audio, 0.02);
    println!(
        "[cont_en] fixture: {}s, onset={:.3}s, offset={:.3}s",
        audio.len() as f32 / SR as f32,
        onset,
        offset
    );
    let mut stt = new_provider("en");
    let rep = run_stream(&mut stt, &audio).await;
    rep.print("cont_en");

    assert_eq!(
        rep.segments.len(),
        1,
        "continuous English speech must stay one segment until the VAD sees silence, got {}",
        rep.segments.len()
    );
    let seg = &rep.segments[0];
    assert!(
        seg.t_close_stream >= offset,
        "segment closed at {:.2}s, before the speech ended at {:.2}s",
        seg.t_close_stream,
        offset
    );
    assert!(
        !seg.quality.text.trim().is_empty(),
        "segment produced an empty transcript"
    );
    assert!(
        has_token(&seg.quality.text, "auditing") && has_token(&seg.quality.text, "pipeline"),
        "transcript must cover the whole utterance, got {:?}",
        seg.quality.text
    );
    let start_lat = seg.t_start_stream - onset;
    assert!(
        start_lat < 1.5,
        "SpeechStart fired {:.2}s after onset",
        start_lat
    );
    println!("[cont_en] VAD commit latency = {:.3}s", start_lat);
}

/// Latency/RTF baseline on short utterances, run twice per language to
/// separate first-call (cold) cost from steady-state inference.
#[tokio::test]
#[ignore = "STT audit: real Whisper + VAD models (make test-stt)"]
async fn stt_audit_latency_baseline() {
    skip_if_no_models();
    let es = load_wav_f32(&ensure_fixture_short_es().unwrap()).unwrap();
    let en = load_wav_f32(&ensure_fixture_short_en().unwrap()).unwrap();

    let stt_es = new_provider("es");
    let stt_en = new_provider("en");

    // Warm-up (Metal context init, model pages) — expect the biggest cost.
    let (q_warm, ms_warm) = run_transcribe_complete(stt_es.as_ref(), &es);
    println!(
        "[baseline] es warm-up: {}ms RTF={:.3} text={:?}",
        ms_warm,
        ms_warm as f32 * 1e-3 / (es.len() as f32 / SR as f32),
        q_warm.text
    );
    // Steady state: 2nd and 3rd calls.
    let (q2, ms2) = run_transcribe_complete(stt_es.as_ref(), &es);
    let (_q3, ms3) = run_transcribe_complete(stt_es.as_ref(), &es);
    let dur = es.len() as f32 / SR as f32;
    println!(
        "[baseline] es steady: #2 {}ms RTF={:.3} | #3 {}ms RTF={:.3}",
        ms2,
        ms2 as f32 * 1e-3 / dur,
        ms3,
        ms3 as f32 * 1e-3 / dur
    );
    let (_qe_warm, msw) = run_transcribe_complete(stt_en.as_ref(), &en);
    let (qe2, msw2) = run_transcribe_complete(stt_en.as_ref(), &en);
    let dure = en.len() as f32 / SR as f32;
    println!(
        "[baseline] en warm-up: {}ms | steady: {}ms RTF={:.3}",
        msw,
        msw2,
        msw2 as f32 * 1e-3 / dure
    );

    assert!(
        !q2.text.trim().is_empty(),
        "es short utterance not transcribed"
    );
    assert!(
        has_token(&q2.text, "hola") || has_token(&q2.text, "sistema"),
        "es short transcript not recognizable: {:?}",
        q2.text
    );
    assert!(
        has_token(&qe2.text, "hello") || has_token(&qe2.text, "audio"),
        "en short transcript not recognizable: {:?}",
        qe2.text
    );
    // Steady-state RTF budget: < 0.5 on GPU-accelerated builds (the M4 Pro /
    // CUDA reference machines); CPU-only builds run the same model much
    // slower, so they get a loose sanity ceiling instead.
    let rtf = ms3 as f32 * 1e-3 / dur;
    let gpu_accelerated = cfg!(target_os = "macos") || cfg!(feature = "cuda");
    let rtf_budget = if gpu_accelerated { 0.5 } else { 2.5 };
    println!("[baseline] steady-state RTF = {rtf:.3} (budget {rtf_budget}, gpu={gpu_accelerated})");
    assert!(
        rtf < rtf_budget,
        "steady-state RTF too high: {:.3} (budget {rtf_budget})",
        rtf
    );
    // Warm-up should be slower than steady state (informational check, loose).
    println!(
        "[baseline] warm/steady ratio es = {:.2}",
        ms_warm as f32 / ms3 as f32
    );
}

/// Non-speech input (1.5 s white noise + 1.5 s silence) must NOT produce a
/// usable transcription end-to-end: either the VAD never commits a segment,
/// or the transcript must be empty/gate-rejectable.
#[tokio::test]
#[ignore = "STT audit: real Whisper + VAD models (make test-stt)"]
async fn stt_audit_noise_rejected() {
    skip_if_no_models();
    // 1.5 s white noise (deterministic LCG) + 1.5 s silence.
    let mut rng: u64 = 0x1234_5678_9abc_def0;
    let mut noise: Vec<f32> = (0..SR * 15 / 10)
        .map(|_| {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (((rng >> 40) as f32) / (1u64 << 24) as f32 - 0.5) * 0.5
        })
        .collect();
    noise.extend(std::iter::repeat_n(0.0f32, SR * 15 / 10));

    let mut stt = new_provider("es");
    let rep = run_stream(&mut stt, &noise).await;
    rep.print("noise");
    // If a segment was committed, its transcript must be rejectable by the
    // default NoSpeechGate or empty.
    for (i, seg) in rep.segments.iter().enumerate() {
        let gate = seneschal_core::stt::NoSpeechGate::default();
        assert!(
            gate.should_reject(&seg.quality) || seg.quality.text.trim().is_empty(),
            "noise produced an un-rejectable transcript seg{}: {:?} (ns={:.2} lp={:.2} cr={:.2})",
            i,
            seg.quality.text,
            seg.quality.no_speech_prob,
            seg.quality.avg_logprob,
            seg.quality.compression_ratio
        );
    }
}
