//
//  AudioManager.swift
//  voicebot-ios-companion
//
//  Created by Dani Vela on 13/06/2026.
//

import Foundation
import AVFoundation
import Combine

final class AudioManager: ObservableObject {
    @Published private(set) var isCapturing = false
    @Published private(set) var isPlaying = false
    @Published private(set) var microphonePermissionGranted = false
    
    private var audioEngine: AVAudioEngine?
    private var playerNode: AVAudioPlayerNode?
    private var audioSession: AVAudioSession
    
    private var captureContinuation: AsyncStream<[Float]>.Continuation?
    private var captureStream: AsyncStream<[Float]>?
    
    init() {
        self.audioSession = AVAudioSession.sharedInstance()
    }
    
    var capturedAudio: AsyncStream<[Float]> {
        if captureStream == nil {
            captureStream = AsyncStream { continuation in
                self.captureContinuation = continuation
            }
        }
        return captureStream!
    }
    
    func requestMicrophonePermission() async -> Bool {
        return await withCheckedContinuation { continuation in
            audioSession.requestRecordPermission { granted in
                DispatchQueue.main.async {
                    self.microphonePermissionGranted = granted
                    continuation.resume(returning: granted)
                }
            }
        }
    }
    
    func startCapture() async throws {
        guard microphonePermissionGranted else {
            throw AudioError.microphonePermissionDenied
        }
        
        try audioSession.setCategory(.playAndRecord, mode: .voiceChat, options: [.defaultToSpeaker, .allowBluetoothA2DP])
        try audioSession.setActive(true)
        
        let engine = AVAudioEngine()
        let inputNode = engine.inputNode
        let mixer = engine.mainMixerNode
        
        let playerNode = AVAudioPlayerNode()
        engine.attach(playerNode)
        let playbackFormat = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: AudioFormat.sampleRate, channels: AudioFormat.channels, interleaved: false)!
        engine.connect(playerNode, to: mixer, format: playbackFormat)
        
        let inputFormat = inputNode.outputFormat(forBus: 0)
        
        inputNode.installTap(onBus: 0, bufferSize: 1024, format: inputFormat) { [weak self] buffer, time in
            guard let self else { return }
            let samples = self.extractFloatSamples(from: buffer)
            let resampled = AudioResampler.resample(samples: samples, fromRate: inputFormat.sampleRate, toRate: AudioFormat.sampleRate)
            self.captureContinuation?.yield(resampled)
        }
        
        audioEngine = engine
        self.playerNode = playerNode
        
        engine.prepare()
        try engine.start()
        
        isCapturing = true
    }
    
    func stopCapture() {
        guard let engine = audioEngine else { return }
        
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        engine.reset()
        
        audioEngine = nil
        playerNode = nil
        isCapturing = false
    }
    
    func play(_ samples: [Float]) async {
        guard let player = playerNode else { return }
        
        guard !samples.isEmpty else { return }
        
        let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: AudioFormat.sampleRate, channels: AudioFormat.channels, interleaved: false)!
        guard let audioBuffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(samples.count)) else { return }
        
        audioBuffer.frameLength = audioBuffer.frameCapacity
        guard let channelData = audioBuffer.floatChannelData else { return }
        let floatBuffer = UnsafeMutableBufferPointer<Float>(start: channelData[0], count: samples.count)
        _ = floatBuffer.initialize(from: samples)
        
        isPlaying = true
        if #available(iOS 17.0, *) {
            Task {
                await player.scheduleBuffer(audioBuffer)
                await MainActor.run { [weak self] in
                    self?.isPlaying = false
                }
            }
        } else {
            player.scheduleBuffer(audioBuffer) { [weak self] in
                Task { @MainActor [weak self] in
                    self?.isPlaying = false
                }
            }
        }
        if !player.isPlaying {
            player.play(at: nil)
        }
    }
    
    func stopPlayback() {
        playerNode?.stop()
        isPlaying = false
    }
    
    private func extractFloatSamples(from buffer: AVAudioPCMBuffer) -> [Float] {
        guard let channelData = buffer.floatChannelData else { return [] }
        let frameCount = Int(buffer.frameLength)
        return Array(UnsafeBufferPointer(start: channelData[0], count: frameCount))
    }
    
    deinit {
        stopCapture()
        stopPlayback()
    }
}

enum AudioError: Error, Sendable {
    case microphonePermissionDenied
    case sessionConfigurationFailed
    case engineStartFailed
}
