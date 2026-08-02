//
//  WatchRelayService.swift
//  voicebot-ios-companion
//
//  Created by Dani Vela on 13/06/2026.
//

import Foundation
import WatchConnectivity

/// Bridges audio + glance state between the Watch (WCSession) and Seneschal (WebSocket/Control).
///
/// Routes audio based on Watch connectivity:
/// - Watch connected: Forwards WebSocket audio → Watch speaker
/// - Watch disconnected: Calls audioCallback for iPhone playback
///
/// Also forwards Watch audio → WebSocket for STT, and pushes pipeline state / last assistant line.
final class WatchRelayService: NSObject {
    
    private let websocketManager: WebSocketManager
    private let session: WCSession
    private var audioRoutingTask: Task<Void, Never>?
    
    /// Callback for iPhone audio playback (when Watch is disconnected).
    private var audioCallback: ((Data) -> Void)?

    /// Last values pushed to the watch (application context + live messages).
    private var lastPipelineState: String = "unknown"
    private var lastAssistantLine: String = ""
    private var hostSessionActive: Bool = false
    
    init(websocketManager: WebSocketManager) {
        self.websocketManager = websocketManager
        self.session = WCSession.default
        super.init()
        configureSession()
    }
    
    // MARK: - Configuration
    
    private func configureSession() {
        session.delegate = self
        session.activate()
    }
    
    /// Set callback for iPhone audio playback.
    /// Called when Watch is disconnected and audio should play on iPhone.
    func setAudioCallback(_ callback: @escaping (Data) -> Void) {
        self.audioCallback = callback
    }
    
    /// Start the relay. Call when WebSocket connects.
    func startRelay() {
        guard audioRoutingTask == nil else { return }
        
        audioRoutingTask = Task {
            for await audioData in self.websocketManager.audioData {
                // Route audio based on Watch connectivity
                if self.session.isReachable {
                    // Watch connected - forward to Watch
                    self.forwardToWatch(audioData)
                } else {
                    // Watch disconnected - play on iPhone
                    self.audioCallback?(audioData)
                }
            }
        }
    }
    
    /// Stop the relay. Call when WebSocket disconnects.
    func stopRelay() {
        audioRoutingTask?.cancel()
        audioRoutingTask = nil
    }
    
    /// Forward audio to WebSocket (from Watch).
    func forwardToWebSocket(_ audioData: Data) {
        Task {
            try? await self.websocketManager.send(audioData: audioData)
        }
    }
    
    /// Notify the Watch that the LLM response has ended.
    func notifyWatchResponseEnd() {
        sendWatchMessage(["type": "response_end"])
    }

    /// Push host pipeline state token (`idle` | `listening` | …) to the Watch.
    func notifyWatchPipelineState(_ state: String) {
        lastPipelineState = state
        hostSessionActive = true
        pushApplicationContext()
        sendWatchMessage([
            "type": "pipeline_state",
            "state": state,
        ])
    }

    /// Push a short preview of the last assistant line (truncated for glance UI).
    func notifyWatchLastLine(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        let maxLen = 120
        if trimmed.count > maxLen {
            let idx = trimmed.index(trimmed.startIndex, offsetBy: maxLen)
            lastAssistantLine = String(trimmed[..<idx]) + "…"
        } else {
            lastAssistantLine = trimmed
        }
        pushApplicationContext()
        sendWatchMessage([
            "type": "last_line",
            "text": lastAssistantLine,
        ])
    }

    /// Tell the Watch whether the iPhone is connected to the host.
    func notifyWatchHostSession(active: Bool) {
        hostSessionActive = active
        if !active {
            lastPipelineState = "unknown"
        }
        pushApplicationContext()
        sendWatchMessage([
            "type": "host_session",
            "active": active,
        ])
    }

    private func sendWatchMessage(_ payload: [String: Any]) {
        guard session.activationState == .activated else { return }
        guard session.isReachable else { return }
        session.sendMessage(
            payload,
            replyHandler: nil,
            errorHandler: { error in
                NSLog("WatchRelayService: sendMessage \(payload["type"] ?? "?") failed: \(error.localizedDescription)")
            }
        )
    }

    /// Persist glance state for when the watch wakes offline from the phone UI.
    private func pushApplicationContext() {
        guard session.activationState == .activated else { return }
        let ctx: [String: Any] = [
            "pipeline_state": lastPipelineState,
            "last_line": lastAssistantLine,
            "host_session": hostSessionActive,
        ]
        do {
            try session.updateApplicationContext(ctx)
        } catch {
            NSLog("WatchRelayService: updateApplicationContext failed: \(error.localizedDescription)")
        }
    }
    
    /// Forward audio to Watch.
    private func forwardToWatch(_ audioData: Data) {
        guard session.isReachable else { return }
        
        let tempDir = FileManager.default.temporaryDirectory
        let fileURL = tempDir.appendingPathComponent("voicebot_audio_\(UUID().uuidString).dat")
        
        do {
            try audioData.write(to: fileURL, options: .atomic)
            session.transferFile(fileURL, metadata: nil)
        } catch {
            NSLog("WatchRelayService: transfer failed: \(error.localizedDescription)")
        }
    }
}

// MARK: - WCSessionDelegate

extension WatchRelayService: WCSessionDelegate {
    
    func session(_ session: WCSession, activationDidCompleteWith activationState: WCSessionActivationState, error: Error?) {
        switch activationState {
        case .activated:
            NSLog("WatchRelayService: Watch activated")
            // Re-push glance state so a newly reachable watch gets current pipeline/last line.
            pushApplicationContext()
        case .inactive, .notActivated:
            NSLog("WatchRelayService: Watch deactivated")
        @unknown default:
            break
        }
    }
    
    func session(_ session: WCSession, didReceiveMessageData messageData: Data, replyHandler: @escaping (Data) -> Void) {
        // Audio from Watch → forward to WebSocket
        self.forwardToWebSocket(messageData)
        replyHandler(Data())
    }
    
    func session(_ session: WCSession, didReceiveMessage message: [String: Any]) {
        if let type = message["type"] as? String {
            switch type {
            case "recording_started":
                NSLog("WatchRelayService: recording started")
            case "recording_stopped":
                NSLog("WatchRelayService: recording stopped")
            default:
                break
            }
        }
    }
    
    func sessionReachabilityDidChange(_ session: WCSession) {
        NSLog("WatchRelayService: reachability changed: \(session.isReachable)")
        if session.isReachable {
            pushApplicationContext()
            sendWatchMessage([
                "type": "pipeline_state",
                "state": lastPipelineState,
            ])
            if !lastAssistantLine.isEmpty {
                sendWatchMessage([
                    "type": "last_line",
                    "text": lastAssistantLine,
                ])
            }
        }
    }
    
    #if os(iOS)
    func sessionDidBecomeInactive(_ session: WCSession) {
        NSLog("WatchRelayService: session became inactive")
    }
    
    func sessionDidDeactivate(_ session: WCSession) {
        NSLog("WatchRelayService: session deactivated")
        session.activate()
    }
    #endif
}
