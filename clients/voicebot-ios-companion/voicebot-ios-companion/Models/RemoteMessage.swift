//
//  RemoteMessage.swift
//  voicebot-ios-companion
//
//  Created by Dani Vela on 13/06/2026.
//

import Foundation

/// JSON message types matching Voicebot's `src/remote/protocol.rs`.
/// All message types use a `type` discriminator for polymorphic serialization.
enum RemoteMessage: Codable, Equatable, Sendable {
    // MARK: - Client → Server
    
    /// Initiates a session with the Voicebot server.
    case sessionStart(sampleRate: UInt32)
    
    /// Cancels the active LLM→TTS pipeline (barge-in).
    case bargeIn
    
    // MARK: - Server → Client
    
    /// Acknowledges session start.
    case sessionReady
    
    /// STT transcription (partial or final).
    case transcript(text: String)
    
    /// LLM sentence ready for TTS synthesis.
    case responseText(text: String)
    
    /// LLM generation complete.
    case responseEnd
    
    /// TTS audio binary frames about to follow.
    case audioStart
    
    /// TTS audio frames finished.
    case audioEnd
    
    /// Error notification.
    case error(message: String)
    
    // MARK: - Codable
    
    enum CodingKeys: String, CodingKey {
        case type
        case sampleRate
        case text
        case message
    }
    
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let type = try container.decode(String.self, forKey: .type)
        
        switch type {
        case "session.start":
            let sampleRate = try container.decode(UInt32.self, forKey: .sampleRate)
            self = .sessionStart(sampleRate: sampleRate)
        case "barge_in":
            self = .bargeIn
        case "session.ready":
            self = .sessionReady
        case "transcript":
            let text = try container.decode(String.self, forKey: .text)
            self = .transcript(text: text)
        case "response.text":
            let text = try container.decode(String.self, forKey: .text)
            self = .responseText(text: text)
        case "response.end":
            self = .responseEnd
        case "audio.start":
            self = .audioStart
        case "audio.end":
            self = .audioEnd
        case "error":
            let message = try container.decode(String.self, forKey: .message)
            self = .error(message: message)
        default:
            throw DecodingError.dataCorruptedError(
                forKey: .type,
                in: container,
                debugDescription: "Unknown message type: \(type)"
            )
        }
    }
    
    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        
        switch self {
        case .sessionStart(let sampleRate):
            try container.encode("session.start", forKey: .type)
            try container.encode(sampleRate, forKey: .sampleRate)
        case .bargeIn:
            try container.encode("barge_in", forKey: .type)
        case .sessionReady:
            try container.encode("session.ready", forKey: .type)
        case .transcript(let text):
            try container.encode("transcript", forKey: .type)
            try container.encode(text, forKey: .text)
        case .responseText(let text):
            try container.encode("response.text", forKey: .type)
            try container.encode(text, forKey: .text)
        case .responseEnd:
            try container.encode("response.end", forKey: .type)
        case .audioStart:
            try container.encode("audio.start", forKey: .type)
        case .audioEnd:
            try container.encode("audio.end", forKey: .type)
        case .error(let message):
            try container.encode("error", forKey: .type)
            try container.encode(message, forKey: .message)
        }
    }
    
    // MARK: - Helpers
    
    /// Encode this message to JSON Data.
    func jsonData() throws -> Data {
        try JSONEncoder.default.encode(self)
    }
    
    /// Decode a RemoteMessage from JSON Data.
    static func parse(_ data: Data) throws -> RemoteMessage {
        try JSONDecoder.default.decode(RemoteMessage.self, from: data)
    }
}

// MARK: - Encoder/Decoder Instances

extension JSONEncoder {
    static let `default`: JSONEncoder = {
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return encoder
    }()
}

extension JSONDecoder {
    static let `default`: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return decoder
    }()
}
