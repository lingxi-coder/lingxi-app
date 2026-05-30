// M8-P12 skeleton — Kotlin impl of the Rust-declared `CameraControl` callback
// interface. Rust calls these; M9 backs them with CameraX + ActivityResult
// (PickVisualMedia).
package com.lingxi.code

import com.lingxi.code.bindings.CameraControl
import com.lingxi.code.bindings.CameraError
import com.lingxi.code.bindings.CapturePhotoOpts
import com.lingxi.code.bindings.CapturedImage

class AndroidCameraImpl : CameraControl {
    override suspend fun capturePhoto(opts: CapturePhotoOpts): CapturedImage {
        // TODO(M9): CameraX ImageCapture honoring opts.cameraPosition.
        throw CameraError.Other("Unimplemented (M8 skeleton)")
    }

    override suspend fun pickFromLibrary(): CapturedImage {
        // TODO(M9): ActivityResultContracts.PickVisualMedia.
        throw CameraError.Other("Unimplemented (M8 skeleton)")
    }
}
