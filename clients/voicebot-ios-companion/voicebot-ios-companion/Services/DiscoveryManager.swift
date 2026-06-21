//
//  DiscoveryManager.swift
//  voicebot-ios-companion
//
//  Created by Dani Vela on 13/06/2026.
//

import Foundation
import Combine

struct VoicebotService: Identifiable, Hashable, Sendable {
    let id: String
    let host: String
    var port: UInt16
    let name: String
    
    var wsURL: URL? {
        URL(string: "ws://\(host):\(port)/ws")
    }
}

final class DiscoveryManager: ObservableObject, Sendable {
    @Published var discoveredServices: [VoicebotService] = []
    @Published var selectedHost: String = ""
    @Published var selectedPort: String = "9090"
    @Published var selectedControlPort: String = "9090"
    
    init() {
        loadLastUsedHost()
    }
    
    var manualURL: URL? {
        guard !selectedHost.isEmpty, let port = UInt16(selectedPort), port > 0 else { return nil }
        return URL(string: "ws://\(selectedHost):\(port)/ws")
    }
    
    func browse() {
        // Server does not advertise via Bonjour; manual entry is the primary path.
        // This stub can be extended if server-side mDNS advertising is added.
    }
    
    func stopBrowse() {
        // No-op: Bonjour browsing not active.
    }
    
    func connectURL() -> URL? {
        if let url = manualURL {
            return url
        }
        
        if let service = discoveredServices.first, let url = service.wsURL {
            return url
        }
        
        return nil
    }
    
    private func saveLastUsedHost() {
        UserDefaults.standard.set(selectedHost, forKey: "lastUsedHost")
        UserDefaults.standard.set(selectedPort, forKey: "lastUsedPort")
        UserDefaults.standard.set(selectedControlPort, forKey: "lastUsedControlPort")
    }
    
    private func loadLastUsedHost() {
        if let host = UserDefaults.standard.string(forKey: "lastUsedHost") {
            selectedHost = host
        }
        if let port = UserDefaults.standard.string(forKey: "lastUsedPort") {
            selectedPort = port
        }
        if let controlPort = UserDefaults.standard.string(forKey: "lastUsedControlPort") {
            selectedControlPort = controlPort
        }
    }
}
