//
//  HistoryClient.swift
//  voicebot-ios-companion
//

import Foundation

struct SessionInfo: Codable, Identifiable {
    let id: String
    let created_at: String
    let is_active: Bool
}

struct HistoryMessage: Codable, Identifiable {
    let id: Int
    let role: String
    let content: String
    let timestamp: String
}

class HistoryClient {
    private let baseURL: String

    init(host: String, controlPort: String) {
        self.baseURL = "http://\(host):\(controlPort)"
    }

    func fetchSessions() async throws -> [SessionInfo] {
        let url = URL(string: "\(baseURL)/control/sessions")!
        let (data, response) = try await URLSession.shared.data(from: url)
        guard let httpResponse = response as? HTTPURLResponse,
              (200...299).contains(httpResponse.statusCode) else {
            throw URLError(.badServerResponse)
        }
        return try JSONDecoder().decode([SessionInfo].self, from: data)
    }

    private static let dateFormatter: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds, .withColonSeparatorInTimeZone]
        return f
    }()

    func fetchMessages(sessionId: String) async throws -> [StoredMessage] {
        let url = URL(string: "\(baseURL)/control/sessions/\(sessionId)/messages")!
        let (data, response) = try await URLSession.shared.data(from: url)
        guard let httpResponse = response as? HTTPURLResponse,
              (200...299).contains(httpResponse.statusCode) else {
            throw URLError(.badServerResponse)
        }
        let serverMessages = try JSONDecoder().decode([HistoryMessage].self, from: data)
        return serverMessages.compactMap { msg -> StoredMessage? in
            guard let date = Self.dateFormatter.date(from: msg.timestamp) else {
                NSLog("HistoryClient: failed to parse timestamp '\(msg.timestamp)' for message \(msg.id)")
                return nil
            }
            return StoredMessage(
                id: String(msg.id),
                role: msg.role,
                text: msg.content,
                timestamp: date.timeIntervalSince1970
            )
        }
    }
}
