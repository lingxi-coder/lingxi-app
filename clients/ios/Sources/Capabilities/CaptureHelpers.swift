// CaptureHelpers.swift — USER-facing capability affordances (parity with the
// Android `rememberVoiceCapture` / `rememberCameraCapture` / `rememberShare`).
//
// The 7 device capabilities are ALSO engine-driven via tools (tool-speech,
// tool-camera, tool-share, …) through the generated `Ios*` callback interfaces.
// THESE helpers are the *user* entry points that mirror what Android's Composer /
// MessageBubble expose — and they REUSE the exact same native impls
// (`SttImpl` / `CameraImpl` / `ShareImpl`) the engine bridges onto, so a UI tap
// and a tool invocation ride the identical launch path:
//
//   * Hold-to-talk STT  → SttImpl.transcribe(...)        → fills the composer draft
//   * Camera / library  → CameraImpl.capture/pick(...)   → composer attachment chip
//   * Share a reply     → ShareImpl.share(text:...)      → UIActivityViewController
//
// All three are @MainActor and degrade to a no-op on a host where the native
// capability frameworks aren't present (so SwiftUI previews / non-device builds
// stay usable). NO secrets here.

import Foundation
import Observation
import SwiftUI

#if canImport(UIKit)
    import UIKit
#endif

// MARK: - Composer attachment

/// A photo the user captured via the composer's camera affordance, ready to be
/// reviewed before sending. Mirrors Android's `ComposerAttachment` — `image` is
/// the decoded preview the composer renders; `width`/`height` carry the source
/// dimensions surfaced by the capture (the `CapturedImageFfi` carrier the engine
/// bridges onto `traits::CapturedImage`).
struct ComposerAttachment: Identifiable, Equatable {
    let id = UUID()
    #if canImport(UIKit)
        let image: UIImage
    #endif
    let width: Int
    let height: Int

    // `UIImage` isn't `Equatable`, so identity is keyed on the stable `id`
    // (each capture produces a distinct attachment).
    static func == (lhs: ComposerAttachment, rhs: ComposerAttachment) -> Bool {
        lhs.id == rhs.id
    }
}

// MARK: - Hold-to-talk STT capture

/// One hold-to-talk transcription outcome (mirrors Android `VoiceCaptureResult`).
enum VoiceCaptureResult: Equatable {
    case transcript(String)
    case permissionDenied
    case empty
    case failed(String)
}

enum VoiceCapturePhase: Equatable {
    case idle
    case listening
    case finishing
}

/// Narrow session seam used by both hold-to-talk and Flow Mode. Keeping the
/// lifecycle outside SwiftUI makes press/release/cancel behavior deterministic
/// and lets tests prove that a cancelled recording never submits a transcript.
@MainActor
protocol VoiceTranscriptionSession: AnyObject {
    func transcribe(language: String?) async throws -> String
    func finishRecording()
    func cancelRecognition()
}

#if canImport(Speech) && canImport(AVFoundation)
    @MainActor
    private final class SystemVoiceTranscriptionSession: VoiceTranscriptionSession {
        private let implementation = SttImpl()

        func transcribe(language: String?) async throws -> String {
            try await implementation.transcribe(language: language)
        }

        func finishRecording() {
            implementation.finishRecording()
        }

        func cancelRecognition() {
            implementation.cancelRecognition()
        }
    }
#endif

/// Drives a single hold-to-talk transcription through the same `SttImpl`
/// (`SFSpeechRecognizer` + mic tap) the engine bridges onto its STT seam.
///
/// `SttImpl` requests speech-recognition + microphone authorization before
/// opening the tap. This helper owns the UI-facing press lifecycle and maps the
/// generated `SpeechFfiError` onto a stable result.
@MainActor
@Observable
final class VoiceCapture {
    typealias Completion = @MainActor (VoiceCaptureResult) -> Void

    private struct ActiveCapture {
        let id: UUID
        let session: any VoiceTranscriptionSession
        let task: Task<Void, Never>
    }

    private let makeSession: @MainActor () -> (any VoiceTranscriptionSession)?
    private var activeCapture: ActiveCapture?
    private(set) var phase: VoiceCapturePhase = .idle

    init() {
        #if canImport(Speech) && canImport(AVFoundation)
            makeSession = { SystemVoiceTranscriptionSession() }
        #else
            makeSession = { nil }
        #endif
    }

    /// Test-only seam is internal so the hosted XCTest target can inject a
    /// deterministic recognizer without opening the microphone.
    init(makeSession: @escaping @MainActor () -> any VoiceTranscriptionSession) {
        self.makeSession = { makeSession() }
    }

    /// Begin opening the microphone immediately. A matching `finish()` ends the
    /// audio request and lets Speech return its final result; `cancel()` tears the
    /// whole session down and deliberately suppresses completion delivery.
    func start(language: String? = nil, completion: @escaping Completion) {
        cancel()
        guard let session = makeSession() else {
            completion(.failed("speech unavailable on this platform"))
            return
        }

        let id = UUID()
        phase = .listening
        let task = Task { [weak self, session] in
            let result: VoiceCaptureResult
            do {
                let text = try await session.transcribe(language: language)
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                result = text.isEmpty ? .empty : .transcript(text)
            } catch is CancellationError {
                return
            } catch {
                result = Self.mapError(error)
            }

            guard !Task.isCancelled else { return }
            self?.complete(id: id, result: result, completion: completion)
        }
        activeCapture = ActiveCapture(id: id, session: session, task: task)
    }

    func finish() {
        guard phase == .listening, let activeCapture else { return }
        phase = .finishing
        activeCapture.session.finishRecording()
    }

    func cancel() {
        guard let activeCapture else {
            phase = .idle
            return
        }
        self.activeCapture = nil
        phase = .idle
        activeCapture.session.cancelRecognition()
        activeCapture.task.cancel()
    }

    private func complete(id: UUID, result: VoiceCaptureResult, completion: Completion) {
        guard activeCapture?.id == id else { return }
        activeCapture = nil
        phase = .idle
        completion(result)
    }

    #if canImport(Speech) && canImport(AVFoundation)
        private static func mapError(_ error: Error) -> VoiceCaptureResult {
            guard let ffi = error as? SpeechFfiError else { return .failed("\(error)") }
            switch ffi {
            case .PermissionDenied:
                return .permissionDenied
            case .NoSpeech:
                return .empty
            case let .Retriable(message):
                return .failed(message)
            case let .Other(message):
                return .failed(message)
            case .Unavailable:
                return .failed("speech recognizer unavailable")
            }
        }
    #else
        private static func mapError(_ error: Error) -> VoiceCaptureResult {
            .failed("\(error)")
        }
    #endif
}

// MARK: - Camera capture

/// One camera/library capture outcome (mirrors Android `CameraCaptureResult`).
enum CameraCaptureResult: Equatable {
    case captured(ComposerAttachment)
    case cancelled
    case permissionDenied
    case failed(String)
}

/// Drives a single on-device photo capture / library pick through the same
/// `CameraImpl` (`UIImagePickerController`) the engine bridges onto its camera
/// seam, then decodes the returned `CapturedImageFfi` JPEG into a
/// `ComposerAttachment` for the composer thumbnail (the device-vision analog of
/// how a transcript surfaces in the draft).
@MainActor
final class CameraCapture {
    /// `library == true` opens the photo library; otherwise the live camera.
    func capture(fromLibrary library: Bool) async -> CameraCaptureResult {
        #if canImport(UIKit) && canImport(AVFoundation)
            let impl = CameraImpl()
            do {
                let ffi = library
                    ? try await impl.pickFromLibrary()
                    : try await impl.capturePhoto(front: false, allowEditing: false)
                guard let image = UIImage(data: ffi.jpegBytes) else {
                    return .failed("could not decode captured image")
                }
                return .captured(ComposerAttachment(
                    image: image,
                    width: Int(ffi.width),
                    height: Int(ffi.height)))
            } catch let error {
                return mapError(error)
            }
        #else
            return .failed("camera unavailable on this platform")
        #endif
    }

    #if canImport(UIKit) && canImport(AVFoundation)
        private func mapError(_ error: Error) -> CameraCaptureResult {
            guard let ffi = error as? CameraFfiError else { return .failed("\(error)") }
            switch ffi {
            case .Cancelled:
                return .cancelled
            case .PermissionDenied:
                return .permissionDenied
            case .DeviceUnavailable:
                return .failed("camera/library unavailable")
            case let .Other(message):
                return .failed(message)
            }
        }
    #endif
}

// MARK: - Share

/// Presents the native share sheet for a message's text through the same
/// `ShareImpl` (`UIActivityViewController` via `Presenter`) the engine bridges
/// onto its share seam — so a bubble share and a `tool-share` invocation are the
/// identical launch path (mirrors Android `rememberShare`). Fire-and-forget: no
/// permission gate, the result is ignored.
@MainActor
enum ShareCapture {
    static func share(text: String) {
        #if canImport(UIKit)
            let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty else { return }
            Task {
                _ = try? await ShareImpl().share(text: trimmed, url: nil, imageBytes: nil)
            }
        #endif
    }
}
