//
//  WatchViewModel.swift
//  voicebot-watchos-companion Watch App
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

@MainActor
final class WatchViewModel: NSObject, ObservableObject {
    @Published var appState: WatchAppState = .idle
    @Published var isConnected = false
    @Published var isRecording = false
    @Published var statusText = "Tap to Start"
    
    private var audioManager: WatchAudioManager?
    
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
                self.statusText = "Listening..."
                WKInterfaceDevice.current().play(.start)
                
                // Notify iPhone that recording started
                WCSession.default.sendMessage(
                    ["type": "recording_started"],
                    replyHandler: nil,
                    errorHandler: { error in
                        NSLog("Watch: sendMessage error: \(error.localizedDescription)")
                    }
                )
                
                // Send audio chunks to iPhone via transferUserInfo (reliable, queued delivery)
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
                self.statusText = "Tap to Talk"
            }
        }
    }
    
    func stopRecording() {
        audioManager?.stopCapture()
        audioManager = nil
        self.isRecording = false
        
        // Notify iPhone that recording stopped
        WCSession.default.sendMessage(
            ["type": "recording_stopped"],
            replyHandler: nil,
            errorHandler: { error in
                NSLog("Watch: sendMessage error: \(error.localizedDescription)")
            }
        )
        
        WKInterfaceDevice.current().play(.stop)
        self.appState = .responding
        self.statusText = "Responding..."
    }
}

extension WatchViewModel: WCSessionDelegate {
    func session(_ session: WCSession, activationDidCompleteWith activationState: WCSessionActivationState, error: Error?) {
        if activationState == .activated {
            self.isConnected = true
            self.appState = .connected
            self.statusText = "Tap to Talk"
        } else {
            self.isConnected = false
            self.appState = .idle
            self.statusText = "Disconnected"
        }
    }
    
    func session(_ session: WCSession, didReceiveMessage message: [String: Any]) {
        if let type = message["type"] as? String {
            if type == "response_end" {
                self.appState = .connected
                self.statusText = "Tap to Talk"
                WKInterfaceDevice.current().play(.success)
            }
        }
    }
    
    func session(_ session: WCSession, didReceiveMessageData data: Data) {
        // Received audio from iPhone - play it
        if let am = audioManager {
            am.playAudio(data)
        }
    }
    
    func sessionReachabilityDidChange(_ session: WCSession) {
        if !session.isReachable {
            self.isConnected = false
            self.appState = .idle
            self.statusText = "Disconnected"
        } else if session.activationState == .activated {
            self.isConnected = true
            self.appState = .connected
            self.statusText = "Tap to Talk"
        }
    }
}
