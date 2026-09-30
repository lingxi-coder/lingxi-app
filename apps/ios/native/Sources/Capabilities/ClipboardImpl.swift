// ClipboardImpl.swift — iOS native clipboard capability (parity with Android
// ClipboardController.kt).
//
// Conforms to the generated `IosClipboard` UniFFI callback interface. The engine
// (tool-clipboard) calls `setText(text:)` / `getText()`; we read/write
// `UIPasteboard.general`. `getText` returns nil when the pasteboard holds no
// text. Engine-driven only — no user-facing affordance.

import Foundation

#if canImport(UIKit)
    import UIKit

    /// Native clipboard over `UIPasteboard.general`.
    final class ClipboardImpl: IosClipboard, @unchecked Sendable {
        func setText(text: String) async throws {
            await MainActor.run { UIPasteboard.general.string = text }
        }

        func getText() async throws -> String? {
            await MainActor.run { UIPasteboard.general.hasStrings ? UIPasteboard.general.string : nil }
        }
    }
#endif
