//
//  AdaptiveLayoutTests.swift
//  voicebot-ios-companionTests
//

import Testing
import SwiftUI
@testable import voicebot_ios_companion

struct AdaptiveLayoutTests {

    @Test func compactDoesNotUseSplit() {
        #expect(AdaptiveLayout.usesSplitLayout(horizontalSizeClass: .compact) == false)
        #expect(AdaptiveLayout.usesSplitLayout(horizontalSizeClass: nil) == false)
    }

    @Test func regularUsesSplit() {
        #expect(AdaptiveLayout.usesSplitLayout(horizontalSizeClass: .regular) == true)
    }

    @Test func readableCapsAreSensible() {
        #expect(AdaptiveLayout.bubbleMaxWidth < AdaptiveLayout.conversationMaxWidth)
        #expect(AdaptiveLayout.connectionFormMaxWidth <= AdaptiveLayout.conversationMaxWidth)
        #expect(AdaptiveLayout.timelineColumnMin <= AdaptiveLayout.timelineColumnIdeal)
        #expect(AdaptiveLayout.timelineColumnIdeal <= AdaptiveLayout.timelineColumnMax)
    }
}
