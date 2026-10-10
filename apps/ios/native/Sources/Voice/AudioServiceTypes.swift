import Foundation

enum AudioServiceFailure: Error, LocalizedError {
    case permissionDenied
    case busy
    case cancelled
    case timeout
    case noSpeech
    case notRecording
    case unavailable
    case unsupported
    case modelMissing
    case voiceMissing
    case invalidRequest
    case synthesisFailed(String, operationStarted: Bool)
    case nativeFailure(String)
    case mediaTooLarge

    var errorDescription: String? {
        switch self {
        case .permissionDenied: "Audio permission was denied."
        case .busy: "Another audio operation is using the device."
        case .cancelled: "The audio operation was cancelled."
        case .timeout: "The audio operation timed out."
        case .noSpeech: "No speech was recognized."
        case .notRecording: "The recording is no longer active."
        case .unavailable: "The requested audio provider is unavailable."
        case .unsupported: "The requested audio operation or format is unsupported."
        case .modelMissing: "The selected offline audio model is unavailable."
        case .voiceMissing: "The selected voice is unavailable."
        case .invalidRequest: "The audio request is invalid."
        case let .synthesisFailed(message, _): "Speech synthesis failed: \(message)"
        case let .nativeFailure(message): "Native audio operation failed: \(message)"
        case .mediaTooLarge: "The audio payload exceeds the supported size."
        }
    }
}

struct AudioPcmOutput: Equatable, Sendable {
    let pcm: Data
    let sampleRateHz: UInt32
}

struct IOSAudioRecording: Sendable {
    let audioBytes: Data
    let mimeType: String
}

protocol AudioRecordingDriving: AnyObject, Sendable {
    func installActivityChangeHandler(_ handler: (@Sendable () -> Void)?)
    func startRecordingOwned(
        operationID: String,
        ownerID: String,
        sampleRateHz: UInt32,
        format: String,
        maximumBytes: UInt64
    ) async throws -> String
    func stopRecordingOwned(handle: String, ownerID: String, maximumBytes: UInt64) async throws -> IOSAudioRecording
    func recordingLevel(handle: String, ownerID: String) -> Float?
    func isRecordingOwned(handle: String?, ownerID: String) -> Bool
    func cancel(startOperationID: String) async
    func end(ownerID: String) async
    func stopAll() async
}

extension AudioRecordingDriving {
    func recordingLevel(handle: String, ownerID: String) -> Float? { nil }
    func installActivityChangeHandler(_: (@Sendable () -> Void)?) {}
}

enum SpeechRecognitionError: Error {
    case PermissionDenied
    case NoSpeech
    case Unavailable
    case Busy
    case Retriable(message: String)
    case Other(message: String)
}
