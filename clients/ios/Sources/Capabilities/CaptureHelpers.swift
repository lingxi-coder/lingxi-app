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

/// Drives a single hold-to-talk transcription through the same `SttImpl`
/// (`SFSpeechRecognizer` + mic tap) the engine bridges onto its STT seam.
///
/// `SttImpl` already requests speech-recognition + microphone authorization
/// before opening the tap (see `SttImpl.requestAuthorization`), so this helper
/// just invokes it and maps the generated `SpeechFfiError` onto a UI-friendly
/// result — exactly as Android's `VoiceCapture.transcribe` maps `SpeechFfiException`.
@MainActor
final class VoiceCapture {
    func transcribe(language: String? = nil) async -> VoiceCaptureResult {
        #if canImport(Speech) && canImport(AVFoundation)
            do {
                let text = try await SttImpl().transcribe(language: language)
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                return text.isEmpty ? .empty : .transcript(text)
            } catch let error {
                return mapError(error)
            }
        #else
            return .failed("speech unavailable on this platform")
        #endif
    }

    #if canImport(Speech) && canImport(AVFoundation)
        private func mapError(_ error: Error) -> VoiceCaptureResult {
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
