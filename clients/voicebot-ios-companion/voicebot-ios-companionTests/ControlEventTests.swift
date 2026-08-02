//
//  ControlEventTests.swift
//  voicebot-ios-companionTests
//
//  Fixtures aligned with doc/design/190-ios-companion-v2.md (PR1 wire samples).
//

import Testing
import Foundation
@testable import voicebot_ios_companion

struct ControlEventFixtureTests {

    @Test func stateChangedThinking() throws {
        let json = #"{"type":"state_changed","state":"thinking","utterance_id":42}"#
        let event = try ControlEvent.parseSSEJSON(json)
        guard case .stateChanged(let state, let utteranceId, let pauseReason) = event else {
            Issue.record("Expected stateChanged")
            return
        }
        #expect(state == "thinking")
        #expect(utteranceId == 42)
        #expect(pauseReason == nil)
        #expect(CompanionPipelineState(hostToken: state) == .thinking)
    }

    @Test func stateChangedPaused() throws {
        let json =
            #"{"type":"state_changed","state":"paused","utterance_id":null,"pause_reason":"consolidation"}"#
        let event = try ControlEvent.parseSSEJSON(json)
        guard case .stateChanged(let state, let utteranceId, let pauseReason) = event else {
            Issue.record("Expected stateChanged")
            return
        }
        #expect(state == "paused")
        #expect(utteranceId == nil)
        #expect(pauseReason == "consolidation")
        #expect(CompanionPipelineState(hostToken: state) == .paused)
    }

    @Test func agentPermissionRequested() throws {
        let json = """
        {"type":"agent_permission_requested","task_id":"t1","agent_name":"hermes","description":"bash: ls","options":[{"id":"allow","label":"Allow once","kind":"allow"},{"id":"deny","label":"Deny","kind":"reject"}]}
        """
        let event = try ControlEvent.parseSSEJSON(json)
        guard case .agentPermissionRequested(let taskId, let agent, let desc, let options) = event
        else {
            Issue.record("Expected agentPermissionRequested")
            return
        }
        #expect(taskId == "t1")
        #expect(agent == "hermes")
        #expect(desc == "bash: ls")
        #expect(options.count == 2)
        #expect(options[0].id == "allow")
        #expect(options[0].kind == "allow")
        #expect(options[1].id == "deny")
    }

    @Test func agentPermissionResolved() throws {
        let json = #"{"type":"agent_permission_resolved","task_id":"t1","option_id":"allow"}"#
        let event = try ControlEvent.parseSSEJSON(json)
        guard case .agentPermissionResolved(let taskId, let optionId) = event else {
            Issue.record("Expected agentPermissionResolved")
            return
        }
        #expect(taskId == "t1")
        #expect(optionId == "allow")
    }

    @Test func agentTaskCompleted() throws {
        let json =
            #"{"type":"agent_task_completed","task_id":"t1","objective":"list files","result":"ok\n"}"#
        let event = try ControlEvent.parseSSEJSON(json)
        guard case .agentTaskCompleted(let taskId, let objective, let result) = event else {
            Issue.record("Expected agentTaskCompleted")
            return
        }
        #expect(taskId == "t1")
        #expect(objective == "list files")
        #expect(result == "ok\n")
    }

    @Test func unknownTypeBecomesUnknown() throws {
        let json = #"{"type":"future_event_xyz","foo":1}"#
        let event = try ControlEvent.parseSSEJSON(json)
        guard case .unknown(let type) = event else {
            Issue.record("Expected unknown")
            return
        }
        #expect(type == "future_event_xyz")
    }

    @Test func parseSSEBlockDataPrefix() {
        let block = "data: {\"type\":\"mute_changed\",\"muted\":true}"
        let event = ControlSSEClient.parseSSEBlock(block)
        guard case .muteChanged(let muted) = event else {
            Issue.record("Expected muteChanged from SSE block")
            return
        }
        #expect(muted == true)
    }

    @Test func parseSSEBlockSkipsUnknown() {
        let block = "data: {\"type\":\"not_a_real_event\"}"
        let event = ControlSSEClient.parseSSEBlock(block)
        #expect(event == nil)
    }

    @Test func parseSSEBlockIgnoresKeepAliveComment() {
        let block = ": keep-alive"
        #expect(ControlSSEClient.parseSSEBlock(block) == nil)
    }

    @Test func permissionSlotDecodes() throws {
        let json = """
        [{"task_id":"t1","agent_name":"hermes","description":"bash: ls","options":[{"id":"allow","label":"Allow once","kind":"allow"}],"phase":"pending"}]
        """
        let data = json.data(using: .utf8)!
        let slots = try JSONDecoder().decode([PermissionSlot].self, from: data)
        #expect(slots.count == 1)
        #expect(slots[0].taskId == "t1")
        #expect(slots[0].phase == "pending")
        #expect(slots[0].options[0].id == "allow")
    }

    @Test func controlStateResponseDecodes() throws {
        let json =
            #"{"state":"listening","utterance_id":3,"tts_muted":false,"pause_reason":null}"#
        let data = json.data(using: .utf8)!
        let state = try JSONDecoder().decode(ControlStateResponse.self, from: data)
        #expect(state.state == "listening")
        #expect(state.utteranceId == 3)
        #expect(state.ttsMuted == false)
        #expect(state.pipelineState == .listening)
    }

    @Test func pipelineStateTokens() {
        #expect(CompanionPipelineState(hostToken: "idle") == .idle)
        // Host may send lowercase tokens; mapping is case-insensitive for safety.
        #expect(CompanionPipelineState(hostToken: "Listening") == .listening)
        #expect(CompanionPipelineState(hostToken: "thinking") == .thinking)
        #expect(CompanionPipelineState(hostToken: "nope") == .unknown)
    }
}
