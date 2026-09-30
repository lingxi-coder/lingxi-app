import Foundation
import SwiftUI

/// One font for the whole terminal.
///
/// `renderedString` stamps a font onto every run of its `AttributedString`, and
/// a run-level font attribute beats the enclosing `.font()` modifier — so a
/// view that sets its own size silently loses to whatever the buffer wrote.
/// That is exactly what happened when the screen moved to 13pt and the buffer
/// kept `.body`: the shell's prompt tail and the caret share one row in
/// `promptRow`, and they were rendering at two different sizes. Both sides read
/// this constant now, so there is one place to change and no way to disagree.
enum TerminalTypography {
    /// `.footnote`, not `size: 13`: they are the same 13pt at the default
    /// content size, but the text style follows Dynamic Type while the
    /// literal ignores the user's accessibility text sizes entirely — a
    /// regression from the `.body`-relative fonts this replaced, on exactly
    /// the screen whose whole job is reading long output (and whose failure
    /// notices and restart link are rendered in this font too).
    static let font = Font.system(.footnote, design: .monospaced)
    static let boldFont = Font.system(.footnote, design: .monospaced).bold()
}

enum TerminalNamedColor: Int, CaseIterable, Hashable, Sendable {
    case black = 0
    case red = 1
    case green = 2
    case yellow = 3
    case blue = 4
    case magenta = 5
    case cyan = 6
    case white = 7
    case brightBlack = 8
    case brightRed = 9
    case brightGreen = 10
    case brightYellow = 11
    case brightBlue = 12
    case brightMagenta = 13
    case brightCyan = 14
    case brightWhite = 15

    fileprivate var color: Color {
        switch self {
        case .black: return Color(red: 0.10, green: 0.11, blue: 0.12)
        case .red: return Color(red: 0.76, green: 0.25, blue: 0.25)
        case .green: return Color(red: 0.35, green: 0.63, blue: 0.28)
        case .yellow: return Color(red: 0.76, green: 0.62, blue: 0.22)
        case .blue: return Color(red: 0.25, green: 0.44, blue: 0.77)
        case .magenta: return Color(red: 0.67, green: 0.35, blue: 0.73)
        case .cyan: return Color(red: 0.25, green: 0.64, blue: 0.74)
        case .white: return Color(red: 0.84, green: 0.85, blue: 0.86)
        case .brightBlack: return Color(red: 0.36, green: 0.40, blue: 0.44)
        case .brightRed: return Color(red: 0.97, green: 0.44, blue: 0.42)
        case .brightGreen: return Color(red: 0.56, green: 0.81, blue: 0.35)
        case .brightYellow: return Color(red: 0.96, green: 0.81, blue: 0.33)
        case .brightBlue: return Color(red: 0.44, green: 0.64, blue: 0.98)
        case .brightMagenta: return Color(red: 0.86, green: 0.56, blue: 0.97)
        case .brightCyan: return Color(red: 0.46, green: 0.84, blue: 0.95)
        case .brightWhite: return Color(red: 0.97, green: 0.98, blue: 0.99)
        }
    }
}

enum TerminalColorValue: Hashable, Sendable {
    case named(TerminalNamedColor)
    case indexed(UInt8)
    case rgb(UInt8, UInt8, UInt8)

    fileprivate var color: Color {
        switch self {
        case let .named(named):
            return named.color
        case let .rgb(r, g, b):
            return Color(
                red: Double(r) / 255.0,
                green: Double(g) / 255.0,
                blue: Double(b) / 255.0
            )
        case let .indexed(index):
            if let named = TerminalNamedColor(rawValue: Int(index)) {
                return named.color
            }
            if index >= 16, index <= 231 {
                let value = Int(index - 16)
                let r = value / 36
                let g = (value / 6) % 6
                let b = value % 6
                func channel(_ component: Int) -> Double {
                    component == 0 ? 0.0 : (Double(component) * 40.0 + 55.0) / 255.0
                }
                return Color(
                    red: channel(r),
                    green: channel(g),
                    blue: channel(b)
                )
            }
            let level = (Double(index) - 232.0) / 23.0
            let clamped = min(max(level, 0.0), 1.0)
            return Color(.sRGB, white: clamped, opacity: 1.0)
        }
    }
}

struct TerminalTextStyle: Hashable, Sendable {
    var foreground: TerminalColorValue?
    var background: TerminalColorValue?
    var bold = false
    var italic = false
    var underline = false
}

struct TerminalCell: Hashable, Sendable {
    var character: Character
    var style: TerminalTextStyle
}

struct TerminalBufferLine: Identifiable, Equatable, Sendable {
    let id: Int
    var cells: [TerminalCell] = []

    var text: String {
        String(cells.map(\.character))
    }

    var isEmpty: Bool {
        cells.isEmpty
    }

    func renderedString(defaultForeground: Color = .primary) -> AttributedString {
        var rendered = AttributedString()
        var segment = ""
        var currentStyle: TerminalTextStyle?

        func flush() {
            guard !segment.isEmpty else { return }
            var attributed = AttributedString(segment)
            if let currentStyle {
                attributed.foregroundColor = currentStyle.foreground?.color ?? defaultForeground
                if let background = currentStyle.background?.color {
                    attributed.backgroundColor = background
                }
                if currentStyle.bold {
                    attributed.font = TerminalTypography.boldFont
                } else {
                    attributed.font = TerminalTypography.font
                }
                attributed.underlineStyle = currentStyle.underline ? .single : .init(rawValue: 0)
                attributed.inlinePresentationIntent = currentStyle.italic ? .emphasized : nil
            } else {
                attributed.foregroundColor = defaultForeground
                attributed.font = TerminalTypography.font
            }
            rendered += attributed
            segment.removeAll(keepingCapacity: true)
        }

        for cell in cells {
            if currentStyle != cell.style {
                flush()
                currentStyle = cell.style
            }
            segment.append(cell.character)
        }
        flush()
        if rendered.characters.isEmpty {
            var empty = AttributedString(" ")
            empty.foregroundColor = .clear
            empty.font = TerminalTypography.font
            return empty
        }
        return rendered
    }
}

struct TerminalScreenBuffer: Equatable, Sendable {
    private(set) var lines: [TerminalBufferLine]
    private(set) var cursorRow: Int
    private(set) var cursorColumn: Int
    private var nextLineID: Int
    let maxScrollback: Int

    init(maxScrollback: Int = 2_000) {
        self.lines = [TerminalBufferLine(id: 0)]
        self.cursorRow = 0
        self.cursorColumn = 0
        self.nextLineID = 1
        self.maxScrollback = max(1, maxScrollback)
    }

    var plainText: String {
        lines.map(\.text).joined(separator: "\n")
    }

    mutating func reset() {
        self = Self(maxScrollback: maxScrollback)
    }

    mutating func put(_ character: Character, style: TerminalTextStyle) {
        ensureCursorVisible()
        var line = lines[cursorRow]
        while line.cells.count < cursorColumn {
            line.cells.append(TerminalCell(character: " ", style: style))
        }
        let cell = TerminalCell(character: character, style: style)
        if cursorColumn < line.cells.count {
            line.cells[cursorColumn] = cell
        } else {
            line.cells.append(cell)
        }
        lines[cursorRow] = line
        cursorColumn += 1
    }

    mutating func newline() {
        cursorRow += 1
        cursorColumn = 0
        ensureCursorVisible()
    }

    mutating func carriageReturn() {
        cursorColumn = 0
    }

    mutating func backspace() {
        cursorColumn = max(0, cursorColumn - 1)
    }

    mutating func tab(style: TerminalTextStyle) {
        let nextStop = ((cursorColumn / 4) + 1) * 4
        while cursorColumn < nextStop {
            put(" ", style: style)
        }
    }

    mutating func moveCursor(rows deltaRows: Int = 0, columns deltaColumns: Int = 0) {
        cursorRow = max(0, cursorRow + deltaRows)
        ensureCursorVisible()
        cursorColumn = max(0, cursorColumn + deltaColumns)
    }

    mutating func setCursor(row: Int? = nil, column: Int? = nil) {
        if let row {
            cursorRow = max(0, row)
        }
        ensureCursorVisible()
        if let column {
            cursorColumn = max(0, column)
        }
    }

    mutating func clearLine(mode: Int) {
        ensureCursorVisible()
        var line = lines[cursorRow]
        switch mode {
        case 1:
            guard !line.cells.isEmpty else { break }
            let end = min(cursorColumn, line.cells.count)
            if end > 0 {
                line.cells.replaceSubrange(0..<end, with: Array(repeating: TerminalCell(character: " ", style: TerminalTextStyle()), count: end))
            }
        case 2:
            line.cells.removeAll(keepingCapacity: true)
        default:
            if cursorColumn < line.cells.count {
                line.cells.removeSubrange(cursorColumn..<line.cells.count)
            }
        }
        lines[cursorRow] = line
    }

    mutating func clearScreen(mode: Int) {
        switch mode {
        case 2:
            reset()
        case 1:
            for row in 0..<min(cursorRow + 1, lines.count) {
                lines[row].cells.removeAll(keepingCapacity: true)
            }
        default:
            for row in cursorRow..<lines.count {
                if row == cursorRow {
                    clearLine(mode: 0)
                } else {
                    lines[row].cells.removeAll(keepingCapacity: true)
                }
            }
        }
    }

    private mutating func ensureCursorVisible() {
        while cursorRow >= lines.count {
            lines.append(TerminalBufferLine(id: nextLineID))
            nextLineID += 1
        }
        trimScrollbackIfNeeded()
    }

    private mutating func trimScrollbackIfNeeded() {
        while lines.count > maxScrollback {
            lines.removeFirst()
            cursorRow = max(0, cursorRow - 1)
        }
    }
}

struct TerminalUTF8Decoder: Sendable {
    private var pending: [UInt8] = []

    mutating func append(_ data: Data) -> String {
        pending.append(contentsOf: data)
        var output = String.UnicodeScalarView()
        var index = 0

        while index < pending.count {
            let result = decodeScalar(startingAt: index)
            switch result {
            case let .scalar(scalar, consumed):
                output.append(scalar)
                index += consumed
            case let .replacement(consumed):
                output.append("\u{FFFD}")
                index += consumed
            case .incomplete:
                pending.removeFirst(index)
                return String(output)
            }
        }

        pending.removeAll(keepingCapacity: true)
        return String(output)
    }

    private enum DecodeResult {
        case scalar(UnicodeScalar, consumed: Int)
        case replacement(consumed: Int)
        case incomplete
    }

    private func decodeScalar(startingAt index: Int) -> DecodeResult {
        let byte = pending[index]
        if byte < 0x80 {
            return .scalar(UnicodeScalar(byte), consumed: 1)
        }
        if byte < 0xC2 {
            return .replacement(consumed: 1)
        }

        func continuation(_ offset: Int) -> UInt8? {
            let target = index + offset
            guard target < pending.count else { return nil }
            let value = pending[target]
            return (value & 0xC0) == 0x80 ? value : nil
        }

        if byte <= 0xDF {
            guard let b1 = continuation(1) else {
                return index + 1 < pending.count ? .replacement(consumed: 1) : .incomplete
            }
            let value = ((UInt32(byte & 0x1F)) << 6) | UInt32(b1 & 0x3F)
            guard let scalar = UnicodeScalar(value) else { return .replacement(consumed: 2) }
            return .scalar(scalar, consumed: 2)
        }

        if byte <= 0xEF {
            guard let b1 = continuation(1) else {
                return index + 1 < pending.count ? .replacement(consumed: 1) : .incomplete
            }
            guard let b2 = continuation(2) else {
                return index + 2 < pending.count ? .replacement(consumed: 1) : .incomplete
            }
            if byte == 0xE0, b1 < 0xA0 { return .replacement(consumed: 1) }
            if byte == 0xED, b1 >= 0xA0 { return .replacement(consumed: 1) }
            let value =
                (UInt32(byte & 0x0F) << 12)
                | (UInt32(b1 & 0x3F) << 6)
                | UInt32(b2 & 0x3F)
            guard let scalar = UnicodeScalar(value) else { return .replacement(consumed: 3) }
            return .scalar(scalar, consumed: 3)
        }

        guard byte <= 0xF4 else {
            return .replacement(consumed: 1)
        }
        guard let b1 = continuation(1) else {
            return index + 1 < pending.count ? .replacement(consumed: 1) : .incomplete
        }
        guard let b2 = continuation(2) else {
            return index + 2 < pending.count ? .replacement(consumed: 1) : .incomplete
        }
        guard let b3 = continuation(3) else {
            return index + 3 < pending.count ? .replacement(consumed: 1) : .incomplete
        }
        if byte == 0xF0, b1 < 0x90 { return .replacement(consumed: 1) }
        if byte == 0xF4, b1 >= 0x90 { return .replacement(consumed: 1) }
        let value =
            (UInt32(byte & 0x07) << 18)
            | (UInt32(b1 & 0x3F) << 12)
            | (UInt32(b2 & 0x3F) << 6)
            | UInt32(b3 & 0x3F)
        guard let scalar = UnicodeScalar(value) else { return .replacement(consumed: 4) }
        return .scalar(scalar, consumed: 4)
    }
}

struct TerminalANSIParser: Sendable {
    private enum State: Equatable, Sendable {
        case ground
        case escape
        case csi(String)
    }

    private var state: State = .ground
    private(set) var style = TerminalTextStyle()

    mutating func consume(text: String, buffer: inout TerminalScreenBuffer) {
        for scalar in text.unicodeScalars {
            consume(scalar: scalar, buffer: &buffer)
        }
    }

    private mutating func consume(scalar: UnicodeScalar, buffer: inout TerminalScreenBuffer) {
        switch state {
        case .ground:
            switch scalar.value {
            case 0x1B:
                state = .escape
            case 0x08:
                buffer.backspace()
            case 0x09:
                buffer.tab(style: style)
            case 0x0A:
                buffer.newline()
            case 0x0D:
                buffer.carriageReturn()
            default:
                if !isControl(scalar) {
                    buffer.put(Character(scalar), style: style)
                }
            }
        case .escape:
            if scalar == "[" {
                state = .csi("")
            } else if scalar == "c" {
                style = TerminalTextStyle()
                buffer.reset()
                state = .ground
            } else {
                state = .ground
            }
        case let .csi(parameters):
            if scalar.value >= 0x40, scalar.value <= 0x7E {
                applyCSI(final: Character(scalar), parameters: parameters, buffer: &buffer)
                state = .ground
            } else {
                state = .csi(parameters + String(scalar))
            }
        }
    }

    private mutating func applyCSI(final: Character, parameters: String, buffer: inout TerminalScreenBuffer) {
        let parts = parameters.split(separator: ";", omittingEmptySubsequences: false).map { Int($0) ?? 0 }
        switch final {
        case "m":
            applySGR(parts)
        case "A":
            buffer.moveCursor(rows: -(parts.first ?? 1))
        case "B":
            buffer.moveCursor(rows: parts.first ?? 1)
        case "C":
            buffer.moveCursor(columns: parts.first ?? 1)
        case "D":
            buffer.moveCursor(columns: -(parts.first ?? 1))
        case "G":
            buffer.setCursor(column: max(0, (parts.first ?? 1) - 1))
        case "H", "f":
            let row = max(0, (parts.first ?? 1) - 1)
            let column = max(0, (parts.dropFirst().first ?? 1) - 1)
            buffer.setCursor(row: row, column: column)
        case "J":
            buffer.clearScreen(mode: parts.first ?? 0)
        case "K":
            buffer.clearLine(mode: parts.first ?? 0)
        default:
            break
        }
    }

    private mutating func applySGR(_ parts: [Int]) {
        let values = parts.isEmpty ? [0] : parts
        var index = 0
        while index < values.count {
            let value = values[index]
            switch value {
            case 0:
                style = TerminalTextStyle()
            case 1:
                style.bold = true
            case 3:
                style.italic = true
            case 4:
                style.underline = true
            case 22:
                style.bold = false
            case 23:
                style.italic = false
            case 24:
                style.underline = false
            case 30...37:
                style.foreground = .named(TerminalNamedColor(rawValue: value - 30) ?? .white)
            case 39:
                style.foreground = nil
            case 40...47:
                style.background = .named(TerminalNamedColor(rawValue: value - 40) ?? .black)
            case 49:
                style.background = nil
            case 90...97:
                style.foreground = .named(TerminalNamedColor(rawValue: value - 82) ?? .brightWhite)
            case 100...107:
                style.background = .named(TerminalNamedColor(rawValue: value - 92) ?? .brightBlack)
            case 38, 48:
                let isForeground = value == 38
                if index + 2 < values.count, values[index + 1] == 5 {
                    let color = TerminalColorValue.indexed(UInt8(clamping: values[index + 2]))
                    if isForeground {
                        style.foreground = color
                    } else {
                        style.background = color
                    }
                    index += 2
                } else if index + 4 < values.count, values[index + 1] == 2 {
                    let color = TerminalColorValue.rgb(
                        UInt8(clamping: values[index + 2]),
                        UInt8(clamping: values[index + 3]),
                        UInt8(clamping: values[index + 4])
                    )
                    if isForeground {
                        style.foreground = color
                    } else {
                        style.background = color
                    }
                    index += 4
                }
            default:
                break
            }
            index += 1
        }
    }

    private func isControl(_ scalar: UnicodeScalar) -> Bool {
        scalar.value < 0x20 || scalar.value == 0x7F
    }
}
