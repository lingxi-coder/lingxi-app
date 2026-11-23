import Foundation

/// A selected member's inline display text. Offsets use UTF-16, as on the wire.
struct TeamMentionEntity: Codable, Equatable, Sendable {
    let userId: String
    var offset: Int
    let length: Int

    static let limit = 200
    private static let query = try! NSRegularExpression(pattern: #"(^|[\s(（])@([^\r\n@]{0,120})\z"#)

    static func at(_ text: String, caret: Int, entities: [Self]) -> MentionAt? {
        let before = String(decoding: text.utf16.prefix(caret), as: UTF16.self) as NSString
        guard let match = query.firstMatch(in: before as String, range: NSRange(location: 0, length: before.length)) else { return nil }
        let range = match.range(at: 2), start = range.location - 1
        guard !entities.contains(where: { start >= $0.offset && start < $0.offset + $0.length }) else { return nil }
        return MentionAt(start: start, query: before.substring(with: range))
    }

    static func searchKey(_ value: String) -> String { JSText.trim(JSText.collapseSpaces(value)).lowercased() }

    static func replacing(_ entities: [Self], start: Int, end: Int, insertedLength: Int) -> [Self] {
        let delta = insertedLength - (end - start)
        return entities.compactMap { entity in
            if entity.offset + entity.length <= start { return entity }
            guard entity.offset >= end else { return nil }
            var moved = entity
            moved.offset += delta
            return moved
        }
    }

    /// Prefer verified editor coordinates to the conservative fallback for ambiguous text.
    static func shifted(_ entities: [Self], from before: String, to after: String,
                        selection: Range<Int>? = nil, caretAfter: Int? = nil) -> [Self] {
        guard !entities.isEmpty else { return [] }
        let old = Array(before.utf16), new = Array(after.utf16)
        let delta = new.count - old.count
        func fits(_ start: Int, _ end: Int) -> Bool {
            let inserted = end - start + delta
            return start >= 0 && end >= start && end <= old.count && inserted >= 0
                && old.prefix(start).elementsEqual(new.prefix(start))
                && old.dropFirst(end).elementsEqual(new.dropFirst(start + inserted))
        }
        if let selection, caretAfter == nil || selection.upperBound + delta == caretAfter,
           fits(selection.lowerBound, selection.upperBound) {
            return replacing(entities, start: selection.lowerBound, end: selection.upperBound, insertedLength: selection.count + delta)
        }
        if let caretAfter, delta != 0 {
            let start = caretAfter - max(0, delta), end = caretAfter - delta
            if fits(start, end) { return replacing(entities, start: start, end: end, insertedLength: max(0, delta)) }
        }
        guard old != new else { return entities }
        let common = min(old.count, new.count)
        var prefix = 0, suffix = 0
        while prefix < common, old[prefix] == new[prefix] { prefix += 1 }
        while suffix < common, old[old.count - suffix - 1] == new[new.count - suffix - 1] { suffix += 1 }
        let start = min(prefix, common - suffix), end = old.count - min(suffix, common - prefix)
        return replacing(entities, start: start, end: end, insertedLength: end - start + delta)
    }

    static func replacementEnd(_ text: String, start: Int, caret: Int, name: String) -> Int {
        let body = text as NSString
        func key(through end: Int) -> String {
            searchKey(body.substring(with: NSRange(location: start + 1, length: end - start - 1)))
        }
        let typed = key(through: caret), target = searchKey(name)
        func startsNamePart(_ value: String) -> Bool { (" " + target).contains(" " + value) }
        guard !typed.isEmpty, typed != target, startsNamePart(typed) else { return caret }
        let tail = body.substring(from: caret)
        var end = caret, matched = caret
        for index in tail.indices {
            let char = tail[index]
            let next = tail.index(after: index)
            if char.isNewline || char == "@" { break }
            end += char.utf16.count
            if end > start + 1 + max(120, name.utf16.count) { break }
            if char.unicodeScalars.allSatisfy(AvatarIdentity.isSpace) { continue }
            let extended = key(through: end)
            guard startsNamePart(extended) else { break }
            if next == tail.endIndex || tail[next].isPunctuation || tail[next].unicodeScalars.allSatisfy(AvatarIdentity.isSpace) {
                matched = end
            }
            if extended == target { break }
        }
        return matched
    }

    static func valid(_ entities: [Self], in text: String) -> [Self] {
        guard !entities.isEmpty else { return [] }
        return valid(entities, in: Array(text.utf16))
    }

    static func valid(_ entities: [Self], in units: [UInt16]) -> [Self] {
        var end = 0
        guard entities.count <= limit else { return [] }
        for entity in entities {
            guard entity.offset >= end, entity.offset < units.count, entity.length >= 2, entity.length <= units.count - entity.offset,
                  units[entity.offset] == 64 else { return [] }
            end = entity.offset + entity.length
            if end < units.count, (0xDC00...0xDFFF).contains(units[end]) { return [] }
        }
        return entities
    }

    var json: [String: Any] { ["userId": userId, "offset": offset, "length": length] }
}
