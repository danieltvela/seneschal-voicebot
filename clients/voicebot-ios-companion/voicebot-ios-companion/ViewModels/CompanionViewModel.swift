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
    @Published var selectedControlPort: String = "9090"
    @Published var isGenerating = false

    private let discoveryManager: DiscoveryManager
    private let messageStore: MessageStore
    private var webSocketManager: WebSocketManager?
    private var relayService: WatchRelayService?
    private let audioManager: AudioManager
    private var historyClient: HistoryClient?
    private var cancellables = Set<AnyCancellable>()
    private var audioTask: Task<Void, Never>?
    private var messageTask: Task<Void, Never>?
    private var bindingTasks: [Task<Void, Never>] = []

    init(discoveryManager: DiscoveryManager? = nil, audioManager: AudioManager? = nil) {
        self.discoveryManager = discoveryManager ?? .init()
        self.audioManager = audioManager ?? .init()
        self.messageStore = MessageStore()

        self.selectedHost = self.discoveryManager.selectedHost
        self.selectedPort = self.discoveryManager.selectedPort
        self.selectedControlPort = self.discoveryManager.selectedControlPort

        // Restore local history
        loadLocalHistory()
    }

    func connect() async {
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
        relayService = WatchRelayService(websocketManager: webSocketManager!)
        relayService?.setAudioCallback { [weak self] data in
            self?.handleIncomingAudio(data)
        }
        relayService?.startRelay()
        historyClient = HistoryClient(host: selectedHost, controlPort: selectedControlPort)
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

        relayService?.stopRelay()
        webSocketManager?.disconnect()
        audioManager.stopCapture()
        audioManager.stopPlayback()

        webSocketManager = nil
        relayService = nil
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

    // MARK: - History

    private func loadLocalHistory() {
        let stored = messageStore.load()
        chatMessages = stored.compactMap { msg -> ChatMessage? in
            guard let role = ChatRole(rawValue: msg.role) else { return nil }
            let date = Date(timeIntervalSince1970: msg.timestamp)
            return ChatMessage(role: role, text: msg.text, timestamp: date)
        }
    }

    private func persistMessage(_ message: ChatMessage) {
        let stored = StoredMessage(
            id: message.id.uuidString,
            role: message.role.rawValue,
            text: message.text,
            timestamp: message.timestamp.timeIntervalSince1970
        )
        messageStore.append([stored])
    }

    private func updatePersistedMessage(_ message: ChatMessage, at index: Int) {
        let allStored = messageStore.load()
        guard index < allStored.count else { return }
        var updated = allStored
        updated[index] = StoredMessage(
            id: message.id.uuidString,
            role: message.role.rawValue,
            text: message.text,
            timestamp: message.timestamp.timeIntervalSince1970
        )
        messageStore.save(updated)
    }

    private func fetchHistoryFromServer() async {
        guard let client = historyClient else { return }
        do {
            let sessions = try await client.fetchSessions()
            guard let currentSession = sessions.first(where: { $0.is_active }) ?? sessions.first else {
                NSLog("HistoryClient: no sessions found")
                return
            }
            let serverMessages = try await client.fetchMessages(sessionId: currentSession.id)

            if !serverMessages.isEmpty {
                let converted: [ChatMessage] = serverMessages.compactMap { msg -> ChatMessage? in
                  guard let role = ChatRole(rawValue: msg.role.lowercased()) else { return nil }
                    let date = Date(timeIntervalSince1970: msg.timestamp)
                    return ChatMessage(role: role, text: msg.text, timestamp: date)
                }
                // Replace local with server-truth (server is source of truth)
                chatMessages = converted
                // Persist to local store
                let stored = converted.map { StoredMessage(
                    id: $0.id.uuidString,
                    role: $0.role.rawValue,
                    text: $0.text,
                    timestamp: $0.timestamp.timeIntervalSince1970
                )}
                messageStore.save(stored)
            }
        } catch {
            errorMessage = "Failed to load history: \(error.localizedDescription)"
            NSLog("History fetch failed: \(error.localizedDescription)")
        }
    }

    // MARK: - Audio Playback

    private func handleIncomingAudio(_ data: Data) {
        Task { @MainActor in
            let samples = int16ToFloat(data)
            await self.audioManager.play(samples)
        }
    }

    // MARK: - Bindings

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
    }

    private func handleMessage(_ message: RemoteMessage) async {
        switch message {
        case .transcript(let text):
            let msg = ChatMessage(role: .user, text: text, timestamp: Date())
            chatMessages.append(msg)
            persistMessage(msg)

        case .responseText(let text):
            isGenerating = true
            if var last = chatMessages.last, last.role == .assistant {
                last.text += text
                let lastIndex = chatMessages.count - 1
                chatMessages[lastIndex] = last
                updatePersistedMessage(last, at: lastIndex)
            } else {
                let msg = ChatMessage(role: .assistant, text: text, timestamp: Date())
                chatMessages.append(msg)
                persistMessage(msg)
            }

        case .responseEnd:
            isGenerating = false
            relayService?.notifyWatchResponseEnd()

        case .audioStart:
            break

        case .audioEnd:
            break

        case .sessionReady:
            connectionState = .connected
            startAudioStreaming()
            // Fetch server history in background
            Task { await fetchHistoryFromServer() }

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
