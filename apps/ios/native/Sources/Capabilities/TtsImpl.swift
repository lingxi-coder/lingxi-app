// TtsImpl.swift — iOS native text-to-speech capability (parity with Android
// SystemTextToSpeechTts.kt).
//
// Silent renderer used by the app-scoped AudioService. It returns PCM16 mono
// audio and the actual sample rate using `AVSpeechSynthesizer`'s
// buffer-callback API (`write(_:toBufferCallback:)`, iOS 13+), convert each
// produced `AVAudioPCMBuffer` to 16-bit signed little-endian mono, and return
// the concatenated frames. Errors remain in the local audio error domain.

import Foundation

#if canImport(AVFoundation)
    import AVFoundation

    /// Native TTS over `AVSpeechSynthesizer`, rendering to PCM16 mono.
    final class TtsImpl: @unchecked Sendable {
        // Held strongly for the duration of a synthesis so the callback fires.
        private let synthesizer = AVSpeechSynthesizer()

        func render(
            text: String,
            configuration: AudioConfigurationV4,
            route: AudioRouteResolution,
            maxPayloadBytes: UInt64
        ) async throws -> AudioPcmOutput {
            let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !text.isEmpty else { throw AudioServiceFailure.invalidRequest }
            guard route.status == .ready, let effective = route.effective else {
                throw AudioServiceFailure.unavailable
            }
            guard maxPayloadBytes > 0, maxPayloadBytes <= UInt64(Int.max) else {
                throw AudioServiceFailure.invalidRequest
            }
            let maximumBytes = Int(maxPayloadBytes)
            let language = resolveAudioLanguageForNativeDevice(
                configured: configuration.language,
                deviceLocale: Locale.autoupdatingCurrent.identifier
            )

            switch effective.source {
            case .offline:
                guard let modelID = effective.modelId,
                      let model = GeneratedVoiceModelCatalog.byID(modelID),
                      model.kind == .tts,
                      let directory = VoiceModelFiles.modelRoot(for: model)
                else { throw AudioServiceFailure.modelMissing }
                let voiceID = effective.voiceId ?? model.voices.first?.id
                guard let voiceID,
                      let voiceIndex = model.voices.firstIndex(where: { $0.id == voiceID })
                else { throw AudioServiceFailure.voiceMissing }
                let rendered: (pcm: Data, sampleRate: UInt32)
                do {
                    rendered = try await SherpaSpeechRenderer.render(
                        text: text,
                        modelID: modelID,
                        modelDirectory: directory,
                        speakerID: Int32(voiceIndex),
                        speed: configuration.rate,
                        maximumBytes: maxPayloadBytes
                    )
                } catch {
                    if error is CancellationError || error is AudioServiceFailure { throw error }
                    throw AudioServiceFailure.synthesisFailed(error.localizedDescription, operationStarted: false)
                }
                let output = AudioPcmOutput(pcm: rendered.pcm, sampleRateHz: rendered.sampleRate)
                try validate(output, maximumBytes: maximumBytes)
                return output
            case .system:
                return try await renderSystem(
                    text: text,
                    language: language,
                    voiceID: effective.voiceId,
                    rate: configuration.rate,
                    maximumBytes: maximumBytes
                )
            default:
                throw AudioServiceFailure.unavailable
            }
        }

        private func renderSystem(
            text: String,
            language: String,
            voiceID: String?,
            rate: Double,
            maximumBytes: Int
        ) async throws -> AudioPcmOutput {
            let utterance = AVSpeechUtterance(string: text)
            if let voiceID, voiceID != "default" {
                guard let voice = AVSpeechSynthesisVoice(identifier: voiceID),
                      Self.languageBase(voice.language) == Self.languageBase(language)
                else { throw AudioServiceFailure.voiceMissing }
                utterance.voice = voice
            } else {
                utterance.voice = AVSpeechSynthesisVoice(language: language)
            }
            utterance.rate = VoiceCapabilityModel.utteranceRate(from: rate)

            let collector = PcmCollector(maximumBytes: maximumBytes)
            return try await withTaskCancellationHandler {
                try await withCheckedThrowingContinuation { (cont: CheckedContinuation<AudioPcmOutput, Error>) in
                    collector.install(cont)
                    self.synthesizer.write(utterance) { buffer in
                        guard let pcm = buffer as? AVAudioPCMBuffer else {
                            // A zero-length buffer signals end-of-stream.
                            collector.finish()
                            return
                        }
                        if pcm.frameLength == 0 {
                            collector.finish()
                            return
                        }
                        if !collector.append(pcm) {
                            Task { @MainActor in
                                _ = self.synthesizer.stopSpeaking(at: .immediate)
                            }
                        }
                    }
                }
            } onCancel: {
                collector.cancel()
                Task { @MainActor in
                    _ = self.synthesizer.stopSpeaking(at: .immediate)
                }
            }
        }

        private func validate(_ output: AudioPcmOutput, maximumBytes: Int) throws {
            guard !output.pcm.isEmpty, output.pcm.count.isMultiple(of: MemoryLayout<Int16>.size),
                  output.sampleRateHz > 0
            else { throw AudioServiceFailure.synthesisFailed("provider returned invalid PCM", operationStarted: false) }
            guard output.pcm.count <= maximumBytes else { throw AudioServiceFailure.mediaTooLarge }
        }

        private static func languageBase(_ identifier: String) -> String {
            identifier.replacingOccurrences(of: "_", with: "-")
                .split(separator: "-")
                .first
                .map(String.init)?
                .lowercased() ?? ""
        }
    }

    /// Accumulates rendered PCM buffers, converting to interleaved PCM16 mono LE,
    /// and resolves the continuation once on end-of-stream.
    private final class PcmCollector: @unchecked Sendable {
        private let lock = NSLock()
        private let maximumBytes: Int
        private var pcm = Data()
        private var sampleRate: UInt32 = 16000
        private var continuation: CheckedContinuation<AudioPcmOutput, Error>?
        private var result: Result<AudioPcmOutput, Error>?

        init(maximumBytes: Int) {
            self.maximumBytes = maximumBytes
        }

        func install(_ continuation: CheckedContinuation<AudioPcmOutput, Error>) {
            lock.lock()
            if let result {
                lock.unlock()
                continuation.resume(with: result)
                return
            }
            self.continuation = continuation
            lock.unlock()
        }

        func append(_ buffer: AVAudioPCMBuffer) -> Bool {
            lock.lock()
            guard result == nil else {
                lock.unlock()
                return false
            }
            sampleRate = UInt32(buffer.format.sampleRate)
            let frames = Int(buffer.frameLength)
            guard frames > 0 else {
                lock.unlock()
                return true
            }
            guard frames <= (maximumBytes - pcm.count) / MemoryLayout<Int16>.size else {
                lock.unlock()
                complete { _, _ in .failure(AudioServiceFailure.mediaTooLarge) }
                return false
            }

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
            lock.unlock()
            return true
        }

        func finish() {
            complete { pcm, sampleRate in
                guard !pcm.isEmpty, sampleRate > 0, pcm.count.isMultiple(of: MemoryLayout<Int16>.size) else {
                    return .failure(AudioServiceFailure.synthesisFailed("system provider returned no PCM", operationStarted: false))
                }
                return .success(AudioPcmOutput(pcm: pcm, sampleRateHz: sampleRate))
            }
        }

        func cancel() {
            complete { _, _ in .failure(CancellationError()) }
        }

        private func complete(
            _ makeResult: (Data, UInt32) -> Result<AudioPcmOutput, Error>
        ) {
            lock.lock()
            guard result == nil else {
                lock.unlock()
                return
            }
            let terminalResult = makeResult(pcm, sampleRate)
            result = terminalResult
            let continuation = self.continuation
            self.continuation = nil
            lock.unlock()
            continuation?.resume(with: terminalResult)
        }
    }
#endif
