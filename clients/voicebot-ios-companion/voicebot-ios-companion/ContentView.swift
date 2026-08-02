//
//  ContentView.swift
//  voicebot-ios-companion
//

import SwiftUI

struct ContentView: View {
    @StateObject private var viewModel = CompanionViewModel()
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                if viewModel.isSessionActive {
                    StatusBarView()
                        .transition(.move(edge: .top).combined(with: .opacity))
                } else {
                    ConnectionControlsView()
                        .transition(.move(edge: .top).combined(with: .opacity))
                }

                Divider()

                ConversationView()
                    .opacity(viewModel.isSessionActive ? 1.0 : 0.4)

                if viewModel.isSessionActive {
                    ComposerView()
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
            }
            .animation(.easeInOut(duration: 0.3), value: viewModel.isSessionActive)
            .navigationTitle("Seneschal")
            .environmentObject(viewModel)
        }
        .onChange(of: scenePhase) { phase in
            viewModel.handleScenePhase(phase)
        }
    }
}

#Preview {
    ContentView()
}
