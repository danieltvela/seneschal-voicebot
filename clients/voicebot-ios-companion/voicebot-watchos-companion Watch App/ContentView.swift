//
//  ContentView.swift
//  voicebot-watchos-companion Watch App
//
//  PTT + pipeline state color/text + last assistant line preview.
//

import SwiftUI

struct ContentView: View {
    @ObservedObject var viewModel: WatchViewModel

    var body: some View {
        ZStack {
            Color.black.ignoresSafeArea()

            VStack(spacing: 10) {
                pipelineBadge
                statusIndicator
                pttButton
                lastLinePreview
            }
            .padding(.horizontal, 6)
        }
        .accessibilityElement(children: .contain)
    }

    private var pipelineBadge: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(pipelineColor)
                .frame(width: 8, height: 8)
                .accessibilityHidden(true)
            Text(viewModel.pipelineState.displayLabel)
                .font(.caption2.weight(.semibold))
                .foregroundColor(pipelineColor)
                .lineLimit(1)
        }
        .padding(.top, 4)
        .accessibilityLabel("Pipeline \(viewModel.pipelineState.displayLabel)")
    }

    private var statusIndicator: some View {
        Text(viewModel.statusText)
            .font(.caption)
            .foregroundColor(.white.opacity(0.9))
            .multilineTextAlignment(.center)
            .lineLimit(2)
            .minimumScaleFactor(0.8)
            .accessibilityLabel(viewModel.statusText)
    }

    private var pttButton: some View {
        Button(action: {
            if viewModel.isRecording {
                viewModel.stopRecording()
            } else {
                viewModel.startRecording()
            }
        }) {
            ZStack {
                Circle()
                    .fill(buttonColor)
                    .frame(width: 72, height: 72)

                Image(systemName: viewModel.isRecording ? "stop.fill" : "mic.fill")
                    .font(.title2)
                    .foregroundColor(.white)
            }
        }
        .buttonStyle(.plain)
        .disabled(!viewModel.isConnected)
        .accessibilityLabel(viewModel.isRecording ? "Stop recording" : "Push to talk")
        .accessibilityHint("Hold-style: tap to start, tap again to stop")
    }

    private var lastLinePreview: some View {
        Group {
            if !viewModel.lastLine.isEmpty {
                Text(viewModel.lastLine)
                    .font(.caption2)
                    .foregroundColor(.white.opacity(0.75))
                    .lineLimit(2)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 4)
                    .accessibilityLabel("Last reply: \(viewModel.lastLine)")
            } else {
                Text(" ")
                    .font(.caption2)
                    .accessibilityHidden(true)
            }
        }
        .frame(maxWidth: .infinity)
        .frame(minHeight: 28)
    }

    private var buttonColor: Color {
        if viewModel.isRecording {
            return .red
        }
        switch viewModel.pipelineState {
        case .listening:
            return .green
        case .thinking:
            return .blue
        case .speaking:
            return .purple
        case .paused:
            return .orange
        case .idle:
            return viewModel.hostSessionActive ? .blue : .gray
        case .unknown:
            return viewModel.isConnected ? .blue.opacity(0.7) : .gray
        }
    }

    private var pipelineColor: Color {
        switch viewModel.pipelineState {
        case .idle: return .secondary
        case .listening: return .green
        case .thinking: return .blue
        case .speaking: return .purple
        case .paused: return .orange
        case .unknown: return .gray
        }
    }
}
