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
            let preferences = VoicePreferencesSnapshot.load()
            if let voice {
                let selection = VoicePreferencesSnapshot.normalizeVoiceSelection(voice)
                if selection.hasPrefix("sherpa:") {
                    guard let parsed = VoiceRuntimeResolver.parseSherpaVoice(selection),
                          let model = GeneratedVoiceModelCatalog.byID(parsed.modelID),
                          model.voices.contains(where: { $0.id == parsed.voiceID }),
                          VoiceModelFiles.modelRoot(for: model) != nil
                    else { throw SpeechFfiError.Unavailable }
                }
            }
            let route = VoiceRuntimeResolver.speechRoute(
                preferences: preferences,
                voiceOverride: voice
            )
            if let voice {
                let selection = VoicePreferencesSnapshot.normalizeVoiceSelection(voice)
                if selection.hasPrefix("sherpa:") {
                    guard case .sherpa = route else { throw SpeechFfiError.Unavailable }
                }
            }
            if case let .sherpa(_, modelID, _, speakerID, modelDirectory) = route {
                let rendered = try await SherpaSpeechRenderer.render(
                    text: text,
                    modelID: modelID,
                    modelDirectory: modelDirectory,
                    speakerID: speakerID,
                    speed: preferences.rate
                )
                return TtsAudioFfi(pcm: rendered.pcm, sampleRateHz: rendered.sampleRate)
            }

            let utterance = AVSpeechUtterance(string: text)
            guard case let .system(configuredLanguage, configuredVoice) = route else {
                throw SpeechFfiError.Unavailable
            }
            let explicitSystemVoice = voice.map {
                VoicePreferencesSnapshot.normalizeVoiceSelection($0).hasPrefix("system:")
            } == true
            if let identifier = configuredVoice,
               identifier != "default",
               let v = AVSpeechSynthesisVoice(identifier: identifier),
               VoiceCapabilityModel.normalizedLocaleIdentifier(v.language)
                .split(separator: "-").first.map(String.init)?.lowercased()
                == VoiceCapabilityModel.normalizedLocaleIdentifier(configuredLanguage)
                .split(separator: "-").first.map(String.init)?.lowercased() {
                utterance.voice = v
            } else if explicitSystemVoice {
                throw SpeechFfiError.Unavailable
            } else if let v = AVSpeechSynthesisVoice(language: configuredLanguage) {
                utterance.voice = v
            }
            utterance.rate = VoiceCapabilityModel.utteranceRate(from: preferences.rate)

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
