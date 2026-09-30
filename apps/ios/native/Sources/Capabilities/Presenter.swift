// Presenter.swift — shared helper to present a UIKit view controller from the
// active scene's top-most view controller. Used by CameraImpl / ShareImpl, which
// must show system UI (image picker, activity sheet) from SwiftUI.

import Foundation

#if canImport(UIKit)
    import UIKit

    enum Presenter {
        /// The top-most presented view controller of the foreground-active window
        /// scene's key window, or nil if no scene is active.
        @MainActor
        static func topViewController() -> UIViewController? {
            let scene = UIApplication.shared.connectedScenes
                .compactMap { $0 as? UIWindowScene }
                .first(where: { $0.activationState == .foregroundActive })
                ?? UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.first
            let keyWindow = scene?.windows.first(where: { $0.isKeyWindow }) ?? scene?.windows.first
            var top = keyWindow?.rootViewController
            while let presented = top?.presentedViewController {
                top = presented
            }
            return top
        }
    }
#endif
