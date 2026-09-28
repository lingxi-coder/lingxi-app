import Combine
import Foundation
import OSLog
import SwiftUI

#if canImport(harness_runtimeFFI)
import AuthenticationServices
import UIKit
import harness_runtimeFFI
#endif

/// Convert the shared send DTO into the URL-shaped UI media projection used by
/// live and restored messages. The URL is a data URL so the transcript remains
/// self-contained across session switches and process restarts.
func uiImages(from images: [ImageRefDto]) -> [MessageImage] {
    images.map {
        MessageImage(
            mediaType: $0.mediaType,
            url: "data:\($0.mediaType);base64,\($0.base64)"
        )
    }
}

func uiMessageImages(from images: [MessageImageDto]) -> [MessageImage] {
    images.map { MessageImage(mediaType: $0.mediaType, url: $0.url) }
}
