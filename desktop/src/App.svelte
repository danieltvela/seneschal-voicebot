<script lang="ts">
  import { onMount } from "svelte";
  import {
    connectionLabel,
    daemonLabel,
    formatReconnect,
    initStatus,
    pipelineLabel,
    restartDaemon,
    status,
  } from "./lib/status";

  let restarting = $state(false);
  let restartError = $state<string | null>(null);

  onMount(() => {
    void initStatus();
  });

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
</script>

<main>
  <header>
    <h1>Seneschal</h1>
    <p class="subtitle">Desktop spike — live PipelineState over Control SSE</p>
  </header>

  {#if $status}
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
    margin: 4px 0 28px;
    color: #9aa7b5;
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
