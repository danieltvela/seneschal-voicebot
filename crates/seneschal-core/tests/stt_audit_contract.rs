//! STT audit — contract scenarios from `doc/stt-contract.md`.
//!
//! These tests lock the recurring regressions the project suffered:
//!   1. lost utterance onset  → P1 (commit budget) + P4 (first word present)
//!   2. lost utterance tail   → P5 (no premature split) + P4 (last word present)
//!   3. missed end of speech  → P5 (segment closes at silence threshold)
//!   4. slow start detection  → P1 (commit budget)
//!   5. barge-in at the acoustic onset → P2 (SpeechStart fires on the first
//!      speech probe; STT/VAD are never cancelled; the LLM request, splitter
//!      and TTS queue + playback stop immediately)
//!
//! Run: `make test-stt` (or `cargo test -- --ignored stt --test-threads=1`).
//! Provider selection via STT_AUDIT_PROVIDER (whisper | openai), issue #17.

#[path = "stt_common/mod.rs"]
mod stt_common;

use stt_common::*;

/// DIAGNOSTIC: Silero probability per 100 ms / 200 ms probe over the live
/// fixture onset. Not matched by the `stt` test filter; run explicitly with
/// `cargo test -p seneschal-core --test stt_audit_contract vad_probe_calibration
/// -- --ignored --nocapture` when touching VAD thresholds or probe sizes.
#[tokio::test]
#[ignore = "diagnostic: real VAD model (make test-stt)"]
async fn vad_probe_calibration() {
    skip_if_no_models();
    use seneschal_core::stt::segmenter::SileroDetector;
    use seneschal_core::stt::whisper::load_vad;

    let audio = load_wav_f32(&ensure_fixture_live_es().unwrap()).unwrap();
    let vad = load_vad(&model_paths().1, None).unwrap();
    use seneschal_core::stt::SpeechDetector;

    let mut det = SileroDetector::new(vad);
    println!("[calib] -- 100 ms probes --");
    for i in 0..60 {
        let chunk = &audio[i * 1600..(i + 1) * 1600];
        let p = det.probe(chunk);
        println!("[calib] probe {i} ({:.2}s): prob={:.3}", i as f32 * 0.1, p);
    }
    // fresh VAD context for a clean 200 ms run
    let vad2 = load_vad(&model_paths().1, None).unwrap();
    let mut det2 = SileroDetector::new(vad2);
    println!("[calib] -- 200 ms probes --");
    for i in 0..30 {
        let chunk = &audio[i * 3200..(i + 1) * 3200];
        let p = det2.probe(chunk);
        println!(
            "[calib] probe200 {i} ({:.2}s): prob={:.3}",
            i as f32 * 0.2,
            p
        );
    }

    // Window resolution: the segmenter charges silence at this granularity, so
    // the printout shows how much of each probe is really below 0.45.
    let mut vad3 = load_vad(&model_paths().1, None).unwrap();
    println!("[calib] -- window probs (200 ms probes) --");
    for i in 0..25 {
        let chunk = &audio[i * 3200..(i + 1) * 3200];
        let _ = vad3.detect_speech(chunk);
        let probs = vad3.get_probs();
        let rounded: Vec<f32> = probs.iter().map(|p| (p * 100.0).round() / 100.0).collect();
        let silent = probs.iter().filter(|&&p| p < 0.45).count();
        println!(
            "[calib] windows {i} ({:.2}s): n={} silent={silent} {:?}",
            i as f32 * 0.2,
            probs.len(),
            rounded
        );
    }
}

/// Level robustness: a capture 30–40 dB below full scale (peak 0.01–0.03, what
/// the reference USB speakerphone delivers, and what the ALSA mixer cannot
/// raise — its capture control is a switch, not a gain) must still be detected
/// as speech. Without the VAD input normalization Silero reports 0.01–0.4 for
/// clear speech, so the silence timeout fired mid-utterance and the LLM
/// answered while the user was still talking.
#[tokio::test]
#[ignore = "STT audit: real Silero VAD model (make test-stt)"]
async fn stt_audit_quiet_capture_es() {
    skip_if_no_models();
    use seneschal_core::stt::segmenter::{
        SegmenterConfig, SegmenterEvent, SileroDetector, VadSegmenter,
    };

    let audio = load_wav_f32(&ensure_fixture_cont_es().unwrap()).unwrap();
    let offset = speech_offset(&audio, 0.02);
    let src_peak = audio.iter().fold(0.0f32, |m, s| m.max(s.abs()));

    for target_peak in [0.03f32, 0.01, 0.003] {
        let gain = target_peak / src_peak;
        let quiet: Vec<f32> = audio.iter().map(|s| s * gain).collect();
        let cfg = SegmenterConfig {
            silence_ms: 1000,
            vad_start_threshold: 0.65,
            vad_end_threshold: 0.45,
            vad_confirm_probes: 2,
            post_roll_ms: 200,
            trim_leading_silence: false,
            probe_ms: 200,
            preroll_ms: 300,
        };
        let mut seg = VadSegmenter::new(
            SileroDetector::new(
                seneschal_core::stt::whisper::load_vad(&model_paths().1, None).unwrap(),
            ),
            cfg,
        )
        .expect("segmenter");

        let mut fed = 0usize;
        let mut starts = 0usize;
        let mut closes: Vec<(f32, f32)> = Vec::new();
        let drain = |seg: &mut VadSegmenter<SileroDetector>,
                     fed: &mut usize,
                     starts: &mut usize,
                     closes: &mut Vec<(f32, f32)>,
                     chunk: &[f32]| {
            *fed += chunk.len();
            for ev in seg.feed(chunk) {
                match ev {
                    SegmenterEvent::SpeechStart { .. } => *starts += 1,
                    SegmenterEvent::SpeechAborted { .. } => {}
                    SegmenterEvent::SegmentReady { audio, .. } => {
                        closes.push((*fed as f32 / SR as f32, audio.len() as f32 / SR as f32))
                    }
                }
            }
        };
        for chunk in quiet.chunks(1600) {
            drain(&mut seg, &mut fed, &mut starts, &mut closes, chunk);
        }
        let silence = vec![0.0f32; SR * 4];
        for chunk in silence.chunks(1600) {
            drain(&mut seg, &mut fed, &mut starts, &mut closes, chunk);
        }

        println!(
            "[quiet peak={target_peak}] starts={starts} closes={closes:?} (speech offset {offset:.2}s)"
        );
        assert_eq!(starts, 1, "peak {target_peak}: exactly one utterance");
        assert_eq!(
            closes.len(),
            1,
            "peak {target_peak}: quiet speech must not be cut mid-utterance, got {closes:?}"
        );
        let (close_t, len) = closes[0];
        assert!(
            close_t >= offset,
            "peak {target_peak}: segment closed at {close_t:.2}s, before the speech ended ({offset:.2}s)"
        );
        assert!(
            len >= offset * 0.9,
            "peak {target_peak}: the segment must keep the utterance ({len:.2}s of {offset:.2}s)"
        );
    }
}

/// P5 (accounting): a pause shorter than the configured silence window must not
/// close the segment. Silence is charged at the detector's window resolution,
/// so pauses of 600 ms and 800 ms stay inside one utterance with
/// `silence_ms = 1000` (probe-granular counting charged a whole 200 ms probe per
/// boundary probe and closed the turn).
#[tokio::test]
#[ignore = "STT audit: real Whisper + VAD models (make test-stt)"]
async fn stt_audit_pause_tolerance_es() {
    skip_if_no_models();
    let audio = load_wav_f32(&ensure_fixture_pauses_es().unwrap()).unwrap();
    let mut stt = new_provider_with_silence_ms("es", 1000);
    let rep = run_stream(&mut stt, &audio).await;
    rep.print("pauses_es");

    assert_eq!(
        rep.segments.len(),
        1,
        "600/800 ms pauses must stay inside a 1000 ms silence window, got {} segments",
        rep.segments.len()
    );
    let text = &rep.segments[0].quality.text;
    check_tokens("pauses_es", text, &["tolerancia", "segunda", "tercera"]);
    assert!(
        has_token(text, "tolerancia") && has_token(text, "tercera"),
        "the transcript must cover all three phrases, got {text:?}"
    );
}

/// P1 + P4: a soft (ramped) onset must still (a) commit within the budget and
/// (b) transcribe the first words — the real-audio pre-roll keeps the onset
/// the VAD threshold crossed late.
#[tokio::test]
#[ignore = "STT audit: real Whisper + VAD models (make test-stt)"]
async fn stt_audit_onset_es() {
    skip_if_no_models();
    let audio = load_wav_f32(&ensure_fixture_onset_es().unwrap()).unwrap();
    let onset = speech_onset(&audio, 0.02);
    println!(
        "[onset_es] fixture: {:.2}s, onset={:.3}s",
        audio.len() as f32 / SR as f32,
        onset
    );

    let mut stt = new_provider("es");
    let rep = run_stream(&mut stt, &audio).await;
    rep.print("onset_es");

    assert_eq!(
        rep.segments.len(),
        1,
        "a single short utterance must stay one segment, got {:#?}",
        rep.segments
            .iter()
            .map(|s| (&s.t_start_stream, &s.t_close_stream))
            .collect::<Vec<_>>()
    );
    let seg = &rep.segments[0];
    let commit = seg.t_start_stream - onset;
    println!(
        "[onset_es] P1 commit latency = {:.3}s (start@{:.3} vs onset@{:.3})",
        commit, seg.t_start_stream, onset
    );
    // The soft ramp is the pathological case for Silero on synthetic audio:
    // the model's own probability lags the acoustic onset by seconds. What
    // the contract guarantees (and what the old system did NOT do) is that
    // the pre-commit onset audio is preserved via pre-roll + accumulation —
    // asserted through the transcript below. The commit ceiling is an
    // absurdity guard only.
    assert!(
        commit < 4.0,
        "P1: SpeechStart fired {commit:.3}s after the acoustic onset (absurdity ceiling 4 s)"
    );
    let hits = check_tokens(
        "onset_es",
        &seg.quality.text,
        &["buenas", "tardes", "frase", "palabra"],
    );
    assert!(
        hits >= 3
            && has_token(&seg.quality.text, "buenas")
            && has_token(&seg.quality.text, "tardes"),
        "P4: soft-onset transcript lost the first words ({hits}/4): {:?}",
        seg.quality.text
    );
}

/// P5 + P4: a fading tail must not split the utterance, and the last words
/// must appear in the transcript.
#[tokio::test]
#[ignore = "STT audit: real Whisper + VAD models (make test-stt)"]
async fn stt_audit_tail_es() {
    skip_if_no_models();
    let audio = load_wav_f32(&ensure_fixture_tail_es().unwrap()).unwrap();
    let offset = speech_offset(&audio, 0.02);
    println!(
        "[tail_es] fixture: {:.2}s, speech offset={:.3}s",
        audio.len() as f32 / SR as f32,
        offset
    );

    let mut stt = new_provider("es");
    let rep = run_stream(&mut stt, &audio).await;
    rep.print("tail_es");

    assert_eq!(
        rep.segments.len(),
        1,
        "P5: the fading tail must not split the utterance into {} segments",
        rep.segments.len()
    );
    let seg = &rep.segments[0];
    // The segment must close after the last real speech, never before it.
    assert!(
        seg.t_close_stream >= offset,
        "P5: segment closed at {:.3}s before the speech offset {:.3}s",
        seg.t_close_stream,
        offset
    );
    let hits = check_tokens(
        "tail_es",
        &seg.quality.text,
        &["frase", "desvaneci", "palabra", "final"],
    );
    assert!(
        hits >= 3,
        "P4: fading-tail transcript lost words ({hits}/4): {:?}",
        seg.quality.text
    );
}

/// P3: while the user speaks, at least one live partial must arrive before
/// SpeechEnd (live re-decode, never full-buffer-at-close). Also prints the
/// budget table (commit / close / inference RTF).
#[tokio::test]
#[ignore = "STT audit: real Whisper + VAD models (make test-stt)"]
async fn stt_audit_live_partial_es() {
    skip_if_no_models();
    let audio = load_wav_f32(&ensure_fixture_live_es().unwrap()).unwrap();
    let onset = speech_onset(&audio, 0.02);
    let offset = speech_offset(&audio, 0.02);
    println!(
        "[live_es] fixture: {:.2}s, onset={:.3}s, offset={:.3}s",
        audio.len() as f32 / SR as f32,
        onset,
        offset
    );

    let mut stt = new_provider("es");
    let rep = run_stream(&mut stt, &audio).await;
    rep.print("live_es");

    assert!(!rep.segments.is_empty(), "the utterance must close");
    let last_close = rep.segments.last().unwrap().t_close_stream;
    let partials_before_close = rep
        .partials
        .iter()
        .filter(|(t, p)| !p.trim().is_empty() && *t <= last_close)
        .count();
    println!(
        "[live_es] P3 partials before close: {partials_before_close} (last close @{last_close:.3}s)"
    );
    assert!(
        partials_before_close >= 1,
        "P3: live transcription must emit at least one partial before SpeechEnd"
    );
    // P1 on a normal (non-ramped) utterance: measured commit on synthetic
    // audio is 0.6-0.8 s (Silero's own probability lags on espeak; real
    // voices score higher). The audit budget is 1.0 s with margin.
    let commit = rep.segments[0].t_start_stream - onset;
    println!("[live_es] P1 commit latency = {commit:.3}s");
    assert!(
        commit < 1.0,
        "P1: SpeechStart fired {commit:.3}s after the acoustic onset (budget 1.0 s)"
    );
}
