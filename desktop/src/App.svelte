<script lang="ts">
  import { onMount } from "svelte";
  import Audio from "./lib/components/Audio.svelte";
  import Models from "./lib/components/Models.svelte";
  import Onboarding from "./lib/components/Onboarding.svelte";
  import Settings from "./lib/components/Settings.svelte";
  import {
    abandonOnboarding,
    beginOnboarding,
    loadOnboardingState,
    type OnboardingStateWire,
  } from "./lib/onboarding";
  import {
    connectionLabel,
    daemonLabel,
    formatReconnect,
    initStatus,
    pipelineLabel,
    restartDaemon,
    status,
  } from "./lib/status";
  import {
    checkForUpdate,
    installUpdate,
    nextUpdateProgress,
    relaunchAfterUpdate,
    shouldCheckUpdates,
    updateProgressLabel,
    type UpdateInfo,
    type UpdateProgressEvent,
    type UpdateProgressState,
  } from "./lib/update";

  type View = "status" | "settings" | "models" | "audio" | "wizard";
  let view = $state<View>("status");
  let restarting = $state(false);
  let restartError = $state<string | null>(null);
  let onboardingState = $state<OnboardingStateWire | null>(null);
  let updateAvailable = $state<UpdateInfo | null>(null);
  let updateProgress = $state<UpdateProgressState | null>(null);
  let updateError = $state<string | null>(null);
  let installingUpdate = $state(false);
  let updateInstalled = $state(false);

  onMount(async () => {
    void initStatus();
    void checkUpdates();
    try {
      onboardingState = await loadOnboardingState();
      if (!onboardingState.completed && onboardingState.in_progress) {
        view = "wizard";
      }
    } catch {
      // The wizard is opt-in when the state store is unavailable.
    }
  });

  async function openWizard(): Promise<void> {
    try {
      onboardingState = await beginOnboarding();
      view = "wizard";
    } catch (error) {
      restartError = error instanceof Error ? error.message : String(error);
    }
  }

  async function closeWizard(): Promise<void> {
    try {
      onboardingState = await abandonOnboarding();
    } catch (error) {
      restartError = error instanceof Error ? error.message : String(error);
    }
    view = "status";
  }

  function completeWizard(): void {
    view = "status";
  }

  async function onRestart(): Promise<void> {
    restarting = true;
    restartError = null;
    try {
      await restartDaemon();
    } catch (error) {
      restartError = error instanceof Error ? error.message : String(error);
    } finally {
      restarting = false;
    }
  }

  async function checkUpdates(): Promise<void> {
    if (!shouldCheckUpdates(import.meta.env.MODE)) return;
    try {
      updateAvailable = await checkForUpdate();
    } catch (error) {
      updateError = error instanceof Error ? error.message : String(error);
    }
  }

  function onUpdateProgress(event: UpdateProgressEvent): void {
    updateProgress = nextUpdateProgress(updateProgress, event);
  }

  async function onInstallUpdate(): Promise<void> {
    installingUpdate = true;
    updateError = null;
    updateProgress = null;
    try {
      await installUpdate(onUpdateProgress);
      updateInstalled = true;
      await relaunchAfterUpdate();
    } catch (error) {
      updateError = error instanceof Error ? error.message : String(error);
    } finally {
      installingUpdate = false;
    }
  }
</script>

<main>
  <header>
    <h1>Seneschal</h1>
    <p class="subtitle">Desktop shell — live pipeline state and settings</p>
    <nav aria-label="App views">
      <button
        type="button"
        class:active={view === "status"}
        onclick={() => (view = "status")}
      >
        Status
      </button>
      <button
        type="button"
        class:active={view === "settings"}
        onclick={() => (view = "settings")}
      >
        Settings
      </button>
      <button
        type="button"
        class:active={view === "models"}
        onclick={() => (view = "models")}
      >
        Models
      </button>
      <button
        type="button"
        class:active={view === "audio"}
        onclick={() => (view = "audio")}
      >
        Audio
      </button>
      {#if onboardingState && !onboardingState.completed}
        <button
          type="button"
          class:active={view === "wizard"}
          onclick={() => (view = "wizard")}
        >
          Setup
        </button>
      {/if}
    </nav>
  </header>

  {#if updateAvailable && !updateProgress}
    <section class="update" aria-label="Update available">
      <div class="update-head">
        <strong>Update {updateAvailable.version} available</strong>
        <button
          type="button"
          class="inline"
          onclick={onInstallUpdate}
          disabled={installingUpdate}
        >
          Install and restart
        </button>
      </div>
      {#if updateAvailable.notes}
        <p class="update-notes">{updateAvailable.notes}</p>
      {/if}
    </section>
  {:else if updateProgress}
    <section class="update" aria-label="Installing update">
      <strong>{updateProgressLabel(updateProgress)}</strong>
    </section>
  {:else if updateInstalled}
    <section class="update" aria-label="Update installed">
      <strong>Update installed. Restarting…</strong>
    </section>
  {/if}

  {#if updateError}
    <p class="error" role="alert">{updateError}</p>
  {/if}

  {#if view === "wizard" && onboardingState}
    <Onboarding
      onComplete={completeWizard}
      onAbandon={closeWizard}
    />
  {:else if view === "settings"}
    <Settings onRunSetup={openWizard} />
  {:else if view === "models"}
    <Models />
  {:else if view === "audio"}
    <Audio />
  {:else if $status}
    <section aria-label="Desktop status">
      <div class="rows">
        <div class="row">
          <span>Daemon</span>
          <strong class="daemon {$status.daemon}">
            {daemonLabel($status.daemon)}
          </strong>
        </div>
        <div class="row">
          <span>Control</span>
          <strong class="connection {$status.connection}">
            {connectionLabel($status.connection)}
          </strong>
        </div>
        <div class="row">
          <span>Last reconnect</span>
          <strong>{formatReconnect($status.lastReconnectMs)}</strong>
        </div>
      </div>

      {#if $status.pipeline}
        <div class="pipeline {$status.pipeline.state}">
          <span class="state">{pipelineLabel($status.pipeline.state)}</span>
          {#if $status.pipeline.utteranceId !== null}
            <span class="meta">utterance #{$status.pipeline.utteranceId}</span>
          {/if}
          {#if $status.pipeline.pauseReason}
            <span class="meta">pause: {$status.pipeline.pauseReason}</span>
          {/if}
          {#if $status.pipeline.ttsMuted}
            <span class="meta">TTS muted</span>
          {/if}
        </div>
      {:else}
        <div class="pipeline empty">Waiting for PipelineState…</div>
      {/if}

      {#if $status.lastError || restartError}
        <p class="error" role="alert">
          {restartError ?? $status.lastError}
        </p>
      {/if}

      <button
        type="button"
        onclick={onRestart}
        disabled={restarting || $status.daemon === "restarting"}
      >
        {restarting || $status.daemon === "restarting"
          ? "Restarting…"
          : "Restart daemon"}
      </button>
    </section>
  {:else}
    <section aria-label="Desktop status">
      <p>Loading desktop status…</p>
    </section>
  {/if}
</main>

<style>
  main {
    min-height: 100vh;
    margin: 0;
    padding: 32px;
    box-sizing: border-box;
    background: #101317;
    color: #e7ecf2;
    font-family: system-ui, sans-serif;
  }

  h1 {
    margin: 0;
    font-size: 28px;
    letter-spacing: 0.03em;
  }

  .subtitle {
    margin: 4px 0 16px;
    color: #9aa7b5;
  }

  .update {
    max-width: 560px;
    margin-bottom: 20px;
    padding: 16px 20px;
    border: 1px solid #38475c;
    border-radius: 12px;
    background: #202b38;
  }

  .update-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }

  .update-head button.inline {
    width: auto;
    margin: 0;
    padding: 8px 12px;
  }

  .update-notes {
    margin: 10px 0 0;
    color: #c8d2dd;
    font-size: 14px;
    white-space: pre-wrap;
  }

  nav {
    display: flex;
    gap: 8px;
    margin-bottom: 24px;
  }

  nav button {
    width: auto;
    margin: 0;
    padding: 8px 14px;
    background: #171c23;
  }

  nav button.active {
    background: #2d3a4a;
    border-color: #5a8dd6;
  }

  section {
    max-width: 560px;
    padding: 20px;
    border: 1px solid #263141;
    border-radius: 12px;
    background: #171c23;
  }

  .rows {
    display: grid;
    gap: 10px;
    margin-bottom: 20px;
  }

  .row {
    display: flex;
    justify-content: space-between;
    gap: 16px;
  }

  .row span {
    color: #9aa7b5;
  }

  .pipeline {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 20px;
    border-radius: 10px;
    background: #202833;
    font-size: 24px;
    font-weight: 650;
  }

  .pipeline.empty {
    color: #9aa7b5;
    font-size: 16px;
    font-weight: 400;
  }

  .meta {
    color: #9aa7b5;
    font-size: 13px;
    font-weight: 400;
  }

  .pipeline.idle { background: #202833; }
  .pipeline.listening { background: #223a2c; }
  .pipeline.thinking { background: #332f22; }
  .pipeline.speaking { background: #283450; }
  .pipeline.paused { background: #33303f; }

  .error {
    margin: 16px 0 0;
    color: #ff9c9c;
    font-size: 14px;
  }

  button {
    width: 100%;
    margin-top: 20px;
    padding: 12px 16px;
    border: 1px solid #38475c;
    border-radius: 8px;
    background: #2d3a4a;
    color: #e7ecf2;
    font-size: 15px;
    cursor: pointer;
  }

  button:disabled {
    opacity: 0.6;
    cursor: wait;
  }
</style>
