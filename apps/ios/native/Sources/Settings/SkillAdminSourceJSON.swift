import Foundation

/// A JavaScript string kept as its original UTF-16 code units. `displayValue`
/// is for text rendering only; identity and protocol requests use `codeUnits`.
struct SkillAdminSourceText: Hashable {
    let codeUnits: [UInt16]

    var displayValue: String { String(decoding: codeUnits, as: UTF16.self) }
}

struct SkillAdminIdentity: Hashable, Identifiable {
    let codeUnits: [UInt16]

    init(codeUnits: [UInt16]) {
        self.codeUnits = codeUnits
    }

    init(_ sourceText: SkillAdminSourceText) {
        self.codeUnits = sourceText.codeUnits
    }

    /// A stable SwiftUI identity that cannot collapse an isolated surrogate
    /// onto the replacement character's identity.
    var id: String {
        "utf16:" + codeUnits.map { String($0, radix: 16) }.joined(separator: "-")
    }

    var displayValue: String { SkillAdminSourceText(codeUnits: codeUnits).displayValue }
}

struct SkillAdminCatalogEntry: Equatable, Identifiable {
    let identity: SkillAdminIdentity
    let name: SkillAdminSourceText
    let source: String
    let writable: Bool
    let revision: String?
    let readonlyReason: String?

    var id: String { identity.id }
}

struct SkillAdminTrashEntry: Equatable, Identifiable {
    let identity: SkillAdminIdentity
    let name: SkillAdminSourceText
    let source: String
    let revision: String?

    var id: String { identity.id }
}

struct SkillAdminCatalog: Equatable {
    let entries: [SkillAdminCatalogEntry]
    let trash: [SkillAdminTrashEntry]

    static func parse(_ json: String) -> Self? {
        guard
            case let .object(root)? = SkillAdminJSONParser.parse(json),
            case let .array(rawEntries)? = root["entries"],
            case let .array(rawTrash)? = root["trash"]
        else { return nil }

        var entries: [SkillAdminCatalogEntry] = []
        entries.reserveCapacity(rawEntries.count)
        for value in rawEntries {
            guard let entry = SkillAdminCatalogEntry.parse(value) else { return nil }
            entries.append(entry)
        }

        var trash: [SkillAdminTrashEntry] = []
        trash.reserveCapacity(rawTrash.count)
        for value in rawTrash {
            guard let entry = SkillAdminTrashEntry.parse(value) else { return nil }
            trash.append(entry)
        }
        return Self(entries: entries, trash: trash)
    }
}

struct SkillAdminDocument: Equatable {
    let identity: SkillAdminIdentity
    let name: SkillAdminSourceText
    let source: String
    let markdown: String
    let writable: Bool
    let revision: String?
    let readonlyReason: String?

    static func parse(_ json: String) -> Self? {
        guard
            case let .object(root)? = SkillAdminJSONParser.parse(json),
            let identity = root["id"]?.sourceText.map(SkillAdminIdentity.init),
            let name = root["name"]?.sourceText,
            let source = root["source"]?.displayString,
            let markdown = root["markdown"]?.displayString,
            let writable = root["writable"]?.booleanValue,
            let revision = root.optionalDisplayString(forKey: "revision"),
            let readonlyReason = root.optionalDisplayString(forKey: "readonlyReason")
        else { return nil }
        return Self(
            identity: identity,
            name: name,
            source: source,
            markdown: markdown,
            writable: writable,
            revision: revision,
            readonlyReason: readonlyReason
        )
    }

    func matches(selectedIdentity: SkillAdminIdentity?) -> Bool {
        selectedIdentity.map { $0 == identity } ?? true
    }
}

private struct SkillAdminJSONKey: Hashable {
    let codeUnits: [UInt16]

    init(_ value: String) {
        codeUnits = Array(value.utf16)
    }

    init(codeUnits: [UInt16]) {
        self.codeUnits = codeUnits
    }
}

private extension Dictionary where Key == SkillAdminJSONKey, Value == SkillAdminJSONValue {
    subscript(key: String) -> SkillAdminJSONValue? {
        self[SkillAdminJSONKey(key)]
    }

    func optionalDisplayString(forKey key: String) -> String?? {
        guard let value = self[key] else { return .some(nil) }
        return value.optionalDisplayString
    }
}

private indirect enum SkillAdminJSONValue {
    case object([SkillAdminJSONKey: SkillAdminJSONValue])
    case array([SkillAdminJSONValue])
    case string(SkillAdminSourceText)
    case number(String)
    case boolean(Bool)
    case null

    subscript(key: String) -> SkillAdminJSONValue? {
        guard case let .object(fields) = self else { return nil }
        return fields[SkillAdminJSONKey(key)]
    }

    var sourceText: SkillAdminSourceText? {
        guard case let .string(value) = self else { return nil }
        return value
    }

    var displayString: String? { sourceText?.displayValue }

    var booleanValue: Bool? {
        guard case let .boolean(value) = self else { return nil }
        return value
    }

    var optionalDisplayString: String?? {
        switch self {
        case let .string(value): .some(value.displayValue)
        case .null: .some(nil)
        default: nil
        }
    }

    func optionalDisplayString(forKey key: String) -> String?? {
        guard let value = self[key] else { return .some(nil) }
        return value.optionalDisplayString
    }
}

private extension SkillAdminCatalogEntry {
    static func parse(_ value: SkillAdminJSONValue) -> Self? {
        guard
            let identity = value["id"]?.sourceText.map(SkillAdminIdentity.init),
            let name = value["name"]?.sourceText,
            let source = value["source"]?.displayString,
            let writable = value["writable"]?.booleanValue,
            let revision = value.optionalDisplayString(forKey: "revision"),
            let readonlyReason = value.optionalDisplayString(forKey: "readonlyReason")
        else { return nil }
        return Self(
            identity: identity,
            name: name,
            source: source,
            writable: writable,
            revision: revision,
            readonlyReason: readonlyReason
        )
    }
}

private extension SkillAdminTrashEntry {
    static func parse(_ value: SkillAdminJSONValue) -> Self? {
        guard
            let identity = value["trashId"]?.sourceText.map(SkillAdminIdentity.init),
            let name = value["name"]?.sourceText,
            let source = value["source"]?.displayString,
            let revision = value.optionalDisplayString(forKey: "revision")
        else { return nil }
        return Self(identity: identity, name: name, source: source, revision: revision)
    }
}

/// Strict JSON parser for Skill Admin payloads. Unlike Foundation's JSON
/// decoders, it keeps each `\\uXXXX` escape as its individual UTF-16 unit.
/// Object fields use last-value-wins semantics, matching the host's JSON
/// projection and preserving the established duplicate-member policy.
private enum SkillAdminJSONParser {
    static func parse(_ json: String) -> SkillAdminJSONValue? {
        var parser = Parser(bytes: Array(json.utf8))
        guard let value = parser.parseValue(depth: 0) else { return nil }
        parser.skipWhitespace()
        return parser.isAtEnd ? value : nil
    }

    private struct Parser {
        private static let maximumNestingDepth = 256
        let bytes: [UInt8]
        var offset = 0

        var isAtEnd: Bool { offset == bytes.count }

        mutating func parseValue(depth: Int) -> SkillAdminJSONValue? {
            guard depth <= Self.maximumNestingDepth else { return nil }
            skipWhitespace()
            guard let byte = peek else { return nil }
            switch byte {
            case 0x7b: return parseObject(depth: depth)
            case 0x5b: return parseArray(depth: depth)
            case 0x22: return parseString().map { .string(SkillAdminSourceText(codeUnits: $0)) }
            case 0x74: return parseLiteral("true", value: .boolean(true))
            case 0x66: return parseLiteral("false", value: .boolean(false))
            case 0x6e: return parseLiteral("null", value: .null)
            case 0x2d, 0x30...0x39: return parseNumber().map(SkillAdminJSONValue.number)
            default: return nil
            }
        }

        mutating func parseObject(depth: Int) -> SkillAdminJSONValue? {
            guard consume(0x7b) else { return nil }
            skipWhitespace()
            if consume(0x7d) { return .object([:]) }

            var fields: [SkillAdminJSONKey: SkillAdminJSONValue] = [:]
            while true {
                guard let keyUnits = parseString() else { return nil }
                skipWhitespace()
                guard consume(0x3a), let value = parseValue(depth: depth + 1) else { return nil }
                fields[SkillAdminJSONKey(codeUnits: keyUnits)] = value
                skipWhitespace()
                if consume(0x7d) { return .object(fields) }
                guard consume(0x2c) else { return nil }
                skipWhitespace()
            }
        }

        mutating func parseArray(depth: Int) -> SkillAdminJSONValue? {
            guard consume(0x5b) else { return nil }
            skipWhitespace()
            if consume(0x5d) { return .array([]) }

            var values: [SkillAdminJSONValue] = []
            while true {
                guard let value = parseValue(depth: depth + 1) else { return nil }
                values.append(value)
                skipWhitespace()
                if consume(0x5d) { return .array(values) }
                guard consume(0x2c) else { return nil }
                skipWhitespace()
            }
        }

        mutating func parseString() -> [UInt16]? {
            guard consume(0x22) else { return nil }
            var units: [UInt16] = []
            while let byte = peek {
                offset += 1
                switch byte {
                case 0x22:
                    return units
                case 0x5c:
                    guard let escaped = peek else { return nil }
                    offset += 1
                    switch escaped {
                    case 0x22, 0x5c, 0x2f: units.append(UInt16(escaped))
                    case 0x62: units.append(0x0008)
                    case 0x66: units.append(0x000c)
                    case 0x6e: units.append(0x000a)
                    case 0x72: units.append(0x000d)
                    case 0x74: units.append(0x0009)
                    case 0x75:
                        guard let unit = parseHexUnit() else { return nil }
                        units.append(unit)
                    default: return nil
                    }
                case 0x00...0x1f:
                    return nil
                case 0x20...0x7f:
                    units.append(UInt16(byte))
                default:
                    guard let (scalar, length) = decodeUTF8Scalar(at: offset - 1) else { return nil }
                    offset = offset - 1 + length
                    appendUTF16(scalar, to: &units)
                }
            }
            return nil
        }

        mutating func parseHexUnit() -> UInt16? {
            guard offset + 4 <= bytes.count else { return nil }
            var value: UInt16 = 0
            for byte in bytes[offset..<(offset + 4)] {
                let digit: UInt16
                switch byte {
                case 0x30...0x39: digit = UInt16(byte - 0x30)
                case 0x41...0x46: digit = UInt16(byte - 0x41 + 10)
                case 0x61...0x66: digit = UInt16(byte - 0x61 + 10)
                default: return nil
                }
                value = (value << 4) | digit
            }
            offset += 4
            return value
        }

        mutating func parseNumber() -> String? {
            let start = offset
            _ = consume(0x2d)
            guard let first = peek else { return nil }
            if first == 0x30 {
                offset += 1
                if let next = peek, (0x30...0x39).contains(next) { return nil }
            } else if (0x31...0x39).contains(first) {
                offset += 1
                while let next = peek, (0x30...0x39).contains(next) { offset += 1 }
            } else {
                return nil
            }

            if consume(0x2e) {
                guard let digit = peek, (0x30...0x39).contains(digit) else { return nil }
                while let digit = peek, (0x30...0x39).contains(digit) { offset += 1 }
            }
            if let marker = peek, marker == 0x65 || marker == 0x45 {
                offset += 1
                if let sign = peek, sign == 0x2b || sign == 0x2d { offset += 1 }
                guard let digit = peek, (0x30...0x39).contains(digit) else { return nil }
                while let digit = peek, (0x30...0x39).contains(digit) { offset += 1 }
            }
            return String(decoding: bytes[start..<offset], as: UTF8.self)
        }

        mutating func parseLiteral(_ literal: String, value: SkillAdminJSONValue) -> SkillAdminJSONValue? {
            let expected = Array(literal.utf8)
            guard offset + expected.count <= bytes.count,
                  bytes[offset..<(offset + expected.count)].elementsEqual(expected) else { return nil }
            offset += expected.count
            return value
        }

        func decodeUTF8Scalar(at start: Int) -> (UInt32, Int)? {
            let first = bytes[start]
            let length: Int
            var scalar: UInt32
            switch first {
            case 0xc2...0xdf:
                length = 2
                scalar = UInt32(first & 0x1f)
            case 0xe0...0xef:
                length = 3
                scalar = UInt32(first & 0x0f)
            case 0xf0...0xf4:
                length = 4
                scalar = UInt32(first & 0x07)
            default:
                return nil
            }
            guard start + length <= bytes.count else { return nil }
            for index in (start + 1)..<(start + length) {
                let continuation = bytes[index]
                guard (0x80...0xbf).contains(continuation) else { return nil }
                if index == start + 1 {
                    if first == 0xe0 && continuation < 0xa0 { return nil }
                    if first == 0xed && continuation > 0x9f { return nil }
                    if first == 0xf0 && continuation < 0x90 { return nil }
                    if first == 0xf4 && continuation > 0x8f { return nil }
                }
                scalar = (scalar << 6) | UInt32(continuation & 0x3f)
            }
            return (scalar, length)
        }

        mutating func skipWhitespace() {
            while let byte = peek, byte == 0x20 || byte == 0x09 || byte == 0x0a || byte == 0x0d {
                offset += 1
            }
        }

        var peek: UInt8? { bytes.indices.contains(offset) ? bytes[offset] : nil }

        mutating func consume(_ byte: UInt8) -> Bool {
            guard peek == byte else { return false }
            offset += 1
            return true
        }
    }

    private static func appendUTF16(_ scalar: UInt32, to units: inout [UInt16]) {
        if scalar <= 0xffff {
            units.append(UInt16(scalar))
        } else {
            let value = scalar - 0x10000
            units.append(UInt16(0xd800 + (value >> 10)))
            units.append(UInt16(0xdc00 + (value & 0x3ff)))
        }
    }
}
