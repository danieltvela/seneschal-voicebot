# End-of-Speech Detection: Research Baseline

**Status:** research baseline — inputs for future investigations and experiments
**Date:** 2026-10-14
**Related:** [stt-contract.md](stt-contract.md) (P1–P6, the current mechanism), [env-vars.md](env-vars.md) (canonical knobs), `tasks/stt-audit-report-2026-08-25.md` (internal audit, source of the measured numbers)

This document consolidates (1) the current Seneschal end-of-speech mechanism and its measured behavior, (2) what human conversation research says about turn-taking timing, (3) what production voice systems ship, and (4) the state of the art in semantic / audio-native end-of-turn (EOT) detection. It closes with options, open questions, and a test plan. Sources are numbered `[S#]` and listed at the end.

---

## 1. Problem statement

Seneschal serves two conflicting use cases over one VAD configuration:

- **Fast commands and snappy replies** — the user expects a response almost immediately after the last word.
- **Long conversations and extended explanations** — the user pauses mid-monologue to think; the assistant must not treat a thinking pause as the end of the turn.

With a single fixed silence threshold these goals trade off directly: lowering the threshold reduces latency but increases false cuts (the pipeline fires mid-utterance), and raising it does the opposite [S13]. The rest of this document maps the design space beyond that single knob.

## 2. Current mechanism in Seneschal

The canonical mechanism is specified in [stt-contract.md](stt-contract.md) (P1–P6) and implemented once in the shared `VadSegmenter` (`crates/seneschal-core/src/stt/segmenter.rs`), used by the `whisper`, `parakeet` and `openai` providers alike [S14].

Canonical values (compiled-in fallback defaults, `seneschal.pro.toml:14-30`; all overridable by environment):

| Knob | Value | Role |
|---|---|---|
| `VAD_SILENCE_MS` | 300 | Below-threshold audio required to close the segment |
| `VAD_START_THRESHOLD` | 0.65 | Speech probability to open a segment |
| `VAD_END_THRESHOLD` | 0.45 | Probability floor that keeps a segment open (hysteresis) |
| `VAD_CONFIRM_PROBES` | 2 | Speech decisions within a sliding N+1 window before commit |
| `VAD_PROBE_MS` | 200 | Probe window (100 ms makes Silero probabilities oscillate) |
| `VAD_PREROLL_MS` | 300 | Real-audio pre-roll prepended to confirmed segments |
| `VAD_POST_ROLL_MS` | 200 | Grace after the timeout in which resumed speech cancels the closure |

Mechanics that matter when tuning [S24]:

- **Hysteresis:** silence only accumulates on audio below `VAD_END_THRESHOLD`; a probe with speech resets the count. A pause shorter than `VAD_SILENCE_MS` can therefore never close the turn, and a probe whose windows straddle the threshold cannot add up to a timeout.
- **Post-roll is a one-shot grace:** after the silence timeout fires, resumed speech during the post-roll window cancels the closure.
- **Silero lag:** the detector's own reaction adds ~100–200 ms on top of the configured value [S13][S22].
- **No length cap:** while the VAD keeps reporting speech the utterance stays open (contract P5).

### Measured behavior (internal audit, 2026-08-25, Mac mini M4 Pro, ggml-large-v3-turbo + Silero v5.1.2)

- End-to-end latency from last spoken sample to transcript was **~0.9–1.9 s**, dominated by **blocking STT inference** (450–950 ms, +~400 ms for the first segment with language `auto`), with the VAD queue contributing little (RTF 0.03–0.11) [S22]. At audit time the 300 ms tail quantized to 200 ms probes into a real 400–600 ms close; the current segmenter counts silence at Silero's window resolution (`segmenter.rs:605-632`), so that quantization penalty should be smaller today — unmeasured.
- Known failure (M3): in continuous **English** speech the VAD cut **mid-sentence** at 12.8 s of a 26.3 s segment, on a **comma pause** (EN comma pauses are 150–300 ms). `VAD_END_THRESHOLD=0.45` + 300 ms tail are fragile against EN prosody [S21].

### Configuration drift to be aware of

- The `.env` on the dev host currently sets `VAD_SILENCE_MS=500` (line 98; an earlier revision set 1000) and `STT_PROVIDER=parakeet` (line 85, same `VadSegmenter` per the contract). Both diverge from the canonical 300.
- `WhisperSTTVADConfig::default()` (`whisper.rs:54,58`) embeds `silence_ms: 500, probe_ms: 100` as a test fallback; production values flow from the compiled config via `provider.rs:53-57`.
- Stale docs contradict the canonical value: `readme.md:685` claims default 200 ms (and advises 150–200 at `readme.md:723`; `readme.md:432` says 300), `doc/doc.md:212` claims 500 with "Range: 400–1500" and calls the knob the "biggest single lever", and `doc/ANALISIS-MEJORAS-SENECHAL.md:27` proposes adaptive VAD against an obsolete 1.5 s default. `doc/env-vars.md` is the authority.

## 3. What humans do (turn-taking reference)

- Between-turn gaps in human conversation average **~200 ms** (typical range 100–300 ms), while producing language takes 600+ ms — listeners must predict the end of the other's turn to answer on time [S7].
- The rhythm is close to universal: across 10 very different languages, responses to polar questions averaged 7–468 ms with modes in 0–200 ms [S8].
- Most transitions are very brief: 51–55 % of gaps are under 200 ms and 70–82 % under 500 ms; same-speaker continuation pauses run only ~140 ms longer than between-turn gaps; gaps or overlaps under 120 ms are not perceived as such at all [S9].
- Silence becomes socially marked beyond that range: gaps ≥ 700 ms correlate with "dispreferred" (rejecting) responses — detectable experimentally from ~600 ms — and gaps over 300 ms already reduce unqualified acceptances [S10].
- In spontaneous **monologue**, long thinking pauses form their own class (peak ~1585 ms carrying 14 % of the weight; 2.8 % of pauses exceed 2000 ms) — a class absent in read speech [S11].
- Classic perceived-latency limits: 0.1 s feels instant, 1.0 s is the limit for uninterrupted flow of thought, 10 s is the limit of attention [S12].

Design implication: a conversational assistant that fires after ~200–300 ms of silence tracks human timing, while dictation-style monologue needs a threshold near the thinking-pause class (~1.5–2.5 s). This is the root of the two-profile split proposed below.

## 4. What production systems ship

| System | End-of-turn silence | Neighboring strategy |
|---|---|---|
| OpenAI Realtime (`server_vad`) | 500 ms [S1] | prefix padding 300 ms; threshold 0.5 |
| OpenAI Realtime (`semantic_vad`) | — [S2] | dynamic timeout from turn-end probability; max 2/4/8 s (low/medium/high); default `auto` = medium |
| LiveKit Agents | 0.5–3.0 s [S3] | VAD mode: max(VAD silence, 0.5 s); Silero plugin default 0.55 s |
| Pipecat | 0.2 s [S4] | Smart Turn v3 by default; silence fallback 0.6 s |
| Silero VAD (library) | 100 ms [S5] | threshold 0.5; speech padding 30 ms |
| Deepgram | 500 ms [S13] | configurable (`endpointing`), fully disableable |
| Dialogflow CX | unpublished [S6] | "smart endpointing": heuristic timeout from transcript + intent, distinguishing natural pauses from completed utterances |
| **Seneschal (canonical)** | **300 ms** [S20] | end threshold 0.45; post-roll 200 ms |

Pattern: product fixed thresholds cluster in **200–500 ms**, and the trend is to pair a short silence with a model that decides whether the turn actually ended. Dialogflow is the outlier that publishes no number at all.

*Coverage note:* Alexa/AVS publishes no numeric endpointing default (attempted docs returned 404), and the survey was limited to frameworks verifiable via primary-document fetches (web search was unavailable during the research run); Retell, Vocode, Bland, Gemini Live and others were not checked.

## 5. The limits of a fixed threshold

- The tradeoff is structural: lower threshold ⇒ lower latency but more false cuts; higher ⇒ the reverse [S13].
- The Let's Go spoken dialog system ran with 700 ms fixed; no registered silence ever lasted longer (the system itself truncated thinking pauses) and user turns erroneously split by the endpointer had to be heuristically re-merged when separated by ≤ 1200 ms [S14].
- Context-aware tuning works: a decision tree over dialogue features cut the median live endpointing latency of Let's Go from 957 ms to 561 ms (up to 24 % at equal false-cut rate) — faster after closed questions (800 vs 965 ms) and confirmations (324 vs 965 ms), somewhat slower after open questions [S15].

## 6. Semantic and audio-native turn detection

Detectors that decide "turn complete" from signal rather than from silence alone cover both usage extremes:

- **LiveKit Turn Detector v1.0** encodes audio directly, fusing meaning with acoustic indices (intonation, pitch, rhythm). On the open `eot-bench` — where every intra-turn pause is a decision point that must be waited out to respond fast on the last one — it reached **9.9 % false cuts at a 300 ms budget** (vs 12.9 % for Deepgram Flux and 27.7 % for ultraVAD) and **4.5 % at 600 ms** (vs 5.5 % Soniox, 9.9 % Flux) [S16].
- **Pipecat Smart Turn v3** (Daily) — 8 MB model, decides "turn complete" from the raw waveform without seeing the transcript, ~12 ms CPU inference: **94.31 % accuracy in English, 91.97 % in Spanish** (range 81.3–97.1 % across 23 languages) [S17].
- **LiveKit text turn detector**: 99.3 % true positives, 85–96.3 % true negatives across 14 languages [S17].
- Both kinds of detector sit **on top of a short-pause VAD** and force closure if the turn keeps classifying as incomplete — the VAD remains the fast path and safety net [S17].
- **Prosody** correlates with turn finality (latency reductions up to 81 % at constant false-cut rate) but proved redundant as a standalone signal on a noisy telephone corpus; its place today is fused with semantics [S18].

*Caveat:* the eot-bench and Smart Turn numbers are vendor-reported on the vendors' own open benchmarks; no independent replication was found. Smart Turn figures come from the v3 announcement (Sep 2025); the current repo README describes v3.2 without an accuracy table.

## 7. Options for Seneschal

Ordered roughly by effort. A and B are tunings of the existing mechanism; C–E change the design.

| # | Option | Evidence for | Risk / cost |
|---|---|---|---|
| A | **Shorten the fixed tail to 200 ms** | The audit's quick win [S19]; aligns with human gap timing [S7][S9] and the 200–500 ms industry cluster | The EN comma-pause cut (M3) would get worse unless paired with B; audit demands re-validation against the §3.1 fixtures |
| B | **Harden the close: `VAD_END_THRESHOLD` 0.45 → 0.30, or a `min_segment_ms` (~4 s) guard before allowing a mid-utterance close** | Audit recommendation for M3 [S19]; OpenAI ships 0.5 with 300 ms padding [S1]; Silero lib pads speech by 30 ms [S5] | Higher threshold keeps segments open through low-energy speech; needs re-testing on `cont_en.wav` (must stop cutting at 12.8 s) and `short_*.wav` (must still close fast) |
| C | **Per-context threshold tuning** (decision tree over dialogue features: question type, confirmation, partial-transcript completeness) | −24 % median latency at equal false-cut rate [S15] | Requires logging dialogue features and a labeled corpus of false cuts; 2008 telephone-system evidence, generality unverified |
| D | **Dictation profile**: `VAD_SILENCE_MS` 1500–2500 ms in a dedicated mode | Already documented as the project's own guidance [S23]; thinking-pause class peaks ~1585 ms [S11] | Needs an explicit mode switch (voice command, keyword, or UI) to avoid degrading conversational latency |
| E | **Semantic / audio-native EOT detector** on top of the short-pause VAD | Smart Turn v3: 91.97 % ES accuracy, 8 MB, ~12 ms CPU [S17]; LiveKit v1.0: 9.9 % false cuts @300 ms [S16]; OpenAI semantic_vad as the API-level precedent [S2] | New model in the pipeline (ONNX runtime integration), vendor-reported benchmarks, no Rust-native precedent in this repo |

A pragmatic combination supported by the evidence: **B first** (fix the false-cut failure), then **A** (regain latency), with **E** as the structural route to serve fast commands and long explanations under one configuration; **D** covers dictation until E exists.

## 8. Open questions

1. What is the actual current close latency at `VAD_SILENCE_MS=300` with the window-resolution counter in `segmenter.rs`? (The audit's 400–600 ms figure predates it.)
2. Does 200 ms regress the EN comma-pause fixture, and does B neutralize that regression? (No A/B exists; the audit explicitly demands re-validation, lines 181/185.)
3. Would a Spanish continuous-speech fixture reproduce the M3 failure? (The audit only caught EN; ES prosody was untested for mid-sentence cuts.)
4. How much of the remaining 0.9–1.9 s is attackable by live-transcription reuse vs. VAD tuning? (Latency is dominated by blocking inference, so VAD gains may be smaller than they look.)
5. Can Smart Turn v3 (ONNX, 8 MB) run inside `seneschal-core` with acceptable CPU/GPU cost, and does its accuracy hold on Seneschal's own fixtures (espeak-generated speech is known-unhuman)?
6. Which repo value should be canonical for dictation, and how is the mode switched? (`env-vars.md` documents 1500–2500 ms but no mode exists.)

## 9. Test and measurement plan

Metrics, following eot-bench methodology [S16] where applicable:

- **False-cut rate**: fraction of intra-turn pauses (comma pauses, thinking pauses) after which the pipeline fired mid-utterance.
- **Added latency**: end-of-speech → `SegmentReady`, measured with the `performance` log target; report the distribution (median/p95), not just the mean.
- Sweep `VAD_SILENCE_MS` ∈ {100, 200, 300, 500} × `VAD_END_THRESHOLD` ∈ {0.30, 0.45, 0.55} on the existing fixtures.

Fixtures and harness (already in place from the audit):

- `make test-stt` regenerates and runs the `target/stt-fixtures/` suite (`STT_FORCE_REGEN=1` to force); `cont_en.wav` is the mid-sentence-cut regression case; `short_*.wav` guard fast closure on real short phrases.
- Add a Spanish continuous-speech fixture (`cont_es.wav`) and a thinking-pause fixture (deliberate ≥1 s intra-utterance pauses) to cover the dictation profile.
- When benching an EOT model, label every intra-turn pause in the fixtures as a decision point, as eot-bench does, and report the false-cut/latency curve per budget.

## 10. Sources

- [S1] OpenAI API Reference — Realtime sessions (`ServerVad`); VAD guide — https://developers.openai.com/api/docs/api-reference/realtime-sessions · https://developers.openai.com/api/docs/guides/realtime-vad
- [S2] OpenAI API Reference — Realtime sessions (`SemanticVad`); cross-check: openai-python SDK types — https://developers.openai.com/api/docs/api-reference/realtime-sessions · https://raw.githubusercontent.com/openai/openai-python/main/src/openai/types/realtime/realtime_audio_input_turn_detection.py
- [S3] LiveKit Docs — Turn-taking tuning; livekit/agents Silero VAD plugin source — https://docs.livekit.io/agents/logic/turns/tuning.md · https://raw.githubusercontent.com/livekit/agents/main/livekit-plugins/livekit-plugins-silero/livekit/plugins/silero/vad.py
- [S4] Pipecat Docs — Speech Input & Turn Detection; User Turn Strategies — https://docs.pipecat.ai/pipecat/learn/speech-input.md · https://docs.pipecat.ai/api-reference/server/utilities/turn-management/user-turn-strategies.md
- [S5] snakers4/silero-vad — src/silero_vad/utils_vad.py — https://raw.githubusercontent.com/snakers4/silero-vad/master/src/silero_vad/utils_vad.py
- [S6] Dialogflow CX — Agents (Smart endpointing); googleapis dialogflow protos — https://docs.cloud.google.com/dialogflow/cx/docs/concept/agent · https://raw.githubusercontent.com/googleapis/googleapis/master/google/cloud/dialogflow/cx/v3/agent.proto
- [S7] Levinson & Torreira (2015), "Timing in turn-taking and its implications for processing models of language", Frontiers in Psychology 6:731 — https://europepmc.org/articles/PMC4464110
- [S8] Stivers et al. (2009), "Universals and cultural variation in turn-taking in conversation", PNAS 106(26):10587–10592 — https://pubmed.ncbi.nlm.nih.gov/19553212/ (per-language data via Table 1 of [S7])
- [S9] Heldner & Edlund (2010), "Pauses, gaps and overlaps in conversations", J. Phonetics 38(4):555–568 — https://doi.org/10.1016/j.wocn.2010.08.002 (figures as transcribed in [S7])
- [S10] Levinson & Torreira (2015), citing Kendrick & Torreira (2015), Roberts et al. (2011), ten Bosch et al. (2005) — https://europepmc.org/articles/PMC4464110
- [S11] Campione & Véronis (2002), "A Large-Scale Multilingual Study of Silent Pause Duration", Speech Prosody 2002 — https://www.isca-archive.org/speechprosody_2002/campione02_speechprosody.pdf
- [S12] Jakob Nielsen (NN/g), "Response Times: The 3 Important Limits" — https://www.nngroup.com/articles/response-times-3-important-limits/
- [S13] Deepgram Endpointing docs; Raux & Eskenazi, SIGdial 2008 — https://developers.deepgram.com/docs/endpointing · https://aclanthology.org/W08-0101.pdf
- [S14] Raux & Eskenazi (2008), §3.1; Seneschal STT/VAD contract — https://aclanthology.org/W08-0101.pdf · [stt-contract.md](stt-contract.md)
- [S15] Raux & Eskenazi (2008), "Optimizing Endpointing Thresholds using Dialogue Features in a Spoken Dialogue System", SIGdial 2008 — https://aclanthology.org/W08-0101.pdf
- [S16] LiveKit, "Solving end-of-turn detection: Turn Detector v1.0"; eot-bench — https://livekit.com/blog/solving-end-of-turn-detection · https://github.com/livekit/eot-bench
- [S17] Daily/Pipecat, "Announcing Smart Turn v3"; LiveKit turn detector docs; Pipecat Smart Turn overview — https://www.daily.co/blog/announcing-smart-turn-v3-with-cpu-inference-in-just-12ms/ · https://docs.livekit.io/agents/logic/turns/turn-detector.md · https://docs.pipecat.ai/api-reference/server/utilities/turn-detection/smart-turn-overview.md
- [S18] Raux & Eskenazi (2008), §3.3.4/§4.2/§5; LiveKit Turn Detector v1.0 blog — https://aclanthology.org/W08-0101.pdf · https://livekit.com/blog/solving-end-of-turn-detection
- [S19] Seneschal internal audit, "Auditoría y pruebas del sistema STT" (2026-08-25), recommendations 3 and 5 — `tasks/stt-audit-report-2026-08-25.md:181,185`
- [S20] `seneschal.pro.toml:14-30`; [env-vars.md](env-vars.md)
- [S21] Seneschal internal audit (2026-08-25), finding M3 — `tasks/stt-audit-report-2026-08-25.md:12,164`
- [S22] Seneschal internal audit (2026-08-25), §3.2 latency model; contract P5 — `tasks/stt-audit-report-2026-08-25.md:95-99` · [stt-contract.md](stt-contract.md)
- [S23] [env-vars.md](env-vars.md) — `VAD_SILENCE_MS` dictation guidance
- [S24] Contract P5 hysteresis/post-roll; `segmenter.rs:605-637` — [stt-contract.md](stt-contract.md) · `crates/seneschal-core/src/stt/segmenter.rs`
