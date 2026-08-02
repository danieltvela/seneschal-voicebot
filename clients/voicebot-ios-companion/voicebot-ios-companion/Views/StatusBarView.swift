//
//  StatusBarView.swift
//  voicebot-ios-companion
//
//  Pipeline + link health + mute/disconnect for connected sessions.
//

import SwiftUI

struct StatusBarView: View {
    @EnvironmentObject var vm: CompanionViewModel
    var onOpenTimeline: (() -> Void)?
    /// Compact layouts open timeline as a sheet; regular width shows a live column.
    var showsTimelineButton: Bool = true

    var body: some View {
        VStack(spacing: 6) {
            // Status chips wrap under large Dynamic Type; controls stay tappable.
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 8) {
                    statusCluster
                    Spacer(minLength: 4)
                    controlCluster
                }
                VStack(alignment: .leading, spacing: 8) {
                    statusCluster
                    controlCluster
                }
            }

            if let banner = vm.controlBanner {
                Text(banner)
                    .font(.caption2)
                    .foregroundColor(.orange)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .accessibilityIdentifier("controlBanner")
            }

            // Compact chip when sheet was dismissed with "Later" (full UI is PermissionSheet).
            if let pending = vm.pendingPermission, vm.permissionSheetDismissedByUser {
                Button {
                    vm.reopenPermissionSheet()
                } label: {
                    HStack(spacing: 6) {
                        Image(systemName: "hand.raised.fill")
                        Text("Permission pending")
                            .fontWeight(.medium)
                        Text("· \(pending.agentName.isEmpty ? pending.taskId : pending.agentName)")
                            .lineLimit(1)
                        Spacer()
                        Text("Open")
                            .fontWeight(.semibold)
                    }
                    .font(.subheadline)
                    .foregroundColor(.orange)
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color.orange.opacity(0.12))
                    .cornerRadius(8)
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("permissionPendingChip")
                .accessibilityLabel("Permission pending from \(pending.agentName)")
                .accessibilityHint("Opens the permission approval sheet")
            }
        }
        .padding()
        .background(Color(.systemGray6))
        .accessibilityElement(children: .contain)
    }

    private var statusCluster: some View {
        HStack(spacing: 8) {
            linkDot(vm.audioLink, label: "Audio")
            linkDot(vm.controlLink, label: "Control")

            if vm.pipelineState != .unknown {
                Text(vm.pipelineState.rawValue.capitalized)
                    .font(.subheadline.weight(.semibold))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3)
                    .background(pipelineColor.opacity(0.15))
                    .foregroundColor(pipelineColor)
                    .clipShape(Capsule())
                    .accessibilityIdentifier("pipelineStateBadge")
                    .accessibilityLabel("Pipeline state \(vm.pipelineState.rawValue)")
            }

            if let chip = vm.classificationChip {
                Text(chip)
                    .font(.caption.weight(.medium))
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(Color.indigo.opacity(0.12))
                    .foregroundColor(.indigo)
                    .clipShape(Capsule())
                    .accessibilityIdentifier("classificationChip")
                    .accessibilityLabel("Classification \(chip)")
            }

            if vm.ttsMuted {
                Image(systemName: "speaker.slash.fill")
                    .font(.subheadline)
                    .foregroundColor(.secondary)
                    .accessibilityLabel("TTS is muted")
            }
        }
        .accessibilityElement(children: .contain)
    }

    private var controlCluster: some View {
        HStack(spacing: 8) {
            if showsTimelineButton {
                Button {
                    onOpenTimeline?()
                } label: {
                    Image(systemName: "list.bullet.rectangle")
                    if !vm.timeline.isEmpty {
                        Text("\(vm.timeline.count)")
                            .font(.caption.monospacedDigit())
                    }
                }
                .buttonStyle(.bordered)
                .controlSize(.regular)
                .accessibilityLabel(
                    vm.timeline.isEmpty
                        ? "Open timeline"
                        : "Open timeline, \(vm.timeline.count) events"
                )
                .accessibilityHint("Shows tools, system, and agent events")
                .accessibilityIdentifier("timelineButton")
            }

            if vm.isControlConnectedOrReconnecting {
                Button {
                    vm.toggleMute()
                } label: {
                    Image(systemName: vm.ttsMuted ? "speaker.slash.fill" : "speaker.wave.2.fill")
                }
                .buttonStyle(.bordered)
                .controlSize(.regular)
                .accessibilityLabel(vm.ttsMuted ? "Unmute TTS" : "Mute TTS")
                .accessibilityHint("Toggles text-to-speech playback on the host")
                .accessibilityIdentifier("muteButton")
            }

            if vm.hasAudioPath {
                Button {
                    vm.bargeIn()
                } label: {
                    Image(systemName: "waveform.badge.mic")
                }
                .buttonStyle(.bordered)
                .controlSize(.regular)
                .accessibilityLabel("Barge in")
                .accessibilityHint("Interrupts the current spoken response")
                .accessibilityIdentifier("bargeInButton")
            }

            Button("Disconnect") {
                vm.disconnect()
            }
            .buttonStyle(.bordered)
            .controlSize(.regular)
            .accessibilityIdentifier("disconnectButton")
            .accessibilityHint("Closes audio and control connections")
        }
    }

    private func linkDot(_ state: LinkState, label: String) -> some View {
        HStack(spacing: 3) {
            Circle()
                .fill(color(for: state))
                .frame(width: 8, height: 8)
                .accessibilityHidden(true)
            Text(label)
                .font(.caption)
                .foregroundColor(.secondary)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(label) link \(linkLabel(state))")
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
