import { get, writable } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  applySettings,
  settings,
  validateSettings,
  type SettingsApplyResponse,
  type SettingsField,
} from "./settings";

export type AudioPermission =
  | "not_required"
  | "not_determined"
  | "restricted"
  | "denied"
  | "authorized";

export interface AudioLevel {
  rms: number;
  peak: number;
}

export interface AudioDevice {
  name: string;
  selector: string;
  is_default: boolean;
  sample_rate: number | null;
  channels: number | null;
  sample_format: string | null;
}

export interface AudioDeviceInventory {
  inputs: AudioDevice[];
  outputs: AudioDevice[];
}

export interface AudioStatus {
  capture_active: boolean;
  capture_error: string | null;
  output_available: boolean;
  microphone_permission: AudioPermission;
  configured_input_device: string | null;
  configured_output_device: string | null;
  stt_provider: string;
  tts_provider: string;
  test_in_progress: string | null;
}

export type AudioTestKind = "preview_tts" | "speaker_test" | "mic_test" | "loopback";

export type AudioTestState = "idle" | "running" | "pass" | "fail" | "cancelled";

export interface AudioTestResult {
  kind: "mic" | "loopback" | "preview_tts" | "speaker_test";
  transcript: string | null;
  peak: number;
  rms: number;
  duration_secs: number;
  provider: string | null;
  text?: string;
}

export interface AudioApiError {
  code: string;
  message: string;
  hint?: string;
}

export interface AudioTestUiState {
  state: AudioTestState;
  result: AudioTestResult | null;
  error: AudioApiError | null;
  hint: string | null;
}

const IDLE_TEST: AudioTestUiState = {
  state: "idle",
  result: null,
  error: null,
  hint: null,
};

export const audioLevel = writable<AudioLevel | null>(null);
export const audioDevices = writable<AudioDeviceInventory | null>(null);
export const audioStatus = writable<AudioStatus | null>(null);
export const audioTests = writable<Record<AudioTestKind, AudioTestUiState>>({
  preview_tts: { ...IDLE_TEST },
  speaker_test: { ...IDLE_TEST },
  mic_test: { ...IDLE_TEST },
  loopback: { ...IDLE_TEST },
});

let unlistenAudioLevel: UnlistenFn | null = null;
let testGeneration = 0;

export async function initAudio(): Promise<void> {
  if (!unlistenAudioLevel) {
    unlistenAudioLevel = await listen<AudioLevel>(
      "desktop-audio-level",
      (event) => audioLevel.set(event.payload),
    );
  }
  await refreshAudio();
}

/** Clear meter/test state when Control disconnects (for example during restart). */
export function resetAudioForDisconnect(): void {
  audioLevel.set(null);
  audioTests.set({
    preview_tts: { ...IDLE_TEST },
    speaker_test: { ...IDLE_TEST },
    mic_test: { ...IDLE_TEST },
    loopback: { ...IDLE_TEST },
  });
}

export async function refreshAudio(): Promise<void> {
  const [devices, status] = await Promise.all([
    invoke<AudioDeviceInventory>("get_audio_devices"),
    invoke<AudioStatus>("get_audio_status"),
  ]);
  audioDevices.set(devices);
  audioStatus.set(status);
}

export function deviceBySelector(
  inventory: AudioDeviceInventory | null,
  direction: "input" | "output",
  selector: string | null,
): AudioDevice | undefined {
  if (!inventory || !selector) return undefined;
  const devices = direction === "input" ? inventory.inputs : inventory.outputs;
  return devices.find((device) => device.selector === selector);
}

/** Empty string is the form's representation of the system default (`null`). */
export function deviceFormValue(selector: string | null | undefined): string {
  return selector ?? "";
}

export async function applyDeviceSettings(
  inputSelector: string | null,
  outputSelector: string | null,
): Promise<SettingsApplyResponse | null> {
  const snapshot = get(settings);
  const patch: Record<string, unknown> = {};
  for (const [key, selector] of [
    ["audio_input_device", inputSelector],
    ["audio_output_device", outputSelector],
  ] as const) {
    const field = snapshot?.fields.find((candidate) => candidate.key === key);
    const current = settingText(field);
    const next = deviceFormValue(selector);
    if (next !== current) {
      patch[key] = next === "" ? null : next;
    }
  }

  if (Object.keys(patch).length === 0) {
    return null;
  }

  const validation = await validateSettings(patch);
  if (!validation.ok) {
    const message = validation.errors
      .map((error) => `${error.field}: ${error.message}`)
      .join("; ");
    throw new Error(message);
  }

  // The shell waits for the post-restart Control connection before returning.
  const response = await applySettings(patch);
  settings.set(response.snapshot);
  await refreshAudio();
  return response;
}

function settingText(field: SettingsField | undefined): string {
  if (!field || field.value === null || field.value === undefined) return "";
  return String(field.value);
}

export function parseAudioError(error: unknown): AudioApiError {
  const text = error instanceof Error ? error.message : String(error);
  try {
    // Tauri command errors are strings; the shell serializes structured HTTP
    // errors as `code: message (HTTP status)`.
    const code = text.slice(0, text.indexOf(":")).trim();
    if (code && /^[a-z][a-z_]+$/.test(code)) {
      const message = text.slice(text.indexOf(":") + 1).trim();
      return { code, message };
    }
  } catch {
    // fall through to generic text
  }
  return { code: "audio_request_failed", message: text };
}

export function diagnosticHint(error: AudioApiError | null, status: AudioStatus | null): string | null {
  if (error?.hint) return error.hint;
  if (!error) return null;
  switch (error.code) {
    case "capture_unavailable":
      if (status?.microphone_permission === "denied") {
        return "Grant microphone access, then apply the input device again.";
      }
      if (
        status?.configured_input_device &&
        !deviceBySelector(get(audioDevices), "input", status.configured_input_device)
      ) {
        return "The configured input device is not present. Select another device and apply.";
      }
      return "No active microphone. Select an input device, apply it, then retry.";
    case "output_unavailable":
      return "No usable output device. Select an output device and apply.";
    case "audio_busy":
      return "Wait until the voice pipeline is idle, then run the test again.";
    case "provider_error":
      return "The active STT/TTS backend failed. Check its service/model settings.";
    case "test_timed_out":
      return "The backend did not answer in time. Check the STT service and try again.";
    case "cancelled":
      return "Test cancelled by barge-in or shutdown.";
    default:
      return null;
  }
}

/** UI classification for the end-to-end mic test. */
export function classifyMicResult(
  result: AudioTestResult,
): { state: AudioTestState; hint: string | null } {
  if (result.peak <= 0.01) {
    return {
      state: "fail",
      hint: "The microphone is silent. Check that it is not muted and input gain is above zero.",
    };
  }
  if (!result.transcript?.trim()) {
    return {
      state: "fail",
      hint: "Audio was captured but STT returned no transcript. Speak clearly and check the STT backend.",
    };
  }
  return { state: "pass", hint: null };
}

/** Convert linear RMS to a 0–100 meter percentage with a -60 dB floor. */
export function meterPercent(linear: number): number {
  const dbfs = 20 * Math.log10(Math.max(linear, 1e-6));
  return Math.round(Math.min(100, Math.max(0, ((dbfs + 60) / 60) * 100)));
}

export function formatDbfs(linear: number): string {
  if (linear <= 0) return "-∞ dBFS";
  return `${(20 * Math.log10(linear)).toFixed(1)} dBFS`;
}

/** Simple peak hold with linear decay for the VU meter. */
export function peakHold(previous: number, incoming: number, decay = 0.015): number {
  return incoming >= previous ? incoming : Math.max(incoming, previous - decay);
}

async function runAudioTest(
  kind: AudioTestKind,
  action: () => Promise<AudioTestResult>,
): Promise<AudioTestResult | null> {
  const generation = ++testGeneration;
  setTest(kind, { state: "running", result: null, error: null, hint: null });
  try {
    const result = await action();
    if (generation !== testGeneration) return null;
    let ui: AudioTestUiState = { state: "pass", result, error: null, hint: null };
    if (kind === "mic_test") {
      const classified = classifyMicResult(result);
      ui = { ...ui, state: classified.state, hint: classified.hint };
    }
    setTest(kind, ui);
    await refreshAudio();
    return result;
  } catch (error) {
    if (generation !== testGeneration) return null;
    const apiError = parseAudioError(error);
    setTest(kind, {
      state: apiError.code === "cancelled" ? "cancelled" : "fail",
      result: null,
      error: apiError,
      hint: diagnosticHint(apiError, get(audioStatus)),
    });
    await refreshAudio();
    return null;
  }
}

function setTest(kind: AudioTestKind, state: AudioTestUiState): void {
  audioTests.update((tests) => ({ ...tests, [kind]: state }));
}

export async function runTtsPreview(): Promise<void> {
  await runAudioTest("preview_tts", async () =>
    invoke<AudioTestResult>("tts_preview"),
  );
}

export async function runSpeakerTest(): Promise<void> {
  await runAudioTest("speaker_test", async () =>
    invoke<AudioTestResult>("speaker_test"),
  );
}

export async function runMicTest(durationSecs = 5): Promise<void> {
  await runAudioTest("mic_test", async () =>
    invoke<AudioTestResult>("mic_test", { durationSecs }),
  );
}

export async function runLoopbackTest(durationSecs = 5): Promise<void> {
  await runAudioTest("loopback", async () =>
    invoke<AudioTestResult>("loopback_test", { durationSecs }),
  );
}

export function clearManualVerdicts(): void {
  audioTests.update((tests) => ({
    ...tests,
    speaker_test: { ...IDLE_TEST },
    loopback: { ...IDLE_TEST },
  }));
}
