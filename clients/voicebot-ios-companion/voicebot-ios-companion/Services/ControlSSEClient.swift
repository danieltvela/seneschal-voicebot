//
//  ControlSSEClient.swift
//  voicebot-ios-companion
//
//  SSE client for `GET /control/events`. Reconnect with exponential backoff 1s→30s.
//

import Foundation

/// Streams decoded `ControlEvent`s from the Control SSE endpoint.
/// Unknown event types are logged and skipped (never terminate the stream).
final class ControlSSEClient: @unchecked Sendable {
    private let baseURL: String
    private let session: URLSession
    private var task: Task<Void, Never>?
    private var continuation: AsyncStream<ControlEvent>.Continuation?

    /// Minimum / maximum reconnect delay (seconds).
    private let minBackoff: TimeInterval = 1
    private let maxBackoff: TimeInterval = 30

    init(host: String, controlPort: String, session: URLSession? = nil) {
        self.baseURL = "http://\(host):\(controlPort)"
        if let session {
            self.session = session
        } else {
            let config = URLSessionConfiguration.default
            config.timeoutIntervalForRequest = 3600
            config.timeoutIntervalForResource = 0
            config.waitsForConnectivity = true
            self.session = URLSession(configuration: config)
        }
    }

    /// Start (or restart) the SSE loop. Cancels any previous stream.
    func events() -> AsyncStream<ControlEvent> {
        stop()
        return AsyncStream { continuation in
            self.continuation = continuation
            continuation.onTermination = { [weak self] _ in
                self?.stop()
            }
            self.task = Task { [weak self] in
                await self?.runLoop(continuation: continuation)
            }
        }
    }

    func stop() {
        task?.cancel()
        task = nil
        continuation?.finish()
        continuation = nil
    }

    // MARK: - Loop

    private func runLoop(continuation: AsyncStream<ControlEvent>.Continuation) async {
        var backoff = minBackoff
        while !Task.isCancelled {
            do {
                try await connectOnce(continuation: continuation)
                // Clean close — reset backoff and reconnect
                backoff = minBackoff
            } catch is CancellationError {
                break
            } catch {
                NSLog("ControlSSEClient: stream error: \(error.localizedDescription)")
            }
            if Task.isCancelled { break }
            let delay = backoff
            backoff = min(maxBackoff, backoff * 2)
            try? await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
        }
        continuation.finish()
    }

    private func connectOnce(continuation: AsyncStream<ControlEvent>.Continuation) async throws {
        guard let url = URL(string: "\(baseURL)/control/events") else {
            throw ControlClientError.invalidURL
        }
        var request = URLRequest(url: url)
        request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        request.setValue("no-cache", forHTTPHeaderField: "Cache-Control")

        let (bytes, response) = try await session.bytes(for: request)
        if let http = response as? HTTPURLResponse, !(200 ... 299).contains(http.statusCode) {
            throw ControlClientError.badStatus(http.statusCode, nil)
        }

        var buffer = ""
        for try await byte in bytes {
            if Task.isCancelled { throw CancellationError() }
            buffer.append(Character(UnicodeScalar(byte)))
            while let range = buffer.range(of: "\n\n") {
                let eventBlock = String(buffer[..<range.lowerBound])
                buffer = String(buffer[range.upperBound...])
                if let event = Self.parseSSEBlock(eventBlock) {
                    continuation.yield(event)
                }
            }
        }
    }

    /// Parse one SSE event block (lines joined by `\n`). Returns nil for comments / empty / unknown.
    static func parseSSEBlock(_ block: String) -> ControlEvent? {
        var dataLines: [String] = []
        for line in block.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(line)
            if line.hasPrefix(":") { continue } // comment / keep-alive
            if line.hasPrefix("data:") {
                var payload = String(line.dropFirst(5))
                if payload.hasPrefix(" ") {
                    payload = String(payload.dropFirst())
                }
                dataLines.append(payload)
            }
        }
        guard !dataLines.isEmpty else { return nil }
        let json = dataLines.joined(separator: "\n")
        do {
            let event = try ControlEvent.parseSSEJSON(json)
            if case .unknown(let type) = event {
                NSLog("ControlSSEClient: skipping unknown event type '\(type)'")
                return nil
            }
            return event
        } catch {
            NSLog("ControlSSEClient: failed to parse event: \(error) data=\(json)")
            return nil
        }
    }
}
