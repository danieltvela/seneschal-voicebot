//
//  ContentView.swift
//  voicebot-ios-companion
//

import SwiftUI

struct ContentView: View {
    @StateObject private var viewModel = CompanionViewModel()
    @Environment(\.scenePhase) private var scenePhase
    @State private var showTimeline = false

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                if viewModel.isSessionActive {
                    StatusBarView(onOpenTimeline: { showTimeline = true })
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
            .sheet(isPresented: $showTimeline) {
                NavigationStack {
                    TimelineView()
                        .environmentObject(viewModel)
                        .toolbar {
                            ToolbarItem(placement: .cancellationAction) {
                                Button("Done") { showTimeline = false }
                            }
                        }
                }
                .presentationDetents([.medium, .large])
            }
            .sheet(isPresented: permissionSheetBinding) {
                if let request = viewModel.pendingPermission {
                    PermissionSheet(request: request)
                        .environmentObject(viewModel)
                }
            }
        }
        .onChange(of: scenePhase) { phase in
            viewModel.handleScenePhase(phase)
        }
    }

    /// Present PermissionSheet when host requests approval and user has not chosen "Later".
    private var permissionSheetBinding: Binding<Bool> {
        Binding(
            get: { viewModel.shouldPresentPermissionSheet },
            set: { presented in
                if !presented {
                    // Swipe-dismiss ≈ Later (keep pending for StatusBar chip).
                    if viewModel.pendingPermission != nil {
                        viewModel.permissionSheetDismissedByUser = true
                    }
                }
            }
        )
    }
}

#Preview {
    ContentView()
}
