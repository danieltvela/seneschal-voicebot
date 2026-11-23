import { describe, expect, it } from "vitest";
import {
  connectionLabel,
  daemonLabel,
  formatReconnect,
  pipelineLabel,
} from "../src/lib/status";

describe("desktop status presentation", () => {
  it("labels every supported pipeline token", () => {
    expect(pipelineLabel("idle")).toBe("Idle");
    expect(pipelineLabel("listening")).toBe("Listening");
    expect(pipelineLabel("thinking")).toBe("Thinking");
    expect(pipelineLabel("speaking")).toBe("Speaking");
    expect(pipelineLabel("paused")).toBe("Paused");
  });

  it("shows unknown wire tokens verbatim", () => {
    expect(pipelineLabel("future_state")).toBe("future_state");
  });

  it("labels daemon and control links", () => {
    expect(daemonLabel("restarting")).toBe("Restarting");
    expect(connectionLabel("connected")).toBe("SSE connected");
  });

  it("formats the measured reconnect budget", () => {
    expect(formatReconnect(null)).toBe("—");
    expect(formatReconnect(1250)).toBe("1250 ms");
  });
});
