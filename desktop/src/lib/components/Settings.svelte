<script lang="ts">
  import { onMount } from "svelte";
  import {
    applySettings,
    loadSettings,
    originLabel,
    settings,
    tierLabel,
    validateSettings,
    type SettingsField,
    type SettingsPatch,
    type SettingsSnapshot,
  } from "../settings";
  import { runTtsPreview } from "../audio";
  import { status } from "../status";

  type FormValue = string | number | boolean | null;

  const CATEGORY_ORDER = ["llm", "stt", "tts", "general", "audio", "network"] as const;
  const CATEGORY_LABELS: Record<string, string> = {
    llm: "LLM",
    stt: "Speech recognition",
    tts: "Speech synthesis",
    general: "General",
    audio: "Audio devices",
    network: "Network",
  };

  const NUMBER_FIELDS = new Set([
    "llm_temperature",
    "llm_max_tokens",
    "avspeech_rate",
    "kokoro_speed",
    "openai_tts_speed",
    "output_volume",
  ]);
  const PORT_FIELDS = new Set(["control_port", "ws_port"]);
  const DEVICE_FIELDS = new Set(["audio_input_device", "audio_output_device"]);
  const BOOL_FIELDS = new Set(["llm_thinking"]);

  let form = $state<Record<string, FormValue>>({});
  let loading = $state(true);
  let saving = $state(false);
  let loadError = $state<string | null>(null);
  let actionError = $state<string | null>(null);
  let actionMessage = $state<string | null>(null);
  let fieldErrors = $state<Record<string, string>>({});
  let previewing = $state(false);

  onMount(async () => {
    try {
      await loadSettings();
    } catch (error) {
      loadError = error instanceof Error ? error.message : String(error);
    } finally {
      loading = false;
    }
  });

  $effect(() => {
    const snapshot = $settings;
    if (snapshot) hydrate(snapshot);
  });

  function hydrate(snapshot: SettingsSnapshot): void {
    const next: Record<string, FormValue> = {};
    for (const field of snapshot.fields) {
      next[field.key] = BOOL_FIELDS.has(field.key)
        ? field.value === true
        : settingInputValue(field.value);
    }
    form = next;
  }

  function settingInputValue(value: unknown): string {
    if (value === null || value === undefined) return "";
    if (typeof value === "boolean") return value ? "true" : "false";
    return String(value);
  }

  function setForm(key: string, value: FormValue): void {
    form = { ...form, [key]: value };
  }

  function fieldsIn(category: string): SettingsField[] {
    return ($settings?.fields ?? [])
      .filter((field) => field.category === category)
      .filter(isVisible);
  }

  function isVisible(field: SettingsField): boolean {
    const provider = form.tts_provider;
    const sttProvider = form.stt_provider;
    if (field.key === "whisper_model" && sttProvider !== "whisper") return false;
    if (field.key === "parakeet_model_dir" && sttProvider !== "parakeet") return false;
    if (
      (sttProvider === "whisper" ||
        sttProvider === "parakeet" ||
        sttProvider === "speech") &&
      (field.key === "stt_url" || field.key === "stt_model")
    ) {
      return false;
    }
    if (provider === "avspeech" && field.key.startsWith("kokoro_")) return false;
    if (provider === "avspeech" && field.key.startsWith("openai_tts_")) return false;
    if (provider === "kokoro" && field.key.startsWith("avspeech_")) return false;
    if (provider === "kokoro" && field.key.startsWith("openai_tts_")) return false;
    if (provider === "openai" && field.key.startsWith("avspeech_")) return false;
    if (provider === "openai" && field.key.startsWith("kokoro_")) return false;
    return true;
  }

  function buildPatch(): SettingsPatch {
    const patch: SettingsPatch = {};
    for (const field of $settings?.fields ?? []) {
      if (!field.editable) continue;
      const raw = form[field.key];
      if (BOOL_FIELDS.has(field.key)) {
        if (field.value !== (raw === true)) patch[field.key] = raw === true;
        continue;
      }
      if (raw === undefined) continue;
      const text = String(raw);
      if (NUMBER_FIELDS.has(field.key) || PORT_FIELDS.has(field.key)) {
        if (text.trim() === "") {
          if (PORT_FIELDS.has(field.key) && field.value !== null) {
            patch[field.key] = null;
          }
          continue;
        }
        const parsed = Number(text);
        if (!Number.isFinite(parsed) || parsed !== Number(field.value)) {
          patch[field.key] = parsed;
        }
        continue;
      }
      if (DEVICE_FIELDS.has(field.key)) {
        const normalized = text.trim();
        const current = field.value === null ? "" : String(field.value);
        if (normalized !== current) {
          patch[field.key] = normalized === "" ? null : normalized;
        }
        continue;
      }
      if (text !== String(field.value ?? "")) {
        patch[field.key] = text;
      }
    }
    return patch;
  }

  async function save(): Promise<void> {
    saving = true;
    actionError = null;
    actionMessage = null;
    fieldErrors = {};
    try {
      const patch = buildPatch();
      if (Object.keys(patch).length === 0) {
        actionMessage = "No changes to apply";
        return;
      }
      const validation = await validateSettings(patch);
      if (!validation.ok) {
        const errors: Record<string, string> = {};
        for (const error of validation.errors) {
          errors[error.field] = error.message;
        }
        fieldErrors = errors;
        actionError = validation.errors
          .map((error) =>
            error.field === "config"
              ? error.message
              : `${error.field}: ${error.message}`,
          )
          .join("; ");
        return;
      }
      const result = await applySettings(patch);
      settings.set(result.snapshot);
      actionMessage =
        result.tier === "restart"
          ? "Saved — restarting the daemon…"
          : "Applied without restarting the daemon";
    } catch (error) {
      actionError = error instanceof Error ? error.message : String(error);
    } finally {
      saving = false;
    }
  }

  async function previewVoice(): Promise<void> {
    previewing = true;
    actionError = null;
    actionMessage = null;
    try {
      await runTtsPreview();
      actionMessage = "Voice preview finished";
    } catch (error) {
      actionError = error instanceof Error ? error.message : String(error);
    } finally {
      previewing = false;
    }
  }
</script>

<section aria-label="Settings">
  {#if loading}
    <p class="muted">Loading settings…</p>
  {:else if loadError}
    <p class="error" role="alert">{loadError}</p>
  {:else if $settings}
    {#each CATEGORY_ORDER as category}
      {#if fieldsIn(category).length > 0}
        <h2>{CATEGORY_LABELS[category] ?? category}</h2>
        <div class="fields">
          {#each fieldsIn(category) as field (field.key)}
            <div class="field">
              <div class="label-row">
                <label for={field.key}>{field.label}</label>
                <span class="badges">
                  <span class="badge origin {field.origin}">
                    {originLabel(field.origin)}
                  </span>
                  <span class="badge tier">{tierLabel(field.tier)}</span>
                </span>
              </div>

              {#if BOOL_FIELDS.has(field.key)}
                <input
                  id={field.key}
                  type="checkbox"
                  checked={form[field.key] === true}
                  disabled={!field.editable || saving}
                  onchange={(event) =>
                    setForm(field.key, event.currentTarget.checked)}
                />
              {:else if field.options?.length}
                <select
                  id={field.key}
                  value={settingInputValue(field.value)}
                  disabled={!field.editable || saving}
                  oninput={(event) => setForm(field.key, event.currentTarget.value)}
                >
                  {#each field.options as option (option)}
                    <option value={option}>{option}</option>
                  {/each}
                </select>
              {:else}
                <input
                  id={field.key}
                  type={NUMBER_FIELDS.has(field.key) || PORT_FIELDS.has(field.key)
                    ? "number"
                    : "text"}
                  value={settingInputValue(field.value)}
                  disabled={!field.editable || saving}
                  oninput={(event) => setForm(field.key, event.currentTarget.value)}
                />
              {/if}

              {#if fieldErrors[field.key]}
                <p class="field-error" role="alert">{fieldErrors[field.key]}</p>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    {/each}

    {#if actionError}
      <p class="error" role="alert">{actionError}</p>
    {:else if actionMessage}
      <p class="success">{actionMessage}</p>
    {/if}

    <div class="actions">
      <button type="button" onclick={save} disabled={saving}>
        {saving ? "Applying…" : "Save settings"}
      </button>
      <button
        type="button"
        class="secondary"
        disabled={previewing || saving || $status?.connection !== "connected"}
        onclick={previewVoice}
      >
        {previewing ? "Previewing…" : "Preview voice"}
      </button>
    </div>

    <p class="muted small">Config file: {$settings.config_file ?? "embedded defaults"}</p>
  {/if}
</section>

<style>
  section {
    max-width: 720px;
    padding: 20px;
    border: 1px solid #263141;
    border-radius: 12px;
    background: #171c23;
  }

  h2 {
    margin: 20px 0 10px;
    font-size: 16px;
    color: #d8e2ee;
  }

  h2:first-of-type {
    margin-top: 0;
  }

  .fields {
    display: grid;
    gap: 14px;
  }

  .field {
    display: grid;
    gap: 6px;
  }

  .label-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  label {
    color: #9aa7b5;
    font-size: 13px;
  }

  input,
  select {
    width: 100%;
    padding: 9px 11px;
    border: 1px solid #38475c;
    border-radius: 7px;
    background: #101317;
    color: #e7ecf2;
    font-size: 14px;
    box-sizing: border-box;
  }

  input[type="checkbox"] {
    width: 18px;
    height: 18px;
    padding: 0;
    accent-color: #5a8dd6;
  }

  input:disabled,
  select:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  .badges {
    display: flex;
    gap: 6px;
  }

  .badge {
    padding: 2px 7px;
    border-radius: 999px;
    font-size: 11px;
    white-space: nowrap;
  }

  .badge.origin.default {
    background: #232b35;
    color: #9aa7b5;
  }

  .badge.origin.file {
    background: #223a2c;
    color: #8fd3a8;
  }

  .badge.origin.environment {
    background: #332f22;
    color: #d8c58a;
  }

  .badge.tier {
    background: #202833;
    color: #9aa7b5;
  }

  .field-error {
    margin: 0;
    color: #ff9c9c;
    font-size: 12px;
  }

  .error {
    margin: 16px 0 0;
    color: #ff9c9c;
    font-size: 14px;
  }

  .success {
    margin: 16px 0 0;
    color: #8fd3a8;
    font-size: 14px;
  }

  .muted {
    color: #9aa7b5;
  }

  .small {
    margin-top: 14px;
    font-size: 12px;
  }

  .actions {
    display: flex;
    gap: 10px;
    margin-top: 20px;
  }

  button {
    flex: 1;
    padding: 11px 16px;
    border: 1px solid #38475c;
    border-radius: 8px;
    background: #2d3a4a;
    color: #e7ecf2;
    font-size: 14px;
    cursor: pointer;
  }

  button.secondary {
    flex: 0 0 140px;
    background: #202833;
  }

  button:disabled {
    opacity: 0.6;
    cursor: wait;
  }
</style>
