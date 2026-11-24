<script lang="ts">
  import { onMount } from "svelte";
  import {
    chatLog,
    initChat,
    parseChatError,
    refreshChatHistory,
    resetChatForDisconnect,
    roleLabel,
    sendTextInput,
    shouldAutoScroll,
    sourceLabel,
    type ChatError,
    type ChatEntry,
  } from "../chat";
  import { status } from "../status";

  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let composer = $state("");
  let sending = $state(false);
  let inputError = $state<ChatError | null>(null);
  let pinned = $state(true);
  let list = $state<HTMLDivElement | null>(null);
  let lastConnection: string | null = null;

  onMount(async () => {
    try {
      await initChat();
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
      void refreshChatHistory().catch(() => undefined);
    } else if (connection !== null) {
      resetChatForDisconnect();
    }
  });

  $effect(() => {
    void $chatLog;
    if (pinned) scrollToBottom();
  });

  function scrollToBottom(): void {
    if (list) list.scrollTop = list.scrollHeight;
  }

  function onScroll(): void {
    if (list) pinned = shouldAutoScroll(list);
  }

  function jumpToLatest(): void {
    pinned = true;
    scrollToBottom();
  }

  let connected = $derived($status?.connection === "connected");

  async function submit(): Promise<void> {
    const text = composer.trim();
    if (!text || sending || !connected) return;
    sending = true;
    inputError = null;
    try {
      await sendTextInput(text);
      composer = "";
      pinned = true;
      scrollToBottom();
    } catch (error) {
      inputError = parseChatError(error);
    } finally {
      sending = false;
    }
  }

  function onKeyDown(event: KeyboardEvent): void {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      void submit();
    }
  }

  function entryBadge(entry: ChatEntry): string | null {
    if (entry.role !== "user" || !entry.source) return null;
    return sourceLabel(entry.source);
  }
</script>

<section class="chat" aria-label="Conversation">
  {#if loading}
    <p class="muted">Loading conversation…</p>
  {:else if loadError}
    <p class="error" role="alert">{loadError}</p>
  {:else}
    <div class="log" bind:this={list} onscroll={onScroll}>
      {#each $chatLog as entry (entry.id)}
        <article class="entry {entry.role}" class:streaming={entry.streaming}>
          <header>
            <span class="role">{roleLabel(entry.role)}</span>
            {#if entryBadge(entry)}
              <span class="badge">{entryBadge(entry)}</span>
            {/if}
          </header>
          <div class="text">
            {entry.text}{#if entry.streaming}<span
                class="cursor"
                aria-hidden="true"
              ></span>{/if}
          </div>
        </article>
      {:else}
        <p class="muted empty">No messages yet. Speak or send a message.</p>
      {/each}
    </div>

    {#if !pinned}
      <button type="button" class="jump" onclick={jumpToLatest}>
        Jump to latest
      </button>
    {/if}

    <form
      class="composer"
      onsubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <textarea
        bind:value={composer}
        onkeydown={onKeyDown}
        rows="3"
        aria-label="Message text"
        placeholder={connected
          ? "Send a message… (Enter to send, Shift+Enter for a new line)"
          : "Control is disconnected"}
        disabled={!connected || sending}
      ></textarea>
      <button type="submit" disabled={!connected || sending || composer.trim() === ""}>
        {sending ? "Sending…" : "Send"}
      </button>
    </form>

    {#if inputError}
      <p class="error" role="alert">
        {inputError.message}
        {#if inputError.hint}
          <span class="hint">{inputError.hint}</span>
        {/if}
      </p>
    {/if}
  {/if}
</section>

<style>
  section.chat {
    display: flex;
    height: 100%;
    max-width: 760px;
    margin: 0 auto;
    flex-direction: column;
    min-height: 0;
  }

  .log {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 16px;
    border: 1px solid #263141;
    border-radius: 12px;
    background: #171c23;
    display: grid;
    gap: 10px;
    align-content: start;
  }

  .entry {
    max-width: 680px;
    padding: 10px 12px;
    border: 1px solid #263141;
    border-radius: 10px;
    background: #101317;
  }

  .entry.user {
    border-color: #33506f;
  }

  .entry.tool,
  .entry.system {
    color: #b6c3d1;
    background: #202833;
  }

  .entry.error {
    color: #ff9c9c;
  }

  header {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 4px;
  }

  .role {
    font-size: 12px;
    font-weight: 650;
    color: #9aa7b5;
  }

  .badge {
    padding: 1px 7px;
    border-radius: 999px;
    background: #223a2c;
    color: #8fd3a8;
    font-size: 10px;
    text-transform: uppercase;
  }

  .text {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-size: 14px;
    line-height: 1.45;
  }

  .cursor {
    display: inline-block;
    width: 7px;
    height: 14px;
    margin-left: 2px;
    vertical-align: text-bottom;
    background: #5a8dd6;
    animation: blink 1s steps(1) infinite;
  }

  @keyframes blink {
    50% {
      opacity: 0;
    }
  }

  .jump {
    width: auto;
    align-self: flex-end;
    margin: 8px 0 0;
    padding: 6px 10px;
    font-size: 12px;
  }

  .composer {
    display: flex;
    gap: 10px;
    margin-top: 12px;
    align-items: flex-end;
  }

  textarea {
    flex: 1;
    min-height: 72px;
    resize: vertical;
    padding: 10px 12px;
    border: 1px solid #38475c;
    border-radius: 8px;
    background: #171c23;
    color: #e7ecf2;
    font-family: inherit;
    font-size: 14px;
    line-height: 1.45;
  }

  .composer button {
    width: auto;
    margin: 0;
    padding: 10px 14px;
  }

  .error {
    margin: 10px 0 0;
    color: #ff9c9c;
    font-size: 13px;
  }

  .hint {
    display: block;
    color: #d8c58a;
  }

  .muted {
    color: #9aa7b5;
  }

  .empty {
    justify-self: center;
    align-self: center;
    margin-top: 30vh;
  }
</style>
