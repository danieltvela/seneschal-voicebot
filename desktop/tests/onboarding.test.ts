import { describe, expect, it } from "vitest";
import {
  buildWizardPatch,
  defaultBackends,
  defaultLanguage,
  defaultModelIds,
  didSkipStep,
  localBackend,
  missingModels,
  nemoReady,
  relevantModels,
  remainingSteps,
  requiredModelClosure,
  stepIndex,
  validationMessage,
  type OnboardingDefaultsWire,
  type OnboardingStateWire,
} from "../src/lib/onboarding";
import type { ModelCatalog, ModelEntry } from "../src/lib/models";
import type { SettingsField, SettingsSnapshot } from "../src/lib/settings";

function field(
  key: string,
  value: unknown,
  overrides: Partial<SettingsField> = {},
): SettingsField {
  return {
    key,
    label: key,
    category: "general",
    value,
    origin: "file",
    tier: "restart",
    editable: true,
    ...overrides,
  };
}

function snapshot(fields: SettingsField[]): SettingsSnapshot {
  return {
    revision: 1,
    config_file: "/tmp/seneschal.pro.toml",
    fields,
    providers: { stt: ["nemotron", "whisper"], tts: ["kokoro", "openai"] },
  };
}

function model(overrides: Partial<ModelEntry> = {}): ModelEntry {
  return {
    id: "kokoro-v1.0",
    label: "Kokoro",
    purpose: "tts",
    backend: "kokoro",
    state: "installed",
    active: false,
    total_bytes: 1,
    requires: [],
    files: [],
    download: null,
    ...overrides,
  };
}

function catalog(models: ModelEntry[]): ModelCatalog {
  return { root: "/models", models };
}

const defaults: OnboardingDefaultsWire = {
  platform: "linux",
  gpu_available: false,
  recommended_stt: "whisper",
  recommended_tts: "kokoro",
};

describe("onboarding helpers", () => {
  it("tracks step position and skips", () => {
    expect(stepIndex(null)).toBe(0);
    expect(stepIndex("models")).toBe(3);
    const state: OnboardingStateWire = {
      version: 1,
      completed: false,
      in_progress: true,
      current_step: "models",
      skipped_steps: ["language"],
    };
    expect(remainingSteps(state)).toEqual(["models", "audio", "finish"]);
    expect(didSkipStep(state, "language")).toBe(true);
    expect(didSkipStep(state, "audio")).toBe(false);
  });

  it("prefers the platform recommendation but respects env-locked values", () => {
    const selection = defaultBackends(
      snapshot([
        field("stt_provider", "whisper", { options: ["nemotron", "whisper"] }),
        field("tts_provider", "kokoro", { options: ["kokoro", "openai"] }),
      ]),
      defaults,
    );
    expect(selection).toEqual({ sttProvider: "whisper", ttsProvider: "kokoro" });

    const locked = defaultBackends(
      snapshot([
        field("stt_provider", "nemotron", {
          editable: false,
          origin: "environment",
          options: ["nemotron", "whisper"],
        }),
        field("tts_provider", "openai", { options: ["kokoro", "openai"] }),
      ]),
      defaults,
    );
    expect(locked).toEqual({ sttProvider: "nemotron", ttsProvider: "kokoro" });
  });

  it("defaults language from the effective settings", () => {
    expect(defaultLanguage(snapshot([field("language", "en")]))).toBe("en");
    expect(defaultLanguage(snapshot([field("language", null)]))).toBe("es");
  });

  it("filters the catalog to the selected backends and their dependencies", () => {
    const entries = [
      model({ id: "silero", backend: "silero", purpose: "vad" }),
      model({ id: "whisper-large", backend: "whisper", purpose: "stt", requires: ["silero"] }),
      model({ id: "kokoro", backend: "kokoro", purpose: "tts" }),
      model({ id: "parakeet", backend: "parakeet", purpose: "stt", requires: ["silero"] }),
    ];
    const cat = catalog(entries);

    const local = relevantModels(cat, { sttProvider: "whisper", ttsProvider: "kokoro" });
    expect(local.map((entry) => entry.id)).toEqual(["silero", "whisper-large", "kokoro"]);

    const native = relevantModels(cat, { sttProvider: "speech", ttsProvider: "avspeech" });
    expect(native).toEqual([]);

    const closure = requiredModelClosure(cat, ["whisper-large"]);
    expect(closure).toEqual(["silero", "whisper-large"]);
  });

  it("preselects installer-equivalent models and reports missing artifacts", () => {
    const entries = [
      model({ id: "silero", backend: "silero", purpose: "vad", state: "missing" }),
      model({
        id: "whisper-large-v3-turbo",
        backend: "whisper",
        purpose: "stt",
        state: "missing",
        requires: ["silero"],
      }),
      model({ id: "kokoro", backend: "kokoro", purpose: "tts", state: "missing" }),
    ];
    const cat = catalog(entries);
    const selected = defaultModelIds(cat, { sttProvider: "whisper", ttsProvider: "kokoro" });
    expect(selected).toEqual(["silero", "whisper-large-v3-turbo", "kokoro"]);
    expect(missingModels(cat, selected).map((entry) => entry.id)).toEqual([
      "silero",
      "whisper-large-v3-turbo",
      "kokoro",
    ]);
    expect(localBackend({ sttProvider: "speech", ttsProvider: "avspeech" })).toBe(false);
  });

  it("requires healthy matching NeMo services before continuing", () => {
    expect(
      nemoReady([
        { container_state: "ready", model_matches: true },
        { container_state: "ready", model_matches: null },
      ]),
    ).toBe(true);
    expect(
      nemoReady([
        { container_state: "stopped", model_matches: true },
        { container_state: "ready", model_matches: true },
      ]),
    ).toBe(false);
    expect(
      nemoReady([
        { container_state: "ready", model_matches: false },
        { container_state: "ready", model_matches: true },
      ]),
    ).toBe(false);
    expect(nemoReady([])).toBe(false);
  });

  it("builds a sparse patch and treats blank API key as keep", () => {
    const snap = snapshot([
      field("language", "es"),
      field("llm_url", "http://127.0.0.1:8000"),
      field("llm_model", "local-model"),
      field("stt_provider", "whisper"),
      field("tts_provider", "kokoro"),
    ]);
    const patch = buildWizardPatch(
      {
        language: "en",
        llmMode: "local",
        llmUrl: "http://127.0.0.1:8000",
        llmModel: "new-model",
        llmApiKey: "  ",
        clearApiKey: false,
        sttProvider: "whisper",
        ttsProvider: "kokoro",
      },
      snap,
    );
    expect(patch).toEqual({ language: "en", llm_model: "new-model" });

    const clearPatch = buildWizardPatch(
      {
        language: "en",
        llmMode: "openai",
        llmUrl: "https://api.openai.com/v1",
        llmModel: "gpt-test",
        llmApiKey: "  secret  ",
        clearApiKey: true,
        sttProvider: "whisper",
        ttsProvider: "kokoro",
      },
      snap,
    );
    expect(clearPatch).toEqual({
      language: "en",
      llm_url: "https://api.openai.com/v1",
      llm_model: "gpt-test",
      llm_api_key: "",
    });
  });

  it("formats validation failures readably", () => {
    expect(
      validationMessage([
        { field: "llm_url", message: "URL cannot be empty" },
        { field: "config", message: "invalid combination" },
      ]),
    ).toBe("llm_url: URL cannot be empty; invalid combination");
  });
});
