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
            Section {
                TextField("Host", text: $tempHost)
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                    .accessibilityIdentifier("hostTextField")
                    .accessibilityLabel("Host address")
                    .accessibilityHint("IP address or hostname of the Seneschal machine")

                TextField("WebSocket Port", text: $tempPort)
                    .keyboardType(.numberPad)
                    .accessibilityIdentifier("portTextField")
                    .accessibilityLabel("WebSocket port")
                    .accessibilityHint("Audio remote port, usually 9090")

                TextField("Control Port", text: $tempControlPort)
                    .keyboardType(.numberPad)
                    .accessibilityIdentifier("controlPortTextField")
                    .accessibilityLabel("Control port")
                    .accessibilityHint("Control API port, usually 9001")
            } header: {
                Text("Server")
            } footer: {
                Text("WS carries mic and TTS audio. Control carries status, mute, text, and timeline events.")
                    .font(.footnote)
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
                .accessibilityLabel(vm.isConnecting ? "Connecting" : "Connect to Seneschal")
                .accessibilityHint("Opens audio WebSocket and Control API to the host")
                .disabled(tempHost.isEmpty || vm.isConnecting)
            }

            if let error = vm.errorMessage {
                Section {
                    Text(error)
                        .foregroundColor(.red)
                        .font(.body)
                        .accessibilityLabel("Connection error: \(error)")
                }
            }
        }
        .formStyle(.grouped)
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
                        .accessibilityHidden(true)
                    Text(err)
                        .font(.subheadline)
                        .foregroundColor(.primary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .fixedSize(horizontal: false, vertical: true)
                    Button {
                        vm.errorMessage = nil
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                            .foregroundColor(.secondary)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Dismiss error")
                }
                .padding(12)
                .background(Color.orange.opacity(0.12))
                .accessibilityIdentifier("conversationErrorBanner")
                .accessibilityElement(children: .combine)
            }

            ScrollViewReader { proxy in
                ScrollView {
                    if vm.chatMessages.isEmpty {
                        VStack(spacing: 12) {
                            Image(systemName: "bubble.left.and.bubble.right")
                                .font(.system(size: 40))
                                .foregroundColor(.secondary.opacity(0.5))
                                .accessibilityHidden(true)
                            Text(vm.isSessionActive ? "No messages yet" : "Connect to Seneschal")
                                .font(.title3.weight(.semibold))
                                .foregroundColor(.secondary)
                                .multilineTextAlignment(.center)
                            Text(
                                vm.isSessionActive
                                    ? "Speak or type a message"
                                    : "Enter host and ports to begin"
                            )
                            .font(.body)
                            .foregroundColor(.secondary.opacity(0.85))
                            .multilineTextAlignment(.center)
                        }
                        .padding(.top, 80)
                        .padding(.horizontal, 24)
                        .frame(maxWidth: .infinity)
                        .accessibilityIdentifier("conversationEmpty")
                        .accessibilityElement(children: .combine)
                    } else {
                        LazyVStack(spacing: 10) {
                            ForEach(vm.chatMessages) { msg in
                                MessageBubble(
                                    message: msg,
                                    showTimestamp: showTimestamps
                                )
                                .id(msg.id.uuidString)
                                .onTapGesture {
                                    withAnimation { showTimestamps.toggle() }
                                }
                                .accessibilityHint("Double tap to show or hide relative time")
                            }
                            if vm.isGenerating {
                                HStack {
                                    TypingIndicator()
                                    Spacer(minLength: 0)
                                }
                                .padding(.leading, 12)
                                .accessibilityLabel("Assistant is generating a response")
                            }
                        }
                        .padding()
                        .frame(maxWidth: AdaptiveLayout.conversationMaxWidth)
                        .frame(maxWidth: .infinity)
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
        VStack(alignment: message.role == .user ? .trailing : .leading, spacing: 2) {
            HStack {
                if message.role == .user {
                    Spacer(minLength: 24)
                    Bubble(text: message.text, isUser: true)
                } else {
                    Bubble(text: message.text, isUser: false)
                    Spacer(minLength: 24)
                }
            }
            if showTimestamp {
                Text(Self.formatter.localizedString(for: message.timestamp, relativeTo: Date()))
                    .font(.caption)
                    .foregroundColor(.secondary)
                    .padding(.horizontal, 4)
                    .accessibilityLabel(
                        "Sent \(Self.formatter.localizedString(for: message.timestamp, relativeTo: Date()))"
                    )
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
            "\(message.role == .user ? "You" : "Seneschal"): \(message.text)"
        )
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
            .font(.body)
            .padding(12)
            // Semantic colors: better contrast + dark mode than white-on-gray.
            .foregroundColor(isUser ? Color.white : Color.primary)
            .background(isUser ? Color.accentColor : Color(.secondarySystemBackground))
            .cornerRadius(16)
            .overlay(
                RoundedRectangle(cornerRadius: 16)
                    .strokeBorder(
                        isUser ? Color.clear : Color.primary.opacity(0.08),
                        lineWidth: 1
                    )
            )
            .frame(maxWidth: AdaptiveLayout.bubbleMaxWidth, alignment: isUser ? .trailing : .leading)
            .fixedSize(horizontal: false, vertical: true)
            .textSelection(.enabled)
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
