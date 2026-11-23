<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import {
    applyDeviceSettings,
    audioDevices,
    audioLevel,
    audioStatus,
    audioTests,
    deviceFormValue,
    diagnosticHint,
    formatDbfs,
    initAudio,
    meterPercent,
    peakHold,
    refreshAudio,
    runLoopbackTest,
    runMicTest,
    runSpeakerTest,
    runTtsPreview,
    resetAudioForDisconnect,
    type AudioTestKind,
  } from "../audio";
  import { loadSettings, settings, type SettingsField } from "../settings";
  import { status } from "../status";

  type ConfirmKind = "speaker_test" | "loopback";

  let selectedInput = $state<string | null>(null);
  let selectedOutput = $state<string | null>(null);
  let applying = $state(false);
  let applyError = $state<string | null>(null);
  let applyMessage = $state<string | null>(null);
  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let confirmTest = $state<ConfirmKind | null>(null);
  let displayedPeak = $state(0);

  onMount(async () => {
    try {
      await Promise.all([initAudio(), loadSettings()]);
    } catch (error) {
      loadError = error instanceof Error ? error.message : String(error);
    } finally {
      loading = false;
    }
  });

  $effect(() => {
    const snapshot = $settings;
    if (!snapshot || applying) return;
    selectedInput = deviceFormValue(
      snapshot.fields.find((field) => field.key === "audio_input_device")?.value as string | null,
    );
    selectedOutput = deviceFormValue(
      snapshot.fields.find((field) => field.key === "audio_output_device")?.value as string | null,
    );
  });

  $effect(() => {
    const connection = $status?.connection;
    if (connection !== "connected") {
      resetAudioForDisconnect();
    }
  });

  $effect(() => {
    const level = $audioLevel;
    if (!level) {
      untrack(() => (displayedPeak = 0));
      return;
    }
    untrack(() => {
      displayedPeak = peakHold(displayedPeak, level.peak);
    });
  });

  function fieldFor(key: string): SettingsField | undefined {
    return $settings?.fields.find((field) => field.key === key);
  }

  function activeDeviceLabel(selector: string | null | undefined): string {
    return selector ? selector : "System default";
  }

  const anyTestRunning = $derived(
    Object.values($audioTests).some((test) => test.state === "running") || applying,
  );
  const busy = $derived($status?.connection !== "connected" || anyTestRunning);

  async function applyDevices(): Promise<void> {
    applying = true;
    applyError = null;
    applyMessage = null;
    try {
      const response = await applyDeviceSettings(selectedInput, selectedOutput);
      applyMessage = response
        ? response.tier === "restart"
          ? "Devices applied — daemon is ready for tests"
          : "Devices applied"
        : "No device changes to apply";
    } catch (error) {
      applyError = error instanceof Error ? error.message : String(error);
    } finally {
      applying = false;
    }
  }

  async function enableMicrophone(): Promise<void> {
    try {
      await invoke("request_audio_permission");
      await refreshAudio();
    } catch (error) {
      applyError = error instanceof Error ? error.message : String(error);
    }
  }

  function testLabel(kind: AudioTestKind): string {
    const labels: Record<AudioTestKind, string> = {
      preview_tts: "Preview voice",
      speaker_test: "Speaker test",
      mic_test: "Microphone test",
      loopback: "Loopback test",
    };
    return labels[kind];
  }

  function testButtonLabel(kind: AudioTestKind): string {
    const labels: Record<AudioTestKind, string> = {
      preview_tts: "Preview",
      speaker_test: "Play tone",
      mic_test: "Record 5 s",
      loopback: "Record & play",
    };
    return labels[kind];
  }

  async function runTest(kind: AudioTestKind): Promise<void> {
    if (kind === "preview_tts") await runTtsPreview();
    if (kind === "speaker_test") await runSpeakerTest();
    if (kind === "mic_test") await runMicTest();
    if (kind === "loopback") await runLoopbackTest();
  }

  async function withTest(kind: AudioTestKind, action: () => Promise<void>): Promise<void> {
    await action();
    const test = $audioTests[kind];
    if (test.state === "pass" && (kind === "speaker_test" || kind === "loopback")) {
      confirmTest = kind;
    }
  }

  function confirmHeard(kind: ConfirmKind, heard: boolean): void {
    confirmTest = null;
    if (heard) return;
    audioTests.update((tests) => ({
      ...tests,
      [kind]: {
        state: "fail",
        result: tests[kind].result,
        error: null,
        hint:
          kind === "speaker_test"
            ? "No tone heard. Check the selected output device and output volume."
            : "No playback heard. Check the selected output device and output volume.",
      },
    }));
  }
</script>

<section aria-label="Audio setup">
  {#if loading}
    <p class="muted">Loading audio devices…</p>
  {:else if loadError}
    <p class="error" role="alert">{loadError}</p>
  {:else}
    <h2>Devices</h2>

    <div class="field">
      <label for="audio-input">Input device</label>
      <select
        id="audio-input"
        bind:value={selectedInput}
        disabled={applying || fieldFor("audio_input_device")?.editable === false}
      >
        <option value="">System default</option>
        {#each $audioDevices?.inputs ?? [] as device (device.selector)}
          <option value={device.selector}>
            {device.name}{device.is_default ? " — default" : ""}
          </option>
        {/each}
      </select>
      <p class="muted small">Active: {activeDeviceLabel($audioStatus?.configured_input_device)}</p>
    </div>

    <div class="field">
      <label for="audio-output">Output device</label>
      <select
        id="audio-output"
        bind:value={selectedOutput}
        disabled={applying || fieldFor("audio_output_device")?.editable === false}
      >
        <option value="">System default</option>
        {#each $audioDevices?.outputs ?? [] as device (device.selector)}
          <option value={device.selector}>
            {device.name}{device.is_default ? " — default" : ""}
          </option>
        {/each}
      </select>
      <p class="muted small">Active: {activeDeviceLabel($audioStatus?.configured_output_device)}</p>
    </div>

    <div class="actions">
      <button type="button" onclick={applyDevices} disabled={applying || $status?.connection !== "connected"}>
        {applying ? "Applying…" : "Apply devices"}
      </button>
    </div>
    {#if applyError}
      <p class="error" role="alert">{applyError}</p>
    {:else if applyMessage}
      <p class="success">{applyMessage}</p>
    {/if}

    <h2>Microphone level</h2>
    <div class="meter" role="img" aria-label="Live microphone level">
      <div class="meter-fill" style="width: {meterPercent(displayedPeak)}%"></div>
    </div>
    <p class="muted small">
      {displayedPeak === 0 ? "No input signal" : `Peak ${formatDbfs(displayedPeak)}`}
      · capture: {$audioStatus?.capture_active ? "active" : "inactive"}
    </p>

    <h2>Tests</h2>
    <div class="test-grid">
      {#each Object.entries($audioTests) as [kind, test] (kind)}
        <article class="test">
          <header>
            <strong>{testLabel(kind as AudioTestKind)}</strong>
            <span class="state {test.state}">{test.state}</span>
          </header>
          {#if test.state === "running"}
            <p class="muted small">
              {kind === "mic_test" || kind === "loopback"
                ? "Recording for 5 seconds — speak now."
                : "Playing…"}
            </p>
          {:else if test.result}
            <p class="result">
              {#if kind === "mic_test"}
                Transcript: {test.result.transcript?.trim() || "(empty)"}<br />
              {:else if kind === "preview_tts"}
                Provider: {test.result.provider ?? "unknown"}<br />
              {:else}
                Playback finished.<br />
              {/if}
              Peak: {formatDbfs(test.result.peak)}
            </p>
          {:else if test.error}
            <p class="error">{test.error.message}</p>
          {/if}
          {#if test.hint}
            <p class="hint">{test.hint}</p>
          {/if}
          {#if confirmTest === kind}
            <div class="confirm">
              <span>Did you hear it?</span>
              <button type="button" onclick={() => confirmHeard(kind, true)}>Yes</button>
              <button type="button" class="secondary" onclick={() => confirmHeard(kind, false)}>No</button>
            </div>
          {/if}
          <button
            type="button"
            disabled={busy || (kind === "mic_test" && $audioStatus?.microphone_permission === "denied")}
            onclick={() => withTest(kind as AudioTestKind, () => runTest(kind as AudioTestKind))}
          >
            {testButtonLabel(kind as AudioTestKind)}
          </button>
        </article>
      {/each}
    </div>

    {#if $audioStatus?.microphone_permission === "not_determined" || $audioStatus?.microphone_permission === "denied"}
      <div class="permission">
        <p class="muted small">
          Microphone permission: {$audioStatus.microphone_permission}
        </p>
        <button type="button" onclick={enableMicrophone} disabled={busy}>
          Enable microphone access
        </button>
        {#if $audioStatus.microphone_permission === "denied"}
          <p class="hint">
            Open System Settings → Privacy & Security → Microphone and allow Seneschal.
          </p>
        {/if}
      </div>
    {/if}

    <h2>Diagnostics</h2>
    <ul class="diagnostics">
      <li>Input: {activeDeviceLabel($audioStatus?.configured_input_device)}</li>
      <li>Output: {activeDeviceLabel($audioStatus?.configured_output_device)}</li>
      <li>Capture: {$audioStatus?.capture_active ? "active" : "inactive"}</li>
      <li>Output device: {$audioStatus?.output_available ? "available" : "unavailable"}</li>
      <li>STT: {$audioStatus?.stt_provider ?? "unknown"}</li>
      <li>TTS: {$audioStatus?.tts_provider ?? "unknown"}</li>
      {#if $audioStatus?.capture_error}
        <li>Last capture error: {$audioStatus.capture_error}</li>
      {/if}
      {#if diagnosticHint($audioTests.mic_test.error, $audioStatus)}
        <li class="hint">{diagnosticHint($audioTests.mic_test.error, $audioStatus)}</li>
      {/if}
    </ul>
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

  .field {
    display: grid;
    gap: 6px;
    margin-bottom: 14px;
  }

  label {
    color: #9aa7b5;
    font-size: 13px;
  }

  select,
  button {
    width: 100%;
    padding: 9px 11px;
    border: 1px solid #38475c;
    border-radius: 7px;
    background: #101317;
    color: #e7ecf2;
    font-size: 14px;
    box-sizing: border-box;
  }

  button {
    margin-top: 10px;
    background: #2d3a4a;
    cursor: pointer;
  }

  button.secondary {
    background: #171c23;
  }

  button:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  .actions {
    display: grid;
    gap: 8px;
  }

  .meter {
    height: 12px;
    overflow: hidden;
    border-radius: 999px;
    background: #202833;
  }

  .meter-fill {
    height: 100%;
    background: linear-gradient(90deg, #3f8f5b, #c9b458, #d26060);
    transition: width 80ms linear;
  }

  .test-grid {
    display: grid;
    gap: 12px;
  }

  .test {
    padding: 12px;
    border: 1px solid #263141;
    border-radius: 10px;
    background: #101317;
  }

  .test header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 10px;
  }

  .state {
    font-size: 12px;
    padding: 2px 7px;
    border-radius: 999px;
    background: #232b35;
  }

  .state.pass { color: #8bd39d; }
  .state.fail { color: #ff9c9c; }
  .state.running { color: #d8c67c; }
  .state.cancelled { color: #9aa7b5; }

  .confirm {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 10px;
    color: #9aa7b5;
    font-size: 13px;
  }

  .confirm button {
    width: auto;
    margin-top: 0;
    padding: 6px 10px;
  }

  .permission {
    margin-top: 16px;
  }

  .diagnostics {
    margin: 0;
    padding-left: 18px;
    color: #9aa7b5;
    font-size: 13px;
  }

  .result,
  .hint {
    font-size: 13px;
    color: #c8d3e0;
  }

  .hint {
    color: #d8c67c;
  }

  .muted {
    color: #9aa7b5;
  }

  .small {
    font-size: 12px;
  }

  .error {
    color: #ff9c9c;
    font-size: 13px;
  }

  .success {
    color: #8bd39d;
    font-size: 13px;
  }
</style>
