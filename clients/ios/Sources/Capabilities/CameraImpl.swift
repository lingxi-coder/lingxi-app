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

        @MainActor
        private func present(
            sourceType: UIImagePickerController.SourceType,
            front: Bool,
            allowEditing: Bool
        ) async throws -> CapturedImageFfi {
            guard let host = Presenter.topViewController() else {
                throw CameraFfiError.Other(message: "no active scene to present from")
            }
            return try await withCheckedThrowingContinuation { cont in
                self.continuation = cont
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
            let image = (info[.editedImage] as? UIImage) ?? (info[.originalImage] as? UIImage)
            picker.dismiss(animated: true)
            guard let image, let jpeg = image.jpegData(compressionQuality: 0.9) else {
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
    }
#endif
