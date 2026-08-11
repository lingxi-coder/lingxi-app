// ToolCallView.swift — one tool call, rendered from the engine's derivation.
//
// The engine ships a `ToolHeaderDto` on the call and a `ToolResultDisplayDto` on
// the result. This view renders THOSE. It must never re-parse `input_json` /
// `result_json` to rebuild a header — four clients each re-deriving the same
// presentation is the drift this whole change exists to delete.
//
// Layout mirrors the terminal:
//
//     ● Update(src/host.rs) (3 edits)        1.2s
//       $ cargo test --all
//       ⎿ Added 18 lines, removed 4 lines
//         <diff or body, collapsible>
//
// COLLAPSE STATE IS NOT HELD HERE. Every list this view lands in recycles its
// rows, so row-local `@State` is lost on scroll and reappears on the wrong row.
// The owner passes `isExpanded` from `ConversationModel.expandedToolCalls`,
// keyed by tool-use id, and gets a toggle callback back.

import SwiftUI

struct ToolCallView: View {
    @Environment(\.theme) private var t
    let trace: ConversationToolTrace
    var isExpanded: Bool = false
    var onToggle: () -> Void = {}

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            headerRow
            if let sub = trace.header?.subLine, !sub.text.isEmpty {
                subLineRow(sub)
            }
            resultBlock
            legacyFallback
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(t.surface)
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(t.border, lineWidth: 0.5))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("conversation.tool-call.\(trace.id)")
    }

    // MARK: header

    private var headerRow: some View {
        HStack(alignment: .center, spacing: 7) {
            Circle()
                .fill(statusColor)
                .frame(width: 6, height: 6)
            titleText
            Spacer(minLength: 4)
            if let elapsed = ConversationExecutionParsing.formatDuration(trace.elapsedMs) {
                Text(elapsed)
                    .font(.system(size: 11))
                    .foregroundColor(t.text4)
            }
        }
    }

    @ViewBuilder
    private var titleText: some View {
        if let header = trace.header {
            HStack(spacing: 0) {
                Text(ToolDisplayText.verbLabel(header))
                    .font(.system(size: 12.5, weight: .semibold))
                    .foregroundColor(t.text)
                    .layoutPriority(2)
                if let primary = header.primary, !primary.isEmpty {
                    Text("(")
                        .font(.system(size: 12.5))
                        .foregroundColor(t.text4)
                        .layoutPriority(2)
                    Text(primary)
                        .font(.system(size: 12.5))
                        .foregroundColor(t.text2)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .layoutPriority(0)
                    Text(")")
                        .font(.system(size: 12.5))
                        .foregroundColor(t.text4)
                        .layoutPriority(2)
                }
                if let qualifier = header.qualifier, !qualifier.isEmpty {
                    // The engine's qualifier carries its own leading space.
                    Text(qualifier)
                        .font(.system(size: 12))
                        .foregroundColor(t.text3)
                        .lineLimit(1)
                        .layoutPriority(1)
                }
            }
        } else {
            // Older engine: no header on the wire. Fall back to the raw name.
            Text(trace.tool)
                .font(.system(size: 12.5, weight: .semibold))
                .foregroundColor(t.text)
        }
    }

    private func subLineRow(_ sub: ConversationToolSubLine) -> some View {
        HStack(alignment: .top, spacing: 6) {
            Text(sub.prefix)
                .font(.system(size: 11.5, design: .monospaced))
                .foregroundColor(t.text4)
            Text(sub.text)
                .font(.system(size: 11.5, design: .monospaced))
                .foregroundColor(t.text2)
                .lineLimit(2)
                .textSelection(.enabled)
            Spacer(minLength: 0)
        }
        .padding(.leading, 13)
    }

    // MARK: result

    @ViewBuilder
    private var resultBlock: some View {
        if let display = trace.display {
            HStack(alignment: .top, spacing: 6) {
                Text("⎿")
                    .font(.system(size: 11.5, design: .monospaced))
                    .foregroundColor(t.text4)
                VStack(alignment: .leading, spacing: 5) {
                    if let headline = ToolDisplayText.headline(display) {
                        Text(headline)
                            .font(.system(size: 12))
                            .foregroundColor(trace.status == .failed ? t.danger : t.text3)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    if showsDetail {
                        if let diff = display.diff {
                            DiffView(diff: diff)
                        }
                        if let body = display.body, !body.isEmpty {
                            Text(body)
                                .font(.system(size: 11.5, design: .monospaced))
                                .foregroundColor(t.text2)
                                .fixedSize(horizontal: false, vertical: true)
                                .textSelection(.enabled)
                        }
                        if display.bodyTruncated {
                            Text("chat_tool_body_truncated")
                                .font(.system(size: 11))
                                .foregroundColor(t.text4)
                        }
                    }
                    if isCollapsible { disclosure(display) }
                }
                Spacer(minLength: 0)
            }
            .padding(.leading, 13)
        }
    }

    private func disclosure(_ display: ConversationToolResultDisplay) -> some View {
        Button(action: onToggle) {
            HStack(spacing: 4) {
                LXIcon(name: .chevron, size: 11, color: t.accent, stroke: 2)
                    .rotationEffect(.degrees(isExpanded ? 180 : 0))
                Text(isExpanded
                    ? String(localized: "chat_tool_show_less")
                    : String(localized: "chat_tool_show_more \(Int(display.bodyLines))"))
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundColor(t.accent)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("conversation.tool-call.\(trace.id).toggle")
    }

    /// `collapsed` is the engine's verdict that the body exceeds the inline
    /// budget. Nothing to hide ⇒ no affordance, whatever the flag says.
    private var isCollapsible: Bool {
        guard let display = trace.display, display.collapsed else { return false }
        return display.diff != nil || !(display.body ?? "").isEmpty
    }

    private var showsDetail: Bool { isExpanded || !isCollapsible }

    // MARK: legacy fallback (older engine — no header / no display)

    @ViewBuilder
    private var legacyFallback: some View {
        if trace.header == nil, let input = trace.inputSummary, !input.isEmpty {
            Text(input)
                .font(.system(size: 12.5))
                .foregroundColor(t.text2)
                .lineLimit(3)
        }
        if trace.display == nil, let output = trace.outputSummary, !output.isEmpty {
            Text(output)
                .font(.system(size: 12))
                .foregroundColor(t.text3)
                .lineLimit(4)
        }
    }

    private var statusColor: Color {
        switch trace.status {
        case .running: return t.accent
        case .completed: return t.ok
        case .failed: return t.danger
        case .cancelled: return t.text3
        }
    }
}

// MARK: - Localized presentation of the engine's derivation

/// Turns the engine's ENGLISH derivation into the user's language.
///
/// The wire ships `verb` + numeric args precisely so a localizing client never
/// has to show the English `label` / `headline`. Those raw strings are used only
/// where the engine deliberately overrode the verb with something no key can
/// express — a subagent type, `REPL`, `Web Search`, an MCP tool name — or where
/// the headline kind IS free text (`failed` / `plain`).
enum ToolDisplayText {

    /// The localized verb, or the engine's override when it set one.
    static func verbLabel(_ header: ConversationToolHeader) -> String {
        switch header.verb {
        case .shell:
            // `Bash`/`Shell`/`PowerShell` carry a count; `REPL` shares the verb
            // but overrides the label and has none.
            if let count = header.count {
                return String(localized: "chat_tool_verb_shell \(Int(count))")
            }
            return header.label
        case .generic:
            // Generic has no canonical English label — the tool name IS it.
            return header.label
        default:
            guard let canonical = canonicalEnglish(header.verb) else { return header.label }
            // A label that differs from the verb's canonical English is an
            // engine override (a proper noun); it must survive verbatim.
            return header.label == canonical ? localizedVerb(header.verb) : header.label
        }
    }

    /// The full one-line title, localized: `verb(primary)qualifier`.
    static func title(_ header: ConversationToolHeader) -> String {
        var out = verbLabel(header)
        if let primary = header.primary, !primary.isEmpty {
            out += "(\(primary))"
        }
        if let qualifier = header.qualifier, !qualifier.isEmpty {
            out += qualifier
        }
        return out
    }

    /// The localized `⎿` headline, or `nil` when the engine had nothing to say.
    static func headline(_ display: ConversationToolResultDisplay) -> String? {
        guard let kind = display.headlineKind else { return display.headline }
        let args = display.headlineArgs
        func arg(_ index: Int) -> Int { index < args.count ? Int(args[index]) : 0 }
        switch kind {
        case .added:
            return String(localized: "chat_result_added \(arg(0))")
        case .removed:
            return String(localized: "chat_result_removed \(arg(0))")
        case .addedRemoved:
            return String(localized: "chat_result_added_removed \(arg(0)) \(arg(1))")
        case .linesRead:
            return String(localized: "chat_result_lines_read \(arg(0))")
        case .linesReadPartial:
            return String(localized: "chat_result_lines_read_partial \(arg(0)) \(arg(1))")
        case .filesFound:
            return String(localized: "chat_result_files_found \(arg(0))")
        case .filesFoundTruncated:
            return String(localized: "chat_result_files_found_truncated \(arg(0))")
        case .linesFound:
            return String(localized: "chat_result_lines_found \(arg(0))")
        case .matchesFound:
            return String(localized: "chat_result_matches_found \(arg(0))")
        case .interrupted:
            return String(localized: "chat_result_interrupted")
        case .noContent:
            return String(localized: "chat_result_no_content")
        case .failed, .plain:
            // Free text produced by the tool itself — there is nothing to key on.
            return display.headline
        }
    }

    private static func localizedVerb(_ verb: ConversationToolVerb) -> String {
        switch verb {
        case .update: return String(localized: "chat_tool_verb_update")
        case .create: return String(localized: "chat_tool_verb_create")
        case .read: return String(localized: "chat_tool_verb_read")
        case .search: return String(localized: "chat_tool_verb_search")
        case .shell: return String(localized: "chat_tool_verb_shell \(1)")
        case .output: return String(localized: "chat_tool_verb_output")
        case .kill: return String(localized: "chat_tool_verb_kill")
        case .fetch: return String(localized: "chat_tool_verb_fetch")
        case .task: return String(localized: "chat_tool_verb_task")
        case .todo: return String(localized: "chat_tool_verb_todo")
        case .skill: return String(localized: "chat_tool_verb_skill")
        case .generic: return ""
        }
    }

    /// `ToolVerb::english()` in `tui-core/src/tool_display/header.rs`. Used only
    /// to detect that the engine overrode the label; if either side changes,
    /// change both.
    private static func canonicalEnglish(_ verb: ConversationToolVerb) -> String? {
        switch verb {
        case .update: return "Update"
        case .create: return "Write"
        case .read: return "Read"
        case .search: return "Search"
        case .shell: return "Running shell command"
        case .output: return "Output"
        case .kill: return "Kill"
        case .fetch: return "Fetch"
        case .task: return "Task"
        case .todo: return "Update Todos"
        case .skill: return "Skill"
        case .generic: return nil
        }
    }
}
