//
//  ConnectionView.swift
//  voicebot-ios-companion
//
//  Created by Dani Vela on 13/06/2026.
//

import SwiftUI

struct ConnectionControlsView: View {
    @EnvironmentObject var vm: CompanionViewModel
    @State private var tempHost = ""
    @State private var tempPort = ""
    @State private var tempControlPort = ""

    private enum Defaults {
        static let hostKey = "lastUsedHost"
        static let portKey = "lastUsedPort"
        static let controlPortKey = "lastUsedControlPort"
    }

    var body: some View {
        Form {
            Section("Server") {
                TextField("Host", text: $tempHost)
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                    .accessibilityIdentifier("hostTextField")

                TextField("WebSocket Port", text: $tempPort)
                    .keyboardType(.numberPad)
                    .accessibilityIdentifier("portTextField")

                TextField("Control Port", text: $tempControlPort)
                    .keyboardType(.numberPad)
                    .accessibilityIdentifier("controlPortTextField")
            }

            Section {
                Button(vm.isConnecting ? "Connecting…" : "Connect") {
                    dismissKeyboard()
                    saveConnectionSettings()
                    vm.selectedHost = tempHost
                    vm.selectedPort = tempPort
                    vm.selectedControlPort = tempControlPort
                    Task { await vm.connect() }
                }
                .accessibilityIdentifier("connectButton")
                .disabled(tempHost.isEmpty || vm.isConnecting)
            }

            if let error = vm.errorMessage {
                Section {
                    Text(error)
                        .foregroundColor(.red)
                        .font(.caption)
                }
            }
        }
        .navigationTitle("Connect to Seneschal")
        .onAppear(perform: loadConnectionSettings)
    }

    private func loadConnectionSettings() {
        tempHost = UserDefaults.standard.string(forKey: Defaults.hostKey) ?? vm.selectedHost
        tempPort = UserDefaults.standard.string(forKey: Defaults.portKey) ?? vm.selectedPort
        tempControlPort = UserDefaults.standard.string(forKey: Defaults.controlPortKey) ?? vm.selectedControlPort
    }

    private func saveConnectionSettings() {
        UserDefaults.standard.set(tempHost, forKey: Defaults.hostKey)
        UserDefaults.standard.set(tempPort, forKey: Defaults.portKey)
        UserDefaults.standard.set(tempControlPort, forKey: Defaults.controlPortKey)
    }

    private func dismissKeyboard() {
        UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
    }
}

// MARK: - Conversation View

struct ConversationView: View {
    @EnvironmentObject var vm: CompanionViewModel
    @State private var showTimestamps = false

    var body: some View {
        VStack(spacing: 0) {
            if let err = vm.errorMessage, vm.isSessionActive {
                HStack(alignment: .top, spacing: 8) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundColor(.orange)
                    Text(err)
                        .font(.caption)
                        .foregroundColor(.primary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    Button {
                        vm.errorMessage = nil
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                            .foregroundColor(.secondary)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Dismiss error")
                }
                .padding(10)
                .background(Color.orange.opacity(0.12))
                .accessibilityIdentifier("conversationErrorBanner")
            }

            ScrollViewReader { proxy in
                ScrollView {
                    if vm.chatMessages.isEmpty {
                        VStack(spacing: 12) {
                            Image(systemName: "bubble.left.and.bubble.right")
                                .font(.system(size: 40))
                                .foregroundColor(.secondary.opacity(0.5))
                            Text(vm.isSessionActive ? "No messages yet" : "Connect to Seneschal")
                                .font(.headline)
                                .foregroundColor(.secondary)
                            Text(
                                vm.isSessionActive
                                    ? "Speak or type a message"
                                    : "Enter host and ports to begin"
                            )
                            .font(.caption)
                            .foregroundColor(.secondary.opacity(0.7))
                        }
                        .padding(.top, 80)
                        .frame(maxWidth: .infinity)
                        .accessibilityIdentifier("conversationEmpty")
                    } else {
                        LazyVStack(spacing: 8) {
                            ForEach(vm.chatMessages) { msg in
                                MessageBubble(
                                    message: msg,
                                    showTimestamp: showTimestamps
                                )
                                .id(msg.id.uuidString)
                                .onTapGesture {
                                    withAnimation { showTimestamps.toggle() }
                                }
                            }
                            if vm.isGenerating {
                                HStack {
                                    TypingIndicator()
                                    Spacer()
                                }
                                .padding(.leading, 12)
                            }
                        }
                        .padding()
                        .accessibilityIdentifier("conversationList")
                        .onChange(of: vm.chatMessages.count) { _ in
                            scrollToLatest(proxy: proxy)
                        }
                        .onChange(of: vm.isGenerating) { _ in
                            scrollToLatest(proxy: proxy)
                        }
                    }
                }
                .onAppear {
                    scrollToLatest(proxy: proxy)
                }
            }
        }
    }

    private func scrollToLatest(proxy: ScrollViewProxy) {
        guard let last = vm.chatMessages.last else { return }
        withAnimation {
            proxy.scrollTo(last.id.uuidString, anchor: .bottom)
        }
    }
}

// MARK: - Message Bubble

private struct MessageBubble: View {
    let message: ChatMessage
    let showTimestamp: Bool

    private static let formatter: RelativeDateTimeFormatter = {
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .abbreviated
        return f
    }()

    var body: some View {
        VStack(spacing: 2) {
            HStack {
                if message.role == .user {
                    Spacer()
                    Bubble(text: message.text, isUser: true)
                } else {
                    Bubble(text: message.text, isUser: false)
                    Spacer()
                }
            }
            if showTimestamp {
                Text(Self.formatter.localizedString(for: message.timestamp, relativeTo: Date()))
                    .font(.caption2)
                    .foregroundColor(.secondary.opacity(0.6))
                    .padding(.horizontal, 4)
            }
        }
    }
}

// MARK: - Typing Indicator

private struct TypingIndicator: View {
    @State private var dotOffset: CGFloat = -8

    var body: some View {
        HStack(spacing: 4) {
            ForEach(0..<3) { i in
                Circle()
                    .fill(Color.gray.opacity(0.5))
                    .frame(width: 6, height: 6)
                    .offset(y: dotOffset)
                    .animation(
                        .easeInOut(duration: 0.6).repeatForever().delay(Double(i) * 0.2),
                        value: dotOffset
                    )
            }
        }
        .padding(12)
        .background(Color.gray.opacity(0.15))
        .cornerRadius(16)
        .onAppear { dotOffset = 8 }
    }
}

// MARK: - Bubble

private struct Bubble: View {
    let text: String
    let isUser: Bool

    var body: some View {
        Text(text)
            .padding(10)
            .foregroundColor(.white)
            .background(isUser ? Color.blue : Color.gray.opacity(0.7))
            .cornerRadius(16)
            .fixedSize(horizontal: false, vertical: true)
    }
}

// MARK: - Connection State Badge

struct ConnectionStateBadge: View {
    let state: ConnectionState

    var body: some View {
        HStack(spacing: 4) {
            Circle()
                .fill(color)
                .frame(width: 8, height: 8)
            Text(label)
                .font(.caption2)
                .foregroundColor(.secondary)
        }
    }

    private var color: Color {
        switch state {
        case .connected: return .green
        case .connecting: return .yellow
        case .disconnected: return .gray
        case .failed: return .red
        case .conflict: return .orange
        }
    }

    private var label: String {
        switch state {
        case .connected: return "Connected"
        case .connecting: return "Connecting..."
        case .disconnected: return "Disconnected"
        case .failed: return "Error"
        case .conflict: return "Audio busy"
        }
    }
}

#Preview {
    ContentView()
}
