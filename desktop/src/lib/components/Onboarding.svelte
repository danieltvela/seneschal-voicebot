<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import Audio from "./Audio.svelte";
  import {
    activateModel,
    downloadModel,
    downloadNemoWeights,
    effectiveProgress,
    formatBytes,
    initModels,
    modelCatalog,
    modelProgress,
    nemoServices,
    phaseLabel,
    refreshModels,
    refreshNemo,
    startNemoService,
    type ModelEntry,
    type NemoService,
  } from "../models";
  import {
    ONBOARDING_STEPS,
    STEP_LABELS,
    abandonOnboarding,
    buildWizardPatch,
    completeOnboarding,
    defaultBackends,
    defaultLanguage,
    defaultModelIds,
    didSkipStep,
    loadOnboardingState,
    localBackend,
    missingModels,
    nemoReady,
    recordOnboardingStep,
    relevantModels,
    requiredModelClosure,
    setOnboardingGreeting,
    validationMessage,
    type OnboardingDefaultsWire,
    type OnboardingStateWire,
    type OnboardingStepId,
    type WizardDraft,
  } from "../onboarding";
  import { loadSettings, settings, validateSettings, applySettings } from "../settings";
  import { restartDaemon } from "../status";

  let {
    onComplete,
    onAbandon,
  }: {
    onComplete: () => void;
    onAbandon: () => void;
  } = $props();

  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let actionError = $state<string | null>(null);
  let actionMessage = $state<string | null>(null);
  let busy = $state(false);
  let downloading = $state<string | null>(null);
  let confirmingGreeting = $state(false);

  let onboardingState = $state<OnboardingStateWire | null>(null);
  let defaults = $state<OnboardingDefaultsWire | null>(null);
  let step = $state<OnboardingStepId>("language");
  let draft = $state<WizardDraft | null>(null);
  let modelIds = $state<string[]>([]);

  const selection = $derived(
    draft ? { sttProvider: draft.sttProvider, ttsProvider: draft.ttsProvider } : null,
  );
  const models = $derived(relevantModels($modelCatalog, selection ?? { sttProvider: "", ttsProvider: "" }));
  const missing = $derived(missingModels($modelCatalog, modelIds));
  const nemoSelected = $derived(
    selection?.sttProvider === "nemotron" && selection?.ttsProvider === "openai",
  );
  const nemoOk = $derived(nemoReady($nemoServices ?? []));
  const localModelsReady = $derived(
    !localBackend(selection ?? { sttProvider: "", ttsProvider: "" }) || missing.length === 0,
  );
  const modelsReady = $derived(localModelsReady && (!nemoSelected || nemoOk));
  const selectedSttModel = $derived(
    modelIds.some((id) => $modelCatalog?.models.find((model) => model.id === id)?.backend === "whisper" ||
      $modelCatalog?.models.find((model) => model.id === id)?.backend === "parakeet"),
  );
  const selectedTtsModel = $derived(
    modelIds.some((id) => $modelCatalog?.models.find((model) => model.id === id)?.backend === "kokoro"),
  );
  const modelsValid = $derived(
    !localBackend(selection ?? { sttProvider: "", ttsProvider: "" }) ||
      ((selection?.sttProvider === "whisper" || selection?.sttProvider === "parakeet"
        ? selectedSttModel
        : true) &&
        (selection?.ttsProvider === "kokoro" ? selectedTtsModel : true)),
  );

  onMount(async () => {
    try {
      const [state, loadedDefaults] = await Promise.all([
        loadOnboardingState(),
        invokeDefaults(),
      ]);
      onboardingState = state;
      defaults = loadedDefaults;
      await Promise.all([loadSettings(), initModels().catch(() => undefined)]);
      if (!$settings) throw new Error("Settings snapshot unavailable");
      const backendSelection = defaultBackends($settings, loadedDefaults);
      draft = {
        language: defaultLanguage($settings),
        llmMode: String($settings.fields.find((field) => field.key === "llm_url")?.value ?? "").includes("api.openai.com")
          ? "openai"
          : "local",
        llmUrl: String($settings.fields.find((field) => field.key === "llm_url")?.value ?? ""),
        llmModel: String($settings.fields.find((field) => field.key === "llm_model")?.value ?? ""),
        llmApiKey: "",
        clearApiKey: false,
        sttProvider: backendSelection.sttProvider,
        ttsProvider: backendSelection.ttsProvider,
      };
      modelIds = defaultModelIds($modelCatalog, backendSelection);
      const storedStep = ONBOARDING_STEPS.find((candidate) => candidate === state.current_step);
      step = storedStep ?? "language";
    } catch (error) {
      loadError = error instanceof Error ? error.message : String(error);
    } finally {
      loading = false;
    }
  });

  async function invokeDefaults(): Promise<OnboardingDefaultsWire> {
    return invoke<OnboardingDefaultsWire>("get_onboarding_defaults");
  }

  async function goTo(next: OnboardingStepId, skipped = false): Promise<void> {
    if (!onboardingState) return;
    busy = true;
    actionError = null;
    try {
      onboardingState = await recordOnboardingStep(step, skipped);
      step = next;
    } catch (error) {
      actionError = error instanceof Error ? error.message : String(error);
    } finally {
      busy = false;
    }
  }

  async function back(): Promise<void> {
    const index = ONBOARDING_STEPS.indexOf(step);
    if (index <= 0) return;
    await goTo(ONBOARDING_STEPS[index - 1], false);
  }

  async function downloadOne(entry: ModelEntry): Promise<void> {
    downloading = entry.id;
    actionError = null;
    try {
      await downloadModel(entry.id);
      await waitForModel(entry.id);
    } catch (error) {
      actionError = error instanceof Error ? error.message : String(error);
    } finally {
      downloading = null;
    }
  }

  async function downloadAllMissing(): Promise<void> {
    downloading = "all";
    actionError = null;
    try {
      for (const id of requiredModelClosure($modelCatalog, modelIds)) {
        const entry = $modelCatalog?.models.find((candidate) => candidate.id === id);
        if (!entry || (entry.state !== "missing" && entry.state !== "partial")) continue;
        await downloadModel(id);
        await waitForModel(id);
      }
      await refreshModels();
    } catch (error) {
      actionError = error instanceof Error ? error.message : String(error);
    } finally {
      downloading = null;
    }
  }

  async function waitForModel(id: string): Promise<void> {
    for (let attempt = 0; attempt < 180; attempt += 1) {
      await refreshModels();
      const entry = $modelCatalog?.models.find((candidate) => candidate.id === id);
      if (entry?.state === "installed") return;
      if (entry?.state === "missing" && attempt > 0) return;
      await new Promise((resolve) => setTimeout(resolve, 500));
    }
  }

  function chooseModel(entry: ModelEntry, checked: boolean): void {
    if (!draft || entry.backend === "silero") return;
    const purpose = entry.purpose;
    let next = modelIds.filter((id) => {
      const candidate = $modelCatalog?.models.find((model) => model.id === id);
      return candidate?.purpose !== purpose;
    });
    if (checked) next.push(entry.id);
    modelIds = requiredModelClosure($modelCatalog, next);
  }

  function isModelChecked(entry: ModelEntry): boolean {
    return modelIds.includes(entry.id);
  }

  async function nemoAction(
    service: NemoService,
    action: "start" | "weights",
  ): Promise<void> {
    downloading = service.id;
    actionError = null;
    try {
      if (action === "start") await startNemoService(service.id);
      else await downloadNemoWeights(service.id);
      await refreshNemo();
    } catch (error) {
      actionError = error instanceof Error ? error.message : String(error);
    } finally {
      downloading = null;
    }
  }

  function llmInputValid(): boolean {
    if (!draft) return false;
    return draft.llmUrl.trim().length > 0 && draft.llmModel.trim().length > 0;
  }

  async function finish(): Promise<void> {
    if (!draft || !onboardingState) return;
    busy = true;
    actionError = null;
    actionMessage = null;
    try {
      // Keep suppression on through config apply and model activation restarts.
      await setOnboardingGreeting(true);
      const patch = buildWizardPatch(draft, $settings!);
      if (Object.keys(patch).length > 0) {
        const validation = await validateSettings(patch);
        if (!validation.ok) {
          actionError = validationMessage(validation.errors);
          await setOnboardingGreeting(true);
          return;
        }
        const response = await applySettings(patch);
        settings.set(response.snapshot);
      }
      // Apply provider/model selections through the existing model activation
      // path so managed paths and VAD dependencies stay consistent.
      for (const id of modelIds) {
        const entry = $modelCatalog?.models.find((candidate) => candidate.id === id);
        if (!entry || entry.backend === "silero" || entry.state !== "installed") continue;
        await activateModel(id);
        await refreshModels();
      }
      // applySettings may already have restarted while suppressed; now switch
      // to the final daemon that starts with the normal greeting.
      await setOnboardingGreeting(false);
      await restartDaemon();
      await Promise.all([loadSettings(), refreshModels(), refreshNemo().catch(() => undefined)]);
      onboardingState = await recordOnboardingStep("finish", false);
      confirmingGreeting = true;
    } catch (error) {
      actionError = error instanceof Error ? error.message : String(error);
      await setOnboardingGreeting(true).catch(() => undefined);
    } finally {
      busy = false;
    }
  }

  async function confirmGreeting(heard: boolean): Promise<void> {
    if (!heard) {
      actionError =
        "No greeting was heard. Check the assistant, voice engines and audio steps, then finish again.";
      confirmingGreeting = false;
      return;
    }
    busy = true;
    actionError = null;
    try {
      onboardingState = await completeOnboarding();
      onComplete();
    } catch (error) {
      actionError = error instanceof Error ? error.message : String(error);
    } finally {
      busy = false;
    }
  }

  async function abandon(): Promise<void> {
    busy = true;
    try {
      onboardingState = await abandonOnboarding();
      onAbandon();
    } catch (error) {
      actionError = error instanceof Error ? error.message : String(error);
    } finally {
      busy = false;
    }
  }

  const canContinue = $derived(
    !busy &&
      (step !== "llm" || llmInputValid()) &&
      (step !== "models" || modelsReady) &&
      (step !== "models" || modelsValid),
  );
</script>

<section aria-label="First-run setup">
  {#if loading}
    <p class="muted">Preparing setup…</p>
  {:else if loadError}
    <p class="error" role="alert">{loadError}</p>
  {:else if draft && onboardingState}
    <header class="wizard-header">
      <div>
        <h2>Set up Seneschal</h2>
        <p class="muted small">
          {defaults?.platform ?? "desktop"}
          {#if defaults?.gpu_available}· GPU detected{/if}
        </p>
      </div>
      <button type="button" class="secondary" onclick={abandon} disabled={busy}>
        Cancel
      </button>
    </header>

    <ol class="steps" aria-label="Setup steps">
      {#each ONBOARDING_STEPS as candidate, index (candidate)}
        <li class:active={candidate === step} class:done={index < ONBOARDING_STEPS.indexOf(step)}>
          {STEP_LABELS[candidate]}
          {#if didSkipStep(onboardingState, candidate)}
            <span class="badge">skipped</span>
          {/if}
        </li>
      {/each}
    </ol>

    {#if step === "language"}
      <div class="field">
        <label for="wizard-language">Language</label>
        <select id="wizard-language" bind:value={draft.language}>
          <option value="es">Español</option>
          <option value="en">English</option>
        </select>
      </div>
      <div class="nav">
        <button type="button" class="secondary" onclick={() => goTo("llm", true)} disabled={busy}>Skip</button>
        <button type="button" onclick={() => goTo("llm")} disabled={busy}>Continue</button>
      </div>
    {:else if step === "llm"}
      <fieldset class="mode">
        <legend>Assistant endpoint</legend>
        <label>
          <input
            type="radio"
            name="llm-mode"
            value="local"
            bind:group={draft.llmMode}
          />
          Local server
        </label>
        <label>
          <input
            type="radio"
            name="llm-mode"
            value="openai"
            bind:group={draft.llmMode}
          />
          OpenAI-compatible API
        </label>
      </fieldset>
      <div class="field">
        <label for="wizard-llm-url">URL</label>
        <input id="wizard-llm-url" type="text" bind:value={draft.llmUrl} placeholder="http://127.0.0.1:8000" />
      </div>
      <div class="field">
        <label for="wizard-llm-model">Model</label>
        <input id="wizard-llm-model" type="text" bind:value={draft.llmModel} />
      </div>
      <div class="field">
        <label for="wizard-llm-key">API key</label>
        <input id="wizard-llm-key" type="password" bind:value={draft.llmApiKey} autocomplete="off" />
        <p class="muted small">Leave blank to keep an existing key.</p>
        <label class="inline">
          <input type="checkbox" bind:checked={draft.clearApiKey} />
          Remove saved key
        </label>
      </div>
      <div class="nav">
        <button type="button" class="secondary" onclick={back} disabled={busy}>Back</button>
        <button type="button" onclick={() => goTo("backends")} disabled={!canContinue}>Continue</button>
      </div>
    {:else if step === "backends" && selection}
      <div class="field">
        <label for="wizard-stt">Speech recognition</label>
        <select id="wizard-stt" bind:value={draft.sttProvider}>
          {#each $settings?.fields.find((field) => field.key === "stt_provider")?.options ?? [] as option (option)}
            <option value={option}>{option}</option>
          {/each}
        </select>
      </div>
      <div class="field">
        <label for="wizard-tts">Speech synthesis</label>
        <select id="wizard-tts" bind:value={draft.ttsProvider}>
          {#each $settings?.fields.find((field) => field.key === "tts_provider")?.options ?? [] as option (option)}
            <option value={option}>{option}</option>
          {/each}
        </select>
      </div>
      {#if defaults}
        <p class="muted small">
          Recommended here: {defaults.recommended_stt} + {defaults.recommended_tts}
        </p>
      {/if}
      <div class="nav">
        <button type="button" class="secondary" onclick={back} disabled={busy}>Back</button>
        <button type="button" onclick={() => goTo("models")} disabled={busy}>Continue</button>
      </div>
    {:else if step === "models" && selection}
      {#if localBackend(selection)}
        <div class="model-grid">
          {#each models as entry (entry.id)}
            {@const progress = effectiveProgress(entry, $modelProgress)}
            <article class="card">
              <label class="model-title">
                <input
                  type="checkbox"
                  checked={isModelChecked(entry)}
                  disabled={entry.backend === "silero" || downloading !== null}
                  onchange={(event) => chooseModel(entry, event.currentTarget.checked)}
                />
                <span>{entry.label}</span>
              </label>
              <p class="muted small">
                {formatBytes(entry.total_bytes)} · {entry.state}
                {#if progress}
                  · {phaseLabel(progress.phase)}
                  {#if progress.total}
                    ({Math.round((progress.downloaded / progress.total) * 100)}%)
                  {/if}
                {/if}
              </p>
              {#if entry.state === "missing" || entry.state === "partial"}
                <button
                  type="button"
                  onclick={() => downloadOne(entry)}
                  disabled={downloading !== null}
                >
                  Download
                </button>
              {/if}
            </article>
          {/each}
        </div>
        <div class="actions">
          <button type="button" onclick={downloadAllMissing} disabled={downloading !== null || missing.length === 0}>
            {downloading === "all" ? "Downloading…" : "Download required"}
          </button>
        </div>
      {:else if nemoSelected}
        {#each $nemoServices ?? [] as service (service.id)}
          <article class="card">
            <strong>{service.label}</strong>
            <p class="muted small">
              {service.container_state} · weights: {service.weights_state}
              {#if service.model_matches === false}· model mismatch{/if}
            </p>
            <div class="actions">
              <button type="button" onclick={() => nemoAction(service, "start")} disabled={downloading !== null}>
                Start
              </button>
              <button type="button" onclick={() => nemoAction(service, "weights")} disabled={downloading !== null}>
                Download weights
              </button>
            </div>
          </article>
        {/each}
        {#if !nemoOk}
          <p class="hint">Start both NeMo services before continuing, or choose local engines.</p>
        {/if}
      {:else}
        <p class="muted">No downloads are required for the selected native engines.</p>
      {/if}
      <div class="nav">
        <button type="button" class="secondary" onclick={back} disabled={busy}>Back</button>
        <button type="button" onclick={() => goTo("audio")} disabled={!canContinue}>Continue</button>
      </div>
    {:else if step === "audio"}
      <Audio />
      <div class="nav">
        <button type="button" class="secondary" onclick={back} disabled={busy}>Back</button>
        <button type="button" onclick={() => goTo("finish", true)} disabled={busy}>Skip</button>
        <button type="button" onclick={() => goTo("finish")} disabled={busy}>Continue</button>
      </div>
    {:else if step === "finish"}
      <div class="summary">
        <p><strong>Language:</strong> {draft.language}</p>
        <p><strong>Assistant:</strong> {draft.llmUrl} · {draft.llmModel}</p>
        <p><strong>Voice:</strong> {selection?.sttProvider} + {selection?.ttsProvider}</p>
        <p><strong>Skipped:</strong> {onboardingState.skipped_steps.join(", ") || "none"}</p>
      </div>
      {#if confirmingGreeting}
        <p>Did you hear Seneschal greet you?</p>
        <div class="nav">
          <button type="button" class="secondary" onclick={() => confirmGreeting(false)} disabled={busy}>No</button>
          <button type="button" onclick={() => confirmGreeting(true)} disabled={busy}>Yes</button>
        </div>
      {:else}
        <div class="nav">
          <button type="button" class="secondary" onclick={back} disabled={busy}>Back</button>
          <button type="button" onclick={finish} disabled={busy}>
            {busy ? "Starting…" : "Start Seneschal"}
          </button>
        </div>
      {/if}
    {/if}

    {#if actionError}
      <p class="error" role="alert">{actionError}</p>
    {:else if actionMessage}
      <p class="success">{actionMessage}</p>
    {/if}
  {/if}
</section>

<style>
  section {
    max-width: 760px;
    padding: 20px;
    border: 1px solid #263141;
    border-radius: 12px;
    background: #171c23;
  }

  .wizard-header {
    display: flex;
    justify-content: space-between;
    align-items: start;
    gap: 12px;
  }

  h2 {
    margin: 0 0 4px;
    font-size: 20px;
    color: #d8e2ee;
  }

  .steps {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    padding: 0;
    margin: 16px 0;
    list-style: none;
    font-size: 12px;
    color: #9aa7b5;
  }

  .steps li {
    padding: 5px 9px;
    border: 1px solid #263141;
    border-radius: 999px;
  }

  .steps li.active {
    color: #e7ecf2;
    border-color: #5a8dd6;
  }

  .steps li.done {
    color: #8fd3a8;
  }

  .badge {
    margin-left: 5px;
    color: #d8c58a;
  }

  .field {
    display: grid;
    gap: 6px;
    margin-bottom: 14px;
  }

  label {
    color: #9aa7b5;
    font-size: 13px;
  }

  label.inline {
    display: flex;
    align-items: center;
    gap: 6px;
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

  input[type="radio"],
  input[type="checkbox"] {
    width: 16px;
    height: 16px;
    accent-color: #5a8dd6;
  }

  .mode {
    display: grid;
    gap: 8px;
    margin-bottom: 14px;
    border: 0;
    padding: 0;
  }

  .mode label {
    display: flex;
    align-items: center;
    gap: 8px;
    color: #e7ecf2;
  }

  .model-grid {
    display: grid;
    gap: 10px;
  }

  .card {
    padding: 12px;
    border: 1px solid #263141;
    border-radius: 10px;
    background: #101317;
  }

  .model-title {
    display: flex;
    align-items: center;
    gap: 8px;
    color: #e7ecf2;
  }

  .summary {
    display: grid;
    gap: 8px;
    font-size: 14px;
  }

  .nav,
  .actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    margin-top: 16px;
  }

  button {
    padding: 9px 13px;
    border: 1px solid #38475c;
    border-radius: 7px;
    background: #2d3a4a;
    color: #e7ecf2;
    font-size: 13px;
    cursor: pointer;
  }

  button.secondary {
    background: #202833;
  }

  button:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  .muted {
    color: #9aa7b5;
  }

  .small {
    font-size: 12px;
  }

  .hint {
    color: #d8c58a;
    font-size: 13px;
  }

  .error {
    margin-top: 14px;
    color: #ff9c9c;
    font-size: 13px;
  }

  .success {
    margin-top: 14px;
    color: #8fd3a8;
    font-size: 13px;
  }
</style>
