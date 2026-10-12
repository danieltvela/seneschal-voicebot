# iOS Companion (Seneschal)

Guide for the **iOS / watchOS companion** in `clients/voicebot-ios-companion/`: a mobile presence of the same Seneschal agent that runs on the host (Mac). The host remains the brain (STT → LLM → TTS, memory, tools, agents). The phone is I/O, observability, and light control on the **LAN**.

> **Design source:** issue [#190](http://tesla.local:3000/danielvela/seneschal-voicebot/issues/190) and `doc/design/190-ios-companion-v2.md`.  
> **Watch constraints:** [APPLE_WATCH_CLIENT.md](APPLE_WATCH_CLIENT.md) (TN3135, PTT, iPhone relay).

---

## Architecture: dual channel

```
┌─────────────────────┐          WS :9090            ┌──────────────────┐
│  iPhone / iPad      │◄──── PCM + session/audio ───►│  Seneschal host  │
│  companion          │          Control :9001       │  (remote+control)│
│                     │◄──── SSE events + REST ─────►│                  │
└──────────┬──────────┘                              └──────────────────┘
           │ WCSession
           ▼
    ┌─────────────┐
    │ Apple Watch │  PTT + status (relay via iPhone)
    └─────────────┘
```

| Plane | Transport | Port (default) | Role |
|-------|-----------|----------------|------|
| **Audio** | WebSocket `/ws` | `WS_PORT=9090` | Mic PCM uplink, TTS binary downlink, session, optional transcript text |
| **Control** | HTTP + SSE | `CONTROL_PORT=9001` | Pipeline state, tokens, tools, system/agent events, mute/input/barge-in/permission |

**Do not** multiplex control events onto WS text frames for v2. Prefer Control for status and commands; keep WS focused on audio reliability.

### Host requirements

```bash
# Example: dual-channel host for companion
export WS_PORT=9090
export CONTROL_PORT=9001
cargo run --features "remote,control" --release
```

- Features: **`remote`** + **`control`**
- Control binds **`0.0.0.0`** (LAN reachable). Treat the LAN as trusted; no API key in alpha.
- Single remote **audio** client: a second phone gets **HTTP 409** on WS → companion enters **Control-only** mode (text/mute/status; no mic).

### App defaults

| Field | Default |
|-------|---------|
| WebSocket port | `9090` |
| Control port | `9001` |

If WS works but Control health fails, the UI shows a banner suggesting Control port **9001**.

---

## Control API surface (companion-relevant)

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/control/health` | Connectivity probe |
| GET | `/control/events` | SSE stream of `ControlEvent` JSON |
| GET | `/control/state` | `{ state, utterance_id, pause_reason?, tts_muted }` |
| GET | `/control/sessions` | History session list |
| GET | `/control/sessions/{id}/messages` | Server history (source of truth on connect) |
| POST | `/control/mute` | `{ "muted": true\|false }` |
| POST | `/control/barge_in` | Interrupt current speech |
| POST | `/control/input` | `{ "text": "…" }` — **409** if agent permission pending |
| GET | `/control/permissions` | Pending permission slots |
| POST | `/control/permission` | `{ "task_id", "option_id" }` — ACP **optionId**, not UI label |

### Structured pipeline state

`state` is a stable token (not Rust Debug):

`idle` | `listening` | `thinking` | `speaking` | `paused`  
Optional `pause_reason` (e.g. `consolidation`).

### SSE notes

- **No** cumulative 1 MiB byte cap (long sessions stay open).
- Lagging clients receive `Error` with message like `Missed N events…` → resync with `GET /control/state` (+ history if needed).
- Unknown event `type` values: iOS client **skips** safely.

### Selected `ControlEvent` types

| `type` | UI use |
|--------|--------|
| `state_changed` | Status bar pipeline chip |
| `transcript` / `llm_token` / `llm_done` | Conversation (prefer Control over WS text when connected) |
| `tool_call` | Timeline |
| `system_notification` / `mcp_notification` | Timeline |
| `classification` | Optional intent chip |
| `agent_task_*` | Timeline lifecycle rows |
| `agent_permission_requested` / `resolved` | PermissionSheet |
| `mute_changed` | Mute toggle sync |
| `error` | Banner / timeline (lag is banner-only) |

Permission options on the wire:

```json
{
  "type": "agent_permission_requested",
  "task_id": "t1",
  "agent_name": "hermes",
  "description": "bash: ls",
  "options": [
    { "id": "allow", "label": "Allow once", "kind": "allow" },
    { "id": "deny", "label": "Deny", "kind": "reject" }
  ]
}
```

POST must send **`option_id`** (`allow`), never the label string.

---

## iOS app map

Path: `clients/voicebot-ios-companion/`

| Module | Role |
|--------|------|
| `Models/RemoteMessage.swift` | WS text protocol |
| `Models/ControlEvent.swift` | Control SSE/REST models, `LinkState`, permission types |
| `Models/TimelineItem.swift` | Secondary event rows |
| `Services/WebSocketManager.swift` | Audio WS; 409 → conflict |
| `Services/ControlClient.swift` | REST |
| `Services/ControlSSEClient.swift` | SSE + reconnect 1s→30s |
| `Services/HistoryClient.swift` | Sessions/messages |
| `ViewModels/CompanionViewModel.swift` | Dual-link SM, token de-dupe, scenePhase mic |
| `Views/StatusBarView.swift` | Links, pipeline, mute, barge-in |
| `Views/ComposerView.swift` | Text → Control input |
| `Views/TimelineView.swift` | Tools / system / agent / errors |
| `Views/PermissionSheet.swift` | Approve/deny via `option_id` |
| `Utilities/AdaptiveLayout.swift` | Compact vs regular (iPad) |

### UI behaviour summary

- **Compact** (iPhone / Slide Over): stack + timeline sheet.
- **Regular** (iPad width): conversation + live timeline column; max readable widths.
- **Token de-dupe:** when Control is up, prefer `llm_token` / `llm_done`; buffer WS `response.text` briefly.
- **Background:** stop mic capture; TTS playback may continue (`UIBackgroundModes` audio).
- **Branding:** user-facing name **Seneschal** (not Jarvis/Voicebot product strings).

---

## Degradation matrix

| WS audio | Control | Behaviour |
|----------|---------|-----------|
| ✓ | ✓ | Full companion (target) |
| ✓ | ✗ | Audio + WS text; limited status/mute/text; banner for Control port |
| ✗ / **409 conflict** | ✓ | **Control-only:** text, mute, history, status, timeline; no remote mic |
| ✗ | ✗ | Offline local history only |

---

## Battery / foreground expectations (alpha)

- Dual open connections (WS + SSE) are intended for **foreground / active session** use.
- SSE reconnects with exponential backoff (1s → 30s) while the user remains “connected”.
- Background: mic stopped; do not claim always-listening on phone or watch.

---

## Manual QA matrix (companion v2)

| Check | Pass criteria |
|-------|----------------|
| Connect dual ports | Health + `session.ready`; pipeline chip updates |
| Speak turn | User bubble + assistant stream; TTS plays |
| Mute | TTS silent; unmute restores |
| Composer text | Appears as user message; host responds |
| Barge-in mid-speech | Pipeline cancels / stops TTS |
| Tool turn | Timeline shows `tool_call` row |
| Agent permission | PermissionSheet → option → host continues |
| Permission Later | Chip reopens sheet |
| Second phone WS | 409 → Control-only banner; text still works |
| Background | Mic stops; return resumes if audio was connected |
| iPad regular | Timeline column visible without sheet |
| Dynamic Type XXXL | Status + composer usable |
| Disconnect | Both links tear down cleanly |

### Automated tests (client)

```bash
cd clients/voicebot-ios-companion
xcodebuild test -scheme voicebot-ios-companion \
  -destination 'platform=iOS Simulator,name=iPhone 17,OS=26.3.1' \
  -only-testing:voicebot-ios-companionTests
```

Covers: `RemoteMessage`, ControlEvent fixtures, TimelineItem mapping, AdaptiveLayout, permission option_id contract.

### Host tests (Control)

```bash
cargo test -p seneschal-control
cargo test -p seneschal-common permission
cargo check --features "remote,control"
```

---

## watchOS (light)

- Watch talks to **iPhone** via `WCSession` (`WatchRelayService`); iPhone owns host sockets.
- PTT / audio session order: follow [APPLE_WATCH_CLIENT.md](APPLE_WATCH_CLIENT.md).
- **Glance surface (PR8):** iPhone pushes `pipeline_state` (`idle`…`paused`), `last_line` (truncated assistant text), and `host_session` via live `sendMessage` + `updateApplicationContext`.
- Watch UI: status color/text from pipeline token, last-line preview (2 lines), PTT mic.
- No full timeline on watch; phone owns detail.
- Always-listening on watch is **out of scope**.

---

## Out of scope (follow-ups)

- WAN / Tailscale / public tunnel + auth
- On-device STT/TTS replacing host pipeline
- Android companion
- Push / Live Activities
- Multi concurrent remote **audio** clients
- Full TUI parity (prompt editor, force-intent, plugin switcher UI)

---

## Related docs

| Doc | Topic |
|-----|--------|
| [design/190-ios-companion-v2.md](design/190-ios-companion-v2.md) | Full design + PR plan |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Control API overview |
| [APPLE_WATCH_CLIENT.md](APPLE_WATCH_CLIENT.md) | Watch networking constraints |
| [env-vars.md](env-vars.md) | `WS_PORT`, `CONTROL_PORT` |
| [build-features.md](build-features.md) | `remote`, `control` features |
| [MAIN_PROCESS.md](MAIN_PROCESS.md) | Process + Control endpoints |

---

## Implementation history (issue #190)

| PR | Summary |
|----|---------|
| Host PR1 | Structured `state_changed`, ControlEvent agent/permission types, SSE longevity |
| Host PR2 | `PermissionGate`, emit twins, `POST /control/permission`, input 409 guard |
| iOS PR3 | Control REST/SSE clients + Codable models |
| iOS PR4a | Dual-link ViewModel, StatusBar, composer, 409 Control-only |
| iOS PR5 | Timeline UI |
| iOS PR6 | PermissionSheet (`option_id`) |
| iOS PR7 | iPad split + a11y |
| watchOS PR8 | Pipeline state + last-line glance via WCSession |
| Docs PR9 | This document + ROADMAP M2.4 alignment |
