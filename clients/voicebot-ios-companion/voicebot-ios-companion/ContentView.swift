//
//  ContentView.swift
//  voicebot-ios-companion
//
//  Adaptive shell: compact stack (iPhone / Slide Over) vs regular split (iPad).
//

import SwiftUI

struct ContentView: View {
    @StateObject private var viewModel = CompanionViewModel()
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.horizontalSizeClass) private var horizontalSizeClass
    @State private var showTimeline = false

    private var usesSplit: Bool {
        AdaptiveLayout.usesSplitLayout(horizontalSizeClass: horizontalSizeClass)
    }

    var body: some View {
        NavigationStack {
            Group {
                if viewModel.isSessionActive {
                    if usesSplit {
                        regularSessionBody
                    } else {
                        compactSessionBody
                    }
                } else {
                    disconnectedBody
                }
            }
            .animation(.easeInOut(duration: 0.3), value: viewModel.isSessionActive)
            .animation(.easeInOut(duration: 0.25), value: usesSplit)
            .navigationTitle("Seneschal")
            .navigationBarTitleDisplayMode(usesSplit ? .inline : .large)
            .environmentObject(viewModel)
            .sheet(isPresented: $showTimeline) {
                NavigationStack {
                    TimelineView()
                        .environmentObject(viewModel)
                        .toolbar {
                            ToolbarItem(placement: .cancellationAction) {
                                Button("Done") { showTimeline = false }
                                    .accessibilityLabel("Close timeline")
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
        // When expanding to regular width, dismiss sheet timeline (column is live).
        .onChange(of: usesSplit) { split in
            if split { showTimeline = false }
        }
    }

    // MARK: - Layouts

    /// iPhone / compact width: stack + timeline sheet.
    private var compactSessionBody: some View {
        VStack(spacing: 0) {
            StatusBarView(
                onOpenTimeline: { showTimeline = true },
                showsTimelineButton: true
            )
            .accessibilityElement(children: .contain)

            Divider()

            ConversationView()
                .frame(maxWidth: .infinity, maxHeight: .infinity)

            ComposerView()
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Seneschal session")
    }

    /// iPad / regular width: conversation + live timeline side by side.
    private var regularSessionBody: some View {
        VStack(spacing: 0) {
            StatusBarView(
                onOpenTimeline: nil,
                showsTimelineButton: false
            )

            Divider()

            HStack(spacing: 0) {
                VStack(spacing: 0) {
                    ConversationView()
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                    ComposerView()
                }
                .frame(maxWidth: .infinity)
                .readableWidth(AdaptiveLayout.conversationMaxWidth)

                Divider()

                TimelineView()
                    .frame(
                        minWidth: AdaptiveLayout.timelineColumnMin,
                        idealWidth: AdaptiveLayout.timelineColumnIdeal,
                        maxWidth: AdaptiveLayout.timelineColumnMax
                    )
                    .frame(maxHeight: .infinity)
                    .background(Color(.secondarySystemBackground).opacity(0.35))
                    .accessibilityLabel("Live event timeline")
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Seneschal session, split view")
    }

    private var disconnectedBody: some View {
        ConnectionControlsView()
            .readableWidth(AdaptiveLayout.connectionFormMaxWidth)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .accessibilityLabel("Connect to Seneschal")
    }

    /// Present PermissionSheet when host requests approval and user has not chosen "Later".
    private var permissionSheetBinding: Binding<Bool> {
        Binding(
            get: { viewModel.shouldPresentPermissionSheet },
            set: { presented in
                if !presented {
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
