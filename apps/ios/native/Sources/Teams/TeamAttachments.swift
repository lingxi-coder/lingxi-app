import CryptoKit
import Foundation

/// A file as a Team message carries it (Desktop `packages/teams/attachments.ts`). `revoked`: its sender took
/// it back and its bytes are gone. The bytes are read through the service every time, so they are only ever
/// shown to someone who reads that conversation now.
struct TeamMessageAttachment: Codable, Equatable, Sendable, Identifiable {
    let attachmentId: String
    let name: String
    let mime: String
    let byteLength: Int
    let sha256: String
    var revoked: Bool? = nil
    var id: String { attachmentId }
    var isImage: Bool { mime == "image/png" || mime == "image/jpeg" }
}

/// On a Bot's reply: a file of the conversation its model was given, as the service checked it.
struct TeamReadFile: Codable, Equatable, Sendable {
    let attachmentId: String
    let name: String
}

/// A file in an unsent message. `sent` is how many of its bytes the service has.
struct TeamDraftFile: Equatable, Sendable, Identifiable {
    enum State: Equatable, Sendable { case uploading, ready, failed }
    let uploadId: String
    let name: String
    let mime: String
    let byteLength: Int
    var state: State = .uploading
    var sent = 0
    /// The service's code, or a local one, when `state` is `failed`.
    var error: String?
    var id: String { uploadId }
}

/// A file the viewer chose, read into memory. `error`: it could not be read or made into a kind Teams takes.
struct TeamChosenFile: Sendable {
    let name: String
    let type: String
    let data: Data
    var error: String? = nil
}

/// A chosen file that did not go into the message, and why.
struct TeamFileRefusal: Equatable, Sendable, Identifiable {
    let name: String
    let code: String
    var id: String { name + "\u{0}" + code }
}

/// What the service answers about an upload.
struct TeamUploadView: Decodable, Sendable {
    let uploadId: String
    let state: String
    let parts: Int
    let partBytes: Int
    var code: String? = nil
}

/// One part of a file read back.
struct TeamAttachmentPart: Decodable, Sendable {
    let parts: Int
    let data: String
}

/// The rules for files in a Team message. Foundation and CryptoKit only, so they are tested on macOS;
/// Android has the same in `teams/TeamAttachments.kt`.
enum TeamAttachments {
    static let partBytes = 700 * 1024, fileBytes = 10 * 1024 * 1024, messageFiles = 5, messageBytes = 20 * 1024 * 1024
    static let types = ["image/png", "image/jpeg", "application/pdf", "text/plain"]
    private static let textEndings = ["txt", "md", "markdown", "csv", "log"]

    /// The type a file is sent as, from what the system says about it and from its name; `nil` for a kind of
    /// file Teams does not take.
    static func type(name: String, declared: String) -> String? {
        let given = declared.lowercased().split(separator: ";").first.map { $0.trimmingCharacters(in: .whitespaces) } ?? ""
        switch given {
        case "image/png", "image/jpeg", "application/pdf": return given
        case "image/jpg": return "image/jpeg"
        case "text/plain", "text/markdown", "text/csv": return "text/plain"
        case "": break
        default: return nil
        }
        let ending = name.split(separator: ".").count > 1 ? name.split(separator: ".").last.map { $0.lowercased() } ?? "" : ""
        switch ending {
        case "png": return "image/png"
        case "jpg", "jpeg": return "image/jpeg"
        case "pdf": return "application/pdf"
        default: return textEndings.contains(ending) ? "text/plain" : nil
        }
    }

    /// A file's own name as the service takes it: no path, no control characters, at most 120 UTF-16 units
    /// with its ending kept.
    static func name(_ name: String) -> String {
        let last = name.split(omittingEmptySubsequences: false, whereSeparator: { $0 == "/" || $0 == "\\" }).last.map(String.init) ?? ""
        let base = String(last.unicodeScalars.filter { $0.value > 0x1f && $0.value != 0x7f }).trimmingCharacters(in: .whitespacesAndNewlines)
        if base.isEmpty || base == "." || base == ".." { return "file" }
        if base.utf16.count <= 120 { return base }
        var ending = ""
        if let dot = base.lastIndex(of: "."), dot != base.startIndex, base[dot...].utf16.count <= 10 { ending = String(base[dot...]) }
        var stem = ""
        for character in base.dropLast(ending.count) {
            if stem.utf16.count + String(character).utf16.count > 120 - ending.utf16.count { break }
            stem.append(character)
        }
        return stem.trimmingCharacters(in: .whitespacesAndNewlines) + ending
    }

    /// 1.4 MB, 320 KB, 12 B.
    static func size(_ bytes: Int) -> String {
        if bytes >= 1024 * 1024 {
            let value = (Double(bytes) / (1024 * 1024) * 10).rounded() / 10
            return (value == value.rounded() ? String(Int(value)) : String(value)) + " MB"
        }
        if bytes >= 1024 { return "\(Int((Double(bytes) / 1024).rounded())) KB" }
        return "\(bytes) B"
    }

    struct Picked: Equatable, Sendable { let name: String; let type: String; let size: Int }
    struct Admission: Equatable, Sendable {
        /// Indices into the chosen files, with the name and type each is sent as.
        var accepted: [(index: Int, name: String, mime: String)] = []
        var refused: [(name: String, code: String)] = []
        static func == (a: Admission, b: Admission) -> Bool {
            a.accepted.map { [String($0.index), $0.name, $0.mime] } == b.accepted.map { [String($0.index), $0.name, $0.mime] }
                && a.refused.map { [$0.name, $0.code] } == b.refused.map { [$0.name, $0.code] }
        }
    }

    /// Which of the chosen files go into a draft that already holds files of `draft` bytes each, in the order
    /// they were chosen, and why each of the others does not: not a kind Teams takes, empty, over 10 MiB, a
    /// sixth file, or more than 20 MiB together.
    static func admit(draft: [Int], files: [Picked]) -> Admission {
        var result = Admission(), count = draft.count, total = draft.reduce(0, +)
        for (index, file) in files.enumerated() {
            let mime = type(name: file.name, declared: file.type), sendName = name(file.name)
            let code: String? = mime == nil ? "ATTACHMENT_TYPE" : file.size == 0 ? "ATTACHMENT_EMPTY" : file.size > fileBytes ? "ATTACHMENT_TOO_LARGE"
                : count >= messageFiles ? "ATTACHMENT_COUNT" : total + file.size > messageBytes ? "ATTACHMENT_TOTAL" : nil
            if let code { result.refused.append((sendName, code)); continue }
            result.accepted.append((index, sendName, mime!)); count += 1; total += file.size
        }
        return result
    }

    static func sha256Hex(_ data: Data) -> String {
        SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }

    /// The parts a file is uploaded in, in order.
    static func parts(_ data: Data, partBytes: Int) -> [Data] {
        guard partBytes > 0 else { return [] }
        return stride(from: 0, to: data.count, by: partBytes).map { data.subdata(in: $0..<min(data.count, $0 + partBytes)) }
    }

    /// Whether an answer will not change by asking again. Anything else (no connection, a busy service) is tried again.
    static func isFinal(_ code: String) -> Bool {
        code.hasPrefix("UPLOAD_") || code.hasPrefix("ATTACHMENT_") || code.hasPrefix("INVALID_")
            || ["CONVERSATION_REQUIRED", "MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "SIGN_IN_REQUIRED", "INVALID_SESSION", "REQUEST_TOO_LARGE", "COMMAND_ID_CONFLICT"].contains(code)
    }

    /// A file of a message put together from its parts and checked against what the message says it is.
    static func assemble(_ parts: [String], file: TeamMessageAttachment) throws -> Data {
        var data = Data()
        for part in parts {
            guard let bytes = Data(base64Encoded: part) else { throw TeamMessageFailure(code: "ATTACHMENT_DAMAGED") }
            data.append(bytes)
        }
        guard data.count == file.byteLength, sha256Hex(data) == file.sha256 else { throw TeamMessageFailure(code: "ATTACHMENT_DAMAGED") }
        return data
    }

    /// What a picture's bytes are: `image/png` or `image/jpeg` by their first bytes, `nil` for anything else (which is
    /// made into a JPEG before it is sent).
    static func pictureType(_ data: Data) -> String? {
        let head = [UInt8](data.prefix(8))
        if head == [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a] { return "image/png" }
        if head.count >= 3, head[0] == 0xff, head[1] == 0xd8, head[2] == 0xff { return "image/jpeg" }
        return nil
    }

    /// Whether the unsent message can be sent: every file is checked, and there is text or a file.
    static func canSend(text: String, files: [TeamDraftFile]) -> Bool {
        guard files.allSatisfy({ $0.state == .ready }) else { return false }
        return files.isEmpty ? TeamsComposeLogic.canSendText(text) : TeamsComposeLogic.byteLength(text) <= TeamsComposeLogic.messageBytes
    }

    /// What a pending bubble or a preview says for a message of files alone.
    static func said(text: String, names: [String]) -> String {
        text.isEmpty ? names.joined(separator: ", ") : text
    }
}
