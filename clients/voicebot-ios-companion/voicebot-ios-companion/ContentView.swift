//
//  ContentView.swift
//  voicebot-ios-companion
//
//  Created by Dani Vela on 13/06/2026.
//

import SwiftUI

struct ContentView: View {
    @StateObject private var viewModel = CompanionViewModel()

    private var isConnected: Bool {
        viewModel.connectionState == .connected || viewModel.connectionState == .connecting
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                if isConnected {
                    ConnectedHeaderView()
                        .transition(.move(edge: .top).combined(with: .opacity))
                } else {
                    ConnectionControlsView()
                        .transition(.move(edge: .top).combined(with: .opacity))
                }

                Divider()

                ConversationView()
                    .opacity(isConnected ? 1.0 : 0.4)
            }
            .animation(.easeInOut(duration: 0.3), value: viewModel.connectionState)
            .navigationTitle("Voicebot")
            .environmentObject(viewModel)
        }
    }
}

#Preview {
    ContentView()
}
