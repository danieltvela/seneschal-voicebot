//
//  voicebot_watchos_companionApp.swift
//  voicebot-watchos-companion Watch App
//

import SwiftUI

@main
struct voicebot_watchos_companionApp: App {
    @StateObject private var viewModel = WatchViewModel()
    
    var body: some Scene {
        WindowGroup {
            ContentView(viewModel: viewModel)
        }
    }
}
