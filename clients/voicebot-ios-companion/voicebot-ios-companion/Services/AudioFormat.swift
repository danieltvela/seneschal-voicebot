//
//  AudioFormat.swift
//  voicebot-ios-companion
//
//  Created by Dani Vela on 13/06/2026.
//

import Foundation

enum AudioFormat {
    static let sampleRate: Double = 16000.0
    static let channels: UInt32 = 1
    static let bytesPerSample: Int = 2
    static let frameSizeSamples: Int = 320
    static let bytesPerFrame: Int = 640
}

func floatToInt16(_ samples: [Float]) -> Data {
    let clamped = samples.map { max(-1.0, min(1.0, $0)) }
    var bytes = [UInt8](repeating: 0, count: clamped.count * 2)
    for (i, sample) in clamped.enumerated() {
        let value = Int16(round(sample * 32767.0))
        bytes[i * 2] = UInt8(value & 0xFF)
        bytes[i * 2 + 1] = UInt8((value >> 8) & 0xFF)
    }
    return Data(bytes)
}

func int16ToFloat(_ data: Data) -> [Float] {
    guard data.count % 2 == 0 else { return [] }
    var samples: [Float] = []
    let byteCount = data.count / 2
    data.withUnsafeBytes { ptr in
        let pointers = ptr.bindMemory(to: Int16.self)
        for i in 0..<byteCount {
            let value = pointers.baseAddress![i]
            samples.append(Float(value) / 32767.0)
        }
    }
    return samples
}
