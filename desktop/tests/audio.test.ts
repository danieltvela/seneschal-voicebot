import { describe, expect, it } from "vitest";
import {
  classifyMicResult,
  deviceBySelector,
  deviceFormValue,
  diagnosticHint,
  formatDbfs,
  meterPercent,
  parseAudioError,
  peakHold,
  type AudioDeviceInventory,
  type AudioStatus,
  type AudioTestResult,
} from "../src/lib/audio";

describe("audio panel helpers", () => {
  it("parses structured shell errors without leaking HTTP internals into the UI", () => {
    const error = parseAudioError(
      "capture_unavailable: Audio capture is not active (HTTP 503)",
    );
    expect(error.code).toBe("capture_unavailable");
    expect(error.message).toContain("Audio capture is not active");
  });

  it("maps missing capture to actionable device/permission hints", () => {
    const status: AudioStatus = {
      capture_active: false,
      capture_error: null,
      output_available: true,
      microphone_permission: "denied",
      configured_input_device: null,
      configured_output_device: null,
      stt_provider: "nemotron",
      tts_provider: "kokoro",
      test_in_progress: null,
    };
    const hint = diagnosticHint(
      { code: "capture_unavailable", message: "Audio capture is not active" },
      status,
    );
    expect(hint).toContain("Grant microphone access");
  });

  it("classifies a silent mic differently from a working mic without transcript", () => {
    const silent: AudioTestResult = {
      kind: "mic",
      transcript: null,
      peak: 0,
      rms: 0,
      duration_secs: 5,
      provider: "nemotron",
    };
    expect(classifyMicResult(silent).hint).toContain("muted");

    const noTranscript: AudioTestResult = { ...silent, peak: 0.3 };
    expect(classifyMicResult(noTranscript).hint).toContain("STT");
  });

  it("maps linear levels to a bounded -60 dBFS meter", () => {
    expect(meterPercent(1)).toBe(100);
    expect(meterPercent(0.001)).toBe(0);
    expect(meterPercent(0)).toBe(0);
    expect(meterPercent(0.1)).toBeGreaterThan(50);
  });

  it("formats and holds peak levels", () => {
    expect(formatDbfs(0)).toBe("-∞ dBFS");
    expect(peakHold(0.8, 0.1, 0.1)).toBeCloseTo(0.7);
    expect(peakHold(0.2, 0.8, 0.1)).toBeCloseTo(0.8);
  });

  it("resolves duplicate devices with the daemon selector format", () => {
    const inventory: AudioDeviceInventory = {
      inputs: [
        {
          name: "Headset",
          selector: "Headset",
          is_default: false,
          sample_rate: 16000,
          channels: 1,
          sample_format: "I16",
        },
        {
          name: "Headset",
          selector: "Headset#1",
          is_default: false,
          sample_rate: 16000,
          channels: 1,
          sample_format: "I16",
        },
      ],
      outputs: [],
    };
    expect(deviceBySelector(inventory, "input", "Headset#1")?.name).toBe("Headset");
    expect(deviceFormValue(null)).toBe("");
  });
});
