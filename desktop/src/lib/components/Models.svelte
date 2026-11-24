<script lang="ts">
  import { onMount } from "svelte";
  import {
    activateModel,
    cancelModelDownload,
    containerStateLabel,
    deleteModel,
    downloadModel,
    downloadNemoWeights,
    effectiveProgress,
    formatBytes,
    initModels,
    modelCatalog,
    modelProgress,
    modelStateLabel,
    nemoServices,
    percentOf,
    phaseLabel,
    purposeLabel,
    refreshModels,
    refreshNemo,
    resetModelProgressForDisconnect,
    startNemoService,
    stopNemoService,
    weightsStateLabel,
    type ModelEntry,
    type NemoService,
  } from "../models";
  import { status } from "../status";

  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let actionError = $state<string | null>(null);
  let actionMessage = $state<string | null>(null);
  let busy = $state<string | null>(null);
  let confirmingDelete = $state<string | null>(null);
  let lastConnection: string | null = null;

  onMount(async () => {
    try {
      await initModels();
    } catch (error) {
      loadError = error instanceof Error ? error.message : String(error);
    } finally {
      loading = false;
    }
  });

  $effect(() => {
    const connection = $status?.connection ?? null;
    if (connection === lastConnection) return;
    lastConnection = connection;
    if (connection === "connected") {
      // Reconnected (e.g. after a model activation restart): resync both.
      void refreshModels().catch(() => undefined);
      void refreshNemo().catch(() => undefined);
    } else if (connection !== null) {
      resetModelProgressForDisconnect();
    }
  });

  async function run(
    id: string,
    action: () => Promise<unknown>,
    message: string,
  ): Promise<void> {
    busy = id;
    actionError = null;
    actionMessage = null;
    try {
      await action();
      actionMessage = message;
    } catch (error) {
      actionError = error instanceof Error ? error.message : String(error);
    } finally {
      busy = null;
    }
  }

  function onDownload(entry: ModelEntry): Promise<void> {
    return run(entry.id, () => downloadModel(entry.id), `Download started for ${entry.label}`);
  }

  function onCancel(entry: ModelEntry): Promise<void> {
    return run(entry.id, () => cancelModelDownload(entry.id), `Download cancelled for ${entry.label}`);
  }

  function onActivate(entry: ModelEntry): Promise<void> {
    return run(entry.id, () => activateModel(entry.id), `${entry.label} activated — daemon restarted`);
  }

  function onDelete(entry: ModelEntry): Promise<void> {
    confirmingDelete = null;
    return run(entry.id, () => deleteModel(entry.id), `${entry.label} deleted`);
  }

  function onNemoStart(service: NemoService): Promise<void> {
    return run(service.id, () => startNemoService(service.id), `${service.label} started`);
  }

  function onNemoStop(service: NemoService): Promise<void> {
    return run(service.id, () => stopNemoService(service.id), `${service.label} stopped`);
  }

  function onNemoWeights(service: NemoService): Promise<void> {
    return run(service.id, () => downloadNemoWeights(service.id), `${service.label} weight download started`);
  }

  function modelBusy(entry: ModelEntry): boolean {
    return busy === entry.id;
  }

  function canDownload(entry: ModelEntry): boolean {
    return (
      entry.state !== "installed" &&
      entry.state !== "downloading" &&
      busy !== entry.id
    );
  }
</script>

<section aria-label="Model manager">
  {#if loading}
    <p class="muted">Loading models…</p>
  {:else if loadError}
    <p class="error" role="alert">{loadError}</p>
  {:else if $modelCatalog}
    <p class="muted small">
      Models root: <code>{$modelCatalog.root}</code>
    </p>

    {#each $modelCatalog.models as entry (entry.id)}
      {@const progress = effectiveProgress(entry, $modelProgress)}
      <article class="card" class:active={entry.active}>
        <header>
          <div class="title">
            <strong>{entry.label}</strong>
            <span class="badge purpose">{purposeLabel(entry.purpose)}</span>
            {#if entry.active}
              <span class="badge active">Active</span>
            {/if}
          </div>
          <span class="meta">
            {formatBytes(entry.total_bytes)} · {modelStateLabel(entry.state)}
          </span>
        </header>

        {#if progress}
          <div class="progress-row">
            <div class="progress">
              <div
                class="bar {progress.phase}"
                style="width: {percentOf(progress.downloaded, progress.total)}%"
              ></div>
            </div>
            <span class="meta">
              {phaseLabel(progress.phase)}
              {#if progress.total}
                · {percentOf(progress.downloaded, progress.total)}%
                · {formatBytes(progress.downloaded)} / {formatBytes(progress.total)}
              {:else}
                · {formatBytes(progress.downloaded)}
              {/if}
            </span>
          </div>
          {#if progress.message}
            <p class="meta">{progress.message}</p>
          {/if}
        {/if}

        <ul class="files">
          {#each entry.files as file (file.path)}
            <li>
              <code>{file.path}</code>
              <span class="meta">
                {formatBytes(file.size_bytes)}
                {#if file.present}
                  {#if file.legacy}
                    · present (legacy)
                  {:else if file.verified === true}
                    · verified
                  {:else}
                    · present
                  {/if}
                {:else}
                  · missing
                {/if}
              </span>
            </li>
          {/each}
        </ul>

        {#if entry.requires.length > 0}
          <p class="meta">Requires: {entry.requires.join(", ")}</p>
        {/if}

        <div class="actions">
          {#if entry.state === "downloading"}
            <button
              type="button"
              disabled={modelBusy(entry)}
              onclick={() => onCancel(entry)}
            >
              Cancel download
            </button>
          {:else if canDownload(entry)}
            <button
              type="button"
              disabled={modelBusy(entry)}
              onclick={() => onDownload(entry)}
            >
              Download
            </button>
          {/if}
          {#if entry.state === "installed" && !entry.active}
            <button
              type="button"
              disabled={modelBusy(entry)}
              onclick={() => onActivate(entry)}
            >
              Use model
            </button>
          {/if}
          {#if !entry.active && entry.state !== "downloading"}
            {#if confirmingDelete === entry.id}
              <button
                type="button"
                class="danger"
                disabled={modelBusy(entry)}
                onclick={() => onDelete(entry)}
              >
                Confirm delete
              </button>
              <button
                type="button"
                class="secondary"
                onclick={() => (confirmingDelete = null)}
              >
                Keep
              </button>
            {:else}
              <button
                type="button"
                class="secondary"
                disabled={modelBusy(entry)}
                onclick={() => (confirmingDelete = entry.id)}
              >
                Delete
              </button>
            {/if}
          {/if}
        </div>
      </article>
    {/each}

    <h2>NeMo services</h2>
    {#if $nemoServices}
      {#each $nemoServices as service (service.id)}
        <article class="card" class:active={service.active}>
          <header>
            <div class="title">
              <strong>{service.label}</strong>
              {#if service.active}
                <span class="badge active">Active</span>
              {/if}
            </div>
            <span class="meta">
              {containerStateLabel(service.container_state)} · weights:
              {weightsStateLabel(service.weights_state)}
            </span>
          </header>

          <p class="meta">
            {service.base_url}
            {#if service.expected_model}
              · expects <code>{service.expected_model}</code>
              {#if service.model_matches === false}
                (mismatch)
              {:else if service.model_matches === true}
                (match)
              {/if}
            {/if}
          </p>
          {#if service.hint}
            <p class="meta hint">{service.hint}</p>
          {/if}

          <ul class="files">
            {#each service.weights as weight (weight.name)}
              <li>
                <code>{weight.name}</code>
                <span class="meta">{weight.present ? "present" : "missing"}</span>
              </li>
            {/each}
          </ul>

          <div class="actions">
            <button
              type="button"
              disabled={busy === service.id || service.container_state === "docker_unavailable"}
              onclick={() => onNemoStart(service)}
            >
              Start
            </button>
            <button
              type="button"
              disabled={busy === service.id || service.container_state === "docker_unavailable"}
              onclick={() => onNemoStop(service)}
            >
              Stop
            </button>
            <button
              type="button"
              class="secondary"
              disabled={busy === service.id || service.container_state === "docker_unavailable"}
              onclick={() => onNemoWeights(service)}
            >
              Download weights
            </button>
          </div>
        </article>
      {/each}
    {:else}
      <p class="muted">Service status unavailable.</p>
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
    max-width: 720px;
    padding: 20px;
    border: 1px solid #263141;
    border-radius: 12px;
    background: #171c23;
  }

  h2 {
    margin: 24px 0 10px;
    font-size: 16px;
    color: #d8e2ee;
  }

  .card {
    padding: 14px;
    margin-bottom: 12px;
    border: 1px solid #263141;
    border-radius: 10px;
    background: #101317;
  }

  .card.active {
    border-color: #3f6f4a;
  }

  header {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 12px;
  }

  .title {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .badge {
    padding: 2px 7px;
    border-radius: 999px;
    font-size: 11px;
    white-space: nowrap;
  }

  .badge.purpose {
    background: #202833;
    color: #9aa7b5;
  }

  .badge.active {
    background: #223a2c;
    color: #8fd3a8;
  }

  .meta {
    color: #9aa7b5;
    font-size: 12px;
  }

  .hint {
    color: #d8c58a;
  }

  .progress-row {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 8px;
  }

  .progress {
    flex: 1;
    height: 8px;
    border-radius: 999px;
    background: #202833;
    overflow: hidden;
  }

  .bar {
    height: 100%;
    background: #5a8dd6;
    transition: width 0.2s ease;
  }

  .bar.completed {
    background: #3f8f5c;
  }

  .bar.failed,
  .bar.cancelled {
    background: #b36a6a;
  }

  .files {
    list-style: none;
    margin: 10px 0 0;
    padding: 0;
    display: grid;
    gap: 4px;
    font-size: 12px;
  }

  code {
    color: #c9d6e4;
    font-size: 12px;
  }

  .actions {
    display: flex;
    gap: 8px;
    margin-top: 12px;
    flex-wrap: wrap;
  }

  button {
    padding: 8px 12px;
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

  button.danger {
    background: #4a2d2d;
    border-color: #7c4a4a;
  }

  button:disabled {
    opacity: 0.55;
    cursor: wait;
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
    margin-bottom: 14px;
    font-size: 12px;
  }
</style>
