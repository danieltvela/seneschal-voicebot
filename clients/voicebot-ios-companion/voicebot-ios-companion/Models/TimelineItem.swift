//
//  TimelineItem.swift
//  voicebot-ios-companion
//
//  Secondary event stream: tools, system, agent lifecycle, errors (not chat bubbles).
//

import Foundation

enum TimelineKind: String, Equatable, Sendable {
    case tool
    case system
    case agentTask
    case error
    case mcp
    case classification
}

struct AgentTaskInfo: Equatable, Sendable {
    var taskId: String
    var agentName: String?
    var status: String
    var objective: String?
    var result: String?
}

struct TimelineItem: Identifiable, Equatable, Sendable {
    let id: String
    var kind: TimelineKind
    var text: String
    var timestamp: Date
    var agent: AgentTaskInfo?
    var toolName: String?
    var isStreaming: Bool
    /// Full text when `text` is truncated for display.
    var detail: String?

    static let displayTruncate = 2_000

    /// Short label for list rows.
    var title: String {
        switch kind {
        case .tool:
            return toolName.map { "Tool: \($0)" } ?? "Tool"
        case .system:
            return "System"
        case .agentTask:
            let status = agent?.status ?? "agent"
            let name = agent?.agentName ?? agent?.taskId ?? "agent"
            return "\(name) · \(status)"
        case .error:
            return "Error"
        case .mcp:
            return "MCP"
        case .classification:
            return "Intent"
        }
    }

    var hasExpandableDetail: Bool {
        if let detail, detail.count > text.count { return true }
        return text.count >= Self.displayTruncate
    }

    static func truncated(_ s: String, limit: Int = displayTruncate) -> (preview: String, full: String?) {
        if s.count <= limit {
            return (s, nil)
        }
        let idx = s.index(s.startIndex, offsetBy: limit)
        return (String(s[..<idx]) + "…", s)
    }

    // MARK: - Factories from ControlEvent

    static func from(controlEvent: ControlEvent, now: Date = Date()) -> TimelineItem? {
        switch controlEvent {
        case .toolCall(let name, let result):
            let (preview, full) = truncated(result)
            return TimelineItem(
                id: "tool-\(UUID().uuidString)",
                kind: .tool,
                text: preview.isEmpty ? "(no result)" : preview,
                timestamp: now,
                agent: nil,
                toolName: name,
                isStreaming: false,
                detail: full
            )

        case .systemNotification(let text):
            let (preview, full) = truncated(text)
            return TimelineItem(
                id: "sys-\(UUID().uuidString)",
                kind: .system,
                text: preview,
                timestamp: now,
                agent: nil,
                toolName: nil,
                isStreaming: false,
                detail: full
            )

        case .mcpNotification(let server, let method):
            return TimelineItem(
                id: "mcp-\(UUID().uuidString)",
                kind: .mcp,
                text: "\(server): \(method)",
                timestamp: now,
                agent: nil,
                toolName: nil,
                isStreaming: false,
                detail: nil
            )

        case .classification(let intent, let level, let forced, _):
            let forcedTag = forced ? " (forced)" : ""
            return TimelineItem(
                id: "cls-\(UUID().uuidString)",
                kind: .classification,
                text: "\(intent) · \(level)\(forcedTag)",
                timestamp: now,
                agent: nil,
                toolName: nil,
                isStreaming: false,
                detail: nil
            )

        case .error(let message):
            // Lag resync is handled as banner; still show non-lag errors on timeline.
            if message.contains("Missed") { return nil }
            let (preview, full) = truncated(message)
            return TimelineItem(
                id: "err-\(UUID().uuidString)",
                kind: .error,
                text: preview,
                timestamp: now,
                agent: nil,
                toolName: nil,
                isStreaming: false,
                detail: full
            )

        case .agentTaskStarted(let taskId, let agentName, let objective):
            return agentItem(
                taskId: taskId,
                agentName: agentName,
                status: "started",
                objective: objective,
                result: nil,
                now: now
            )

        case .agentTaskRunning(let taskId, let objective):
            return agentItem(
                taskId: taskId,
                agentName: nil,
                status: "running",
                objective: objective,
                result: nil,
                now: now
            )

        case .agentTaskDelegated(let taskId, let objective):
            return agentItem(
                taskId: taskId,
                agentName: nil,
                status: "delegated",
                objective: objective,
                result: nil,
                now: now
            )

        case .agentTaskFinalizing(let taskId, let objective):
            return agentItem(
                taskId: taskId,
                agentName: nil,
                status: "finalizing",
                objective: objective,
                result: nil,
                now: now
            )

        case .agentTaskCompleted(let taskId, let objective, let result):
            let (preview, full) = truncated(result)
            return agentItem(
                taskId: taskId,
                agentName: nil,
                status: "completed",
                objective: objective,
                result: preview,
                detail: full,
                now: now
            )

        case .agentTaskFailed(let taskId, let message):
            return agentItem(
                taskId: taskId,
                agentName: nil,
                status: "failed",
                objective: message,
                result: nil,
                now: now
            )

        case .agentPermissionRequested(let taskId, let agentName, let description, _):
            return agentItem(
                taskId: taskId,
                agentName: agentName,
                status: "permission",
                objective: description,
                result: nil,
                now: now
            )

        case .agentPermissionResolved(let taskId, let optionId):
            return agentItem(
                taskId: taskId,
                agentName: nil,
                status: "permission_resolved",
                objective: "option: \(optionId)",
                result: nil,
                now: now
            )

        default:
            return nil
        }
    }

    private static func agentItem(
        taskId: String,
        agentName: String?,
        status: String,
        objective: String?,
        result: String?,
        detail: String? = nil,
        now: Date
    ) -> TimelineItem {
        let body = [objective, result].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: "\n")
        let (preview, fullFromBody) = truncated(body.isEmpty ? status : body)
        return TimelineItem(
            id: "agent-\(taskId)-\(status)-\(UUID().uuidString.prefix(8))",
            kind: .agentTask,
            text: preview,
            timestamp: now,
            agent: AgentTaskInfo(
                taskId: taskId,
                agentName: agentName,
                status: status,
                objective: objective,
                result: result
            ),
            toolName: nil,
            isStreaming: status == "running" || status == "started",
            detail: detail ?? fullFromBody
        )
    }
}
