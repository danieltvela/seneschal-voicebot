//
//  ComposerView.swift
//  voicebot-ios-companion
//
//  Text input → POST /control/input; disabled when permission pending or Control down.
//

import SwiftUI

struct ComposerView: View {
    @EnvironmentObject var vm: CompanionViewModel
    @FocusState private var focused: Bool

    private var canSend: Bool {
        let hasText = !vm.composerText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        let controlUp: Bool = {
            switch vm.controlLink {
            case .connected, .reconnecting: return true
            default: return false
            }
        }()
        return hasText && controlUp && vm.pendingPermission == nil
    }

    var body: some View {
        VStack(spacing: 0) {
            Divider()
            HStack(spacing: 8) {
                TextField(
                    placeholder,
                    text: $vm.composerText,
                    axis: .vertical
                )
                .lineLimit(1 ... 4)
                .textFieldStyle(.roundedBorder)
                .focused($focused)
                .disabled(!controlUp)
                .accessibilityIdentifier("composerTextField")
                .onSubmit {
                    if canSend { vm.sendComposerText() }
                }

                Button {
                    vm.sendComposerText()
                    focused = false
                } label: {
                    Image(systemName: "arrow.up.circle.fill")
                        .font(.title2)
                }
                .disabled(!canSend)
                .accessibilityIdentifier("sendButton")
                .accessibilityLabel("Send message")
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(Color(.systemBackground))
        }
    }

    private var controlUp: Bool {
        switch vm.controlLink {
        case .connected, .reconnecting: return true
        default: return false
        }
    }

    private var placeholder: String {
        if vm.pendingPermission != nil {
            return "Answer permission request first…"
        }
        if !controlUp {
            return "Control offline — text unavailable"
        }
        return "Message Seneschal…"
    }
}
