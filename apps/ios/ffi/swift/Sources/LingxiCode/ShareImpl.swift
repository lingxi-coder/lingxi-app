// M8-P12 skeleton — Swift impl of the Rust-declared `SharingService` callback
// interface. M9 backs it with UIActivityViewController.
import Foundation
import LingxiCodeBindings

final class IosShareImpl: SharingService {
    func share(payload: SharePayload) async throws -> ShareResult {
        // TODO(M9): present UIActivityViewController with payload.text / .url / .imageBytes.
        throw ShareError.Other(message: "Unimplemented (M8 skeleton)")
    }
}
