//
//  WatchViewModel.swift
//  voicebot-watchos-companion Watch App
//
//  PTT surface + glance: pipeline state color/text and last assistant line (via iPhone).
//

import Foundation
import WatchConnectivity
import WatchKit
import Combine

enum WatchAppState: String {
    case idle
    case connecting
    case connected
    case recording
    case responding
}

/// Host pipeline tokens mirrored from the iPhone companion (Control SSE).
enum WatchPipelineState: String {
    case idle
    case listening
    case thinking
    case speaking
    case paused
    case unknown

    init(token: String) {
        self = WatchPipelineState(rawValue: token.lowercased()) ?? .unknown
    }

    var displayLabel: String {
        switch self {
        case .idle: return "Idle"
        case .listening: return "Listening"
        case .thinking: return "Thinking"
        case .speaking: return "Speaking"
        case .paused: return "Paused"
        case .unknown: return "—"
        }
    }
}

@MainActor
final class WatchViewModel: NSObject, ObservableObject {
    @Published var appState: WatchAppState = .idle
    @Published var isConnected = false
    @Published var isRecording = false
    @Published var statusText = "Open iPhone app"
    @Published var pipelineState: WatchPipelineState = .unknown
    /// Truncated last assistant reply for glance (from iPhone).
    @Published var lastLine: String = ""
    /// True when iPhone reports an active Seneschal host session.
    @Published var hostSessionActive = false

    private var audioManager: WatchAudioManager?
    /// Playback-only manager so TTS can play when not recording.
    private var playbackManager: WatchAudioManager?

    override init() {
        super.init()
        setupSession()
    }

    private func setupSession() {
        guard WCSession.isSupported() else {
            self.statusText = "Unavailable"
            return
        }
        WCSession.default.delegate = self
        WCSession.default.activate()
    }

    func startRecording() {
        guard isConnected else { return }

        Task {
            do {
                let am = WatchAudioManager()
                try am.startCapture()
                self.audioManager = am

                self.isRecording = true
                self.appState = .recording
                self.statusText = "Listening…"
                WKInterfaceDevice.current().play(.start)

                WCSession.default.sendMessage(
                    ["type": "recording_started"],
                    replyHandler: nil,
                    errorHandler: { error in
                        NSLog("Watch: sendMessage error: \(error.localizedDescription)")
                    }
                )

                for await audioData in am.capturedAudio {
                    guard WCSession.default.isReachable else { break }
                    WCSession.default.sendMessageData(
                        audioData,
                        replyHandler: nil,
                        errorHandler: { error in
                            NSLog("Watch: sendMessageData error: \(error.localizedDescription)")
                        }
                    )
                }
            } catch {
                self.appState = .connected
                self.refreshStatusText()
            }
        }
    }

    func stopRecording() {
        audioManager?.stopCapture()
        audioManager = nil
        self.isRecording = false

        WCSession.default.sendMessage(
            ["type": "recording_stopped"],
            replyHandler: nil,
            errorHandler: { error in
                NSLog("Watch: sendMessage error: \(error.localizedDescription)")
            }
        )

        WKInterfaceDevice.current().play(.stop)
        self.appState = .responding
        self.statusText = "Responding…"
    }

    private func refreshStatusText() {
        if isRecording {
            statusText = "Listening…"
            return
        }
        if appState == .responding {
            statusText = "Responding…"
            return
        }
        if !isConnected {
            statusText = "Open iPhone app"
            return
        }
        if hostSessionActive {
            statusText = pipelineState == .unknown ? "Tap to Talk" : pipelineState.displayLabel
        } else {
            statusText = "Connect host on iPhone"
        }
    }

    private func applyPipelineToken(_ token: String) {
        pipelineState = WatchPipelineState(token: token)
        if !isRecording, appState != .responding {
            refreshStatusText()
        }
        // Map host pipeline into app chrome when not mid-PTT
        if !isRecording {
            switch pipelineState {
            case .speaking, .thinking:
                if appState != .responding { appState = .responding }
            case .listening:
                break
            case .idle, .paused, .unknown:
                if appState == .responding {
                    appState = .connected
                }
            }
        }
    }

    private func applyContext(_ ctx: [String: Any]) {
        if let state = ctx["pipeline_state"] as? String {
            applyPipelineToken(state)
        }
        if let line = ctx["last_line"] as? String {
            lastLine = line
        }
        if let active = ctx["host_session"] as? Bool {
            hostSessionActive = active
            if !isRecording {
                refreshStatusText()
            }
        }
    }
}

extension WatchViewModel: WCSessionDelegate {
    func session(_ session: WCSession, activationDidCompleteWith activationState: WCSessionActivationState, error: Error?) {
        Task { @MainActor in
            if activationState == .activated {
                self.isConnected = true
                self.appState = .connected
                // Application context may already have pipeline/last_line from iPhone.
                self.applyContext(session.receivedApplicationContext)
                self.refreshStatusText()
            } else {
                self.isConnected = false
                self.appState = .idle
                self.statusText = "Disconnected"
            }
        }
    }

    func session(_ session: WCSession, didReceiveMessage message: [String: Any]) {
        Task { @MainActor in
            guard let type = message["type"] as? String else { return }
            switch type {
            case "response_end":
                self.appState = .connected
                self.refreshStatusText()
                WKInterfaceDevice.current().play(.success)
            case "pipeline_state":
                if let state = message["state"] as? String {
                    self.applyPipelineToken(state)
                }
            case "last_line":
                if let text = message["text"] as? String {
                    self.lastLine = text
                }
            case "host_session":
                if let active = message["active"] as? Bool {
                    self.hostSessionActive = active
                    if !active {
                        self.pipelineState = .unknown
                    }
                    self.refreshStatusText()
                }
            default:
                break
            }
        }
    }

    func session(_ session: WCSession, didReceiveApplicationContext applicationContext: [String: Any]) {
        Task { @MainActor in
            self.applyContext(applicationContext)
        }
    }

    func session(_ session: WCSession, didReceiveMessageData data: Data) {
        Task { @MainActor in
            // Prefer active capture session's player; else dedicated playback manager.
            if let am = self.audioManager {
                am.playAudio(data)
            } else {
                if self.playbackManager == nil {
                    self.playbackManager = WatchAudioManager()
                }
                self.playbackManager?.playAudio(data)
            }
        }
    }

    func sessionReachabilityDidChange(_ session: WCSession) {
        Task { @MainActor in
            if !session.isReachable {
                // Keep last glance data; only mark phone link soft-down.
                self.isConnected = session.activationState == .activated
                if !session.isReachable {
                    // Still activated but not reachable — show soft status
                    if self.isConnected, !self.isRecording {
                        self.statusText = "iPhone unreachable"
                    }
                }
            } else if session.activationState == .activated {
                self.isConnected = true
                if self.appState == .idle {
                    self.appState = .connected
                }
                self.applyContext(session.receivedApplicationContext)
                if !self.isRecording {
                    self.refreshStatusText()
                }
            }
        }
    }
}
