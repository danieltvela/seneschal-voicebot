import { writable } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ChatRole = "user" | "assistant" | "tool" | "system" | "error";
export type ChatSource = "voice" | "text";

export interface ChatEntry {
  id: string;
  role: ChatRole;
  text: string;
  source: ChatSource | null;
  utteranceId: number | null;
  streaming: boolean;
}

export interface ChatSnapshot {
  revision: number;
  entries: ChatEntry[];
}

export type ChatEvent =
  | { type: "snapshot"; snapshot: ChatSnapshot }
  | { type: "upsert"; revision: number; entry: ChatEntry }
  | {
      type: "append_token";
      revision: number;
      utteranceId: number;
      token: string;
    }
  | { type: "remove"; revision: number; entryId: string };

export const chatLog = writable<ChatEntry[]>([]);

let unlistenChat: UnlistenFn | null = null;
let lastRevision: number | null = null;

function applySnapshot(snapshot: ChatSnapshot): void {
  lastRevision = snapshot.revision;
  chatLog.set(snapshot.entries);
}

export function upsertEntry(entries: ChatEntry[], entry: ChatEntry): ChatEntry[] {
  let index = -1;
  for (let candidate = entries.length - 1; candidate >= 0; candidate -= 1) {
    if (entries[candidate].id === entry.id) {
      index = candidate;
      break;
    }
  }
  if (index === -1) return [...entries, entry];
  const next = [...entries];
  next[index] = entry;
  return next;
}

export function appendToken(
  entries: ChatEntry[],
  utteranceId: number,
  token: string,
): ChatEntry[] {
  let index = -1;
  for (let candidate = entries.length - 1; candidate >= 0; candidate -= 1) {
    const entry = entries[candidate];
    if (entry.role === "assistant" && entry.utteranceId === utteranceId) {
      index = candidate;
      break;
    }
  }
  if (index === -1) {
    return [
      ...entries,
      {
        id: `assistant-${utteranceId}`,
        role: "assistant",
        text: token,
        source: null,
        utteranceId,
        streaming: true,
      },
    ];
  }
  const next = [...entries];
  next[index] = {
    ...next[index],
    text: next[index].text + token,
    streaming: true,
  };
  return next;
}

export function removeEntry(entries: ChatEntry[], entryId: string): ChatEntry[] {
  return entries.filter((entry) => entry.id !== entryId);
}

function applyEvent(event: ChatEvent): boolean {
  switch (event.type) {
    case "snapshot":
      applySnapshot(event.snapshot);
      return false;
    case "upsert": {
      const stale = lastRevision !== null && event.revision <= lastRevision;
      const gap =
        lastRevision !== null && event.revision > lastRevision + 1;
      if (stale) return false;
      lastRevision = event.revision;
      chatLog.update((entries) => upsertEntry(entries, event.entry));
      return gap;
    }
    case "append_token": {
      const stale = lastRevision !== null && event.revision <= lastRevision;
      const gap =
        lastRevision !== null && event.revision > lastRevision + 1;
      if (stale) return false;
      lastRevision = event.revision;
      chatLog.update((entries) =>
        appendToken(entries, event.utteranceId, event.token),
      );
      return gap;
    }
    case "remove": {
      const stale = lastRevision !== null && event.revision <= lastRevision;
      const gap =
        lastRevision !== null && event.revision > lastRevision + 1;
      if (stale) return false;
      lastRevision = event.revision;
      chatLog.update((entries) => removeEntry(entries, event.entryId));
      return gap;
    }
  }
}

export async function initChat(): Promise<void> {
  if (unlistenChat) return;
  // Subscribe before the snapshot so updates during startup are not lost. A
  // snapshot older than a received event is ignored by revision.
  unlistenChat = await listen<ChatEvent>("desktop-chat-event", (event) => {
    const gap = applyEvent(event.payload);
    if (gap) void refreshChatSnapshot().catch(() => undefined);
  });
  const initial = await invoke<ChatSnapshot>("chat_snapshot");
  if (lastRevision === null || initial.revision >= lastRevision) {
    applySnapshot(initial);
  }
}

export async function refreshChatSnapshot(): Promise<void> {
  applySnapshot(await invoke<ChatSnapshot>("chat_snapshot"));
}

export async function refreshChatHistory(): Promise<void> {
  applySnapshot(await invoke<ChatSnapshot>("refresh_chat_history"));
}

export async function sendTextInput(text: string): Promise<void> {
  await invoke("send_chat_input", { text });
}

/** A reconnect may have interrupted a streaming response; show it as done. */
export function resetChatForDisconnect(): void {
  chatLog.update((entries) =>
    entries.map((entry) => (entry.streaming ? { ...entry, streaming: false } : entry)),
  );
}

// ── Pure presentation helpers (unit-tested) ─────────────────────────────────

export interface ScrollBox {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
}

/** Keep auto-scroll only while the user is already near the bottom. */
export function shouldAutoScroll(
  box: ScrollBox,
  thresholdPixels = 80,
): boolean {
  const distance = box.scrollHeight - box.scrollTop - box.clientHeight;
  return distance <= thresholdPixels;
}

export interface ChatError {
  code: string | null;
  message: string;
  hint: string | null;
}

/** Shell errors use the `code: message (HTTP status)` convention. */
export function parseChatError(error: unknown): ChatError {
  const message = error instanceof Error ? error.message : String(error);
  const separator = message.indexOf(": ");
  if (separator === -1) {
    return { code: null, message, hint: null };
  }
  const code = message.slice(0, separator);
  const hint =
    code === "permission_pending"
      ? "Answer the pending permission request, then send the message again."
      : null;
  return { code, message, hint };
}

export function roleLabel(role: ChatRole): string {
  const labels: Record<ChatRole, string> = {
    user: "You",
    assistant: "Seneschal",
    tool: "Tool",
    system: "System",
    error: "Error",
  };
  return labels[role] ?? role;
}

export function sourceLabel(source: ChatSource | null): string {
  const labels: Record<ChatSource, string> = {
    voice: "voice",
    text: "text",
  };
  return source === null ? "user" : labels[source];
}
