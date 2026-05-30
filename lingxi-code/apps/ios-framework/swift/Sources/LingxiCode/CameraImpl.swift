// M8-P12 skeleton — Swift implementation of the Rust-declared `CameraControl`
// callback interface. Rust calls these methods; M9 fills the bodies with
// AVCaptureSession (capture) + PHPickerViewController (library).
import Foundation
import LingxiCodeBindings

final class IosCameraImpl: CameraControl {
    func capturePhoto(opts: CapturePhotoOpts) async throws -> CapturedImage {
        // TODO(M9): AVCaptureSession capture honoring opts.position / allowEditing.
        throw CameraError.Other(message: "Unimplemented (M8 skeleton)")
    }

    func pickFromLibrary() async throws -> CapturedImage {
        // TODO(M9): PHPickerViewController flow.
        throw CameraError.Other(message: "Unimplemented (M8 skeleton)")
    }
}
