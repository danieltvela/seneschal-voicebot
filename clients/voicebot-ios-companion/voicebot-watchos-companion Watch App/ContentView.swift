//
//  ContentView.swift
//  voicebot-watchos-companion Watch App
//

import SwiftUI

struct ContentView: View {
    @ObservedObject var viewModel: WatchViewModel
    
    var body: some View {
        ZStack {
            Color.black.ignoresSafeArea()
            
            VStack(spacing: 20) {
                statusIndicator
                pttButton
            }
        }
    }
    
    private var statusIndicator: some View {
        Text(viewModel.statusText)
            .font(.caption)
            .foregroundColor(.white)
            .padding(.top, 20)
    }
    
    private var pttButton: some View {
        Button(action: {
            if viewModel.isRecording {
                viewModel.stopRecording()
            } else {
                viewModel.startRecording()
            }
        }) {
            Circle()
                .fill(buttonColor)
                .frame(width: 80, height: 80)
        }
        .disabled(!viewModel.isConnected)
    }
    
    private var buttonColor: Color {
        switch viewModel.appState {
        case .recording:
            return .red
        case .responding:
            return .yellow
        case .connected:
            return .blue
        default:
            return .gray
        }
    }
}
