//
//  MessageStore.swift
//  voicebot-ios-companion
//

import Foundation

struct StoredMessage: Codable, Identifiable {
    let id: String
    let role: String
    let text: String
    let timestamp: Double
}

@MainActor
class MessageStore {
    private let fileURL: URL

    init() {
        let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first!
        self.fileURL = documents.appendingPathComponent("chat_history.json")
    }

    func load() -> [StoredMessage] {
        guard let data = try? Data(contentsOf: fileURL),
              let messages = try? JSONDecoder().decode([StoredMessage].self, from: data) else {
            return []
        }
        return messages
    }

    func save(_ messages: [StoredMessage]) {
        guard let data = try? JSONEncoder().encode(messages) else { return }
        try? data.write(to: fileURL)
    }

    func append(_ messages: [StoredMessage]) {
        var existing = load()
        for msg in messages {
            if !existing.contains(where: { $0.id == msg.id }) {
                existing.append(msg)
            }
        }
        save(existing)
    }

    func clear() {
        try? FileManager.default.removeItem(at: fileURL)
    }
}
