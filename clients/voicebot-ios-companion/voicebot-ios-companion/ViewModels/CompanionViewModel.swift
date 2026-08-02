//
//  CompanionViewModel.swift
//  voicebot-ios-companion
//
//  Dual-channel companion: Remote WS (audio) + Control REST/SSE (events/commands).
//

import Foundation
import Combine
import SwiftUI

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

private enum TokenSource: Equatable {
    case none
    case control
    case ws
}

@MainActor
final class CompanionViewModel: ObservableObject {
    /// Aggregated UI connection (legacy badge): prefers audio link, else control-only.
    @Published private(set) var connectionState: ConnectionState = .disconnected
    @Published var chatMessages: [ChatMessage] = []
    @Published var errorMessage: String?
    @Published var selectedHost: String = ""
    @Published var selectedPort: String = "9090"
    @Published var selectedControlPort: String = "9001"
    @Published var isGenerating = false
    @Published var composerText: String = ""

    // MARK: Dual links
    @Published private(set) var audioLink: LinkState = .disconnected
    @Published private(set) var controlLink: LinkState = .disconnected
    @Published var pipelineState: CompanionPipelineState = .unknown
    @Published var ttsMuted: Bool = false
    @Published var pendingPermission: PermissionRequest?
    @Published var controlBanner: String?
    /// Ephemeral tool/system/agent/error events from Control SSE (not chat history).
    @Published private(set) var timeline: [TimelineItem] = []
    /// Classification chip when host emits Classification events.
    @Published var classificationChip: String?

    /// Cap timeline growth in long sessions.
    private let maxTimelineItems = 200

    /// True while a connect() is in flight (disables Connect button).
    var isConnecting: Bool {
        if case .connecting = audioLink { return true }
        if case .connecting = controlLink { return true }
        return false
    }

    /// Session is usable for chat UI (audio and/or control).
    var isSessionActive: Bool {
        switch audioLink {
        case .connected, .connecting, .conflict: return true
        default: break
        }
        switch controlLink {
        case .connected, .connecting, .reconnecting: return true
        default: return false
        }
    }

    /// Mic/TTS audio path available.
    var hasAudioPath: Bool {
        if case .connected = audioLink { return true }
        return false
    }

    private let discoveryManager: DiscoveryManager
    private let messageStore: MessageStore
    private var webSocketManager: WebSocketManager?
    private var relayService: WatchRelayService?
    private let audioManager: AudioManager
    private var historyClient: HistoryClient?
    private var controlClient: ControlClient?
    private var controlSSE: ControlSSEClient?

    private var audioTask: Task<Void, Never>?
    private var messageTask: Task<Void, Never>?
    private var controlTask: Task<Void, Never>?
    private var bindingTasks: [Task<Void, Never>] = []
    private var cancellables = Set<AnyCancellable>()
    private var connectGeneration: UInt64 = 0
    private var sceneIsActive = true

    // Token de-dupe (Control SSE vs WS response.text)
    private var activeUtteranceId: UInt64?
    private var tokenSource: TokenSource = .none
    private var wsTokenBuffer: String = ""
    private var wsBufferFlushTask: Task<Void, Never>?

    init(discoveryManager: DiscoveryManager? = nil, audioManager: AudioManager? = nil) {
        self.discoveryManager = discoveryManager ?? .init()
        self.audioManager = audioManager ?? .init()
        self.messageStore = MessageStore()

        self.selectedHost = self.discoveryManager.selectedHost
        self.selectedPort = self.discoveryManager.selectedPort
        self.selectedControlPort = self.discoveryManager.selectedControlPort

        loadLocalHistory()
    }

    // MARK: - Connect / disconnect

    func connect() async {
        // Tear down any prior session first (bumps generation to cancel old tasks).
        disconnect(preserveHostSettings: true)
        connectGeneration &+= 1
        let generation = connectGeneration

        guard !selectedHost.isEmpty else {
            errorMessage = "Host is required"
            return
        }

        audioLink = .connecting
        controlLink = .connecting
        connectionState = .connecting
        errorMessage = nil
        controlBanner = nil

        // Mic only needed for full audio path; still request early for session.ready.
        let granted = await audioManager.requestMicrophonePermission()
        guard generation == connectGeneration else { return }
        if !granted {
            // Allow Control-only if user denies mic (text + status still useful).
            controlBanner = "Microphone denied — Control-only (text/status) if Control is reachable"
        }

        guard let url = URL(string: "ws://\(selectedHost):\(selectedPort)/ws") else {
            errorMessage = "Invalid server address"
            audioLink = .failed("Invalid URL")
            controlLink = .disconnected
            connectionState = .failed("Invalid URL")
            return
        }

        historyClient = HistoryClient(host: selectedHost, controlPort: selectedControlPort)
        controlClient = ControlClient(host: selectedHost, controlPort: selectedControlPort)
        controlSSE = ControlSSEClient(host: selectedHost, controlPort: selectedControlPort)

        startControlPlane(generation: generation)

        webSocketManager = WebSocketManager(url: url)
        relayService = WatchRelayService(websocketManager: webSocketManager!)
        relayService?.setAudioCallback { [weak self] data in
            self?.handleIncomingAudio(data)
        }
        relayService?.startRelay()
        setupBindings(generation: generation)

        messageTask = Task { [weak self] in
            await self?.webSocketManager?.connect()
        }

        // Watch WS manager state for conflict / connect (Combine — iOS 16+)
        if let ws = webSocketManager {
            ws.$state
                .receive(on: DispatchQueue.main)
                .sink { [weak self] state in
                    guard let self, generation == self.connectGeneration else { return }
                    Task { await self.applyWebSocketState(state) }
                }
                .store(in: &cancellables)
        }
    }

    /// - Parameter preserveHostSettings: when true, keep host/port fields (used by reconnect/connect).
    func disconnect(preserveHostSettings: Bool = false) {
        connectGeneration &+= 1

        audioTask?.cancel()
        audioTask = nil
        messageTask?.cancel()
        messageTask = nil
        controlTask?.cancel()
        controlTask = nil
        wsBufferFlushTask?.cancel()
        wsBufferFlushTask = nil

        bindingTasks.forEach { $0.cancel() }
        bindingTasks.removeAll()
        cancellables.removeAll()

        controlSSE?.stop()
        controlSSE = nil
        controlClient = nil

        relayService?.stopRelay()
        webSocketManager?.disconnect()
        audioManager.stopCapture()
        // Keep playback stop on full disconnect
        audioManager.stopPlayback()

        webSocketManager = nil
        relayService = nil
        historyClient = nil

        audioLink = .disconnected
        controlLink = .disconnected
        connectionState = .disconnected
        pipelineState = .unknown
        pendingPermission = nil
        controlBanner = nil
        timeline = []
        classificationChip = nil
        isGenerating = false
        tokenSource = .none
        activeUtteranceId = nil
        wsTokenBuffer = ""
        if !preserveHostSettings {
            errorMessage = nil
        }
    }

    func clearTimeline() {
        timeline = []
    }

    func handleScenePhase(_ phase: ScenePhase) {
        switch phase {
        case .active:
            sceneIsActive = true
            if case .connected = audioLink {
                startAudioStreaming()
            }
        case .inactive, .background:
            sceneIsActive = false
            stopMicOnly()
        @unknown default:
            break
        }
    }

    // MARK: - Controls

    func bargeIn() {
        Task {
            if let client = controlClient, controlLink.isUsableForControl || isControlConnected {
                do {
                    try await client.bargeIn()
                    return
                } catch {
                    NSLog("Control barge_in failed, falling back to WS: \(error.localizedDescription)")
                }
            }
            guard hasAudioPath else { return }
            do {
                try await webSocketManager?.bargeIn()
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }

    func setMute(_ muted: Bool) {
        Task {
            do {
                try await controlClient?.setMute(muted)
                ttsMuted = muted
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }

    func toggleMute() {
        setMute(!ttsMuted)
    }

    func sendComposerText() {
        let text = composerText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        guard pendingPermission == nil else {
            errorMessage = "Permission pending — answer the agent request first"
            return
        }
        sendTextInput(text)
        composerText = ""
    }

    func sendTextInput(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        if pendingPermission != nil {
            errorMessage = "Permission pending — answer the agent request first"
            return
        }
        guard isControlConnected else {
            errorMessage = "Control API not connected — text input unavailable"
            return
        }
        // Show user bubble immediately
        let msg = ChatMessage(role: .user, text: trimmed, timestamp: Date())
        chatMessages.append(msg)
        persistMessage(msg)

        Task {
            do {
                try await controlClient?.sendInput(trimmed)
            } catch let err as ControlClientError {
                if case .badStatus(let code, _) = err, code == 409 {
                    errorMessage = "Permission pending on host — use permission buttons"
                } else {
                    errorMessage = err.localizedDescription
                }
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
                pendingPermission = nil
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }

    // MARK: - Private helpers

    private var isControlConnected: Bool {
        if case .connected = controlLink { return true }
        if case .reconnecting = controlLink { return true }
        return false
    }

    private func applyWebSocketState(_ state: ConnectionState) async {
        switch state {
        case .connecting:
            audioLink = .connecting
            connectionState = .connecting
        case .connected:
            audioLink = .connected
            connectionState = .connected
            if sceneIsActive {
                startAudioStreaming()
            }
            Task { await fetchHistoryFromServer() }
        case .conflict:
            audioLink = .conflict
            stopMicOnly()
            controlBanner =
                "Audio in use on another device — Control-only (text, mute, status)"
            // Prefer control path for overall UI status
            if isControlConnected {
                connectionState = .connected
            } else {
                connectionState = .conflict
            }
        case .failed(let msg):
            audioLink = .failed(msg)
            stopMicOnly()
            // If control is up, stay in Control-only rather than full fail
            if isControlConnected {
                controlBanner = "Audio link failed — Control-only mode"
                connectionState = .connected
            } else {
                connectionState = .failed(msg)
                errorMessage = msg
            }
        case .disconnected:
            audioLink = .disconnected
            stopMicOnly()
            if !isControlConnected {
                connectionState = .disconnected
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
                chatMessages = converted
                let stored = converted.map {
                    StoredMessage(
                        id: $0.id.uuidString,
                        role: $0.role.rawValue,
                        text: $0.text,
                        timestamp: $0.timestamp.timeIntervalSince1970
                    )
                }
                messageStore.save(stored)
            }
        } catch {
            NSLog("History fetch failed: \(error.localizedDescription)")
        }
    }

    // MARK: - Audio

    private func handleIncomingAudio(_ data: Data) {
        Task { @MainActor in
            let samples = int16ToFloat(data)
            await self.audioManager.play(samples)
        }
    }

    private func stopMicOnly() {
        audioTask?.cancel()
        audioTask = nil
        audioManager.stopCapture()
    }

    private func startAudioStreaming() {
        guard sceneIsActive else { return }
        guard case .connected = audioLink else { return }
        guard audioTask == nil else { return }

        audioTask = Task {
            do {
                try await audioManager.startCapture()
                for await samples in audioManager.capturedAudio {
                    guard !Task.isCancelled else { break }
                    if case .connected = audioLink, let ws = webSocketManager {
                        let data = floatToInt16(samples)
                        try? await ws.send(audioData: data)
                    }
                }
            } catch {
                if !Task.isCancelled {
                    self.errorMessage = error.localizedDescription
                }
            }
        }
    }

    // MARK: - Control plane

    private func startControlPlane(generation: UInt64) {
        guard let client = controlClient, let sse = controlSSE else { return }
        controlLink = .connecting
        controlTask = Task { [weak self] in
            guard let self else { return }
            do {
                let health = try await client.healthCheck()
                guard generation == self.connectGeneration else { return }
                if health.status == "healthy" {
                    controlLink = .connected
                    if controlBanner?.contains("Control port") == true {
                        controlBanner = nil
                    }
                    if let state = try? await client.getState() {
                        pipelineState = state.pipelineState
                        ttsMuted = state.ttsMuted
                    }
                    // History also available without WS
                    await fetchHistoryFromServer()
                    // If audio is conflict/failed, still show session as active
                    if case .conflict = audioLink {
                        connectionState = .connected
                    }
                } else {
                    controlLink = .failed("unexpected health status")
                }
            } catch {
                guard generation == self.connectGeneration else { return }
                controlLink = .failed(error.localizedDescription)
                if case .connected = audioLink {
                    controlBanner =
                        "Check Control port (often 9001) — audio works; status/mute/text limited"
                } else {
                    controlBanner =
                        "Check Control port (often 9001)"
                }
                NSLog("Control health failed: \(error.localizedDescription)")
            }

            // Always try SSE (reconnects internally); if health failed, may still open later
            for await event in sse.events() {
                guard generation == self.connectGeneration else { break }
                if Task.isCancelled { break }
                await handleControlEvent(event)
            }
        }
    }

    private func handleControlEvent(_ event: ControlEvent) async {
        switch event {
        case .stateChanged(let state, let utteranceId, _):
            pipelineState = CompanionPipelineState(hostToken: state)
            if let utteranceId {
                if activeUtteranceId != utteranceId {
                    activeUtteranceId = utteranceId
                    tokenSource = .none
                    wsTokenBuffer = ""
                    wsBufferFlushTask?.cancel()
                }
            }
            if case .failed = controlLink {
                controlLink = .connected
            }
            if !isControlConnected {
                controlLink = .connected
            }
            if controlBanner?.contains("Control port") == true {
                controlBanner = nil
            }

        case .muteChanged(let muted):
            ttsMuted = muted

        case .transcript(_, let text):
            // Prefer Control transcript when available (avoids double user bubbles if WS also fires).
            if let last = chatMessages.last, last.role == .user, last.text == text {
                break
            }
            let msg = ChatMessage(role: .user, text: text, timestamp: Date())
            chatMessages.append(msg)
            persistMessage(msg)

        case .llmToken(let utteranceId, let token):
            claimControlTokens(utteranceId: utteranceId)
            appendAssistantToken(token)
            isGenerating = true

        case .llmDone(let utteranceId, let fullText):
            claimControlTokens(utteranceId: utteranceId)
            finalizeAssistant(fullText: fullText)
            isGenerating = false
            tokenSource = .none
            relayService?.notifyWatchResponseEnd()

        case .agentPermissionRequested(let taskId, let agentName, let description, let options):
            pendingPermission = PermissionRequest(
                taskId: taskId,
                agentName: agentName,
                description: description,
                options: options
            )
            appendTimeline(from: event)

        case .agentPermissionResolved(let taskId, _):
            if pendingPermission?.taskId == taskId {
                pendingPermission = nil
            }
            appendTimeline(from: event)

        case .toolCall, .systemNotification, .mcpNotification,
             .agentTaskStarted, .agentTaskRunning, .agentTaskDelegated,
             .agentTaskFinalizing, .agentTaskCompleted, .agentTaskFailed:
            appendTimeline(from: event)

        case .classification(let intent, let level, let forced, _):
            classificationChip = forced ? "\(intent)*" : "\(intent) · \(level)"
            appendTimeline(from: event)

        case .error(let message):
            if message.contains("Missed") {
                controlBanner = "Missed some events — resynced state"
                if let state = try? await controlClient?.getState() {
                    pipelineState = state.pipelineState
                    ttsMuted = state.ttsMuted
                }
            } else {
                errorMessage = message
                appendTimeline(from: event)
            }

        case .ttsStart, .unknown:
            break
        }
    }

    private func appendTimeline(from event: ControlEvent) {
        guard let item = TimelineItem.from(controlEvent: event) else { return }
        // Upsert latest agent row by task_id+status family: keep history of lifecycle steps.
        timeline.append(item)
        if timeline.count > maxTimelineItems {
            timeline.removeFirst(timeline.count - maxTimelineItems)
        }
    }

    private func claimControlTokens(utteranceId: UInt64) {
        activeUtteranceId = utteranceId
        if tokenSource != .control {
            tokenSource = .control
            // Drop any WS-buffered tokens for this utterance
            wsTokenBuffer = ""
            wsBufferFlushTask?.cancel()
            wsBufferFlushTask = nil
        }
    }

    // MARK: - WS bindings

    private func setupBindings(generation: UInt64) {
        guard let ws = webSocketManager else { return }

        let messageBinding = Task { [weak self] in
            for await message in ws.messages {
                guard let self, generation == self.connectGeneration else { return }
                await self.handleMessage(message)
            }
        }
        bindingTasks.append(messageBinding)

        let errorBinding = Task { [weak self] in
            for await error in ws.errors {
                guard let self, generation == self.connectGeneration else { return }
                // Conflict already applied via $state; avoid double banners
                if case WebSocketError.connectionFailed(409) = error {
                    continue
                }
                // Non-conflict errors
                if case .conflict = self.audioLink { continue }
                self.errorMessage = error.localizedDescription
            }
        }
        bindingTasks.append(errorBinding)
    }

    private func handleMessage(_ message: RemoteMessage) async {
        switch message {
        case .transcript(let text):
            // If Control is connected, transcript SSE may also arrive — de-dupe by last user text
            if isControlConnected,
               let last = chatMessages.last,
               last.role == .user,
               last.text == text
            {
                return
            }
            let msg = ChatMessage(role: .user, text: text, timestamp: Date())
            chatMessages.append(msg)
            persistMessage(msg)

        case .responseText(let text):
            await handleWSResponseText(text)

        case .responseEnd:
            await handleWSResponseEnd()

        case .audioStart, .audioEnd:
            break

        case .sessionReady:
            // State also comes from $state publisher; ensure audio starts
            audioLink = .connected
            connectionState = .connected
            if sceneIsActive {
                startAudioStreaming()
            }
            Task { await fetchHistoryFromServer() }

        case .error(let msg):
            errorMessage = msg
            if !isControlConnected {
                connectionState = .failed(msg)
            }
            stopMicOnly()

        case .sessionStart, .bargeIn:
            break
        }
    }

    private func handleWSResponseText(_ text: String) async {
        // Control owns tokens for this utterance
        if tokenSource == .control {
            return
        }

        if isControlConnected && tokenSource == .none {
            // Buffer briefly waiting for Control SSE
            wsTokenBuffer += text
            isGenerating = true
            if wsBufferFlushTask == nil {
                wsBufferFlushTask = Task { [weak self] in
                    try? await Task.sleep(nanoseconds: 500_000_000)
                    guard let self, !Task.isCancelled else { return }
                    await self.flushWSBufferIfStillNeeded()
                }
            }
            return
        }

        // Control down or already committed to WS
        tokenSource = .ws
        appendAssistantToken(text)
        isGenerating = true
    }

    private func flushWSBufferIfStillNeeded() async {
        guard tokenSource != .control else {
            wsTokenBuffer = ""
            return
        }
        if !wsTokenBuffer.isEmpty {
            tokenSource = .ws
            appendAssistantToken(wsTokenBuffer)
            wsTokenBuffer = ""
        }
        wsBufferFlushTask = nil
    }

    private func handleWSResponseEnd() async {
        if tokenSource == .control {
            // Control will finalize via llm_done
            return
        }
        // Commit any remaining buffer
        if !wsTokenBuffer.isEmpty {
            appendAssistantToken(wsTokenBuffer)
            wsTokenBuffer = ""
        }
        isGenerating = false
        tokenSource = .none
        relayService?.notifyWatchResponseEnd()
    }

    // MARK: - Assistant bubble helpers

    private func appendAssistantToken(_ token: String) {
        if var last = chatMessages.last, last.role == .assistant {
            last.text += token
            let lastIndex = chatMessages.count - 1
            chatMessages[lastIndex] = last
            updatePersistedMessage(last, at: lastIndex)
        } else {
            let msg = ChatMessage(role: .assistant, text: token, timestamp: Date())
            chatMessages.append(msg)
            persistMessage(msg)
        }
    }

    private func finalizeAssistant(fullText: String) {
        if var last = chatMessages.last, last.role == .assistant {
            // Prefer full text from Control when available
            if !fullText.isEmpty {
                last.text = fullText
            }
            let lastIndex = chatMessages.count - 1
            chatMessages[lastIndex] = last
            updatePersistedMessage(last, at: lastIndex)
        } else if !fullText.isEmpty {
            let msg = ChatMessage(role: .assistant, text: fullText, timestamp: Date())
            chatMessages.append(msg)
            persistMessage(msg)
        }
    }
}
