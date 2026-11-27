import Foundation

enum TeamTextStyle: Int, CaseIterable {
    case bold = 1, italic = 2, underline = 4, strike = 8

    var title: String {
        switch self {
        case .bold: "teams_format_bold"
        case .italic: "teams_format_italic"
        case .underline: "teams_format_underline"
        case .strike: "teams_format_strike"
        }
    }

    var symbol: String {
        switch self {
        case .bold: "bold"
        case .italic: "italic"
        case .underline: "underline"
        case .strike: "strikethrough"
        }
    }
}

/// The wire format uses UTF-16 offsets so the same runs can be used in every client.
struct TeamMessageFormat: Codable, Equatable, Sendable {
    static let limit = 256
    let offset: Int
    var length: Int
    let style: Int

    var end: Int { offset + length }
    var json: [String: Int] { ["offset": offset, "length": length, "style": style] }

    static func isValid(_ runs: [Self], in text: String) -> Bool {
        if runs.isEmpty { return true }
        guard runs.count <= limit else { return false }
        let units = Array(text.utf16)
        var previousEnd = 0
        for run in runs {
            guard (1...15).contains(run.style), run.offset >= previousEnd, run.offset <= units.count,
                  run.length > 0, run.length <= units.count - run.offset,
                  !splitsSurrogate(units, run.offset), !splitsSurrogate(units, run.end) else { return false }
            previousEnd = run.end
        }
        return true
    }

    private static func splitsSurrogate(_ units: [UInt16], _ index: Int) -> Bool {
        index > 0 && index < units.count && (0xd800...0xdbff).contains(units[index - 1]) &&
            (0xdc00...0xdfff).contains(units[index])
    }

    private static func merged(_ runs: [Self]) -> [Self] {
        var result: [Self] = []
        for run in runs.sorted(by: { $0.offset < $1.offset }) {
            if let last = result.last, last.end == run.offset, last.style == run.style {
                result[result.count - 1].length += run.length
            } else {
                result.append(run)
            }
        }
        return result
    }

    static func replacing(_ runs: [Self], range: Range<Int>, count: Int, insertedStyle: Int = 0) -> [Self] {
        let shift = count - range.count
        var parts: [Self] = []
        for run in runs {
            let leftEnd = min(range.lowerBound, run.end)
            if leftEnd > run.offset { parts.append(.init(offset: run.offset, length: leftEnd - run.offset, style: run.style)) }
            let rightStart = max(range.upperBound, run.offset)
            if run.end > rightStart { parts.append(.init(offset: rightStart + shift, length: run.end - rightStart, style: run.style)) }
        }
        if count > 0, insertedStyle != 0 { parts.append(.init(offset: range.lowerBound, length: count, style: insertedStyle)) }
        return merged(parts)
    }

    static func style(at caret: Int, in runs: [Self]) -> Int {
        runs.first { caret > $0.offset && caret <= $0.end }?.style ?? 0
    }

    static func toggling(_ bit: Int, in range: Range<Int>, runs: [Self]) -> [Self] {
        let boundaries = Set(runs.flatMap { [$0.offset, $0.end] } + [range.lowerBound, range.upperBound]).sorted()
        let parts = zip(boundaries, boundaries.dropFirst()).map { start, end in
            Self(offset: start, length: end - start, style: style(at: end, in: runs))
        }
        let remove = parts.filter { $0.offset >= range.lowerBound && $0.end <= range.upperBound }
            .allSatisfy { $0.style & bit != 0 }
        return merged(parts.compactMap { part in
            let inside = part.offset >= range.lowerBound && part.end <= range.upperBound
            let updated = inside ? (remove ? part.style & ~bit : part.style | bit) : part.style
            return updated == 0 ? nil : Self(offset: part.offset, length: part.length, style: updated)
        })
    }

    static func editRange(from before: String, to after: String) -> Range<Int> {
        let old = Array(before.utf16), new = Array(after.utf16), size = min(old.count, new.count)
        var start = 0, suffix = 0
        while start < size, old[start] == new[start] { start += 1 }
        while suffix < size - start, old[old.count - suffix - 1] == new[new.count - suffix - 1] { suffix += 1 }
        if splitsSurrogate(old, start) || splitsSurrogate(new, start) { start -= 1 }
        if splitsSurrogate(old, old.count - suffix) || splitsSurrogate(new, new.count - suffix) { suffix -= 1 }
        return start..<(old.count - suffix)
    }
}
