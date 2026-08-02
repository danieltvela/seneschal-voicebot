//
//  StatusBarView.swift
//  voicebot-ios-companion
//
//  Pipeline + link health + mute/disconnect for connected sessions.
//

import SwiftUI

struct StatusBarView: View {
    @EnvironmentObject var vm: CompanionViewModel

    var body: some View {
        VStack(spacing: 6) {
            HStack(spacing: 8) {
                linkDot(vm.audioLink, label: "Audio")
                linkDot(vm.controlLink, label: "Control")

                if vm.pipelineState != .unknown {
                    Text(vm.pipelineState.rawValue.capitalized)
                        .font(.caption.weight(.semibold))
                        .padding(.horizontal, 8)
                        .padding(.vertical, 3)
                        .background(pipelineColor.opacity(0.15))
                        .foregroundColor(pipelineColor)
                        .clipShape(Capsule())
                        .accessibilityIdentifier("pipelineStateBadge")
                        .accessibilityLabel("Pipeline \(vm.pipelineState.rawValue)")
                }

                if vm.ttsMuted {
                    Image(systemName: "speaker.slash.fill")
                        .font(.caption)
                        .foregroundColor(.secondary)
                        .accessibilityLabel("TTS muted")
                }

                Spacer()

                if vm.isControlConnectedOrReconnecting {
                    Button {
                        vm.toggleMute()
                    } label: {
                        Image(systemName: vm.ttsMuted ? "speaker.slash.fill" : "speaker.wave.2.fill")
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .accessibilityLabel(vm.ttsMuted ? "Unmute TTS" : "Mute TTS")
                    .accessibilityIdentifier("muteButton")
                }

                if vm.hasAudioPath {
                    Button {
                        vm.bargeIn()
                    } label: {
                        Image(systemName: "waveform.badge.mic")
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .accessibilityLabel("Barge in")
                    .accessibilityIdentifier("bargeInButton")
                }

                Button("Disconnect") {
                    vm.disconnect()
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .accessibilityIdentifier("disconnectButton")
            }

            if let banner = vm.controlBanner {
                Text(banner)
                    .font(.caption2)
                    .foregroundColor(.orange)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .accessibilityIdentifier("controlBanner")
            }

            if let pending = vm.pendingPermission {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Permission: \(pending.description)")
                        .font(.caption)
                        .fontWeight(.medium)
                        .accessibilityIdentifier("permissionDescription")
                    HStack {
                        ForEach(pending.options) { opt in
                            Button(opt.label.isEmpty ? opt.id : opt.label) {
                                vm.resolvePermission(optionId: opt.id)
                            }
                            .buttonStyle(.borderedProminent)
                            .controlSize(.mini)
                            .accessibilityIdentifier("permissionOption-\(opt.id)")
                        }
                    }
                }
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.orange.opacity(0.12))
                .cornerRadius(8)
            }
        }
        .padding()
        .background(Color(.systemGray6))
    }

    private func linkDot(_ state: LinkState, label: String) -> some View {
        HStack(spacing: 3) {
            Circle()
                .fill(color(for: state))
                .frame(width: 7, height: 7)
            Text(label)
                .font(.caption2)
                .foregroundColor(.secondary)
        }
        .accessibilityLabel("\(label) \(linkLabel(state))")
    }

    private func color(for state: LinkState) -> Color {
        switch state {
        case .connected: return .green
        case .connecting, .reconnecting: return .yellow
        case .conflict: return .orange
        case .failed: return .red
        case .disconnected: return .gray
        }
    }

    private func linkLabel(_ state: LinkState) -> String {
        switch state {
        case .connected: return "connected"
        case .connecting: return "connecting"
        case .reconnecting(let n): return "reconnecting \(n)"
        case .conflict: return "conflict"
        case .failed: return "failed"
        case .disconnected: return "disconnected"
        }
    }

    private var pipelineColor: Color {
        switch vm.pipelineState {
        case .idle: return .secondary
        case .listening: return .green
        case .thinking: return .blue
        case .speaking: return .purple
        case .paused: return .orange
        case .unknown: return .gray
        }
    }
}

private extension CompanionViewModel {
    var isControlConnectedOrReconnecting: Bool {
        switch controlLink {
        case .connected, .reconnecting: return true
        default: return false
        }
    }
}
