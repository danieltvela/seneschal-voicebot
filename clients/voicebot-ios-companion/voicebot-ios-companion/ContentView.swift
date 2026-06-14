//
//  ContentView.swift
//  voicebot-ios-companion
//
//  Created by Dani Vela on 13/06/2026.
//

import SwiftUI

struct ContentView: View {
    @StateObject private var viewModel = CompanionViewModel()
    
    var body: some View {
        TabView {
            ConnectionView()
                .tabItem {
                    Label("Connect", systemImage: "wifi")
                }
            
            ConversationView()
                .tabItem {
                    Label("Chat", systemImage: "bubble.left.and.bubble.right")
                }
        }
        .environmentObject(viewModel)
    }
}

#Preview {
    ContentView()
}
