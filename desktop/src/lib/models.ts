import { get, writable } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { settings, type SettingsApplyResponse } from "./settings";

export type ModelPurpose = "stt" | "tts" | "vad" | "speaker";
export type ModelBackend =
  | "whisper"
  | "parakeet"
  | "silero"
  | "kokoro"
  | "nemotron_asr"
  | "magpie_tts";
export type ModelState = "missing" | "partial" | "installed" | "downloading";
export type DownloadPhase =
  | "downloading"
  | "verifying"
  | "completed"
  | "failed"
  | "cancelled";

export interface ModelFileWire {
  path: string;
  url: string;
  size_bytes: number;
  sha256: string | null;
  present: boolean;
  /** `null` for legacy files that have not been re-verified. */
  verified: boolean | null;
  legacy: boolean;
}

export interface DownloadSnapshot {
  download_id: string;
  phase: DownloadPhase;
  downloaded_bytes: number;
  total_bytes: number;
  percent: number;
  current_file: string | null;
  message: string | null;
}

export interface ModelEntry {
  id: string;
  label: string;
  purpose: ModelPurpose;
  backend: ModelBackend;
  state: ModelState;
  active: boolean;
  total_bytes: number;
  requires: string[];
  files: ModelFileWire[];
  download: DownloadSnapshot | null;
}

export interface ModelCatalog {
  root: string;
  models: ModelEntry[];
}

export interface NemoWeightFile {
  name: string;
  present: boolean;
}

export interface NemoService {
  id: string;
  label: string;
  base_url: string;
  container_state:
    | "docker_unavailable"
    | "stopped"
    | "starting"
    | "ready"
    | "model_mismatch";
  ready: boolean | null;
  served_models: string[];
  expected_model: string | null;
  model_matches: boolean | null;
  weights_state: "missing" | "partial" | "ready" | "downloading" | "unknown";
  weights: NemoWeightFile[];
  active: boolean;
  hint?: string;
}

/** SSE progress payload from the shell (camelCase, see `models.rs`). */
export interface ModelProgress {
  modelId: string;
  downloadId: string;
  phase: DownloadPhase;
  downloadedBytes: number;
  totalBytes: number | null;
  currentFile: string | null;
  message: string | null;
}

export const modelCatalog = writable<ModelCatalog | null>(null);
export const modelProgress = writable<Record<string, ModelProgress>>({});
export const nemoServices = writable<NemoService[] | null>(null);

let unlistenModelProgress: UnlistenFn | null = null;

export async function initModels(): Promise<void> {
  if (!unlistenModelProgress) {
    unlistenModelProgress = await listen<ModelProgress>(
      "desktop-model-progress",
      (event) => {
        const progress = event.payload;
        modelProgress.update((all) => ({
          ...all,
          [progress.modelId]: progress,
        }));
      },
    );
  }
  await Promise.all([refreshModels(), refreshNemo()]);
}

/** In-flight progress dies with the daemon; the next refresh re-syncs. */
export function resetModelProgressForDisconnect(): void {
  modelProgress.set({});
}

export async function refreshModels(): Promise<void> {
  const catalog = await invoke<ModelCatalog>("get_models");
  modelCatalog.set(catalog);
}

export async function refreshNemo(): Promise<void> {
  const services = await invoke<NemoService[]>("get_nemo_services");
  nemoServices.set(services);
}

export async function downloadModel(id: string): Promise<void> {
  await invoke("download_model", { id });
  await refreshModels();
}

export async function cancelModelDownload(id: string): Promise<void> {
  await invoke("cancel_model_download", { id });
  await refreshModels();
}

export async function deleteModel(id: string): Promise<void> {
  await invoke("delete_model", { id });
  await refreshModels();
}

/** Applies restart-tier settings; the shell restarts the sidecar inside the
 * Tauri command and resolves after the daemon reconnects. */
export async function activateModel(
  id: string,
): Promise<SettingsApplyResponse> {
  const response = await invoke<SettingsApplyResponse>("activate_model", { id });
  settings.set(response.snapshot);
  await refreshModels();
  return response;
}

export async function startNemoService(id: string): Promise<void> {
  await invoke("start_nemo_service", { id });
  await refreshNemo();
}

export async function stopNemoService(id: string): Promise<void> {
  await invoke("stop_nemo_service", { id });
  await refreshNemo();
}

export async function downloadNemoWeights(id: string): Promise<void> {
  await invoke("download_nemo_weights", { id });
  await refreshNemo();
}

// ── Pure helpers (unit-tested) ──────────────────────────────────────────────

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = "B";
  for (const next of units) {
    if (value < 1024) break;
    value /= 1024;
    unit = next;
  }
  return `${value >= 100 ? Math.round(value) : value.toFixed(1)} ${unit}`;
}

export function percentOf(downloaded: number, total: number | null): number {
  if (!total || total <= 0) return 0;
  return Math.min(100, Math.round((downloaded / total) * 100));
}

export function modelStateLabel(state: ModelState): string {
  const labels: Record<ModelState, string> = {
    missing: "Missing",
    partial: "Partial",
    installed: "Installed",
    downloading: "Downloading",
  };
  return labels[state] ?? state;
}

export function phaseLabel(phase: DownloadPhase): string {
  const labels: Record<DownloadPhase, string> = {
    downloading: "Downloading",
    verifying: "Verifying",
    completed: "Completed",
    failed: "Failed",
    cancelled: "Cancelled",
  };
  return labels[phase] ?? phase;
}

export function purposeLabel(purpose: ModelPurpose): string {
  const labels: Record<ModelPurpose, string> = {
    stt: "STT",
    tts: "TTS",
    vad: "VAD",
    speaker: "Speaker",
  };
  return labels[purpose] ?? purpose;
}

export function containerStateLabel(
  state: NemoService["container_state"],
): string {
  const labels: Record<NemoService["container_state"], string> = {
    docker_unavailable: "Docker unavailable",
    stopped: "Stopped",
    starting: "Starting",
    ready: "Ready",
    model_mismatch: "Model mismatch",
  };
  return labels[state] ?? state;
}

export function weightsStateLabel(
  state: NemoService["weights_state"],
): string {
  const labels: Record<NemoService["weights_state"], string> = {
    missing: "Missing",
    partial: "Partial",
    ready: "Ready",
    downloading: "Downloading",
    unknown: "Unknown",
  };
  return labels[state] ?? state;
}

/** Absolute managed path for a model file (or `null` when not present). */
export function managedFilePath(
  catalog: ModelCatalog | null,
  entry: ModelEntry,
  match?: (path: string) => boolean,
): string | null {
  if (!catalog) return null;
  const file = entry.files.find(
    (candidate) => candidate.present && (!match || match(candidate.path)),
  );
  if (!file) return null;
  const root = catalog.root.replace(/\/+$/, "");
  return `${root}/${file.path}`;
}

export interface ModelOption {
  value: string;
  label: string;
  modelId: string;
  /** Paired voices path for Kokoro bundles. */
  voices?: string;
}

/** Installed-model options for the Settings selectors (issue #35). */
export function modelOptionsForField(
  catalog: ModelCatalog | null,
  key: string,
): ModelOption[] {
  if (!catalog) return [];
  const installed = catalog.models.filter((model) => model.state === "installed");
  switch (key) {
    case "whisper_model":
      return installed
        .filter((model) => model.backend === "whisper")
        .map((model) => ({
          value: managedFilePath(catalog, model) ?? "",
          label: model.label,
          modelId: model.id,
        }))
        .filter((option) => option.value !== "");
    case "vad_model":
      return installed
        .filter((model) => model.backend === "silero")
        .map((model) => ({
          value: managedFilePath(catalog, model) ?? "",
          label: model.label,
          modelId: model.id,
        }))
        .filter((option) => option.value !== "");
    case "parakeet_model_dir": {
      return installed
        .filter((model) => model.backend === "parakeet")
        .map((model) => {
          const first = model.files[0]?.path ?? "";
          const dir = first.includes("/") ? first.slice(0, first.lastIndexOf("/")) : "";
          if (!dir) return null;
          const root = catalog.root.replace(/\/+$/, "");
          return { value: `${root}/${dir}`, label: model.label, modelId: model.id };
        })
        .filter((option): option is ModelOption => option !== null);
    }
    case "kokoro_model":
      return installed
        .filter((model) => model.backend === "kokoro")
        .map((model) => ({
          value: managedFilePath(catalog, model, (path) => path.endsWith(".onnx")) ?? "",
          label: model.label,
          modelId: model.id,
          voices:
            managedFilePath(catalog, model, (path) => path.endsWith(".bin")) ??
            undefined,
        }))
        .filter((option) => option.value !== "");
    case "kokoro_voices":
      return installed
        .filter((model) => model.backend === "kokoro")
        .map((model) => ({
          value: managedFilePath(catalog, model, (path) => path.endsWith(".bin")) ?? "",
          label: model.label,
          modelId: model.id,
        }))
        .filter((option) => option.value !== "");
    default:
      return [];
  }
}

/** Whether a Settings field is backed by the model catalog. */
export function isModelField(key: string): boolean {
  return [
    "whisper_model",
    "parakeet_model_dir",
    "kokoro_model",
    "kokoro_voices",
    "vad_model",
  ].includes(key);
}

/** Progress display for a model: live SSE wins over the catalog snapshot. */
export function effectiveProgress(
  entry: ModelEntry,
  progress: Record<string, ModelProgress>,
): { downloaded: number; total: number | null; phase: DownloadPhase; message: string | null } | null {
  const live = progress[entry.id];
  if (live) {
    return {
      downloaded: live.downloadedBytes,
      total: live.totalBytes,
      phase: live.phase,
      message: live.message,
    };
  }
  if (entry.download) {
    return {
      downloaded: entry.download.downloaded_bytes,
      total: entry.download.total_bytes || entry.total_bytes,
      phase: entry.download.phase,
      message: entry.download.message,
    };
  }
  return null;
}

export function currentModelProgress(id: string): ModelProgress | null {
  return get(modelProgress)[id] ?? null;
}
