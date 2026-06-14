//
//  AudioResampler.swift
//  voicebot-ios-companion
//
//  Created by Dani Vela on 13/06/2026.
//

import Foundation

enum AudioResampler {
    static func resample(samples: [Float], fromRate: Double, toRate: Double) -> [Float] {
        guard !samples.isEmpty, fromRate > 0, toRate > 0 else { return [] }
        guard abs(fromRate - toRate) > 0.1 else { return samples }
        
        let ratio = fromRate / toRate
        let outputCount = Int(Double(samples.count) / ratio)
        guard outputCount > 0 else { return [] }
        
        var output = [Float]()
        output.reserveCapacity(outputCount)
        
        for i in 0..<outputCount {
            let sourceIndex = Double(i) * ratio
            let index0 = Int(floor(sourceIndex))
            let index1 = min(index0 + 1, samples.count - 1)
            let fraction = Float(sourceIndex - Double(index0))
            
            let sample0 = samples[index0]
            let sample1 = samples[index1]
            output.append(sample0 + (sample1 - sample0) * fraction)
        }
        
        return output
    }
}
