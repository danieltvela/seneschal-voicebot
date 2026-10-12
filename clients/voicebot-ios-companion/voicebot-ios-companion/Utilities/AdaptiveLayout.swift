//
//  AdaptiveLayout.swift
//  voicebot-ios-companion
//
//  Size-class-driven layout constants (iPhone compact vs iPad regular / multitasking).
//

import SwiftUI

enum AdaptiveLayout {
    /// Max width for conversation column content on large canvases.
    static let conversationMaxWidth: CGFloat = 720
    /// Readable bubble cap so landscape iPad does not stretch edge-to-edge.
    static let bubbleMaxWidth: CGFloat = 520
    /// Connection form centered width on regular width.
    static let connectionFormMaxWidth: CGFloat = 480
    /// Trailing timeline column on regular width.
    static let timelineColumnMin: CGFloat = 280
    static let timelineColumnIdeal: CGFloat = 320
    static let timelineColumnMax: CGFloat = 400

    /// Use split (conversation + live timeline) when horizontal size class is regular.
    static func usesSplitLayout(horizontalSizeClass: UserInterfaceSizeClass?) -> Bool {
        horizontalSizeClass == .regular
    }
}

// MARK: - Readable width helper

struct ReadableWidthModifier: ViewModifier {
    let maxWidth: CGFloat

    func body(content: Content) -> some View {
        content
            .frame(maxWidth: maxWidth)
            .frame(maxWidth: .infinity)
    }
}

extension View {
    /// Center content and cap width (connection form, conversation column).
    func readableWidth(_ maxWidth: CGFloat) -> some View {
        modifier(ReadableWidthModifier(maxWidth: maxWidth))
    }
}
