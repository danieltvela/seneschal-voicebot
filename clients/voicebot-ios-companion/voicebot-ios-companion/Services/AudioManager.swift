//
//  AudioManager.swift
//  voicebot-ios-companion
//
//  Created by Dani Vela on 13/06/2026.
//

import Foundation
import AVFoundation
import Combine
import os.log

final class AudioManager: ObservableObject {
    @Published private(set) var isCapturing = false
    @Published private(set) var isPlaying = false
    @Published private(set) var microphonePermissionGranted = false

    private static let logger = Logger(
        subsystem: Bundle.main.bundleIdentifier ?? "voicebot-ios-companion",
        category: "AudioManager"
    )

    /// Wire / player format: 16 kHz mono float (matches host TTS frames).
    private static let wireFormat: AVAudioFormat = {
        AVAudioFormat(
            commonFormat: .pcmFormatFloat32,
            sampleRate: AudioFormat.sampleRate,
            channels: AudioFormat.channels,
            interleaved: false
        )!
    }()

    /// Accumulate ~100 ms of TTS before scheduling (host sends 20 ms frames).
    private static let playbackBatchFrames = 1600
    /// Flush leftover if no more chunks arrive for this long.
    private static let playbackFlushNs: UInt64 = 80_000_000

    private var audioEngine: AVAudioEngine?
    private var playerNode: AVAudioPlayerNode?
    private var audioSession: AVAudioSession

    private var captureContinuation: AsyncStream<[Float]>.Continuation?
    private var captureStream: AsyncStream<[Float]>?

    /// Converts hardware mic buffers → 16 kHz mono Float32 for the wire path.
    private var inputConverter: AVAudioConverter?
    private var targetFormat: AVAudioFormat?

    /// Pending TTS samples waiting to be scheduled as a larger buffer.
    private var playbackPending: [Float] = []
    private let playbackLock = NSLock()
    private var playbackFlushTask: Task<Void, Never>?
    private var scheduledBufferCount = 0

    init() {
        self.audioSession = AVAudioSession.sharedInstance()
    }

    /// Fresh single-consumer stream of mono float samples @ `AudioFormat.sampleRate`.
    /// Recreated after each `stopCapture()` so reconnect / scene resume works.
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
        // Idempotent: already running.
        if audioEngine != nil, isCapturing { return }

        // Tear down any half-started engine before reconfiguring the session.
        tearDownEngine()

        try configureSession()

        // Ensure the AsyncStream continuation exists before the tap can fire.
        _ = capturedAudio

        let engine = AVAudioEngine()
        let inputNode = engine.inputNode

        // Query hardware format only after the session is active. A zero rate
        // usually means route/session is not ready (e.g. Bluetooth still settling).
        let inputFormat = inputNode.outputFormat(forBus: 0)
        guard inputFormat.sampleRate > 0, inputFormat.channelCount > 0 else {
            Self.logger.error(
                "Invalid input format after session activate: rate=\(inputFormat.sampleRate) ch=\(inputFormat.channelCount)"
            )
            throw AudioError.invalidInputFormat
        }

        Self.logger.info(
            "Input format: \(inputFormat.sampleRate) Hz, \(inputFormat.channelCount) ch, \(String(describing: inputFormat.commonFormat.rawValue))"
        )
        Self.logger.info(
            "Audio route: \(self.routeDescription)"
        )

        guard let target = AVAudioFormat(
            commonFormat: .pcmFormatFloat32,
            sampleRate: AudioFormat.sampleRate,
            channels: AudioFormat.channels,
            interleaved: false
        ) else {
            throw AudioError.sessionConfigurationFailed
        }
        targetFormat = target

        // Prefer AVAudioConverter (handles Int16/Float32, stereo→mono, rate).
        inputConverter = AVAudioConverter(from: inputFormat, to: target)
        if inputConverter == nil {
            Self.logger.warning("AVAudioConverter unavailable — using linear resampler fallback")
        }

        let playerNode = AVAudioPlayerNode()
        engine.attach(playerNode)
        // Player always feeds 16 kHz mono; the engine resamples to the hardware output.
        engine.connect(playerNode, to: engine.mainMixerNode, format: Self.wireFormat)
        engine.mainMixerNode.outputVolume = 1.0
        playerNode.volume = 1.0

        // Always install the tap in the hardware format (Apple requirement).
        let bufferSize = AVAudioFrameCount(max(1024, inputFormat.sampleRate * 0.02))
        inputNode.installTap(onBus: 0, bufferSize: bufferSize, format: inputFormat) {
            [weak self] buffer, _ in
            guard let self else { return }
            let samples = self.convertToWireSamples(buffer: buffer, inputFormat: inputFormat)
            guard !samples.isEmpty else { return }
            self.captureContinuation?.yield(samples)
        }

        audioEngine = engine
        self.playerNode = playerNode

        engine.prepare()
        try engine.start()
        // Start the player node idle so the first scheduleBuffer is not dropped.
        playerNode.play()

        isCapturing = true
        Self.logger.info("Capture started (player armed), route=\(self.routeDescription)")
    }

    func stopCapture() {
        tearDownEngine()

        // Finish the stream so a later start gets a fresh single-consumer AsyncStream.
        captureContinuation?.finish()
        captureContinuation = nil
        captureStream = nil

        inputConverter = nil
        targetFormat = nil
        isCapturing = false
        Self.logger.info("Capture stopped")
    }

    /// Enqueue host TTS samples (16 kHz mono float). Batches small WS frames for smoother playback.
    func play(_ samples: [Float]) async {
        guard !samples.isEmpty else { return }

        if playerNode == nil || audioEngine?.isRunning != true {
            Self.logger.warning(
                "play() skipped — engine/player not ready (engineRunning=\(self.audioEngine?.isRunning ?? false), player=\(self.playerNode != nil))"
            )
            return
        }

        playbackLock.lock()
        playbackPending.append(contentsOf: samples)
        let pendingCount = playbackPending.count
        let shouldFlush = pendingCount >= Self.playbackBatchFrames
        playbackLock.unlock()

        if shouldFlush {
            flushPlaybackBuffer()
        } else {
            schedulePlaybackFlush()
        }
    }

    /// Force remaining TTS samples out (call on `audio.end`).
    func flushPlayback() {
        playbackFlushTask?.cancel()
        playbackFlushTask = nil
        flushPlaybackBuffer()
    }

    func stopPlayback() {
        playbackFlushTask?.cancel()
        playbackFlushTask = nil
        playbackLock.lock()
        playbackPending.removeAll(keepingCapacity: true)
        playbackLock.unlock()
        playerNode?.stop()
        // Keep player armed for the next utterance while capture is active.
        if isCapturing, let player = playerNode, audioEngine?.isRunning == true {
            player.play()
        }
        scheduledBufferCount = 0
        isPlaying = false
    }

    // MARK: - Session

    /// Configure AVAudioSession for full-duplex voice without kicking Bluetooth headsets.
    ///
    /// Important options:
    /// - `.allowBluetooth` → HFP/SCO (two-way: AirPods mic + headphones). Without this,
    ///   only A2DP output is allowed and activating `playAndRecord` often drops AirPods.
    /// - `.allowBluetoothA2DP` → high-quality stereo output when the route allows it.
    /// - `.defaultToSpeaker` only when there is no external headset, so iPhone doesn't
    ///   force the earpiece — and so we don't override an active Bluetooth route.
    ///
    /// Mode is `.videoChat` (not `.voiceChat`): both enable duplex routing for BT headsets,
    /// but `.voiceChat` enables more aggressive voice-processing that can duck or silence
    /// `AVAudioPlayerNode` TTS on the same engine.
    private func configureSession() throws {
        var options: AVAudioSession.CategoryOptions = [
            .allowBluetooth,
            .allowBluetoothA2DP,
        ]
        if !hasExternalAudioRoute {
            options.insert(.defaultToSpeaker)
        }

        do {
            try audioSession.setCategory(.playAndRecord, mode: .videoChat, options: options)
            // Prefer a common hardware rate; engine/converter handle the wire 16 kHz.
            try? audioSession.setPreferredSampleRate(48_000)
            try? audioSession.setPreferredIOBufferDuration(0.02)
            try audioSession.setActive(true, options: [])
        } catch {
            Self.logger.error("Audio session configuration failed: \(error.localizedDescription)")
            throw AudioError.sessionConfigurationFailed
        }

        Self.logger.info(
            "Session active — category=playAndRecord mode=videoChat externalRoute=\(self.hasExternalAudioRoute) route=\(self.routeDescription)"
        )
    }

    /// True when headphones / Bluetooth / CarPlay / AirPlay is in the current route.
    private var hasExternalAudioRoute: Bool {
        let outputs = audioSession.currentRoute.outputs
        let external: Set<AVAudioSession.Port> = [
            .headphones,
            .bluetoothA2DP,
            .bluetoothHFP,
            .bluetoothLE,
            .airPlay,
            .carAudio,
            .usbAudio,
        ]
        return outputs.contains { external.contains($0.portType) }
    }

    private var routeDescription: String {
        let outs = audioSession.currentRoute.outputs
            .map { "\($0.portName)(\($0.portType.rawValue))" }
            .joined(separator: ",")
        let ins = audioSession.currentRoute.inputs
            .map { "\($0.portName)(\($0.portType.rawValue))" }
            .joined(separator: ",")
        return "in=[\(ins)] out=[\(outs)]"
    }

    // MARK: - Playback helpers

    private func schedulePlaybackFlush() {
        playbackFlushTask?.cancel()
        playbackFlushTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: Self.playbackFlushNs)
            guard let self, !Task.isCancelled else { return }
            self.flushPlaybackBuffer()
        }
    }

    private func flushPlaybackBuffer() {
        playbackLock.lock()
        guard !playbackPending.isEmpty else {
            playbackLock.unlock()
            return
        }
        let samples = playbackPending
        playbackPending.removeAll(keepingCapacity: true)
        playbackLock.unlock()

        scheduleOnPlayer(samples)
    }

    private func scheduleOnPlayer(_ samples: [Float]) {
        guard !samples.isEmpty else { return }
        guard let player = playerNode, let engine = audioEngine, engine.isRunning else {
            Self.logger.warning("scheduleOnPlayer: player/engine not ready, dropping \(samples.count) samples")
            return
        }

        let format = Self.wireFormat
        guard let audioBuffer = AVAudioPCMBuffer(
            pcmFormat: format,
            frameCapacity: AVAudioFrameCount(samples.count)
        ) else {
            Self.logger.error("Failed to allocate PCM buffer (\(samples.count) frames)")
            return
        }

        audioBuffer.frameLength = AVAudioFrameCount(samples.count)
        guard let channelData = audioBuffer.floatChannelData else { return }
        samples.withUnsafeBufferPointer { src in
            channelData[0].update(from: src.baseAddress!, count: samples.count)
        }

        scheduledBufferCount += 1
        let bufferId = scheduledBufferCount
        isPlaying = true

        // Non-blocking schedule — do NOT use the iOS 17 async API that waits until
        // the buffer finishes playing (that serializes poorly under many 20 ms frames).
        player.scheduleBuffer(audioBuffer, completionHandler: { [weak self] in
            DispatchQueue.main.async {
                guard let self else { return }
                self.scheduledBufferCount = max(0, self.scheduledBufferCount - 1)
                if self.scheduledBufferCount == 0 {
                    self.isPlaying = false
                }
            }
        })

        if !player.isPlaying {
            player.play()
        }

        if bufferId == 1 || bufferId % 25 == 0 {
            Self.logger.debug(
                "Scheduled TTS buffer #\(bufferId) frames=\(samples.count) route=\(self.routeDescription)"
            )
        }
    }

    // MARK: - Engine helpers

    private func tearDownEngine() {
        playbackFlushTask?.cancel()
        playbackFlushTask = nil
        playbackLock.lock()
        playbackPending.removeAll(keepingCapacity: false)
        playbackLock.unlock()
        scheduledBufferCount = 0

        if let engine = audioEngine {
            engine.inputNode.removeTap(onBus: 0)
            if engine.isRunning {
                engine.stop()
            }
            engine.reset()
        }
        playerNode?.stop()
        audioEngine = nil
        playerNode = nil
        isPlaying = false
    }

    /// Hardware buffer → mono Float32 @ `AudioFormat.sampleRate`.
    private func convertToWireSamples(
        buffer: AVAudioPCMBuffer,
        inputFormat: AVAudioFormat
    ) -> [Float] {
        guard buffer.frameLength > 0 else { return [] }

        if let converter = inputConverter, let target = targetFormat {
            let ratio = target.sampleRate / inputFormat.sampleRate
            let capacity = AVAudioFrameCount(Double(buffer.frameLength) * ratio) + 32
            guard let converted = AVAudioPCMBuffer(pcmFormat: target, frameCapacity: capacity)
            else { return [] }

            var error: NSError?
            var consumed = false
            let status = converter.convert(to: converted, error: &error) { _, outStatus in
                if consumed {
                    outStatus.pointee = .noDataNow
                    return nil
                }
                consumed = true
                outStatus.pointee = .haveData
                return buffer
            }

            if let error {
                Self.logger.error("Converter error: \(error.localizedDescription)")
                return []
            }
            if status == .error { return [] }

            return extractFloatSamples(from: converted)
        }

        let raw = extractFloatSamples(from: buffer)
        guard !raw.isEmpty else { return [] }
        return AudioResampler.resample(
            samples: raw,
            fromRate: inputFormat.sampleRate,
            toRate: AudioFormat.sampleRate
        )
    }

    private func extractFloatSamples(from buffer: AVAudioPCMBuffer) -> [Float] {
        let frameCount = Int(buffer.frameLength)
        guard frameCount > 0 else { return [] }

        if let channelData = buffer.floatChannelData {
            return Array(UnsafeBufferPointer(start: channelData[0], count: frameCount))
        }

        if let channelData = buffer.int16ChannelData {
            let ptr = channelData[0]
            var samples = [Float](repeating: 0, count: frameCount)
            for i in 0..<frameCount {
                samples[i] = Float(ptr[i]) / Float(Int16.max)
            }
            return samples
        }

        return []
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
    case invalidInputFormat
}
