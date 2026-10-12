---
name: stt-debug-log
description: Quick grep for STT pipeline events from voicebot.pro.log. Use when debugging STT provider issues, VAD state, speech detection, transcription timing, or end_audio behavior.
---

## Use This Skill

Put this line at the top of any opencode prompt that needs STT log analysis:

@stt-debug-log

Then describe what you're looking for (e.g., "check if VAD confirmed speech", "see end_audio timing", "find SpeechEnd events").

## Command

```bash
grep -E "(stt|Speech|Partial|Detect|Finish|end_audio|VAD|silence|accumul|confirm|SpeechEnd|Segment)" voicebot.pro.log | tail -40
```

## What Each Pattern Shows

| Pattern | Meaning |
|---|---|
| `stt` | All STT provider logs (info, debug, error) |
| `Speech` | SpeechStart, SpeechEnd, Speech(partial) events |
| `Partial` | Apple's `DidHypothesizeTranscription` partial results |
| `Detect` | Apple's `DidDetectSpeech` event |
| `Finish` | Apple's `DidFinishRecognition` / `DidFinishSuccessfully` |
| `end_audio` | Proactive `end_audio()` calls from VAD silence timeout |
| `VAD` | VAD state machine transitions (accumulating, confirmed, silence) |
| `silence` | Silence sample counting, timeout triggers |
| `accumul` | Accumulation phase (waiting for speech confirmation) |
| `confirm` | Speech confirmation after vad_confirm_probes |
| `SpeechEnd` | Final transcription emitted to pipeline |
| `Segment` | Pipeline segment timing info |

## Typical Debugging Scenarios

**"VAD not confirming speech"**
Look for `accumul` entries. If you see `Start accumulating` repeatedly but never `Speech confirmed`, the vad_confirm_probes threshold isn't being reached (probes resetting on silence).

**"end_audio not being called"**
Look for `end_audio` entries. If absent after speech, the VAD silence timeout isn't triggering (silence_samples never reaching threshold).

**"First words missing/wrong"**
Look for `Speech confirmed` timestamp vs first `Partial` timestamp. Gap > 400ms means VAD confirmation delay is cutting onset.

**"Long phrases cut short"**
Look for `end_audio` appearing mid-utterance. Check if `post_roll` extended the grace period enough.

**"Task not recreating after finish"**
Look for `Task finished: true` followed by new `accumul` entries. If no new accumulation after task_done, VAD reset is blocking.

## Full Log (if tail -40 isn't enough)

```bash
grep -E "(stt|Speech|Partial|Detect|Finish|end_audio|VAD|silence|accumul|confirm)" voicebot.pro.log
```

## Clear Log (before fresh test)

```bash
> voicebot.pro.log
```