import Foundation

enum VoiceRecognitionRoute: Equatable, Sendable {
    case system(languageIdentifier: String)
    case sherpa(languageIdentifier: String, modelID: String, modelDirectory: URL)
    case unavailable(String)
}


enum VoiceOptionIdentity {
    static let systemDefault = "system_default"
    static func parseOfflineOptionID(_ selection: String) -> (modelID: String, voiceID: String)? {
        guard selection.hasPrefix("offline:") else { return nil }
        let payload = String(selection.dropFirst("offline:".count))
        guard let separator = payload.lastIndex(of: ":") else { return nil }
        let modelID = String(payload[..<separator])
        let voiceID = String(payload[payload.index(after: separator)...])
        guard !modelID.isEmpty, !voiceID.isEmpty else { return nil }
        return (modelID, voiceID)
    }
}
