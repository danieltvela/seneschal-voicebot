//
//  WatchComplication.swift
//  voicebot-watchos-companion Watch App
//

import SwiftUI
import WidgetKit

struct WidgetConfigurationEntry: TimelineEntry {
    let date: Date
}

struct WatchProvider: TimelineProvider {
    func placeholder(in context: TimelineProvider.Context) -> WidgetConfigurationEntry {
        WidgetConfigurationEntry(date: Date())
    }
    
    func getSnapshot(in context: TimelineProvider.Context, completion: @escaping (WidgetConfigurationEntry) -> Void) {
        completion(WidgetConfigurationEntry(date: Date()))
    }
    
    func getTimeline(in context: TimelineProvider.Context, completion: @escaping (Timeline<WidgetConfigurationEntry>) -> Void) {
        let entry = WidgetConfigurationEntry(date: Date())
        completion(Timeline(entries: [entry], policy: .never))
    }
}

struct WatchComplication: Widget {
    static let provider = WatchProvider()
    
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "voicebot-complication", provider: Self.provider) { _ in
            ZStack {
                Circle()
                    .fill(Color.blue)
                
                Image(systemName: "waveform")
                    .font(.caption)
                    .foregroundColor(.white)
            }
            .clipShape(Circle())
        }
    }
}
