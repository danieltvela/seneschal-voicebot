import { describe, expect, it } from "vitest";
import {
  appendToken,
  parseChatError,
  removeEntry,
  roleLabel,
  shouldAutoScroll,
  sourceLabel,
  upsertEntry,
  type ChatEntry,
} from "../src/lib/chat";

function entry(overrides: Partial<ChatEntry> = {}): ChatEntry {
  return {
    id: "user-1",
    role: "user",
    text: "hola",
    source: "voice",
    utteranceId: 1,
    streaming: false,
    ...overrides,
  };
}

describe("chat presentation helpers", () => {
  it("keeps auto-scroll only near the bottom", () => {
    expect(
      shouldAutoScroll({ scrollTop: 920, scrollHeight: 1000, clientHeight: 100 }),
    ).toBe(true);
    expect(
      shouldAutoScroll({ scrollTop: 400, scrollHeight: 1000, clientHeight: 100 }),
    ).toBe(false);
    expect(
      shouldAutoScroll(
        { scrollTop: 500, scrollHeight: 1000, clientHeight: 100 },
        500,
      ),
    ).toBe(true);
  });

  it("maps shell chat errors and the permission-pending hint", () => {
    const pending = parseChatError(
      "permission_pending: resolve the pending permission request before sending text (HTTP 409)",
    );
    expect(pending.code).toBe("permission_pending");
    expect(pending.message).toContain("(HTTP 409)");
    expect(pending.hint).toContain("pending permission request");

    const plain = parseChatError("input_failed: could not submit text (HTTP 503)");
    expect(plain.code).toBe("input_failed");
    expect(plain.hint).toBeNull();

    const unknown = parseChatError("network down");
    expect(unknown.code).toBeNull();
    expect(unknown.message).toBe("network down");
  });

  it("labels roles and message origins", () => {
    expect(roleLabel("user")).toBe("You");
    expect(roleLabel("assistant")).toBe("Seneschal");
    expect(roleLabel("tool")).toBe("Tool");
    expect(sourceLabel("voice")).toBe("voice");
    expect(sourceLabel("text")).toBe("text");
    expect(sourceLabel(null)).toBe("user");
  });

  it("upserts entries by stable id", () => {
    const entries = [entry()];
    const updated = upsertEntry(entries, entry({ text: "hola de nuevo" }));
    expect(updated).toHaveLength(1);
    expect(updated[0].text).toBe("hola de nuevo");

    const added = upsertEntry(entries, entry({ id: "user-2", utteranceId: 2 }));
    expect(added).toHaveLength(2);
    expect(added[1].id).toBe("user-2");
  });

  it("streams assistant tokens into the matching utterance", () => {
    const entries = [
      entry(),
      entry({
        id: "assistant-1",
        role: "assistant",
        text: "buen",
        source: null,
        streaming: true,
      }),
    ];
    const streamed = appendToken(entries, 1, "os dias");
    expect(streamed[1].text).toBe("buenos dias");
    expect(streamed[1].streaming).toBe(true);

    const created = appendToken([], 2, "hola");
    expect(created[0].id).toBe("assistant-2");
    expect(created[0].streaming).toBe(true);
  });

  it("removes an empty finalized assistant row", () => {
    const entries = [entry(), entry({ id: "assistant-1", role: "assistant", text: "" })];
    expect(removeEntry(entries, "assistant-1")).toHaveLength(1);
  });
});
