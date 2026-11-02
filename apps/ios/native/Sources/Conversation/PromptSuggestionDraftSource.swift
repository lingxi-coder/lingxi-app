import Foundation

/// Maps editor changes back onto a suggestion's source UTF-16 units. Swift
/// `String` replaces isolated surrogates when displaying them, but an edit to a
/// different range must not erase those untouched source units.
enum PromptSuggestionDraftSource {
    /// Voice capture can finish after the composer disappears, when its edit
    /// observer is no longer available to update the original UTF-16 units.
    static func appendingTranscript(
        _ transcript: String, to draft: String, sourceCodeUnits: [UInt16]?
    ) -> (text: String, sourceCodeUnits: [UInt16]?) {
        guard !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return (transcript, nil)
        }
        let text = "\(draft) \(transcript)"
        guard let sourceCodeUnits,
              displayCodeUnits(for: sourceCodeUnits) == Array(draft.utf16) else {
            return (text, nil)
        }
        return (text, sourceCodeUnits + [0x20] + Array(transcript.utf16))
    }

    static func applyingEdit(
        sourceCodeUnits: [UInt16],
        previousDisplayCodeUnits: [UInt16],
        newDisplayCodeUnits: [UInt16]
    ) -> [UInt16]? {
        guard displayCodeUnits(for: sourceCodeUnits) == previousDisplayCodeUnits else { return nil }

        var prefix = 0
        while prefix < previousDisplayCodeUnits.count,
              prefix < newDisplayCodeUnits.count,
              previousDisplayCodeUnits[prefix] == newDisplayCodeUnits[prefix] {
            prefix += 1
        }

        var suffix = 0
        while suffix < previousDisplayCodeUnits.count - prefix,
              suffix < newDisplayCodeUnits.count - prefix,
              previousDisplayCodeUnits[previousDisplayCodeUnits.count - suffix - 1]
                == newDisplayCodeUnits[newDisplayCodeUnits.count - suffix - 1] {
            suffix += 1
        }

        let oldEnd = previousDisplayCodeUnits.count - suffix
        let newEnd = newDisplayCodeUnits.count - suffix
        return Array(sourceCodeUnits[..<prefix])
            + Array(newDisplayCodeUnits[prefix..<newEnd])
            + Array(sourceCodeUnits[oldEnd..<sourceCodeUnits.count])
    }

    static func displayCodeUnits(for sourceCodeUnits: [UInt16]) -> [UInt16] {
        Array(String(decoding: sourceCodeUnits, as: UTF16.self).utf16)
    }

    static func trimmingWhitespace(
        sourceCodeUnits: [UInt16],
        displayText: String
    ) -> [UInt16]? {
        let displayCodeUnits = Array(displayText.utf16)
        guard self.displayCodeUnits(for: sourceCodeUnits) == displayCodeUnits else { return nil }

        let whitespace = CharacterSet.whitespacesAndNewlines
        var start = 0
        for scalar in displayText.unicodeScalars {
            guard whitespace.contains(scalar) else { break }
            start += (scalar.value <= 0xFFFF ? 1 : 2)
        }

        var end = displayCodeUnits.count
        for scalar in displayText.unicodeScalars.reversed() {
            guard whitespace.contains(scalar) else { break }
            end -= (scalar.value <= 0xFFFF ? 1 : 2)
        }
        guard start < end else { return [] }
        return Array(sourceCodeUnits[start..<end])
    }
}
