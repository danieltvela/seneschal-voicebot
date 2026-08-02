//
//  TimelineItemTests.swift
//  voicebot-ios-companionTests
//

import Testing
import Foundation
@testable import voicebot_ios_companion

struct TimelineItemTests {

    @Test func toolCallMapsToToolKind() {
        let event = ControlEvent.toolCall(name: "web_search", result: "ok")
        let item = TimelineItem.from(controlEvent: event)
        #expect(item != nil)
        #expect(item?.kind == .tool)
        #expect(item?.toolName == "web_search")
        #expect(item?.text == "ok")
    }

    @Test func systemNotificationMaps() {
        let event = ControlEvent.systemNotification(text: "hello system")
        let item = TimelineItem.from(controlEvent: event)
        #expect(item?.kind == .system)
        #expect(item?.text == "hello system")
    }

    @Test func agentCompletedMapsWithStatus() {
        let event = ControlEvent.agentTaskCompleted(
            taskId: "t1",
            objective: "list files",
            result: "done"
        )
        let item = TimelineItem.from(controlEvent: event)
        #expect(item?.kind == .agentTask)
        #expect(item?.agent?.taskId == "t1")
        #expect(item?.agent?.status == "completed")
        #expect(item?.text.contains("list files") == true || item?.text.contains("done") == true)
    }

    @Test func lagErrorDoesNotCreateTimelineItem() {
        let event = ControlEvent.error(message: "Missed 3 events (subscriber lagged)")
        let item = TimelineItem.from(controlEvent: event)
        #expect(item == nil)
    }

    @Test func realErrorCreatesTimelineItem() {
        let event = ControlEvent.error(message: "LLM timeout")
        let item = TimelineItem.from(controlEvent: event)
        #expect(item?.kind == .error)
        #expect(item?.text == "LLM timeout")
    }

    @Test func truncationProducesDetail() {
        let long = String(repeating: "a", count: 3_000)
        let (preview, full) = TimelineItem.truncated(long, limit: 100)
        #expect(preview.count == 101) // 100 + ellipsis char
        #expect(preview.hasSuffix("…"))
        #expect(full?.count == 3_000)
    }

    @Test func parseToolFromSSEJSON() throws {
        let json = #"{"type":"tool_call","name":"bash","result":"hi"}"#
        let event = try ControlEvent.parseSSEJSON(json)
        let item = TimelineItem.from(controlEvent: event)
        #expect(item?.kind == .tool)
        #expect(item?.toolName == "bash")
    }

    @Test func mcpAndClassification() {
        let mcp = ControlEvent.mcpNotification(serverName: "s", method: "notifications/x")
        #expect(TimelineItem.from(controlEvent: mcp)?.kind == .mcp)

        let cls = ControlEvent.classification(
            intent: "simple",
            level: "heuristic",
            forced: false,
            utteranceId: 1
        )
        #expect(TimelineItem.from(controlEvent: cls)?.kind == .classification)
    }

    @Test func llmTokensDoNotCreateTimeline() {
        let token = ControlEvent.llmToken(utteranceId: 1, token: "hi")
        #expect(TimelineItem.from(controlEvent: token) == nil)
        let state = ControlEvent.stateChanged(state: "idle", utteranceId: nil, pauseReason: nil)
        #expect(TimelineItem.from(controlEvent: state) == nil)
    }
}
