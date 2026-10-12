//
//  ControlEvent.swift
//  voicebot-ios-companion
//
//  Codable mirror of host `ControlEvent` (seneschal-control broadcast.rs).
//  Unknown `type` values decode as `.unknown` so SSE can skip without failing.
//

import Foundation

// MARK: - Pipeline state tokens (host wire)

/// Host Control API pipeline tokens (`idle` | `listening` | …). No Debug-string parsing.
enum CompanionPipelineState: String, Equatable, Sendable, Codable {
    case idle
    case listening
    case thinking
    case speaking
    case paused
    case unknown

    init(hostToken: String) {
        self = CompanionPipelineState(rawValue: hostToken.lowercased()) ?? .unknown
    }
}

// MARK: - Permission wire types

struct PermissionOption: Codable, Identifiable, Equatable, Sendable {
    let id: String
    let label: String
    let kind: String?

    enum CodingKeys: String, CodingKey {
        case id, label, kind
    }
}

struct PermissionSlot: Codable, Identifiable, Equatable, Sendable {
    let taskId: String
    let agentName: String
    let description: String
    let options: [PermissionOption]
    let phase: String

    var id: String { taskId }

    enum CodingKeys: String, CodingKey {
        case taskId = "task_id"
        case agentName = "agent_name"
        case description
        case options
        case phase
    }
}

struct PermissionRequest: Equatable, Sendable, Identifiable {
    let taskId: String
    let agentName: String
    let description: String
    let options: [PermissionOption]

    var id: String { taskId }
}

// MARK: - Dual-channel link state (design #190)

/// Shared shape for audio (WS) and control (SSE/REST) links.
enum LinkState: Equatable, Sendable {
    case disconnected
    case connecting
    case connected
    case reconnecting(attempt: Int)
    case failed(String)
    /// WS only: host HTTP 409 — another remote client holds exclusive audio.
    case conflict

    var isUsableForControl: Bool {
        switch self {
        case .connected, .reconnecting: return true
        default: return false
        }
    }
}

/// Back-compat alias used by PR3 code paths.
typealias ControlLinkState = LinkState

struct ControlHealthResponse: Codable, Equatable, Sendable {
    let status: String
    let service: String
}

struct ControlStateResponse: Codable, Equatable, Sendable {
    let state: String
    let utteranceId: UInt64?
    let ttsMuted: Bool
    let pauseReason: String?

    enum CodingKeys: String, CodingKey {
        case state
        case utteranceId = "utterance_id"
        case ttsMuted = "tts_muted"
        case pauseReason = "pause_reason"
    }

    var pipelineState: CompanionPipelineState {
        CompanionPipelineState(hostToken: state)
    }
}

// MARK: - ControlEvent

/// SSE / Control API event. Property names use explicit snake_case CodingKeys.
enum ControlEvent: Equatable, Sendable {
    case stateChanged(state: String, utteranceId: UInt64?, pauseReason: String?)
    case transcript(utteranceId: UInt64, text: String)
    case llmToken(utteranceId: UInt64, token: String)
    case llmDone(utteranceId: UInt64, fullText: String)
    case ttsStart(utteranceId: UInt64)
    case toolCall(name: String, result: String)
    case muteChanged(muted: Bool)
    case error(message: String)
    case systemNotification(text: String)
    case mcpNotification(serverName: String, method: String)
    case classification(intent: String, level: String, forced: Bool, utteranceId: UInt64?)
    case agentTaskStarted(taskId: String, agentName: String, objective: String)
    case agentTaskRunning(taskId: String, objective: String)
    case agentTaskDelegated(taskId: String, objective: String)
    case agentTaskFinalizing(taskId: String, objective: String)
    case agentTaskCompleted(taskId: String, objective: String, result: String)
    case agentTaskFailed(taskId: String, message: String)
    case agentPermissionRequested(
        taskId: String,
        agentName: String,
        description: String,
        options: [PermissionOption]
    )
    case agentPermissionResolved(taskId: String, optionId: String)
    /// Host added a variant this client does not know yet — skip safely.
    case unknown(type: String)
}

extension ControlEvent {
    enum CodingKeys: String, CodingKey {
        case type
        case state
        case utteranceId = "utterance_id"
        case pauseReason = "pause_reason"
        case text
        case token
        case fullText = "full_text"
        case name
        case result
        case muted
        case message
        case serverName = "server_name"
        case method
        case intent
        case level
        case forced
        case taskId = "task_id"
        case agentName = "agent_name"
        case objective
        case description
        case options
        case optionId = "option_id"
    }

    /// Decode a single SSE `data:` JSON payload. Unknown types → `.unknown` (no throw).
    static func parseSSEData(_ data: Data) throws -> ControlEvent {
        let decoder = JSONDecoder()
        return try decoder.decode(ControlEvent.self, from: data)
    }

    static func parseSSEJSON(_ json: String) throws -> ControlEvent {
        guard let data = json.data(using: .utf8) else {
            throw DecodingError.dataCorrupted(
                .init(codingPath: [], debugDescription: "Invalid UTF-8 in SSE data")
            )
        }
        return try parseSSEData(data)
    }
}

extension ControlEvent: Decodable {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let type = try c.decode(String.self, forKey: .type)

        switch type {
        case "state_changed":
            self = .stateChanged(
                state: try c.decode(String.self, forKey: .state),
                utteranceId: try c.decodeIfPresent(UInt64.self, forKey: .utteranceId),
                pauseReason: try c.decodeIfPresent(String.self, forKey: .pauseReason)
            )
        case "transcript":
            self = .transcript(
                utteranceId: try c.decode(UInt64.self, forKey: .utteranceId),
                text: try c.decode(String.self, forKey: .text)
            )
        case "llm_token":
            self = .llmToken(
                utteranceId: try c.decode(UInt64.self, forKey: .utteranceId),
                token: try c.decode(String.self, forKey: .token)
            )
        case "llm_done":
            self = .llmDone(
                utteranceId: try c.decode(UInt64.self, forKey: .utteranceId),
                fullText: try c.decode(String.self, forKey: .fullText)
            )
        case "tts_start":
            self = .ttsStart(utteranceId: try c.decode(UInt64.self, forKey: .utteranceId))
        case "tool_call":
            self = .toolCall(
                name: try c.decode(String.self, forKey: .name),
                result: try c.decode(String.self, forKey: .result)
            )
        case "mute_changed":
            self = .muteChanged(muted: try c.decode(Bool.self, forKey: .muted))
        case "error":
            self = .error(message: try c.decode(String.self, forKey: .message))
        case "system_notification":
            self = .systemNotification(text: try c.decode(String.self, forKey: .text))
        case "mcp_notification":
            self = .mcpNotification(
                serverName: try c.decode(String.self, forKey: .serverName),
                method: try c.decode(String.self, forKey: .method)
            )
        case "classification":
            self = .classification(
                intent: try c.decode(String.self, forKey: .intent),
                level: try c.decode(String.self, forKey: .level),
                forced: try c.decode(Bool.self, forKey: .forced),
                utteranceId: try c.decodeIfPresent(UInt64.self, forKey: .utteranceId)
            )
        case "agent_task_started":
            self = .agentTaskStarted(
                taskId: try c.decode(String.self, forKey: .taskId),
                agentName: try c.decode(String.self, forKey: .agentName),
                objective: try c.decode(String.self, forKey: .objective)
            )
        case "agent_task_running":
            self = .agentTaskRunning(
                taskId: try c.decode(String.self, forKey: .taskId),
                objective: try c.decode(String.self, forKey: .objective)
            )
        case "agent_task_delegated":
            self = .agentTaskDelegated(
                taskId: try c.decode(String.self, forKey: .taskId),
                objective: try c.decode(String.self, forKey: .objective)
            )
        case "agent_task_finalizing":
            self = .agentTaskFinalizing(
                taskId: try c.decode(String.self, forKey: .taskId),
                objective: try c.decode(String.self, forKey: .objective)
            )
        case "agent_task_completed":
            self = .agentTaskCompleted(
                taskId: try c.decode(String.self, forKey: .taskId),
                objective: try c.decode(String.self, forKey: .objective),
                result: try c.decode(String.self, forKey: .result)
            )
        case "agent_task_failed":
            self = .agentTaskFailed(
                taskId: try c.decode(String.self, forKey: .taskId),
                message: try c.decode(String.self, forKey: .message)
            )
        case "agent_permission_requested":
            self = .agentPermissionRequested(
                taskId: try c.decode(String.self, forKey: .taskId),
                agentName: try c.decode(String.self, forKey: .agentName),
                description: try c.decode(String.self, forKey: .description),
                options: try c.decode([PermissionOption].self, forKey: .options)
            )
        case "agent_permission_resolved":
            self = .agentPermissionResolved(
                taskId: try c.decode(String.self, forKey: .taskId),
                optionId: try c.decode(String.self, forKey: .optionId)
            )
        default:
            self = .unknown(type: type)
        }
    }
}
