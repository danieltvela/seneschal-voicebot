# Apple Watch Client for Seneschal

Guide for building a watchOS app that connects to the Seneschal WebSocket server, streams microphone audio, and plays back TTS responses.

> **Related:** see issue #37 for the full feasibility investigation (latency matrix, NECP/TN3135 context, phased implementation plan).

## Prerequisites

- Xcode 15+ with watchOS 10+ SDK
- Seneschal running with `--features remote` and `WS_PORT` set
- Apple Watch paired with iPhone (or Simulator) — **test on a real device; the simulator always allows low-level networking and may hide NECP denials**

## watchOS Constraints (read first)

Two platform rules drive every design decision below. They come from Apple's [TN3135](https://developer.apple.com/documentation/technotes/tn3135-low-level-networking-on-watchos), [WWDC 2019-716](https://developer.apple.com/videos/play/wwdc2019/716/), and an Apple DTS reply on the developer forums ([thread 773362](https://developer.apple.com/forums/thread/773362)).

### 1. NECP blocks WebSocket/socket access — with an exception for audio streaming

watchOS 9+ restricts low-level networking via **NECP** (Network Extension Control Policy). Opening a `URLSessionWebSocketTask` to a non-allowed endpoint returns:

```
nw_endpoint_flow_failed_with_error [C1 <server> failed parent-flow
(unsatisfied (Path was denied by NECP policy), interface: ipsec2, ipv4, ipv6, proxy)]
```

There is an **explicit exception for audio streaming apps**. To meet it:

1. Declare the background-audio mode in the watch app's `Info.plist`:
   ```xml
   <key>UIBackgroundModes</key>
   <array>
     <string>audio</string>
   </array>
   ```
2. **Activate an `AVAudioSession` with `.playAndRecord` *before* opening the WebSocket**, and keep it active for the entire lifetime of the socket.
3. In the App Store listing, classify the app as a streaming-audio / voice-assistant app.

If the session is not yet active when `webSocketTask(with:).resume()` is called, the connection is denied.

### 2. Microphone in the background is fragile on watchOS

- `AVAudioSession.Category.playAndRecord` is available on watchOS 2.0+, but watchOS suspends audio apps aggressively when the wrist is down.
- For multi-second interactions in the foreground, wrap the conversation in a `WKExtendedRuntimeSession` (see [Watch Considerations](#watchos-considerations) below).
- A true "always listening" watch app is **not** practical on watchOS — the watch is a glance-and-decide device, not a hub. If you need that, route through an iPhone companion via `WCSession`.

### Recommended operating modes

| Mode | Practical? | Notes |
|---|---|---|
| Foreground, push-to-talk | ✅ Yes | Lowest friction. No background needed. **Recommended for v1.** |
| Foreground, hands-free (~30 s) | ✅ Yes with `WKExtendedRuntimeSession` | Good for short assistant queries. |
| Background "always listening" | ⚠️ Fragile | Wrist-raised interactions only. |
| iPhone-relayed (`WCSession` → server) | ✅ Best for battery + connectivity | Watch ↔ iPhone over Bluetooth; iPhone ↔ server over WiFi/cellular. |

### Realistic end-to-end latency

| Path | RTT |
|---|---|
| Watch mic → AVFoundation 16 kHz PCM | 30–100 ms |
| Watch → Server (WiFi, LAN) | 30–100 ms |
| Watch → iPhone → Server (Bluetooth relay) | 150–300 ms |
| Server STT + LLM first token + TTS first chunk | 0.6–2.5 s |
| **Push-to-talk over WiFi (total)** | **~1.5–3 s** |
| **Always-on, Bluetooth relay (total)** | **~2.5–6 s** |

## Project Setup

1. Create a new watchOS App project in Xcode (SwiftUI lifecycle)
2. Set deployment target to **watchOS 10.0** minimum (`URLSessionWebSocketTask` is stable from watchOS 6, but audio improvements landed in watchOS 10)
3. No third-party dependencies needed -- Foundation and AVFAudio are sufficient

### Info.plist / Capabilities

- Add `NSMicrophoneUsageDescription` to Info.plist: `"Seneschal needs microphone access to hear your voice."`
- Enable **Background Modes** capability with **Audio, AirPlay, and Picture in Picture** checked
- Add `UIBackgroundModes` array with `audio` — required to satisfy the TN3135 exception for WebSocket access on watchOS 9+. See [watchOS Constraints](#watchos-constraints-read-first) above.

## Wire Protocol

The seneschal WebSocket server expects:

| Direction | Frame type | Format |
|-----------|-----------|--------|
| Watch -> Server | Binary | PCM i16 little-endian, mono, 16 kHz |
| Server -> Watch | Binary | PCM i16 little-endian, mono, 16 kHz |
| Watch -> Server | Text | JSON control messages |
| Server -> Watch | Text | JSON control messages |

### Control Messages

```
Watch -> Server:
  {"type": "session.start", "sample_rate": 16000}
  {"type": "barge_in"}

Server -> Watch:
  {"type": "session.ready"}
  {"type": "transcript", "text": "..."}
  {"type": "response.text", "text": "..."}
  {"type": "response.end"}
  {"type": "audio.start"}
  {"type": "audio.end"}
  {"type": "error", "message": "..."}
```

## Architecture

```
┌─────────────────────────────────────┐
│           Apple Watch App           │
│                                     │
│  ┌──────────┐    ┌───────────────┐  │
│  │ AudioMgr │───>│ WebSocketMgr  │──────> seneschal:WS_PORT/ws
│  │ (capture)│    │  (send audio) │  │
│  └──────────┘    └───────────────┘  │
│                                     │
│  ┌──────────┐    ┌───────────────┐  │
│  │ AudioMgr │<───│ WebSocketMgr  │<────── seneschal TTS audio
│  │ (play)   │    │ (recv audio)  │  │
│  └──────────┘    └───────────────┘  │
└─────────────────────────────────────┘
```

Three classes:
- **`ContentView`** -- SwiftUI view with a talk button
- **`AudioManager`** -- owns `AVAudioEngine`, handles mic capture + speaker playback
- **`WebSocketManager`** -- owns `URLSessionWebSocketTask`, handles send/receive

## Audio Capture

Use `AVAudioEngine` to tap the microphone at 16 kHz mono Int16 -- this matches the seneschal wire format exactly, so no conversion is needed.

```swift
import AVFAudio

class AudioManager {
    private let engine = AVAudioEngine()
    private let playerNode = AVAudioPlayerNode()

    // 16kHz mono Int16 -- matches seneschal wire protocol
    private let captureFormat = AVAudioFormat(
        commonFormat: .pcmFormatInt16,
        sampleRate: 16000,
        channels: 1,
        interleaved: true
    )!

    func startCapture(onAudio: @escaping (Data) -> Void) throws {
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(.playAndRecord, options: [.defaultToSpeaker])
        try session.setActive(true)

        let inputNode = engine.inputNode
        let inputFormat = inputNode.outputFormat(forBus: 0)

        // Convert from device format to 16kHz Int16
        guard let converter = AVAudioConverter(from: inputFormat, to: captureFormat) else {
            throw AudioError.converterFailed
        }

        // 100ms chunks at 16kHz = 1600 samples
        let bufferSize: AVAudioFrameCount = AVAudioFrameCount(inputFormat.sampleRate * 0.1)

        inputNode.installTap(onBus: 0, bufferSize: bufferSize, format: inputFormat) {
            [captureFormat] buffer, _ in
            // Convert to 16kHz Int16
            let frameCapacity: AVAudioFrameCount = AVAudioFrameCount(
                Double(buffer.frameLength) * 16000.0 / inputFormat.sampleRate
            )
            guard let converted = AVAudioPCMBuffer(
                pcmFormat: captureFormat,
                frameCapacity: frameCapacity
            ) else { return }

            var error: NSError?
            converter.convert(to: converted, error: &error) { _, outStatus in
                outStatus.pointee = .haveData
                return buffer
            }

            if let error { return }

            // Extract raw bytes (i16 LE on Apple silicon)
            guard let int16Data = converted.int16ChannelData else { return }
            let byteCount = Int(converted.frameLength) * 2
            let data = Data(bytes: int16Data[0], count: byteCount)
            onAudio(data)
        }

        engine.prepare()
        try engine.start()
    }

    func stopCapture() {
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
    }
}
```

## WebSocket Connection

```swift
class WebSocketManager: NSObject {
    private var task: URLSessionWebSocketTask?
    var onAudioReceived: ((Data) -> Void)?
    var onTranscript: ((String) -> Void)?
    var onResponseText: ((String) -> Void)?

    func connect(to url: URL) {
        let session = URLSession(configuration: .default, delegate: self, delegateQueue: nil)
        task = session.webSocketTask(with: url)
        task?.resume()

        // Send session.start
        let startMsg = #"{"type": "session.start", "sample_rate": 16000}"#
        task?.send(.string(startMsg)) { error in
            if let error { print("Send error: \(error)") }
        }

        receiveLoop()
    }

    func sendAudio(_ data: Data) {
        task?.send(.data(data)) { _ in }
    }

    func sendBargeIn() {
        let msg = #"{"type": "barge_in"}"#
        task?.send(.string(msg)) { _ in }
    }

    func disconnect() {
        task?.cancel(with: .normalClosure, reason: nil)
        task = nil
    }

    private func receiveLoop() {
        task?.receive { [weak self] result in
            switch result {
            case .success(.data(let data)):
                // Binary frame = TTS audio (i16 LE mono 16kHz)
                self?.onAudioReceived?(data)
            case .success(.string(let text)):
                self?.handleControlMessage(text)
            case .failure(let error):
                print("WS receive error: \(error)")
                return // Stop loop on error
            default:
                break
            }
            // Continue receiving
            self?.receiveLoop()
        }
    }

    private func handleControlMessage(_ json: String) {
        guard let data = json.data(using: .utf8),
              let msg = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let type = msg["type"] as? String else { return }

        switch type {
        case "session.ready":
            print("Session ready")
        case "transcript":
            if let text = msg["text"] as? String {
                onTranscript?(text)
            }
        case "response.text":
            if let text = msg["text"] as? String {
                onResponseText?(text)
            }
        case "audio.start":
            break // Audio frames incoming
        case "audio.end":
            break // Audio for this sentence done
        case "error":
            print("Server error: \(msg["message"] ?? "unknown")")
        default:
            break
        }
    }
}
```

## Audio Playback

Receive i16 LE binary frames from the WebSocket and play them through `AVAudioPlayerNode`.

```swift
extension AudioManager {
    private static let playbackFormat = AVAudioFormat(
        commonFormat: .pcmFormatInt16,
        sampleRate: 16000,
        channels: 1,
        interleaved: true
    )!

    func setupPlayback() {
        engine.attach(playerNode)
        engine.connect(playerNode, to: engine.mainMixerNode, format: Self.playbackFormat)
    }

    func playAudio(_ data: Data) {
        let frameCount = AVAudioFrameCount(data.count / 2) // 2 bytes per Int16 sample
        guard let buffer = AVAudioPCMBuffer(
            pcmFormat: Self.playbackFormat,
            frameCapacity: frameCount
        ) else { return }

        buffer.frameLength = frameCount

        // Copy i16 bytes into the buffer
        data.withUnsafeBytes { rawPtr in
            guard let src = rawPtr.baseAddress else { return }
            memcpy(buffer.int16ChannelData![0], src, data.count)
        }

        if !playerNode.isPlaying {
            playerNode.play()
        }
        playerNode.scheduleBuffer(buffer)
    }

    func stopPlayback() {
        playerNode.stop()
    }
}
```

## Barge-in

Two options:

**Option A: Server-side VAD (recommended)**
Just keep streaming microphone audio to the server. The seneschal's VAD will detect when the user starts speaking and automatically cancel the current TTS playback. No extra work needed on the watch.

**Option B: Client-side barge-in**
If you want faster response, detect audio input locally and send an explicit barge-in signal:

```swift
// When user starts speaking while TTS is playing:
webSocketManager.sendBargeIn()
audioManager.stopPlayback()
```

## Minimal SwiftUI View

```swift
import SwiftUI

struct ContentView: View {
    @StateObject private var viewModel = VoiceViewModel()

    var body: some View {
        VStack(spacing: 16) {
            Text(viewModel.statusText)
                .font(.caption)

            if let transcript = viewModel.lastTranscript {
                Text(transcript)
                    .font(.footnote)
                    .foregroundColor(.secondary)
            }

            Button(action: { viewModel.toggleListening() }) {
                Image(systemName: viewModel.isListening ? "mic.fill" : "mic")
                    .font(.title)
                    .foregroundColor(viewModel.isListening ? .red : .blue)
            }
            .buttonStyle(.plain)
        }
        .onAppear { viewModel.connect() }
        .onDisappear { viewModel.disconnect() }
    }
}
```

## watchOS Considerations

### Battery
- 16 kHz mono Int16 = ~32 KB/s upstream + downstream = ~64 KB/s total
- Manageable over Bluetooth relay or WiFi
- Avoid keeping the microphone open when not needed (use push-to-talk or VAD)

### Extended Runtime
For conversations longer than ~30 seconds, use `WKExtendedRuntimeSession`:

```swift
let session = WKExtendedRuntimeSession()
session.start()
// ... conversation ...
session.invalidate()
```

### Network
- Watch on WiFi: direct connection to seneschal server
- Watch on Bluetooth only: routes through paired iPhone
- Cellular models: can connect directly if on cellular data
- Ensure the seneschal server is reachable from the watch's network

### Audio Session
Always configure before starting audio:

```swift
let session = AVAudioSession.sharedInstance()
try session.setCategory(.playAndRecord, options: [.defaultToSpeaker, .allowBluetooth])
try session.setActive(true)
```

## Watch Face Complications

A complication makes the app discoverable from the watch face and unlocks three additional capabilities beyond "just launch it":

1. **Launch deep link** — tap → app opens directly on the talk view.
2. **Interactive button** — tap → an `AppIntent` runs in the watch app's *audio* process; can start the mic + WebSocket *without* opening the main UI. This is the unique value-add.
3. **Live status** — show `idle`, `listening`, `thinking`, `speaking` on the watch face using a WidgetKit timeline that the main app reloads with `WidgetCenter.shared.reloadAllTimelines()`.

> **Note:** ClockKit (the old complication API) is **deprecated in watchOS 10**. New work should use **WidgetKit** with `Widget` + `TimelineProvider` (or `AppIntentTimelineProvider`). The same widget compiles for both the Smart Stack and the legacy complication slots on the watch face.

### Capability matrix

| What you want | How | Notes |
|---|---|---|
| Tap → open the app on the talk view | `widgetURL(URL("seneschal://listen"))` | Simplest. Requires URL scheme registered in Info.plist. |
| Tap → start listening *without* opening the app | `Button(intent: StartListeningIntent())` where `StartListeningIntent: AppIntent & AudioPlaybackIntent` | `AudioPlaybackIntent` runs in the **app's audio process** in the background — it can activate `AVAudioSession` and open the WebSocket. |
| Show live status on the face | `TimelineEntry` with a `Status` enum; main app calls `WidgetCenter.shared.reloadAllTimelines()` whenever the state changes | Limited by the system reload budget. |
| Show a Live Activity in the Smart Stack | `ActivityKit` `Activity<…>` started when the session begins; ends when it ends | Live Activities appear in the **Smart Stack** on watchOS 10+. |
| Boost background refresh budget | Install a complication on the active watch face | Apple: "The system performs multiple tasks an hour for each app with a complication on the active watch face." |

### Why `AudioPlaybackIntent` is the right protocol

When a button on a complication is tapped, the system normally runs the `AppIntent.perform()` in the **widget extension process**, which is heavily sandboxed (no AVFoundation, limited background time). There are four protocols that change this:

| Protocol | Runs in | Designed for |
|---|---|---|
| `AppIntent` (default) | Widget process | Stateless actions (toggle a setting, save a note) |
| `LiveActivityIntent` | App process, background | Updating a Live Activity |
| `AudioPlaybackIntent` | App process, background | **Audio apps — can activate `AVAudioSession` and stream** |
| `ForegroundContinuableIntent` | App process, background | General background work after a foreground app is backgrounded |

For seneschal, `AudioPlaybackIntent` is the perfect match. The system gives it enough background time to start an audio session, open the WebSocket, and begin streaming — without ever opening the main app UI. While the audio session is active, the TN3135 NECP exception also keeps the WebSocket alive in the background.

### Minimal example: tap-to-listen complication

```swift
import WidgetKit
import SwiftUI
import AppIntents

// 1. The intent. AudioPlaybackIntent lets us activate AVAudioSession in
//    the app's audio process — exactly what the streaming case needs.
struct StartListeningIntent: AudioPlaybackIntent {
    static var title: LocalizedStringResource = "Start listening"
    // openAppWhenRun = false  →  don't open the main UI, just run the action
    static var openAppWhenRun: Bool = false

    @Parameter(title: "Server URL")
    var serverURL: String?

    init() {}

    func perform() async throws -> some IntentResult {
        // Activate the audio session and open the WebSocket.
        // The session must already be configured with .playAndRecord for
        // this to satisfy the TN3135 NECP exception.
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(.playAndRecord,
                                options: [.defaultToSpeaker, .allowBluetooth])
        try session.setActive(true)

        // Hand off to the long-running streaming manager (singleton).
        // The manager keeps the WebSocket open and the audio session active.
        await VoiceStreamManager.shared.start(serverURLString: serverURL)

        // Return a status so the widget timeline can update.
        return .result(value: "listening")
    }
}

// 2. The timeline entry — reflects the current state.
struct SeneschalEntry: TimelineEntry {
    let date: Date
    let status: String  // "idle" | "listening" | "thinking" | "speaking"
    let lastTranscript: String?
}

// 3. The provider. Read state from a shared App Group container.
struct SeneschalProvider: TimelineProvider {
    func placeholder(in context: Context) -> SeneschalEntry {
        SeneschalEntry(date: .now, status: "idle", lastTranscript: nil)
    }
    func getSnapshot(in context: Context,
                     completion: @escaping (SeneschalEntry) -> Void) {
        completion(.init(date: .now,
                         status: SharedState.currentStatus(),
                         lastTranscript: SharedState.currentTranscript()))
    }
    func getTimeline(in context: Context,
                     completion: @escaping (Timeline<SeneschalEntry>) -> Void) {
        let entry = SeneschalEntry(
            date: .now,
            status: SharedState.currentStatus(),
            lastTranscript: SharedState.currentTranscript()
        )
        // Refresh in 5 min unless the app reloads us sooner via
        // WidgetCenter.shared.reloadAllTimelines().
        let next = Calendar.current.date(byAdding: .minute, value: 5, to: .now)!
        completion(Timeline(entries: [entry], policy: .after(next)))
    }
}

// 4. The widget view.
struct SeneschalComplicationView: View {
    let entry: SeneschalEntry
    var body: some View {
        VStack(spacing: 2) {
            Image(systemName: icon(for: entry.status))
                .font(.title2)
            Text(entry.status)
                .font(.caption2)
                .lineLimit(1)
        }
        // Tap on the complication body → deep link to the talk view.
        .widgetURL(URL(string: "seneschal://listen"))
    }
    private func icon(for status: String) -> String {
        switch status {
        case "listening": return "mic.fill"
        case "thinking":  return "brain"
        case "speaking":  return "speaker.wave.2.fill"
        default:          return "mic"
        }
    }
}

// 5. The widget definition.
struct SeneschalWidget: Widget {
    let kind = "SeneschalWidget"
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: kind,
                            provider: SeneschalProvider()) { entry in
            SeneschalComplicationView(entry: entry)
        }
        .configurationDisplayName("Seneschal")
        .description("Talk to your voice assistant.")
        .supportedFamilies([
            .accessoryCircular,
            .accessoryRectangular,
            .accessoryInline,
            .accessoryCorner,
        ])
    }
}
```

### Info.plist additions for complications

```xml
<!-- Register the URL scheme so widgetURL(URL) deep links work. -->
<key>CFBundleURLTypes</key>
<array>
  <dict>
    <key>CFBundleURLName</key>
    <string>com.example.seneschal.watchkitapp</string>
    <key>CFBundleURLSchemes</key>
    <array>
      <string>seneschal</string>
    </array>
  </dict>
</array>
```

The watch app handles the URL in its main scene:

```swift
@main
struct SeneschalWatchApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
                .onOpenURL { url in
                    // seneschal://listen  →  jump straight to the talk view
                    if url.host == "listen" {
                        VoiceState.shared.openTalkView()
                    }
                }
        }
    }
}
```

### Pushing live status to the complication

The main watch app (or the `AudioPlaybackIntent` runner) updates a shared App Group store, then calls `WidgetCenter.shared.reloadAllTimelines()`:

```swift
import WidgetKit

@MainActor
func updateComplication(status: String, transcript: String? = nil) {
    SharedState.setStatus(status)
    SharedState.setTranscript(transcript)
    WidgetCenter.shared.reloadAllTimelines()
}
```

### Background refresh and battery

- Widget timelines render in a **separate process**; the widget extension is not always running.
- Budget: 40–70 reloads/day is a reasonable target. More aggressive updates drain the battery and may be throttled.
- Installing a complication on the **active** watch face **doubles the background refresh budget** vs. an app in the dock.
- `Background URLSession` (`URLSessionConfiguration.background`) is the only networking that survives app suspension — use it to receive WebSocket-equivalent data when the app is not running. (For our real-time WebSocket use case, keep the `AVAudioSession` active instead — see "Recommended pattern" below.)

### Recommended pattern (v1)

For a seneschal complication, ship the simplest path first and add complexity only if user data justifies it:

1. **Complication shows a static `mic` icon** (idle state). Tap → `widgetURL("seneschal://listen")` → app opens directly on the talk view. **No background networking.** Zero risk of NECP denial. Ships in Phase 1 alongside the main app.
2. **Status updates** (Phase 1.5): main app calls `WidgetCenter.shared.reloadAllTimelines()` whenever the audio session state changes, so the icon shows `listening` / `thinking` / `speaking` while the user is interacting.
3. **Interactive button** (Phase 2): add a `Button(intent: StartListeningIntent())` on the rectangular complication. With `AudioPlaybackIntent` and an active `AVAudioSession`, the user can start talking without the app ever opening. This is the "magic" use case for a voice assistant complication.

### Live Activities (optional, Phase 2+)

Live Activities on watchOS appear in the **Smart Stack**, not on the watch face. They are a good fit for a session that lasts more than a few seconds (a long assistant conversation):

```swift
import ActivityKit

struct SeneschalActivityAttributes: ActivityAttributes {
    public struct ContentState: Codable, Hashable {
        var status: String  // "listening" | "thinking" | "speaking"
        var lastTranscript: String?
    }
}

// Start the activity when the session begins
let attributes = SeneschalActivityAttributes()
let content = SeneschalActivityAttributes.ContentState(
    status: "listening", lastTranscript: nil)
let activity = try Activity.request(
    attributes: attributes,
    content: ActivityContent(state: content, staleDate: nil))

// Update as the conversation progresses
await activity.update(ActivityContent(
    state: .init(status: "thinking", lastTranscript: transcript),
    staleDate: nil))
```

The same `WidgetKit` extension renders both the watch face complication and the Live Activity (one codebase, two consumers).

## Testing

> **Test on a real Apple Watch.** The watchOS simulator allows low-level networking unconditionally, so NECP denials will not surface there. See TN3135.

1. Start the seneschal with remote support:
   ```bash
   WS_PORT=9090 cargo run --features remote --release
   ```

2. In the watch app, connect to `ws://<your-mac-ip>:9090/ws`

3. Tap the mic button and speak -- you should see the transcript appear and hear the TTS response

4. Test barge-in by speaking while the assistant is responding

5. (Optional) Verify the NECP exception: temporarily remove `UIBackgroundModes` → confirm the WebSocket connection is denied with `Path was denied by NECP policy`. Restore it → confirm it works. This proves the watch app is meeting the exception requirements.
