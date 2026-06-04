// ShareImpl.swift — iOS native sharing capability (parity with Android
// ShareController.kt).
//
// Conforms to the generated `IosShare` UniFFI callback interface. The engine
// (tool-share) calls `share(text:url:imageBytes:)`; we assemble the activity
// items and present a `UIActivityViewController` from the active scene's top
// view controller, reporting whether the user completed or dismissed it as a
// `ShareResultFfi`. Errors map onto the generated `ShareFfiError`.

import Foundation

#if canImport(UIKit)
    import UIKit

    /// Native share over `UIActivityViewController`.
    final class ShareImpl: IosShare, @unchecked Sendable {
        func share(text: String?, url: String?, imageBytes: Data?) async throws -> ShareResultFfi {
            var items: [Any] = []
            if let text, !text.isEmpty { items.append(text) }
            if let url, let u = URL(string: url) { items.append(u) }
            if let imageBytes, let image = UIImage(data: imageBytes) { items.append(image) }
            guard !items.isEmpty else { throw ShareFfiError.Unsupported }

            return try await present(items: items)
        }

        @MainActor
        private func present(items: [Any]) async throws -> ShareResultFfi {
            guard let host = Presenter.topViewController() else {
                throw ShareFfiError.Other(message: "no active scene to present from")
            }
            return await withCheckedContinuation { (cont: CheckedContinuation<ShareResultFfi, Never>) in
                let vc = UIActivityViewController(activityItems: items, applicationActivities: nil)
                // Required on iPad to anchor the popover; harmless on iPhone.
                vc.popoverPresentationController?.sourceView = host.view
                vc.popoverPresentationController?.sourceRect = CGRect(
                    x: host.view.bounds.midX, y: host.view.bounds.midY, width: 0, height: 0)
                vc.completionWithItemsHandler = { _, completed, _, _ in
                    cont.resume(returning: completed ? .success : .cancelled)
                }
                host.present(vc, animated: true)
            }
        }
    }
#endif
