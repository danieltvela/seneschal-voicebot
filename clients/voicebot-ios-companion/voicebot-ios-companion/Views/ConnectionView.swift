//
//  ConnectionView.swift
//  voicebot-ios-companion
//
//  Created by Dani Vela on 13/06/2026.
//

import SwiftUI

struct ConnectionView: View {
    @EnvironmentObject var vm: CompanionViewModel
    @State private var tempHost = ""
    @State private var tempPort = ""
    
    private enum Defaults {
        static let hostKey = "lastUsedHost"
        static let portKey = "lastUsedPort"
    }
    
    var body: some View {
        Form {
            Section("Server") {
                TextField("Host", text: $tempHost)
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                    .accessibilityIdentifier("hostTextField")
                
                TextField("Port", text: $tempPort)
                    .keyboardType(.numberPad)
                    .accessibilityIdentifier("portTextField")
            }
            
            Section {
                Button(vm.connectionState == .connected ? "Disconnect" : "Connect") {
                    dismissKeyboard()
                    saveConnectionSettings()
                    vm.selectedHost = tempHost
                    vm.selectedPort = tempPort
                    if vm.connectionState == .connected {
                        vm.disconnect()
                    } else {
                        Task { await vm.connect() }
                    }
                }
                .accessibilityIdentifier("connectButton")
                .disabled(tempHost.isEmpty)
            }
            
            if let error = vm.errorMessage {
                Section {
                    Text(error)
                        .foregroundColor(.red)
                        .font(.caption)
                }
            }
        }
        .navigationTitle("Voicebot")
        .onAppear(perform: loadConnectionSettings)
        .onChange(of: vm.connectionState) { newState in
            switch newState {
            case .connected:
                tempHost = vm.selectedHost
                tempPort = vm.selectedPort
            default:
                break
            }
        }
    }
    
    private func loadConnectionSettings() {
        tempHost = UserDefaults.standard.string(forKey: Defaults.hostKey) ?? vm.selectedHost
        tempPort = UserDefaults.standard.string(forKey: Defaults.portKey) ?? vm.selectedPort
    }
    
    private func saveConnectionSettings() {
        UserDefaults.standard.set(tempHost, forKey: Defaults.hostKey)
        UserDefaults.standard.set(tempPort, forKey: Defaults.portKey)
    }
    
    private func dismissKeyboard() {
        UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
    }
}

struct ConversationView: View {
    @EnvironmentObject var vm: CompanionViewModel
    
    var body: some View {
        ScrollView {
            LazyVStack(spacing: 8) {
                ForEach(vm.chatMessages.indices, id: \.self) { i in
                    let msg = vm.chatMessages[i]
                    HStack {
                        if msg.role == .user {
                            Spacer()
                            Bubble(text: msg.text, isUser: true)
                        } else {
                            Bubble(text: msg.text, isUser: false)
                            Spacer()
                        }
                    }
                }
            }
            .padding()
            .accessibilityIdentifier("conversationList")
        }
        .toolbar {
            ToolbarItem(placement: .principal) {
                ConnectionStateBadge(state: vm.connectionState)
                    .accessibilityIdentifier("statusBadge")
            }
            if vm.connectionState == .connected {
                ToolbarItem {
                    Button {
                        vm.bargeIn()
                    } label: {
                        Image(systemName: "mic.slash")
                    }
                }
            }
        }
    }
}

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
        }
    }
    
    private var label: String {
        switch state {
        case .connected: return "Connected"
        case .connecting: return "Connecting..."
        case .disconnected: return "Disconnected"
        case .failed: return "Error"
        }
    }
}

#Preview {
    NavigationStack {
        ConnectionView()
            .environmentObject(CompanionViewModel())
    }
}
