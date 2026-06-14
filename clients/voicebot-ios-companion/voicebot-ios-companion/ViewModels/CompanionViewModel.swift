//
//  CompanionViewModel.swift
//  voicebot-ios-companion
//
//  Created by Dani Vela on 13/06/2026.
//

import Foundation
import Combine

enum ChatRole: String, Sendable {
    case user
    case assistant
}

struct ChatMessage: Identifiable, Sendable {
    let id = UUID()
    let role: ChatRole
    var text: String
    let timestamp: Date
}

@MainActor
final class CompanionViewModel: ObservableObject {
    @Published var connectionState: ConnectionState = .disconnected
    @Published var chatMessages: [ChatMessage] = []
    @Published var errorMessage: String?
    @Published var selectedHost: String = ""
    @Published var selectedPort: String = "9090"
    
    private let discoveryManager: DiscoveryManager
    private var webSocketManager: WebSocketManager?
    private let audioManager: AudioManager
    private var cancellables = Set<AnyCancellable>()
    private var audioTask: Task<Void, Never>?
    private var messageTask: Task<Void, Never>?
    private var bindingTasks: [Task<Void, Never>] = []
    
    init(discoveryManager: DiscoveryManager? = nil, audioManager: AudioManager? = nil) {
        self.discoveryManager = discoveryManager ?? .init()
        self.audioManager = audioManager ?? .init()
        
        self.selectedHost = self.discoveryManager.selectedHost
        self.selectedPort = self.discoveryManager.selectedPort
    }
    
    func connect() async {
        // Tear down any previous connection before starting a new one.
        disconnect()
        
        let granted = await audioManager.requestMicrophonePermission()
        guard granted else {
            errorMessage = "Microphone permission required"
            return
        }
        
        guard let url = URL(string: "ws://\(selectedHost):\(selectedPort)/ws") else {
            errorMessage = "Invalid server address"
            return
        }
        
        webSocketManager = WebSocketManager(url: url)
        setupBindings()
        
        messageTask = Task {
            await webSocketManager?.connect()
        }
        
        connectionState = .connecting
    }
    
    func disconnect() {
        audioTask?.cancel()
        audioTask = nil
        messageTask?.cancel()
        messageTask = nil
        
        bindingTasks.forEach { $0.cancel() }
        bindingTasks.removeAll()
        
        webSocketManager?.disconnect()
        audioManager.stopCapture()
        audioManager.stopPlayback()
        
        webSocketManager = nil
        connectionState = .disconnected
    }
    
    func bargeIn() {
        Task {
            do {
                try await webSocketManager?.bargeIn()
            } catch {
                self.errorMessage = error.localizedDescription
            }
        }
    }
    
    private func setupBindings() {
        guard let ws = webSocketManager else { return }
        
        let messageBinding = Task {
            for await message in ws.messages {
                await handleMessage(message)
            }
        }
        bindingTasks.append(messageBinding)
        
        let errorBinding = Task {
            for await error in ws.errors {
                errorMessage = error.localizedDescription
                connectionState = .failed(error.localizedDescription)
                audioTask?.cancel()
                audioTask = nil
            }
        }
        bindingTasks.append(errorBinding)
        
        let audioBinding = Task {
            for await data in ws.audioData {
                guard !Task.isCancelled else { break }
                let samples = int16ToFloat(data)
                await audioManager.play(samples)
            }
        }
        bindingTasks.append(audioBinding)
    }
    
    private func handleMessage(_ message: RemoteMessage) async {
        switch message {
        case .transcript(let text):
            chatMessages.append(ChatMessage(role: .user, text: text, timestamp: Date()))
            
        case .responseText(let text):
            if var last = chatMessages.last, last.role == .assistant {
                last.text += text
                chatMessages[chatMessages.count - 1] = last
            } else {
                chatMessages.append(ChatMessage(role: .assistant, text: text, timestamp: Date()))
            }
            
        case .responseEnd:
            break
            
        case .audioStart:
            break
            
        case .audioEnd:
            break
            
        case .sessionReady:
            connectionState = .connected
            startAudioStreaming()
            
        case .error(let msg):
            errorMessage = msg
            connectionState = .failed(msg)
            audioTask?.cancel()
            audioTask = nil
            
        case .sessionStart, .bargeIn:
            break
        }
    }
    
    private func startAudioStreaming() {
        guard audioTask == nil else { return }
        
        audioTask = Task {
            do {
                try await audioManager.startCapture()
                for await samples in audioManager.capturedAudio {
                    guard !Task.isCancelled else { break }
                    if connectionState == .connected, let ws = webSocketManager {
                        let data = floatToInt16(samples)
                        try? await ws.send(audioData: data)
                    }
                }
            } catch {
                self.errorMessage = error.localizedDescription
            }
        }
    }
}
