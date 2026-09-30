import Foundation

/// Incrementally turns append-only assistant text into natural, speakable
/// utterances. It owns the uncommitted tail, so a provider may split Markdown
/// fences or punctuation across arbitrary transport deltas.
struct StreamingSpeechSegmenter {
    private static let strongBoundaries: Set<Character> = ["。", "！", "？", "!", "?", "；", ";", "\n"]
    private static let softBoundaries: Set<Character> = ["，", ",", "：", ":", "、"]
    private static let fallbackBoundaries: Set<Character> = strongBoundaries
        .union(softBoundaries)
        .union([" ", "\t"])

    private let minimumSegmentLength: Int
    private let softBoundaryThreshold: Int
    private let hardBoundaryThreshold: Int

    private(set) var receivedText = ""
    private var pendingText = ""
    private var markdownCarry = ""
    private var isInsideFencedCode = false
    private var isFinished = false
    private(set) var detectedTerminalRewrite = false

    init(
        minimumSegmentLength: Int = 6,
        softBoundaryThreshold: Int = 24,
        hardBoundaryThreshold: Int = 64
    ) {
        self.minimumSegmentLength = minimumSegmentLength
        self.softBoundaryThreshold = softBoundaryThreshold
        self.hardBoundaryThreshold = hardBoundaryThreshold
    }

    mutating func append(_ delta: String) -> [String] {
        guard !isFinished, !delta.isEmpty else { return [] }
        receivedText += delta
        pendingText += consumeMarkdown(delta, isFinal: false)
        return drainCompletedSegments()
    }

    /// Flush the stream tail and, when possible, append text that appeared only
    /// in the provider's canonical terminal message. A non-prefix rewrite never
    /// replays already emitted speech.
    mutating func finish(finalText: String) -> [String] {
        guard !isFinished else { return [] }
        isFinished = true
        let streamedSpeakable = Self.sanitize(receivedText)
        let finalSpeakable = Self.sanitize(finalText)

        if streamedSpeakable.isEmpty, !finalSpeakable.isEmpty {
            pendingText = finalSpeakable
            markdownCarry = ""
            isInsideFencedCode = false
        } else if !finalSpeakable.isEmpty,
                  finalSpeakable.hasPrefix(streamedSpeakable) {
            let suffix = String(finalSpeakable.dropFirst(streamedSpeakable.count))
            if !suffix.isEmpty {
                pendingText += suffix
            }
            pendingText += consumeMarkdown("", isFinal: true)
        } else {
            detectedTerminalRewrite = !streamedSpeakable.isEmpty
                && !finalSpeakable.isEmpty
                && streamedSpeakable != finalSpeakable
            pendingText += consumeMarkdown("", isFinal: true)
        }

        var segments = drainCompletedSegments()
        let tail = pendingText.trimmingCharacters(in: .whitespacesAndNewlines)
        pendingText = ""
        if !tail.isEmpty { segments.append(tail) }
        return segments
    }

    static func sanitize(_ text: String) -> String {
        var segmenter = StreamingSpeechSegmenter()
        return segmenter.consumeMarkdown(text, isFinal: true)
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private mutating func consumeMarkdown(_ delta: String, isFinal: Bool) -> String {
        let characters = Array(markdownCarry + delta)
        markdownCarry = ""
        var output = ""
        var index = 0

        while index < characters.count {
            if characters[index] == "`" {
                var runEnd = index
                while runEnd < characters.count, characters[runEnd] == "`" {
                    runEnd += 1
                }
                let count = runEnd - index
                if !isFinal, runEnd == characters.count, count < 3 {
                    markdownCarry = String(repeating: "`", count: count)
                    break
                }
                if count >= 3 {
                    isInsideFencedCode.toggle()
                }
                // Backtick markers are formatting, not spoken content. Inline
                // code content remains prose-compatible; fenced blocks do not.
                index = runEnd
                continue
            }

            if !isInsideFencedCode {
                output.append(characters[index])
            }
            index += 1
        }

        if isFinal, !markdownCarry.isEmpty, !isInsideFencedCode {
            markdownCarry = ""
        }
        return output
    }

    private mutating func drainCompletedSegments() -> [String] {
        var segments: [String] = []
        while let length = nextBoundaryLength() {
            let boundary = pendingText.index(pendingText.startIndex, offsetBy: length)
            let raw = String(pendingText[..<boundary])
            pendingText.removeSubrange(..<boundary)
            let segment = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            if !segment.isEmpty { segments.append(segment) }
        }
        return segments
    }

    private func nextBoundaryLength() -> Int? {
        let characters = Array(pendingText)
        guard characters.count >= minimumSegmentLength else { return nil }
        let scanLimit = min(characters.count, hardBoundaryThreshold)

        for (offset, character) in characters.prefix(scanLimit).enumerated()
        where Self.strongBoundaries.contains(character) && offset + 1 >= minimumSegmentLength {
            return offset + 1
        }

        if characters.count >= softBoundaryThreshold {
            var softLength: Int?
            for (offset, character) in characters.prefix(scanLimit).enumerated() {
                if offset + 1 >= minimumSegmentLength,
                   Self.softBoundaries.contains(character) {
                    softLength = offset + 1
                }
            }
            if let softLength { return softLength }
        }

        guard characters.count >= hardBoundaryThreshold else { return nil }
        var fallbackLength: Int?
        for (offset, character) in characters.prefix(scanLimit).enumerated() {
            if offset + 1 >= minimumSegmentLength,
               Self.fallbackBoundaries.contains(character) {
                fallbackLength = offset + 1
            }
        }
        return fallbackLength ?? hardBoundaryThreshold
    }
}
