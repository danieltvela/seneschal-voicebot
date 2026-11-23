import { invoke } from "@tauri-apps/api/core";
import type { ModelCatalog, ModelEntry } from "./models";
import type { SettingsSnapshot } from "./settings";

export type OnboardingStepId =
  | "language"
  | "llm"
  | "backends"
  | "models"
  | "audio"
  | "finish";

export const ONBOARDING_STEPS: OnboardingStepId[] = [
  "language",
  "llm",
  "backends",
  "models",
  "audio",
  "finish",
];

export const STEP_LABELS: Record<OnboardingStepId, string> = {
  language: "Language",
  llm: "Assistant",
  backends: "Voice engines",
  models: "Models",
  audio: "Audio",
  finish: "Finish",
};

export interface OnboardingStateWire {
  version: number;
  completed: boolean;
  in_progress: boolean;
  current_step: string | null;
  skipped_steps: string[];
}

export interface OnboardingDefaultsWire {
  platform: string;
  gpu_available: boolean;
  recommended_stt: string;
  recommended_tts: string;
}

export interface BackendSelection {
  sttProvider: string;
  ttsProvider: string;
}

export interface WizardDraft {
  language: string;
  llmMode: "local" | "openai";
  llmUrl: string;
  llmModel: string;
  llmApiKey: string;
  clearApiKey: boolean;
  sttProvider: string;
  ttsProvider: string;
}

export async function loadOnboardingState(): Promise<OnboardingStateWire> {
  return invoke<OnboardingStateWire>("get_onboarding_state");
}

export async function beginOnboarding(): Promise<OnboardingStateWire> {
  return invoke<OnboardingStateWire>("begin_onboarding");
}

export async function recordOnboardingStep(
  step: OnboardingStepId,
  skipped: boolean,
): Promise<OnboardingStateWire> {
  return invoke<OnboardingStateWire>("record_onboarding_step", { step, skipped });
}

export async function abandonOnboarding(): Promise<OnboardingStateWire> {
  return invoke<OnboardingStateWire>("abandon_onboarding");
}

export async function completeOnboarding(): Promise<OnboardingStateWire> {
  return invoke<OnboardingStateWire>("complete_onboarding");
}

/** Suppress or enable the daemon startup greeting for subsequent restarts. */
export async function setOnboardingGreeting(active: boolean): Promise<void> {
  await invoke("set_onboarding_greeting", { active });
}

export function stepIndex(step: string | null): number {
  const index = ONBOARDING_STEPS.indexOf(step as OnboardingStepId);
  return index < 0 ? 0 : index;
}

export function remainingSteps(state: OnboardingStateWire): OnboardingStepId[] {
  return ONBOARDING_STEPS.slice(stepIndex(state.current_step));
}

export function didSkipStep(state: OnboardingStateWire, step: OnboardingStepId): boolean {
  return state.skipped_steps.includes(step);
}

function field(snapshot: SettingsSnapshot, key: string) {
  return snapshot.fields.find((candidate) => candidate.key === key);
}

function text(value: unknown): string {
  return value === null || value === undefined ? "" : String(value);
}

/** Prefer an env-locked effective value; otherwise use the platform recommendation. */
export function defaultBackends(
  snapshot: SettingsSnapshot,
  defaults: OnboardingDefaultsWire | null,
): BackendSelection {
  const pick = (key: string, recommended: string | undefined): string => {
    const setting = field(snapshot, key);
    const current = text(setting?.value);
    if (setting && !setting.editable && current) return current;
    if (recommended && (setting?.options ?? []).includes(recommended)) return recommended;
    if (current && (setting?.options ?? []).includes(current)) return current;
    return (setting?.options ?? [])[0] ?? "";
  };
  return {
    sttProvider: pick("stt_provider", defaults?.recommended_stt),
    ttsProvider: pick("tts_provider", defaults?.recommended_tts),
  };
}

export function defaultLanguage(snapshot: SettingsSnapshot): string {
  const current = text(field(snapshot, "language")?.value);
  return current === "en" ? "en" : "es";
}

/** A provider is selectable only when this exact daemon build compiled it. */
export function providerOptions(
  snapshot: SettingsSnapshot,
  direction: "stt" | "tts",
): string[] {
  const fieldKey = direction === "stt" ? "stt_provider" : "tts_provider";
  const options = field(snapshot, fieldKey)?.options ?? [];
  return options.filter((option) => option.length > 0);
}

/** Local catalog entries needed by the selected backends (NeMo/native are absent). */
export function relevantModels(
  catalog: ModelCatalog | null,
  selection: BackendSelection,
): ModelEntry[] {
  if (!catalog) return [];
  return catalog.models.filter((model) => {
    if (model.backend === "silero") {
      return selection.sttProvider === "whisper" || selection.sttProvider === "parakeet";
    }
    if (model.backend === "whisper") return selection.sttProvider === "whisper";
    if (model.backend === "parakeet") return selection.sttProvider === "parakeet";
    if (model.backend === "kokoro") return selection.ttsProvider === "kokoro";
    return false;
  });
}

/** Follow `requires` so VAD is downloaded before its STT consumer. */
export function requiredModelClosure(
  catalog: ModelCatalog | null,
  selectedIds: string[],
): string[] {
  if (!catalog) return [];
  const byId = new Map(catalog.models.map((model) => [model.id, model]));
  const selected = new Set(selectedIds);
  const queue = [...selectedIds];
  while (queue.length > 0) {
    const model = byId.get(queue.pop() ?? "");
    for (const required of model?.requires ?? []) {
      if (!selected.has(required)) {
        selected.add(required);
        queue.push(required);
      }
    }
  }
  return catalog.models
    .filter((model) => selected.has(model.id))
    .map((model) => model.id);
}

export function missingModels(
  catalog: ModelCatalog | null,
  modelIds: string[],
): ModelEntry[] {
  if (!catalog) return [];
  return catalog.models.filter(
    (model) =>
      modelIds.includes(model.id) &&
      (model.state === "missing" || model.state === "partial"),
  );
}

/** Sensible initial selections; large-v3-turbo matches the installer default. */
export function defaultModelIds(
  catalog: ModelCatalog | null,
  selection: BackendSelection,
): string[] {
  if (!catalog) return [];
  const ids: string[] = [];
  if (selection.sttProvider === "whisper") {
    const preferred =
      catalog.models.find((model) => model.id === "whisper-large-v3-turbo") ??
      catalog.models.find((model) => model.backend === "whisper");
    if (preferred) ids.push(preferred.id);
  }
  if (selection.sttProvider === "parakeet") {
    const parakeet = catalog.models.find((model) => model.backend === "parakeet");
    if (parakeet) ids.push(parakeet.id);
  }
  if (selection.ttsProvider === "kokoro") {
    const kokoro = catalog.models.find((model) => model.backend === "kokoro");
    if (kokoro) ids.push(kokoro.id);
  }
  return requiredModelClosure(catalog, ids);
}

export function localBackend(selection: BackendSelection): boolean {
  return (
    selection.sttProvider === "whisper" ||
    selection.sttProvider === "parakeet" ||
    selection.ttsProvider === "kokoro"
  );
}

/** Nemo needs both containers healthy and the served ASR id to match config. */
export function nemoReady(services: { container_state: string; model_matches: boolean | null }[]): boolean {
  return (
    services.length > 0 &&
    services.every(
      (service) => service.container_state === "ready" && service.model_matches !== false,
    )
  );
}

/** Build a sparse Settings patch; blank key means "keep", clearApiKey means "". */
export function buildWizardPatch(
  draft: WizardDraft,
  snapshot: SettingsSnapshot,
): Record<string, unknown> {
  const patch: Record<string, unknown> = {};
  const setText = (key: string, value: string | null): void => {
    const setting = field(snapshot, key);
    if (!setting?.editable) return;
    const next = value?.trim() ?? "";
    const current = text(setting.value);
    if (next !== current) patch[key] = next;
  };

  setText("language", draft.language);
  setText("llm_url", draft.llmUrl);
  setText("llm_model", draft.llmModel);
  if (draft.clearApiKey) {
    patch.llm_api_key = "";
  } else if (draft.llmApiKey.trim()) {
    patch.llm_api_key = draft.llmApiKey.trim();
  }
  setText("stt_provider", draft.sttProvider);
  setText("tts_provider", draft.ttsProvider);
  return patch;
}

/** Human-readable validation failure for the finish step. */
export function validationMessage(errors: { field: string; message: string }[]): string {
  return errors
    .map((error) => (error.field === "config" ? error.message : `${error.field}: ${error.message}`))
    .join("; ");
}
