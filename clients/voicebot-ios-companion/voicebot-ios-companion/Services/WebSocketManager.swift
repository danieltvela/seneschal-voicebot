//
//  WebSocketManager.swift
//  voicebot-ios-companion
//
//  Created by Dani Vela on 13/06/2026.
//

import Foundation
import Combine
import os.log

enum ConnectionState: Equatable, Sendable {
    case disconnected
    case connecting
    case connected
    case failed(String)
}

enum WebSocketError: Error, Sendable {
    case connectionFailed(Int)
    case alreadyConnected
    case sendFailed
    case decodeFailed(Error)
    case interrupted
}

final class WebSocketManager: ObservableObject {
    @Published private(set) var state: ConnectionState = .disconnected
    @Published private(set) var errorMessage: String?
    
    private static let logger = Logger(subsystem: Bundle.main.bundleIdentifier ?? "voicebot-ios-companion", category: "WebSocket")
    
    private var session: URLSession?
    private var webSocketTask: URLSessionWebSocketTask?
    private let url: URL
    private var messageContinuation: AsyncStream<RemoteMessage>.Continuation?
    private var errorContinuation: AsyncStream<Error>.Continuation?
    private var audioContinuation: AsyncStream<Data>.Continuation?
    private var messageStream: AsyncStream<RemoteMessage>?
    private var errorStream: AsyncStream<Error>?
    private var audioStream: AsyncStream<Data>?
    private var reconnectTimer: Timer?
    private var reconnectAttempt = 0
    private let maxReconnectAttempts = 5
    private let baseReconnectDelay: TimeInterval = 1.0
    private var isManuallyDisconnected = false
    
    init(url: URL) {
        self.url = url
    }
    
    var messages: AsyncStream<RemoteMessage> {
        if messageStream == nil {
            messageStream = AsyncStream { continuation in
                self.messageContinuation = continuation
            }
        }
        return messageStream!
    }
    
    var errors: AsyncStream<Error> {
        if errorStream == nil {
            errorStream = AsyncStream { continuation in
                self.errorContinuation = continuation
            }
        }
        return errorStream!
    }
    
    var audioData: AsyncStream<Data> {
        if audioStream == nil {
            audioStream = AsyncStream { continuation in
                self.audioContinuation = continuation
            }
        }
        return audioStream!
    }
    
    func connect() async {
        isManuallyDisconnected = false
        
        guard state != .connected else {
            errorMessage = "Already connected"
            return
        }
        
        state = .connecting
        errorMessage = nil
        
        if session == nil {
            session = URLSession(configuration: .default, delegate: nil, delegateQueue: nil)
        }
        
        webSocketTask = session?.webSocketTask(with: url)
        
        guard let webSocketTask else {
            await self.handleConnectionError(NSError(domain: "WebSocketManager", code: 1, userInfo: [NSLocalizedDescriptionKey: "Failed to create WebSocket task"]))
            return
        }
        
        webSocketTask.resume()
        
        do {
            let startMessage = RemoteMessage.sessionStart(sampleRate: 16000)
            try await send(startMessage)
            
            for try await message in webSocketTask.messages {
                await self.handleIncomingMessage(message)
            }
        } catch {
            await self.handleConnectionError(error)
        }
    }
    
    func disconnect() {
        isManuallyDisconnected = true
        cancelReconnect()
        webSocketTask?.cancel(with: .normalClosure, reason: nil)
        webSocketTask = nil
        state = .disconnected
    }
    
    func send(_ message: RemoteMessage) async throws {
        guard let webSocketTask else {
            throw WebSocketError.sendFailed
        }
        let data = try message.jsonData()
        guard let text = String(data: data, encoding: .utf8) else {
            throw WebSocketError.sendFailed
        }
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            webSocketTask.send(.string(text)) { error in
                if let error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume(returning: ())
                }
            }
        }
    }
    
    func send(audioData: Data) async throws {
        guard let webSocketTask else {
            throw WebSocketError.sendFailed
        }
        Self.logger.debug("WS send binary: \(audioData.count) bytes \(Self.formatBuffer(audioData))")
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            webSocketTask.send(.data(audioData)) { error in
                if let error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume(returning: ())
                }
            }
        }
    }
    
    func bargeIn() async throws {
        try await send(.bargeIn)
    }
    
    private static func formatBuffer(_ data: Data) -> String {
        let preview = 10
        guard data.count > preview * 2 else {
            return "[" + data.map { String(format: "%02x", $0) }.joined() + "]"
        }
        let head = data.prefix(preview).map { String(format: "%02x", $0) }.joined()
        let tail = data.suffix(preview).map { String(format: "%02x", $0) }.joined()
        return "[\(head) ... \(tail)]"
    }
    
    private func handleIncomingMessage(_ webSocketMessage: URLSessionWebSocketTask.Message) async {
        switch webSocketMessage {
        case .data(let data):
            // The server sends TTS audio as binary frames. JSON messages arrive as .string.
            Self.logger.debug("WS recv binary: \(data.count) bytes \(Self.formatBuffer(data))")
            audioContinuation?.yield(data)
            
        case .string(let string):
            do {
                let data = string.data(using: .utf8)!
                let message = try RemoteMessage.parse(data)
                messageContinuation?.yield(message)
                
                if case .sessionReady = message {
                    state = .connected
                    reconnectAttempt = 0
                } else if case .error(let msg) = message {
                    errorMessage = msg
                    state = .failed(msg)
                }
            } catch {
                errorContinuation?.yield(WebSocketError.decodeFailed(error))
            }
        @unknown default:
            break
        }
    }
    
    private func handleConnectionError(_ error: Error) async {
        guard !isManuallyDisconnected else { return }
        
        let nsError = error as NSError
        // NSURLErrorBadServerResponse (-1011) is what URLSession reports when the
        // server returns a non-101 status code during the WebSocket handshake.
        // Voicebot returns HTTP 409 Conflict when another remote client is already connected.
        if nsError.code == 409 || nsError.code == NSURLErrorBadServerResponse {
            errorMessage = "Voicebot is already connected from another device"
            state = .failed(errorMessage ?? "Connection failed")
            return
        }
        
        errorMessage = error.localizedDescription
        state = .failed(error.localizedDescription)
        
        if !isManuallyDisconnected && reconnectAttempt < maxReconnectAttempts {
            let delay = baseReconnectDelay * Double(reconnectAttempt)
            reconnectAttempt += 1
            try? await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
            Task {
                await self.connect()
            }
        }
    }
    
    private func cancelReconnect() {
        reconnectTimer?.invalidate()
        reconnectTimer = nil
        reconnectAttempt = 0
    }
}

extension URLSessionWebSocketTask {
    var messages: AsyncStream<Message> {
        AsyncStream { continuation in
            func read() {
                self.receive { result in
                    switch result {
                    case .success(let message):
                        continuation.yield(message)
                        read()
                    case .failure:
                        continuation.finish()
                    }
                }
            }
            read()
        }
    }
}
