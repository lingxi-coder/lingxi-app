// M8-P12 skeleton — Swift impl of the Rust-declared `VoiceRecorder` callback
// interface. M9 backs it with AVAudioRecorder.
import Foundation
import LingxiCodeBindings

final class IosVoiceImpl: VoiceRecorder {
    func startRecording(opts: VoiceRecordingOpts) async throws {
        // TODO(M9): configure AVAudioSession + AVAudioRecorder at opts.sampleRateHz.
        throw VoiceError.Other(message: "Unimplemented (M8 skeleton)")
    }

    func stopRecording() async throws -> VoiceRecording {
        // TODO(M9): stop + return the encoded audio.
        throw VoiceError.Other(message: "Unimplemented (M8 skeleton)")
    }

    func isRecording() async -> Bool { false }
}
