//
//  WatchRelayService.swift
//  voicebot-ios-companion
//
//  Created by Dani Vela on 13/06/2026.
//

import Foundation
import WatchKit

/// Bridges audio between the Watch (WCSession) and Voicebot (WebSocket).
///
/// Routes audio based on Watch connectivity:
/// - Watch connected: Forwards WebSocket audio → Watch speaker
/// - Watch disconnected: Calls audioCallback for iPhone playback
///
/// Also forwards Watch audio → WebSocket for STT.
final class WatchRelayService: NSObject {
    
    private let websocketManager: WebSocketManager
    private let session: WCSession
    private var audioRoutingTask: Task<Void, Never>?
    
    /// Callback for iPhone audio playback (when Watch is disconnected).
    private var audioCallback: ((Data) -> Void)?
    
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
                if self.session.isComplicationEnabled {
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
    
    /// Forward audio to Watch.
    private func forwardToWatch(_ audioData: Data) {
        guard session.isReachable else { return }
        
        let tempDir = FileManager.default.temporaryDirectory
        let fileURL = tempDir.appendingPathComponent("voicebot_audio_\(UUID().uuidString).dat")
        
        do {
            try audioData.write(to: fileURL, options: .atomic)
            try session.transferFile(fileURL)
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
        case .inactive, .notActivated:
            NSLog("WatchRelayService: Watch deactivated")
        @unknown default:
            break
        }
    }
    
    func session(_ session: WCSession, didReceiveMessageData messageData: Data, handler: @escaping (Data) -> Void) {
        // Audio from Watch → forward to WebSocket
        self.forwardToWebSocket(messageData)
        handler(Data())
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
    }
}
