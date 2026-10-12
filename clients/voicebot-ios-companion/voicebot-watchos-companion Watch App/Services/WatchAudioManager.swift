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
    
    // Reusable playback engine (fixes choppy audio from creating new engine per chunk)
    private var playbackEngine: AVAudioEngine?
    private var playbackPlayer: AVAudioPlayerNode?
    
    var capturedAudio: AsyncStream<Data> {
        if captureStream == nil {
            captureStream = AsyncStream { continuation in
                self.captureContinuation = continuation
            }
        }
        return captureStream!
    }
    
    // MARK: - Capture
    
    func startCapture() throws {
        guard audioEngine == nil else { return }
        
        let engine = AVAudioEngine()
        let inputNode = engine.inputNode
        let inputFormat = inputNode.outputFormat(forBus: 0)
        
        // Create converter to 16kHz mono int16
        guard let outputFormat = AVAudioFormat(
            commonFormat: .pcmFormatInt16,
            sampleRate: 16000,
            channels: 1,
            interleaved: true
        ) else {
            throw NSError(domain: "WatchAudioManager", code: -1,
                          userInfo: [NSLocalizedDescriptionKey: "Failed to create output format"])
        }
        
        guard let converter = AVAudioConverter(from: inputFormat, to: outputFormat) else {
            throw NSError(domain: "WatchAudioManager", code: -2,
                          userInfo: [NSLocalizedDescriptionKey: "Failed to create converter"])
        }
        
        inputNode.installTap(onBus: 0, bufferSize: 4096, format: inputFormat) { [weak self] buffer, _ in
            guard let self = self else { return }
            
            let frameCapacity = AVAudioFrameCount(outputFormat.sampleRate / Double(inputFormat.sampleRate) * Double(buffer.frameLength))
            guard let convertedBuffer = AVAudioPCMBuffer(pcmFormat: outputFormat, frameCapacity: frameCapacity) else {
                return
            }
            
            var error: NSError?
            let inputBlock: AVAudioConverterInputBlock = { _, outStatus in
                outStatus.pointee = .haveData
                return buffer
            }
            
            converter.convert(to: convertedBuffer, error: &error, withInputFrom: inputBlock)
            
            if let error = error {
                NSLog("WatchAudioManager: conversion error: \(error)")
                return
            }
            
            let data = self.bufferToData(convertedBuffer)
            self.captureContinuation?.yield(data)
        }
        
        engine.prepare()
        try engine.start()
        self.audioEngine = engine
    }
    
    func stopCapture() {
        audioEngine?.inputNode.removeTap(onBus: 0)
        audioEngine?.stop()
        audioEngine = nil
        captureContinuation?.finish()
        captureContinuation = nil
        captureStream = nil
    }
    
    // MARK: - Playback
    
    func playAudio(_ data: Data) {
        // Reuse playback engine to avoid gaps between chunks
        if playbackEngine == nil {
            setupPlaybackEngine()
        }
        
        guard let engine = playbackEngine,
              let player = playbackPlayer,
              engine.isRunning else {
            // Engine not running, try to set up again
            setupPlaybackEngine()
            return
        }
        
        guard let format = AVAudioFormat(
            commonFormat: .pcmFormatInt16,
            sampleRate: 16000,
            channels: 1,
            interleaved: true
        ) else { return }
        
        let frameCount = AVAudioFrameCount(data.count / Int(format.streamDescription.pointee.mBytesPerFrame))
        guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frameCount) else { return }
        buffer.frameLength = frameCount
        
        // Copy data into buffer
        data.withUnsafeBytes { rawPtr in
            guard let ptr = rawPtr.baseAddress?.assumingMemoryBound(to: Int16.self) else { return }
            if let channelData = buffer.int16ChannelData {
                channelData.pointee.update(from: ptr, count: Int(frameCount))
            }
        }
        
        player.scheduleBuffer(buffer, at: nil, options: .interrupts, completionHandler: nil)
        
        if !player.isPlaying {
            player.play()
        }
    }
    
    func stopPlayback() {
        playbackPlayer?.stop()
        playbackEngine?.stop()
        playbackEngine = nil
        playbackPlayer = nil
    }
    
    // MARK: - Private
    
    private func setupPlaybackEngine() {
        let engine = AVAudioEngine()
        let player = AVAudioPlayerNode()
        
        engine.attach(player)
        engine.connect(player, to: engine.mainMixerNode, format: nil)
        engine.prepare()
        
        do {
            try engine.start()
            player.play()
            self.playbackEngine = engine
            self.playbackPlayer = player
        } catch {
            NSLog("WatchAudioManager: playback engine start failed: \(error)")
        }
    }
    
    private func bufferToData(_ buffer: AVAudioPCMBuffer) -> Data {
        guard let channelData = buffer.int16ChannelData else { return Data() }
        let frameCount = Int(buffer.frameLength)
        let byteCount = frameCount * MemoryLayout<Int16>.size
        return Data(bytes: channelData.pointee, count: byteCount)
    }
}
