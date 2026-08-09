// VoiceImpl.swift — iOS native mic-recorder capability (parity with Android
// RecorderController.kt).
//
// Conforms to the generated `IosVoice` UniFFI callback interface. The engine
// (tool-voice) drives start/stop/is_recording; we capture the raw mic with
// `AVAudioRecorder` to an AAC/m4a file in the app's temp dir, then return the
// encoded bytes + MIME type as a `VoiceRecordingFfi` on stop. Errors map onto
// the generated `VoiceFfiError`. This is the RAW recorder, distinct from
// `SttImpl` (the system recognizer).

import Foundation

#if canImport(AVFoundation)
    import AVFoundation

    /// Native mic recorder over `AVAudioRecorder` producing AAC/m4a.
    final class VoiceImpl: NSObject, IosVoice, @unchecked Sendable {
        private let lock = NSLock()
        private var recorder: AVAudioRecorder?
        private var fileURL: URL?
        private var audioLease: VoiceAudioSessionCoordinator.Lease?

        func startRecording(sampleRateHz: UInt32, format: String) async throws {
            try await requestMicAuthorization()

            let lease: VoiceAudioSessionCoordinator.Lease
            do {
                lease = try await VoiceAudioSessionCoordinator.shared.acquire(.recording)
            } catch is VoiceAudioSessionCoordinator.CoordinationError {
                // Contention, not failure: FlowMode (or a hold-to-talk
                // capture) owns the session. Typed so a local app can say
                // "try again in a moment" rather than surfacing an opaque
                // error the user cannot act on.
                throw VoiceFfiError.Busy
            } catch {
                throw VoiceFfiError.Other(message: "audio session: \(error.localizedDescription)")
            }

            let url = FileManager.default.temporaryDirectory
                .appendingPathComponent("lingxi-voice-\(UUID().uuidString).m4a")
            let settings: [String: Any] = [
                AVFormatIDKey: kAudioFormatMPEG4AAC,
                AVSampleRateKey: Double(sampleRateHz),
                AVNumberOfChannelsKey: 1,
                AVEncoderAudioQualityKey: AVAudioQuality.high.rawValue,
            ]
            do {
                let rec = try AVAudioRecorder(url: url, settings: settings)
                guard rec.record() else {
                    throw VoiceFfiError.Other(message: "recorder failed to start")
                }
                store(recorder: rec, fileURL: url, audioLease: lease)
            } catch let e as VoiceFfiError {
                try? FileManager.default.removeItem(at: url)
                await VoiceAudioSessionCoordinator.shared.release(lease)
                throw e
            } catch {
                try? FileManager.default.removeItem(at: url)
                await VoiceAudioSessionCoordinator.shared.release(lease)
                throw VoiceFfiError.Other(message: error.localizedDescription)
            }
        }

        func stopRecording() async throws -> VoiceRecordingFfi {
            let (rec, url, lease) = takeRecorder()

            guard let rec, let url, let lease else { throw VoiceFfiError.NotRecording }
            rec.stop()
            await VoiceAudioSessionCoordinator.shared.release(lease)

            do {
                let data = try Data(contentsOf: url)
                try? FileManager.default.removeItem(at: url)
                return VoiceRecordingFfi(audioBytes: data, mimeType: "audio/m4a")
            } catch {
                throw VoiceFfiError.Other(message: "read recording: \(error.localizedDescription)")
            }
        }

        func isRecording() async -> Bool {
            recordingState()
        }

        private func requestMicAuthorization() async throws {
            let granted = await AVAudioApplication.requestRecordPermission()
            guard granted else { throw VoiceFfiError.PermissionDenied }
        }

        private func store(
            recorder: AVAudioRecorder,
            fileURL: URL,
            audioLease: VoiceAudioSessionCoordinator.Lease
        ) {
            lock.lock()
            defer { lock.unlock() }
            self.recorder = recorder
            self.fileURL = fileURL
            self.audioLease = audioLease
        }

        private func takeRecorder() -> (
            AVAudioRecorder?,
            URL?,
            VoiceAudioSessionCoordinator.Lease?
        ) {
            lock.lock()
            defer { lock.unlock() }
            let current = (recorder, fileURL, audioLease)
            recorder = nil
            fileURL = nil
            audioLease = nil
            return current
        }

        private func recordingState() -> Bool {
            lock.lock()
            defer { lock.unlock() }
            return recorder?.isRecording ?? false
        }
    }
#endif
