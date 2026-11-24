<script lang="ts">
  import { pipelineLabel, status } from "../status";
</script>

<footer aria-label="Pipeline status">
  {#if $status?.pipeline}
    <div class="pipeline {$status.pipeline.state}">
      <span class="state">{pipelineLabel($status.pipeline.state)}</span>
      {#if $status.pipeline.pauseReason}
        <span class="meta">pause: {$status.pipeline.pauseReason}</span>
      {/if}
      {#if $status.pipeline.ttsMuted}
        <span class="meta">TTS muted</span>
      {/if}
    </div>
  {:else}
    <div class="pipeline empty">Waiting for pipeline state…</div>
  {/if}
</footer>

<style>
  footer {
    flex: none;
    margin: 16px -32px 0;
    padding: 10px 32px;
    border-top: 1px solid #263141;
    background: #101317f2;
  }

  .pipeline {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 12px;
    border-radius: 8px;
    background: #202833;
    font-size: 15px;
    font-weight: 650;
  }

  .pipeline.empty {
    color: #9aa7b5;
    font-weight: 400;
  }

  .meta {
    color: #9aa7b5;
    font-size: 12px;
    font-weight: 400;
  }

  .pipeline.idle { background: #202833; }
  .pipeline.listening { background: #223a2c; }
  .pipeline.thinking { background: #332f22; }
  .pipeline.speaking { background: #283450; }
  .pipeline.paused { background: #33303f; }
</style>
