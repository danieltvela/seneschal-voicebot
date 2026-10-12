//
//  voicebot_ios_companionTests.swift
//  voicebot-ios-companionTests
//
//  Created by Dani Vela on 13/06/2026.
//

import Testing
import Foundation
@testable import voicebot_ios_companion

struct RemoteMessageTests {

    @Test func sessionStartEncodesCorrectJSON() throws {
        let message = RemoteMessage.sessionStart(sampleRate: 16000)
        let data = try message.jsonData()
        let json = String(data: data, encoding: .utf8)!
        
        #expect(json.contains("\"type\":\"session.start\""))
        #expect(json.contains("\"sample_rate\":16000"))
    }
    
    @Test func sessionStartDecodesCorrectly() throws {
        let original = RemoteMessage.sessionStart(sampleRate: 16000)
        let data = try original.jsonData()
        let decoded = try RemoteMessage.parse(data)
        
        if case .sessionStart(let rate) = decoded {
            #expect(rate == 16000)
        } else {
            #expect(false, "Expected .sessionStart case")
        }
    }
    
    @Test func bargeInEncodesCorrectJSON() throws {
        let message = RemoteMessage.bargeIn
        let data = try message.jsonData()
        let json = String(data: data, encoding: .utf8)!
        
        #expect(json.contains("\"type\":\"barge_in\""))
    }
    
    @Test func transcriptRoundTrip() throws {
        let message = RemoteMessage.transcript(text: "hola")
        let data = try message.jsonData()
        let decoded = try RemoteMessage.parse(data)
        
        if case .transcript(let text) = decoded {
            #expect(text == "hola")
        } else {
            #expect(false, "Expected .transcript case")
        }
    }
    
    @Test func responseTextRoundTrip() throws {
        let message = RemoteMessage.responseText(text: "Hello")
        let data = try message.jsonData()
        let decoded = try RemoteMessage.parse(data)
        
        if case .responseText(let text) = decoded {
            #expect(text == "Hello")
        } else {
            #expect(false, "Expected .responseText case")
        }
    }
    
    @Test func errorRoundTrip() throws {
        let message = RemoteMessage.error(message: "already connected")
        let data = try message.jsonData()
        let decoded = try RemoteMessage.parse(data)
        
        if case .error(let msg) = decoded {
            #expect(msg == "already connected")
        } else {
            #expect(false, "Expected .error case")
        }
    }
    
    @Test func sessionReadyRoundTrip() throws {
        let message = RemoteMessage.sessionReady
        let data = try message.jsonData()
        let decoded = try RemoteMessage.parse(data)
        #expect(decoded == message)
    }
    
    @Test func audioStartRoundTrip() throws {
        let message = RemoteMessage.audioStart
        let data = try message.jsonData()
        let decoded = try RemoteMessage.parse(data)
        #expect(decoded == message)
    }
    
    @Test func audioEndRoundTrip() throws {
        let message = RemoteMessage.audioEnd
        let data = try message.jsonData()
        let decoded = try RemoteMessage.parse(data)
        #expect(decoded == message)
    }
    
    @Test func responseEndRoundTrip() throws {
        let message = RemoteMessage.responseEnd
        let data = try message.jsonData()
        let decoded = try RemoteMessage.parse(data)
        #expect(decoded == message)
    }
    
    @Test func unknownMessageTypeThrows() throws {
        let data = #"{"type":"unknown"}"#.data(using: .utf8)!
        #expect(throws: DecodingError.self) {
            try RemoteMessage.parse(data)
        }
    }
    
    @Test func malformedJSONThrows() throws {
        let data = "{invalid json".data(using: .utf8)!
        #expect(throws: DecodingError.self) {
            try RemoteMessage.parse(data)
        }
    }
}

struct AudioFormatTests {

    @Test func floatToInt16ClampsAndConverts() throws {
        let samples: [Float] = [0.0, 1.0, -1.0, 0.5, -0.5]
        let data = floatToInt16(samples)
        let bytes = [UInt8](data)
        
        // 0.0 -> 0 -> [0, 0]
        // 1.0 -> 32767 -> [255, 127]
        // -1.0 -> -32767 -> [1, 128] (rounded from -32767.0)
        // 0.5 -> 16384 -> [0, 64]
        // -0.5 -> -16384 -> [0, 192]
        #expect(bytes.count == 10)
        #expect(bytes[0] == 0 && bytes[1] == 0)
        #expect(bytes[2] == 255 && bytes[3] == 127)
        #expect(bytes[4] == 1 && bytes[5] == 128)
    }
    
    @Test func floatToInt16ClampsOutOfRangeValues() throws {
        let samples: [Float] = [2.0, -2.0, 1.5, -1.5]
        let data = floatToInt16(samples)
        let bytes = [UInt8](data)
        
        // All clamped to ±1.0 -> ±32767
        #expect(bytes.count == 8)
        // 2.0 clamped to 1.0 -> 32767 -> [255, 127]
        #expect(bytes[0] == 255 && bytes[1] == 127)
        // -2.0 clamped to -1.0 -> -32767 -> [1, 128]
        #expect(bytes[2] == 1 && bytes[3] == 128)
    }
    
    @Test func int16ToFloatIsInverse() throws {
        let original: [Float] = [0.0, 1.0, -1.0, 0.5]
        let data = floatToInt16(original)
        let recovered = int16ToFloat(data)
        
        #expect(recovered.count == original.count)
        for (i, (a, b)) in zip(original, recovered).enumerated() {
            #expect(abs(a - b) < 0.001, "Mismatch at index \(i): \(a) vs \(b)")
        }
    }
    
    @Test func int16ToFloatEmptyDataReturnsEmpty() throws {
        let data = Data()
        let result = int16ToFloat(data)
        #expect(result.isEmpty)
    }
    
    @Test func int16ToFloatOddLengthReturnsEmpty() throws {
        let data = Data([0x01])
        let result = int16ToFloat(data)
        #expect(result.isEmpty)
    }
}

struct AudioResamplerTests {

    @Test func resampleDownsamplingProducesExpectedCount() throws {
        let samples = Array(repeating: Float(1.0), count: 480)
        let result = AudioResampler.resample(samples: samples, fromRate: 48000, toRate: 16000)
        
        // 48000 -> 16000 = factor 3; 480 input samples -> ~160 output samples
        #expect(result.count == 160)
    }
    
    @Test func resampleSameRateReturnsOriginal() throws {
        let samples: [Float] = [0.0, 0.5, 1.0, 0.5, 0.0]
        let result = AudioResampler.resample(samples: samples, fromRate: 16000, toRate: 16000)
        
        #expect(result == samples)
    }
    
    @Test func resampleLinearInterpolationBetweenTwoSamples() throws {
        let samples: [Float] = [0.0, 1.0]
        let result = AudioResampler.resample(samples: samples, fromRate: 48000, toRate: 16000)
        
        // 48000 -> 16000, ratio 3.0; output count = floor(2 / 3) = 0
        #expect(result.isEmpty)
    }
    
    @Test func resampleEmptyInputReturnsEmpty() throws {
        let result = AudioResampler.resample(samples: [], fromRate: 48000, toRate: 16000)
        #expect(result.isEmpty)
    }
    
    @Test func resamplePreservesDCComponent() throws {
        let samples = Array(repeating: Float(0.75), count: 3000)
        let result = AudioResampler.resample(samples: samples, fromRate: 48000, toRate: 16000)
        
        #expect(result.count == 1000)
        for sample in result {
            #expect(abs(sample - 0.75) < 0.001)
        }
    }
}
