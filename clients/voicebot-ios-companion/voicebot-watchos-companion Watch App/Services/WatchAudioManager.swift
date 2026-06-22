//
//  WatchAudioManager.swift
//  voicebot-watchos-companion Watch App
//

import Foundation
import AVFoundation

final class WatchAudioManager {
    
    private var audioEngine: AVAudioEngine?
    private var captureContinuation: AsyncStream<Data>.Continuation?
    private var captureStream: AsyncStream<Data>?
    
    var capturedAudio: AsyncStream<Data> {
        if captureStream == nil {
            captureStream = AsyncStream { continuation in
                self.captureContinuation = continuation
            }
        }
        return captureStream!
    }
    
    func startCapture() throws {
        guard audioEngine == nil else { return }
        
        let engine = AVAudioEngine()
        let inputNode = engine.inputNode
        let format = inputNode.outputFormat(forBus: 0)
        
        // Create converter to 16kHz mono
        let outputFormat = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: 16000, channels: AVAudioChannelLayout_mono, interleaved: true)!
        
        let converter = AVAudioConverter(from: format, to: outputFormat)
        
        inputNode.installTap(onBus: 0, bufferSize: 4096, format: format) { [weak self] buffer, time in
            guard let self = self else { return }
            
            if let convertedBuffer = converter?.convert(buffer) {
                // Convert to int16 PCM
                let data = self.convertToPCM(convertedBuffer)
                self.captureContinuation?.yield(data)
            }
        }
        
        engine.prepare()
        try engine.run()
        self.audioEngine = engine
    }
    
    func stopCapture() {
        audioEngine?.stop()
        audioEngine?.inputNode.removeTap(onBus: 0)
        audioEngine?.reset()
        audioEngine = nil
        captureContinuation?.finish()
        captureContinuation = nil
        captureStream = nil
    }
    
    func playAudio(_ data: Data) {
        // Play audio data on the watch speaker
        let player = AVAudioPlayerNode()
        let engine = AVAudioEngine()
        let format = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: 16000, channels: AVAudioChannelLayout_mono, interleaved: true)!
        
        player.scheduleBuffer(AVAudioPCMBuffer(pcmFormat: format, byteLength: data.count)! , at: nil)
        
        engine.attach(player)
        engine.connect(player, to: engine.mainMixerNode, format: format)
        try? engine.run()
        player.play()
    }
    
    private func convertToPCM(_ buffer: AVAudioPCMBuffer) -> Data {
        guard let audioBuffer = buffer.mutableAudioBufferBytes else { return Data() }
        let byteLength = buffer.audioBufferByteLength
        return Data(bytes: audioBuffer, count: byteLength)
    }
}
