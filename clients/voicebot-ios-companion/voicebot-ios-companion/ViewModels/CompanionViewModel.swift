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
    /// Default Control API port (host `CONTROL_PORT=9001`).
    @Published var selectedControlPort: String = "9001"
    @Published var isGenerating = false

    // MARK: Control plane (PR3 networking; UI polish in PR4+)
    @Published var controlLink: ControlLinkState = .disconnected
    @Published var pipelineState: CompanionPipelineState = .unknown
    @Published var ttsMuted: Bool = false
    @Published var pendingPermission: PermissionRequest?
    @Published var controlBanner: String?

    private let discoveryManager: DiscoveryManager
    private let messageStore: MessageStore
    private var webSocketManager: WebSocketManager?
    private var relayService: WatchRelayService?
    private let audioManager: AudioManager
    private var historyClient: HistoryClient?
    private var controlClient: ControlClient?
    private var controlSSE: ControlSSEClient?
    private var cancellables = Set<AnyCancellable>()
    private var audioTask: Task<Void, Never>?
    private var messageTask: Task<Void, Never>?
    private var controlTask: Task<Void, Never>?
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
        controlClient = ControlClient(host: selectedHost, controlPort: selectedControlPort)
        controlSSE = ControlSSEClient(host: selectedHost, controlPort: selectedControlPort)
        setupBindings()
        startControlPlane()
        
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
        controlTask?.cancel()
        controlTask = nil

        bindingTasks.forEach { $0.cancel() }
        bindingTasks.removeAll()

        controlSSE?.stop()
        controlSSE = nil
        controlClient = nil

        relayService?.stopRelay()
        webSocketManager?.disconnect()
        audioManager.stopCapture()
        audioManager.stopPlayback()

        webSocketManager = nil
        relayService = nil
        historyClient = nil
        connectionState = .disconnected
        controlLink = .disconnected
        pipelineState = .unknown
        pendingPermission = nil
        controlBanner = nil
    }

    func bargeIn() {
        Task {
            // Prefer Control REST; fall back to WebSocket barge_in.
            if let client = controlClient {
                do {
                    try await client.bargeIn()
                    return
                } catch {
                    NSLog("Control barge_in failed, falling back to WS: \(error.localizedDescription)")
                }
            }
            do {
                try await webSocketManager?.bargeIn()
            } catch {
                self.errorMessage = error.localizedDescription
            }
        }
    }

    func setMute(_ muted: Bool) {
        Task {
            do {
                try await controlClient?.setMute(muted)
                // Optimistic; SSE MuteChanged will confirm.
                ttsMuted = muted
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }

    func sendTextInput(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        if pendingPermission != nil {
            errorMessage = "Permission pending — answer the agent request first"
            return
        }
        Task {
            do {
                try await controlClient?.sendInput(trimmed)
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }

    func resolvePermission(optionId: String) {
        guard let pending = pendingPermission else { return }
        Task {
            do {
                try await controlClient?.resolvePermission(
                    taskId: pending.taskId,
                    optionId: optionId
                )
                // Cleared on agent_permission_resolved; clear optimistically too.
                pendingPermission = nil
            } catch {
                errorMessage = error.localizedDescription
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

    // MARK: - Control plane

    private func startControlPlane() {
        guard let client = controlClient, let sse = controlSSE else { return }
        controlLink = .connecting
        controlTask = Task { [weak self] in
            guard let self else { return }
            // Health probe — non-fatal if WS works but Control is wrong port.
            do {
                let health = try await client.healthCheck()
                if Task.isCancelled { return }
                if health.status == "healthy" {
                    controlLink = .connected
                    controlBanner = nil
                    if let state = try? await client.getState() {
                        pipelineState = state.pipelineState
                        ttsMuted = state.ttsMuted
                    }
                } else {
                    controlLink = .failed("unexpected health status")
                }
            } catch {
                if Task.isCancelled { return }
                controlLink = .failed(error.localizedDescription)
                controlBanner =
                    "Check Control port (often 9001) — WS may still work without it."
                NSLog("Control health failed: \(error.localizedDescription)")
            }

            for await event in sse.events() {
                if Task.isCancelled { break }
                await handleControlEvent(event)
            }
        }
    }

    private func handleControlEvent(_ event: ControlEvent) async {
        switch event {
        case .stateChanged(let state, _, _):
            pipelineState = CompanionPipelineState(hostToken: state)
            if controlLink != .connected {
                controlLink = .connected
                controlBanner = nil
            }
        case .muteChanged(let muted):
            ttsMuted = muted
        case .llmToken, .llmDone:
            // Tokens will drive conversation UI in PR4; mark generating for now.
            if case .llmToken = event { isGenerating = true }
            if case .llmDone = event { isGenerating = false }
        case .agentPermissionRequested(let taskId, let agentName, let description, let options):
            pendingPermission = PermissionRequest(
                taskId: taskId,
                agentName: agentName,
                description: description,
                options: options
            )
        case .agentPermissionResolved(let taskId, _):
            if pendingPermission?.taskId == taskId {
                pendingPermission = nil
            }
        case .error(let message):
            if message.contains("Missed") {
                // Lag — non-fatal; resync state.
                if let state = try? await controlClient?.getState() {
                    pipelineState = state.pipelineState
                    ttsMuted = state.ttsMuted
                }
            } else {
                errorMessage = message
            }
        default:
            break
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
