//! STT audit — Scenario 1: continuous speech (no pauses) + latency baseline.
//!
//! Feeds >20 s of continuous synthetic speech through the real
//! `WhisperSttProvider::process_audio` pipeline (Silero VAD probes +
//! batched Whisper inference on SpeechEnd) and measures:
//!   * when SpeechStart fires (VAD commit latency vs audio onset),
//!   * whether the 20 s hard segment cap (`MAX_SEGMENT_MS`) cuts the stream,
//!   * when SpeechEnd + transcript arrives after speech offset (VAD silence
//!     tail + inference time),
//!   * RTF = inference wall time / segment audio duration.
//!
//! Run: `make test-stt` (or `cargo test -- --ignored stt --test-threads=1`).

#[path = "stt_common/mod.rs"]
mod stt_common;

use stt_common::*;

/// Continuous Spanish speech (~27 s, max internal pause 240 ms) must be cut
/// by the 20 s hard cap into >= 2 segments, and every segment must yield a
/// non-empty transcript. This test FAILs if the VAD fails to close segments
/// or if inference produces no text.
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

    // Expect at least 2 segments: continuous speech must hit the 20 s cap.
    assert!(
        rep.segments.len() >= 2,
        "expected >=2 segments (20s hard cap on continuous speech), got {}",
        rep.segments.len()
    );
    // First segment must close near the 20 s cap (start ~0 + cap).
    let first_close = rep.segments[0].t_close_stream;
    assert!(
        (19.0..=21.5).contains(&first_close),
        "first segment should close near the 20s cap, closed at {:.2}s",
        first_close
    );
    // Every segment must produce a non-empty transcript.
    for (i, seg) in rep.segments.iter().enumerate() {
        assert!(
            !seg.quality.text.trim().is_empty(),
            "segment {} produced an empty transcript",
            i
        );
    }
    // No segment may be shorter than 5 s here: the cap is 20 s and the
    // stream is continuous, so only the tail fragment can be short.
    let short_head = rep
        .segments
        .iter()
        .take(rep.segments.len() - 1)
        .filter(|s| s.t_close_stream - s.t_start_stream < 5.0)
        .count();
    assert_eq!(
        short_head, 0,
        "VAD unexpectedly cut continuous speech into tiny segments"
    );
    // VAD commit latency: SpeechStart must fire within 1.5 s of audio onset.
    let start_lat = rep.segments[0].t_start_stream - onset;
    assert!(
        start_lat < 1.5,
        "SpeechStart fired {:.2}s after onset (too late)",
        start_lat
    );
    println!(
        "[cont_es] VAD commit latency = {:.3}s (start@{:.3}s vs onset@{:.3}s)",
        start_lat, rep.segments[0].t_start_stream, onset
    );
    // SpeechEnd latency after real speech offset: silence tail (<=~0.6s)
    // + inference of the final segment.
    let last = rep.segments.last().unwrap();
    let end_after_offset = last.t_close_stream + 4.0 - offset; // flush silence is part of the stream
    println!(
        "[cont_es] final segment: close@{:.3}s, speech offset@{:.3}s, infer={}ms",
        last.t_close_stream, offset, last.infer_wall_ms
    );
    assert!(
        end_after_offset < 15.0,
        "SpeechEnd + transcript arrived unrealistically late ({:.1}s)",
        end_after_offset
    );
}

/// Same scenario in English (continuous ~26 s).
#[tokio::test]
#[ignore = "STT audit: real Whisper + VAD models (make test-stt)"]
async fn stt_audit_continuous_en() {
    skip_if_no_models();
    let audio = load_wav_f32(&ensure_fixture_cont_en().unwrap()).unwrap();
    let onset = speech_onset(&audio, 0.02);
    println!(
        "[cont_en] fixture: {}s, onset={:.3}s",
        audio.len() as f32 / SR as f32,
        onset
    );
    let mut stt = new_provider("en");
    let rep = run_stream(&mut stt, &audio).await;
    rep.print("cont_en");

    assert!(
        rep.segments.len() >= 2,
        "expected >=2 segments from continuous English speech, got {}",
        rep.segments.len()
    );
    for (i, seg) in rep.segments.iter().enumerate() {
        assert!(
            !seg.quality.text.trim().is_empty(),
            "segment {} empty transcript",
            i
        );
    }
    let start_lat = rep.segments[0].t_start_stream - onset;
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
    let (q_warm, ms_warm) = run_transcribe_complete(&stt_es, &es);
    println!(
        "[baseline] es warm-up: {}ms RTF={:.3} text={:?}",
        ms_warm,
        ms_warm as f32 * 1e-3 / (es.len() as f32 / SR as f32),
        q_warm.text
    );
    // Steady state: 2nd and 3rd calls.
    let (q2, ms2) = run_transcribe_complete(&stt_es, &es);
    let (_q3, ms3) = run_transcribe_complete(&stt_es, &es);
    let dur = es.len() as f32 / SR as f32;
    println!(
        "[baseline] es steady: #2 {}ms RTF={:.3} | #3 {}ms RTF={:.3}",
        ms2,
        ms2 as f32 * 1e-3 / dur,
        ms3,
        ms3 as f32 * 1e-3 / dur
    );
    let (_qe_warm, msw) = run_transcribe_complete(&stt_en, &en);
    let (qe2, msw2) = run_transcribe_complete(&stt_en, &en);
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
    // Steady-state RTF must be < 0.5 (inference faster than realtime) on the
    // M4 Pro; otherwise interactive voice is unusable.
    assert!(
        ms3 as f32 * 1e-3 / dur < 0.5,
        "steady-state RTF too high: {:.3}",
        ms3 as f32 * 1e-3 / dur
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
    noise.extend(std::iter::repeat(0.0f32).take(SR * 15 / 10));

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
