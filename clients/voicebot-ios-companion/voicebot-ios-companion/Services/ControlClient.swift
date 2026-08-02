//
//  ControlClient.swift
//  voicebot-ios-companion
//
//  REST client for Seneschal Control API (`/control/*`).
//

import Foundation

enum ControlClientError: Error, LocalizedError, Equatable {
    case invalidURL
    case badStatus(Int, String?)
    case decoding(String)
    case transport(String)

    var errorDescription: String? {
        switch self {
        case .invalidURL:
            return "Invalid Control API URL"
        case .badStatus(let code, let body):
            if let body, !body.isEmpty {
                return "Control API HTTP \(code): \(body)"
            }
            return "Control API HTTP \(code)"
        case .decoding(let msg):
            return "Control API decode error: \(msg)"
        case .transport(let msg):
            return msg
        }
    }
}

/// Synchronous-style REST helpers for the Control plane (not SSE).
final class ControlClient: Sendable {
    private let baseURL: String
    private let session: URLSession

    init(host: String, controlPort: String, session: URLSession = .shared) {
        self.baseURL = "http://\(host):\(controlPort)"
        self.session = session
    }

    // MARK: - Health & state

    func healthCheck() async throws -> ControlHealthResponse {
        try await getJSON(path: "/control/health")
    }

    func getState() async throws -> ControlStateResponse {
        try await getJSON(path: "/control/state")
    }

    // MARK: - Actions

    func setMute(_ muted: Bool) async throws {
        try await postJSON(path: "/control/mute", body: ["muted": muted])
    }

    func bargeIn() async throws {
        try await postEmpty(path: "/control/barge_in")
    }

    func sendInput(_ text: String) async throws {
        try await postJSON(path: "/control/input", body: ["text": text])
    }

    func listPermissions() async throws -> [PermissionSlot] {
        try await getJSON(path: "/control/permissions")
    }

    func resolvePermission(taskId: String, optionId: String) async throws {
        try await postJSON(
            path: "/control/permission",
            body: ["task_id": taskId, "option_id": optionId]
        )
    }

    // MARK: - HTTP helpers

    private func url(path: String) throws -> URL {
        guard let url = URL(string: baseURL + path) else {
            throw ControlClientError.invalidURL
        }
        return url
    }

    private func getJSON<T: Decodable>(path: String) async throws -> T {
        let requestURL = try url(path: path)
        let (data, response): (Data, URLResponse)
        do {
            (data, response) = try await session.data(from: requestURL)
        } catch {
            throw ControlClientError.transport(error.localizedDescription)
        }
        try Self.throwIfBadStatus(response, data: data)
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw ControlClientError.decoding(error.localizedDescription)
        }
    }

    private func postEmpty(path: String) async throws {
        var request = URLRequest(url: try url(path: path))
        request.httpMethod = "POST"
        let (data, response): (Data, URLResponse)
        do {
            (data, response) = try await session.data(for: request)
        } catch {
            throw ControlClientError.transport(error.localizedDescription)
        }
        try Self.throwIfBadStatus(response, data: data)
    }

    private func postJSON(path: String, body: [String: Any]) async throws {
        var request = URLRequest(url: try url(path: path))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, response): (Data, URLResponse)
        do {
            (data, response) = try await session.data(for: request)
        } catch {
            throw ControlClientError.transport(error.localizedDescription)
        }
        try Self.throwIfBadStatus(response, data: data)
    }

    private static func throwIfBadStatus(_ response: URLResponse, data: Data) throws {
        guard let http = response as? HTTPURLResponse else {
            throw ControlClientError.badStatus(-1, nil)
        }
        // 204 No Content is success for mute/input/permission/barge_in
        guard (200 ... 299).contains(http.statusCode) else {
            let body = String(data: data, encoding: .utf8)
            throw ControlClientError.badStatus(http.statusCode, body)
        }
    }
}
