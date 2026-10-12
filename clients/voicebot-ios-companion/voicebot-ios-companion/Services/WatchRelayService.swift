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
/// TTS downlink:
/// - **Always** plays on the local companion (iPhone / iPad + AirPods / speaker) via `audioCallback`.
/// - **Also** forwards frames to the Watch when it is reachable (glance / wrist speaker).
///
/// Previously, `isReachable == true` *stole* audio from the phone and sent it only to the Watch,
/// which left AirPods silent whenever a paired Watch was nearby.
///
/// Also forwards Watch mic audio → WebSocket for STT, and pushes pipeline state / last assistant line.
final class WatchRelayService: NSObject {
    
    private let websocketManager: WebSocketManager
    private let session: WCSession?
    private var audioRoutingTask: Task<Void, Never>?
    private var audioRoutingGeneration: UInt64 = 0
    
    /// Callback for local companion TTS playback (iPhone / iPad).
    private var audioCallback: ((Data) -> Void)?

    /// Last values pushed to the watch (application context + live messages).
    private var lastPipelineState: String = "unknown"
    private var lastAssistantLine: String = ""
    private var hostSessionActive: Bool = false
    
    init(websocketManager: WebSocketManager) {
        self.websocketManager = websocketManager
        // WCSession is iPhone↔Watch; on iPad (and when unsupported) skip activation.
        if WCSession.isSupported() {
            self.session = WCSession.default
        } else {
            self.session = nil
        }
        super.init()
        configureSession()
    }
    
    // MARK: - Configuration
    
    private func configureSession() {
        guard let session else {
            NSLog("WatchRelayService: WCSession not supported on this device — local audio only")
            return
        }
        session.delegate = self
        session.activate()
    }
    
    /// Set callback for local companion TTS playback.
    func setAudioCallback(_ callback: @escaping (Data) -> Void) {
        self.audioCallback = callback
    }
    
    /// Start the relay. Call when WebSocket connects.
    func startRelay() {
        guard audioRoutingTask == nil else { return }

        audioRoutingGeneration &+= 1
        let generation = audioRoutingGeneration
        
        audioRoutingTask = Task {
            for await audioData in self.websocketManager.audioData {
                guard !Task.isCancelled else { break }
                guard generation == self.audioRoutingGeneration else { break }

                // Always play on the local device (AirPods / speaker / built-in).
                self.audioCallback?(audioData)

                // Optionally mirror to Watch when reachable.
                if let session = self.session, session.isReachable {
                    self.forwardToWatch(audioData)
                }
            }
        }
    }
    
    /// Stop the relay. Call when WebSocket disconnects.
    func stopRelay() {
        audioRoutingGeneration &+= 1
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
        guard let session, session.activationState == .activated else { return }
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
        guard let session, session.activationState == .activated else { return }
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
        guard let session, session.isReachable else { return }
        
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
