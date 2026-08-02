// TtsImpl.swift — iOS native text-to-speech capability (parity with Android
// SystemTextToSpeechTts.kt).
//
// Conforms to the generated `IosTts` UniFFI callback interface. The engine
// (tool-speech) calls `synthesize(text:voice:)` and expects PCM16 mono audio +
// its sample rate back as a `TtsAudioFfi`. We render with `AVSpeechSynthesizer`'s
// buffer-callback API (`write(_:toBufferCallback:)`, iOS 13+), convert each
// produced `AVAudioPCMBuffer` to 16-bit signed little-endian mono, and return
// the concatenated frames. Errors map onto the generated `SpeechFfiError`.

import Foundation

#if canImport(AVFoundation)
    import AVFoundation

    /// Native TTS over `AVSpeechSynthesizer`, rendering to PCM16 mono.
    final class TtsImpl: IosTts, @unchecked Sendable {
        // Held strongly for the duration of a synthesis so the callback fires.
        private let synthesizer = AVSpeechSynthesizer()

        func synthesize(text: String, voice: String?) async throws -> TtsAudioFfi {
            let utterance = AVSpeechUtterance(string: text)
            let defaults = UserDefaults.standard
            let configuredVoice = defaults.string(forKey: "systemVoiceIdentifier")
            let configuredLanguage = VoiceCapabilityModel.resolvedRecognitionLocaleIdentifier(
                configuredLanguage: defaults.string(forKey: "voiceLanguage"),
                currentLocale: .autoupdatingCurrent
            )
            if let identifier = voice ?? configuredVoice,
               let v = AVSpeechSynthesisVoice(identifier: identifier) {
                utterance.voice = v
            } else if let v = AVSpeechSynthesisVoice(language: configuredLanguage) {
                utterance.voice = v
            }
            let speed = defaults.object(forKey: "voiceSpeed") == nil ? 1 : defaults.double(forKey: "voiceSpeed")
            utterance.rate = VoiceCapabilityModel.utteranceRate(from: speed)

            return try await withCheckedThrowingContinuation { (cont: CheckedContinuation<TtsAudioFfi, Error>) in
                let collector = PcmCollector()
                self.synthesizer.write(utterance) { buffer in
                    guard let pcm = buffer as? AVAudioPCMBuffer else {
                        // A zero-length buffer signals end-of-stream.
                        collector.finish(cont)
                        return
                    }
                    if pcm.frameLength == 0 {
                        collector.finish(cont)
                        return
                    }
                    collector.append(pcm)
                }
            }
        }
    }

    /// Accumulates rendered PCM buffers, converting to interleaved PCM16 mono LE,
    /// and resolves the continuation once on end-of-stream.
    private final class PcmCollector: @unchecked Sendable {
        private let lock = NSLock()
        private var pcm = Data()
        private var sampleRate: UInt32 = 16000
        private var done = false

        func append(_ buffer: AVAudioPCMBuffer) {
            lock.lock(); defer { lock.unlock() }
            sampleRate = UInt32(buffer.format.sampleRate)
            let frames = Int(buffer.frameLength)
            guard frames > 0 else { return }

            if let int16 = buffer.int16ChannelData {
                // Already PCM16 — take channel 0 as little-endian bytes.
                let ptr = int16[0]
                pcm.append(Data(bytes: ptr, count: frames * MemoryLayout<Int16>.size))
            } else if let float = buffer.floatChannelData {
                // Float32 → clamp + scale to Int16 LE.
                let ptr = float[0]
                var out = [UInt8]()
                out.reserveCapacity(frames * 2)
                for i in 0 ..< frames {
                    let clamped = max(-1.0, min(1.0, ptr[i]))
                    let s = Int16(clamped * 32767.0)
                    out.append(UInt8(truncatingIfNeeded: s))
                    out.append(UInt8(truncatingIfNeeded: s >> 8))
                }
                pcm.append(contentsOf: out)
            }
        }

        func finish(_ cont: CheckedContinuation<TtsAudioFfi, Error>) {
            lock.lock(); defer { lock.unlock() }
            if done { return }
            done = true
            if pcm.isEmpty {
                cont.resume(throwing: SpeechFfiError.Unavailable)
            } else {
                cont.resume(returning: TtsAudioFfi(pcm: pcm, sampleRateHz: sampleRate))
            }
        }
    }
#endif
