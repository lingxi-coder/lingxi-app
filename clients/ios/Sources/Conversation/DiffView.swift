// DiffView.swift — renders the engine's `StructuredDiffDto`.
//
// Everything here is presentation ONLY. The rows, their line numbers, their
// hunk indices, their word-diff emphasis and their per-run syntax classes were
// all derived once in Rust; this view never parses a patch, never splits a
// string, and never computes a line number.
//
// Three constraints that shape the layout:
//
//  1. Segments are PRE-SPLIT. An attributed line is built by APPENDING runs in
//     order — never by indexing the joined string. Rust indexes by UTF-8 byte
//     and Swift by grapheme, which is exactly why no offsets cross the wire.
//  2. ONE horizontal scroll view wraps ALL rows. Per-row scrolling would let
//     the gutters drift out of alignment the moment one row is wider.
//  3. Backgrounds are derived here from `kind` / `emph`. The terminal's are
//     alpha-over-black blends that are only valid over a black terminal, so
//     they are deliberately absent from the wire.

import SwiftUI

struct DiffView: View {
    @Environment(\.theme) private var t
    let diff: ConversationStructuredDiff
    /// Show the `filePath` + ±counts caption. Off inside `ToolCallView`, whose
    /// header already names the file.
    var showsFilePath: Bool = false

    private static let fontSize: CGFloat = 11.5
    private static let rowFont = Font.system(size: 11.5, design: .monospaced)

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if showsFilePath, let path = diff.filePath, !path.isEmpty {
                caption(path)
            }
            ScrollView(.horizontal, showsIndicators: false) {
                // A plain (non-lazy) VStack: it sizes to the widest row and then
                // lays every child out at that width, which is what lets a row
                // background span the full scrolled content instead of stopping
                // at its own text.
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(entries) { entry in
                        switch entry {
                        case .separator:
                            separatorRow
                        case let .row(_, row):
                            rowView(row)
                        }
                    }
                    if diff.truncatedRows > 0 {
                        truncatedRow
                    }
                }
                .padding(.vertical, 4)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, 2)
        .background(t.windowBg.opacity(0.55))
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(t.border, lineWidth: 0.5))
        .accessibilityElement(children: .contain)
        .accessibilityLabel(accessibilitySummary)
    }

    // MARK: rows

    /// One rendered entry: a real row, or the `⋯` drawn where the hunk index
    /// changed between two consecutive rows.
    private enum Entry: Identifiable {
        case separator(Int)
        case row(Int, ConversationDiffRow)

        var id: String {
            switch self {
            case let .separator(index): return "sep-\(index)"
            case let .row(index, _): return "row-\(index)"
            }
        }
    }

    private var entries: [Entry] {
        var out: [Entry] = []
        out.reserveCapacity(diff.rows.count + 2)
        var previousHunk: UInt32?
        for (index, row) in diff.rows.enumerated() {
            if let previousHunk, previousHunk != row.hunk {
                out.append(.separator(index))
            }
            out.append(.row(index, row))
            previousHunk = row.hunk
        }
        return out
    }

    private func rowView(_ row: ConversationDiffRow) -> some View {
        HStack(alignment: .top, spacing: 0) {
            Text(gutter(for: row))
                .font(Self.rowFont)
                .monospacedDigit()
                .foregroundColor(gutterColor(row.kind))
                .fixedSize()
            Text(sigil(row.kind))
                .font(Self.rowFont)
                .foregroundColor(gutterColor(row.kind))
                .fixedSize()
            Text(attributed(row))
                .font(Self.rowFont)
                .textSelection(.enabled)
                .fixedSize(horizontal: true, vertical: false)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 1)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(background(row.kind))
    }

    private var separatorRow: some View {
        Text("⋯")
            .font(Self.rowFont)
            .foregroundColor(t.text4)
            .padding(.horizontal, 8)
            .padding(.vertical, 1)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var truncatedRow: some View {
        Text("chat_diff_more_rows \(Int(diff.truncatedRows))")
            .font(.system(size: 11))
            .foregroundColor(t.text4)
            .padding(.horizontal, 8)
            .padding(.top, 3)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func caption(_ path: String) -> some View {
        HStack(spacing: 8) {
            Text(path)
                .font(.system(size: 11, weight: .medium, design: .monospaced))
                .foregroundColor(t.text3)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 4)
            if diff.additions > 0 {
                Text("+\(diff.additions)")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundColor(t.diffAddGutter)
            }
            if diff.removals > 0 {
                Text("-\(diff.removals)")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundColor(t.diffRemoveGutter)
            }
        }
        .padding(.horizontal, 8)
        .padding(.top, 4)
    }

    // MARK: text

    /// Build the row's attributed text by APPENDING each pre-split run. There is
    /// no string indexing anywhere in this function, deliberately.
    private func attributed(_ row: ConversationDiffRow) -> AttributedString {
        var out = AttributedString()
        for segment in row.segments {
            var piece = AttributedString(segment.text)
            piece.foregroundColor = SyntaxPalette.foreground(for: segment, in: t)
            var font = Self.rowFont
            if segment.bold { font = font.bold() }
            if segment.italic { font = font.italic() }
            piece.font = font
            if segment.underline { piece.underlineStyle = Text.LineStyle.single }
            if segment.emph {
                piece.backgroundColor = row.kind == .remove ? t.diffRemoveWordBg : t.diffAddWordBg
            }
            out.append(piece)
        }
        return out
    }

    /// Right-align the line number in a `gutterWidth`-wide monospaced column by
    /// padding the STRING, so alignment never depends on a measured glyph width.
    private func gutter(for row: ConversationDiffRow) -> String {
        let digits = String(row.lineNo)
        let width = max(Int(diff.gutterWidth), digits.count)
        return String(repeating: " ", count: width - digits.count) + digits + " "
    }

    private func sigil(_ kind: ConversationDiffLineKind) -> String {
        switch kind {
        case .add: return "+ "
        case .remove: return "- "
        case .context: return "  "
        }
    }

    private func background(_ kind: ConversationDiffLineKind) -> Color {
        switch kind {
        case .add: return t.diffAddBg
        case .remove: return t.diffRemoveBg
        case .context: return .clear
        }
    }

    private func gutterColor(_ kind: ConversationDiffLineKind) -> Color {
        switch kind {
        case .add: return t.diffAddGutter
        case .remove: return t.diffRemoveGutter
        case .context: return t.text4
        }
    }

    private var accessibilitySummary: String {
        let path = diff.filePath ?? ""
        return "\(path) +\(diff.additions) -\(diff.removals)"
    }
}
