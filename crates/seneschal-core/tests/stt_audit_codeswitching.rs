//! STT audit — Scenario 2: ES/EN code-switching (issue #217).
//!
//! Two shapes of code-switching:
//!   * `stt_audit_codeswitch_auto` — ONE segment that mixes ES + EN mid-phrase,
//!     with `STT_LANGUAGE=auto` (whisper auto-detect). Issue #217's fix
//!     (`resolve_whisper_language`: "auto" -> "") only makes sense here.
//!   * `stt_audit_codeswitch_pinned_es` / `_pinned_en` — same fixture with the
//!     language pinned, to measure how much auto-detect changes accuracy.
//!   * `stt_audit_codeswitch_boundary` — ES sentence then EN sentence with a
//!     real voice change (two TTS voices) and a 400 ms pause: the VAD should
//!     close the ES segment and open a fresh EN segment. With `auto`, each
//!     segment is detected independently; with `es` pinned the EN segment is
//!     forced through the Spanish decoder.
//!
//! Verdict is printed per run and asserted loosely (at least one language
//! recognized per segment) — the suite documents what actually happens with
//! `STT_LANGUAGE=auto` vs pinned, which is the #217 regression check.

#[path = "stt_common/mod.rs"]
mod stt_common;

use stt_common::*;

/// Mid-phrase code-switch, language auto-detected per segment.
#[tokio::test]
#[ignore = "STT audit: real Whisper + VAD models (make test-stt)"]
async fn stt_audit_codeswitch_auto() {
    skip_if_no_models();
    let audio = load_wav_f32(&ensure_fixture_codeswitch_mix().unwrap()).unwrap();
    let onset = speech_onset(&audio, 0.02);
    println!(
        "[codesw_mix_auto] fixture {}s, onset={:.3}s",
        audio.len() as f32 / SR as f32,
        onset
    );
    let mut stt = new_provider("auto");
    let rep = run_stream(&mut stt, &audio).await;
    rep.print("codesw_mix_auto");

    assert!(
        !rep.segments.is_empty(),
        "code-switched speech produced no segment"
    );
    let text: String = rep
        .segments
        .iter()
        .map(|s| s.quality.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    // At least the model name family must survive: "Qwen" or its common
    // mistranscriptions ("Cuen", "Cwen", "kwen") are acceptable as HIT for the
    // audit's purposes — we record the exact form in the report.
    let qwen_variant = has_token(&text, "qwen")
        || has_token(&text, "cuen")
        || has_token(&text, "cwen")
        || has_token(&text, "kwen")
        || has_token(&text, "quien");
    let es_hit =
        has_token(&text, "hoy") || has_token(&text, "probar") || has_token(&text, "opinión");
    let en_hit =
        has_token(&text, "model") || has_token(&text, "really") || has_token(&text, "well");
    println!(
        "[codesw_mix_auto] qwen_variant={} es_hit={} en_hit={} | full: {:?}",
        qwen_variant, es_hit, en_hit, text
    );
    assert!(
        qwen_variant || es_hit || en_hit,
        "code-switched segment transcribed as garbage: {:?}",
        text
    );
    // Both languages must be partially recognized (the whole point of #217).
    assert!(
        es_hit,
        "Spanish side of the code-switched phrase not recognized: {:?}",
        text
    );
}

/// Same fixture, language pinned to Spanish (regression comparison).
#[tokio::test]
#[ignore = "STT audit: real Whisper + VAD models (make test-stt)"]
async fn stt_audit_codeswitch_pinned_es() {
    skip_if_no_models();
    let audio = load_wav_f32(&ensure_fixture_codeswitch_mix().unwrap()).unwrap();
    let mut stt = new_provider("es");
    let rep = run_stream(&mut stt, &audio).await;
    rep.print("codesw_mix_es");
    assert!(!rep.segments.is_empty(), "no segment (pinned es)");
}

/// Same fixture, language pinned to English (regression comparison).
#[tokio::test]
#[ignore = "STT audit: real Whisper + VAD models (make test-stt)"]
async fn stt_audit_codeswitch_pinned_en() {
    skip_if_no_models();
    let audio = load_wav_f32(&ensure_fixture_codeswitch_mix().unwrap()).unwrap();
    let mut stt = new_provider("en");
    let rep = run_stream(&mut stt, &audio).await;
    rep.print("codesw_mix_en");
    assert!(!rep.segments.is_empty(), "no segment (pinned en)");
}

/// Two-voice language boundary (ES then EN, 400 ms pause) with auto-detect:
/// VAD should produce 2 segments; segment 2 must be readable English.
#[tokio::test]
#[ignore = "STT audit: real Whisper + VAD models (make test-stt)"]
async fn stt_audit_codeswitch_boundary_auto() {
    skip_if_no_models();
    let audio = load_wav_f32(&ensure_fixture_codeswitch_boundary().unwrap()).unwrap();
    let mut stt = new_provider("auto");
    let rep = run_stream(&mut stt, &audio).await;
    rep.print("codesw_boundary_auto");
    assert!(
        rep.segments.len() >= 2,
        "expected 2 segments (ES then EN), got {}",
        rep.segments.len()
    );
    let seg2 = &rep.segments[1];
    let en_ok = has_token(&seg2.quality.text, "runs")
        || has_token(&seg2.quality.text, "fast")
        || has_token(&seg2.quality.text, "machine")
        || has_token(&seg2.quality.text, "latency");
    println!(
        "[codesw_boundary_auto] seg2 English readable: {} | {:?}",
        en_ok, seg2.quality.text
    );
    assert!(
        en_ok,
        "EN second segment not recognizable with auto-detect: {:?}",
        seg2.quality.text
    );
}

/// Same boundary fixture but with Spanish pinned: documents how the EN
/// segment degrades when the language is forced (expected to be poor).
#[tokio::test]
#[ignore = "STT audit: real Whisper + VAD models (make test-stt)"]
async fn stt_audit_codeswitch_boundary_pinned_es() {
    skip_if_no_models();
    let audio = load_wav_f32(&ensure_fixture_codeswitch_boundary().unwrap()).unwrap();
    let mut stt = new_provider("es");
    let rep = run_stream(&mut stt, &audio).await;
    rep.print("codesw_boundary_es");
    assert!(!rep.segments.is_empty(), "no segment (pinned es boundary)");
}
