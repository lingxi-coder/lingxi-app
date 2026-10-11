// CameraImpl.swift — iOS native camera capability (parity with Android
// CameraController.kt).
//
// Conforms to the generated `IosCamera` UniFFI callback interface. The engine
// (tool-camera) calls `capturePhoto(front:allowEditing:)` (live capture via the
// native camera UI) or `pickFromLibrary()` (system photo library). We present a
// `UIImagePickerController` from the active scene's top view controller, request
// camera authorization for capture, JPEG-encode the result, and return a
// `CapturedImageFfi`. Errors map onto the generated `CameraFfiError`.

import Foundation

#if canImport(UIKit) && canImport(AVFoundation)
    import AVFoundation
    import UIKit

    /// Native camera over `UIImagePickerController` (capture + library pick).
    final class CameraImpl: NSObject, IosCamera, @unchecked Sendable {
        private var continuation: CheckedContinuation<CapturedImageFfi, Error>?
        private var picker: UIImagePickerController?
        /// Downscale applied to the NEXT delivered image, when the caller
        /// asked for one. Full-size delivery (the engine's camera tool) leaves
        /// it nil.
        private var pendingScaling: (maxDimension: UInt32, quality: Float)?

        func capturePhoto(front: Bool, allowEditing: Bool) async throws -> CapturedImageFfi {
            guard UIImagePickerController.isSourceTypeAvailable(.camera) else {
                throw CameraFfiError.DeviceUnavailable
            }
            try await requestCameraAuthorization()
            return try await present(sourceType: .camera, front: front, allowEditing: allowEditing)
        }

        func pickFromLibrary() async throws -> CapturedImageFfi {
            guard UIImagePickerController.isSourceTypeAvailable(.photoLibrary) else {
                throw CameraFfiError.DeviceUnavailable
            }
            return try await present(sourceType: .photoLibrary, front: false, allowEditing: false)
        }

        /// Capture, downscaled and re-encoded natively.
        ///
        /// A provider's vision endpoint
        /// needs far less than a 12 MP original, and Rust has no image
        /// codec on this build to shrink it after the fact — so the scaling
        /// lives here, where UIKit already has the decoded image.
        func capturePhotoSized(
            front: Bool,
            allowEditing: Bool,
            maxDimension: UInt32,
            jpegQuality: Float
        ) async throws -> CapturedImageFfi {
            guard UIImagePickerController.isSourceTypeAvailable(.camera) else {
                throw CameraFfiError.DeviceUnavailable
            }
            try await requestCameraAuthorization()
            return try await present(
                sourceType: .camera,
                front: front,
                allowEditing: allowEditing,
                scaling: (maxDimension, jpegQuality))
        }

        func pickFromLibrarySized(
            maxDimension: UInt32,
            jpegQuality: Float
        ) async throws -> CapturedImageFfi {
            guard UIImagePickerController.isSourceTypeAvailable(.photoLibrary) else {
                throw CameraFfiError.DeviceUnavailable
            }
            return try await present(
                sourceType: .photoLibrary,
                front: false,
                allowEditing: false,
                scaling: (maxDimension, jpegQuality))
        }

        @MainActor
        private func present(
            sourceType: UIImagePickerController.SourceType,
            front: Bool,
            allowEditing: Bool,
            scaling: (maxDimension: UInt32, quality: Float)? = nil
        ) async throws -> CapturedImageFfi {
            guard let host = Presenter.topViewController() else {
                throw CameraFfiError.Other(message: "no active scene to present from")
            }
            return try await withCheckedThrowingContinuation { cont in
                // One slot, one picker — exactly like `LocationImpl`. Without
                // this guard a second concurrent capture (a double-tapped
                // button is two requests) overwrites
                // `continuation`, so the FIRST one is dropped unresumed: the
                // Swift runtime logs a leaked-continuation misuse and the
                // caller's `await` — which no timeout covers on the camera
                // path — never returns.
                guard self.continuation == nil else {
                    cont.resume(throwing: CameraFfiError.Other(
                        message: "another camera request is already in flight"))
                    return
                }
                self.continuation = cont
                self.pendingScaling = scaling
                let picker = UIImagePickerController()
                picker.sourceType = sourceType
                picker.allowsEditing = allowEditing
                picker.delegate = self
                if sourceType == .camera {
                    picker.cameraDevice = front ? .front : .rear
                }
                self.picker = picker
                host.present(picker, animated: true)
            }
        }

        private func requestCameraAuthorization() async throws {
            switch AVCaptureDevice.authorizationStatus(for: .video) {
            case .authorized:
                return
            case .notDetermined:
                let granted: Bool = await withCheckedContinuation { cont in
                    AVCaptureDevice.requestAccess(for: .video) { cont.resume(returning: $0) }
                }
                guard granted else { throw CameraFfiError.PermissionDenied }
            default:
                throw CameraFfiError.PermissionDenied
            }
        }

        @MainActor
        private func finish(_ result: Result<CapturedImageFfi, Error>) {
            guard let cont = continuation else { return }
            continuation = nil
            picker = nil
            pendingScaling = nil
            switch result {
            case let .success(img): cont.resume(returning: img)
            case let .failure(err): cont.resume(throwing: err)
            }
        }
    }

    extension CameraImpl: UIImagePickerControllerDelegate, UINavigationControllerDelegate {
        func imagePickerController(
            _ picker: UIImagePickerController,
            didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]
        ) {
            let original = (info[.editedImage] as? UIImage) ?? (info[.originalImage] as? UIImage)
            picker.dismiss(animated: true)
            let scaling = pendingScaling
            guard let original else {
                finish(.failure(CameraFfiError.Other(message: "could not encode captured image")))
                return
            }
            let image = scaling.map { CameraImpl.downscaled(original, maxDimension: CGFloat($0.maxDimension)) } ?? original
            let quality = scaling.map { CGFloat($0.quality) } ?? 0.9
            guard let jpeg = image.jpegData(compressionQuality: quality) else {
                finish(.failure(CameraFfiError.Other(message: "could not encode captured image")))
                return
            }
            let ffi = CapturedImageFfi(
                jpegBytes: jpeg,
                width: UInt32(image.size.width * image.scale),
                height: UInt32(image.size.height * image.scale))
            finish(.success(ffi))
        }

        func imagePickerControllerDidCancel(_ picker: UIImagePickerController) {
            picker.dismiss(animated: true)
            finish(.failure(CameraFfiError.Cancelled))
        }

        /// Fit `image` inside `maxDimension` on its longer side, preserving
        /// aspect ratio. Already-small images are returned untouched — no
        /// upscaling, which would only add bytes.
        static func downscaled(_ image: UIImage, maxDimension: CGFloat) -> UIImage {
            let pixelWidth = image.size.width * image.scale
            let pixelHeight = image.size.height * image.scale
            let longest = max(pixelWidth, pixelHeight)
            guard longest > maxDimension, longest > 0 else { return image }
            let ratio = maxDimension / longest
            let target = CGSize(width: (pixelWidth * ratio).rounded(), height: (pixelHeight * ratio).rounded())
            let format = UIGraphicsImageRendererFormat.default()
            // Draw in PIXELS: the default format would re-apply the device
            // scale and hand back an image `scale`× larger than asked for.
            format.scale = 1
            return UIGraphicsImageRenderer(size: target, format: format).image { _ in
                image.draw(in: CGRect(origin: .zero, size: target))
            }
        }
    }
#endif
