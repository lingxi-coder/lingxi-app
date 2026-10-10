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
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @FocusState private var isFocused: Bool
    let trace: ConversationToolTrace
    var isExpanded: Bool = false
    /// Timeline mode uses Codex's borderless, dense row treatment. The legacy
    /// execution card can keep the original surface by leaving this false.
    var compact: Bool = false
    var onToggle: () -> Void = {}

    var body: some View {
        if let rows = trace.questionAnswers, trace.status == .completed {
            AnsweredQuestionsView(rows: rows, isExpanded: !isExpanded, onToggle: onToggle)
                .accessibilityIdentifier("conversation.tool-call.\(trace.id)")
        } else if let document = trace.planDocument {
            PlanDocumentCard(document: document)
        } else {
            toolContent
        }
    }

    private var toolContent: some View {
        VStack(alignment: .leading, spacing: 5) {
            if compact {
                if isCollapsible {
                    Button(action: onToggle) { compactHeader }
                        .buttonStyle(.plain)
                        .focused($isFocused)
                        .accessibilityLabel(ConversationDesktopTimeline.summary([trace]))
                        .accessibilityValue(isExpanded ? "Expanded" : "Collapsed")
                } else {
                    compactHeader
                }
            } else {
                headerRow
            }
            if !compact, let sub = trace.header?.subLine, !sub.text.isEmpty {
                subLineRow(sub)
            }
            if !compact || isExpanded {
                resultBlock
                legacyFallback
            }
        }
        // Desktop tool rows are borderless and use a 40pt disclosure target;
        // keep the compact transcript on that rhythm while the legacy card
        // retains its padded surface treatment.
        .padding(.horizontal, compact ? 0 : 10)
        .padding(.vertical, compact ? 2 : 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(compact ? Color.clear : t.surface)
        .clipShape(RoundedRectangle(cornerRadius: compact ? 0 : 10))
        .overlay {
            if !compact {
                RoundedRectangle(cornerRadius: 10).stroke(t.border, lineWidth: 0.5)
            }
        }
        .overlay(alignment: .bottom) {
            if compact {
                Rectangle()
                    .fill(t.border.opacity(0.45))
                    .frame(height: 0.5)
                    .padding(.leading, 32)
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityValue(trace.status.label)
        .accessibilityIdentifier("conversation.tool-call.\(trace.id)")
    }

    private var compactHeader: some View {
        HStack(spacing: 8) {
            LXIcon(name: ToolDisplayText.icon(header: trace.header, tool: trace.tool), size: 20,
                   color: trace.status == .failed ? t.danger : t.text3, stroke: 1.8)
                .accessibilityIdentifier("conversation.tool-call.\(trace.id).icon.\(ToolDisplayText.icon(header: trace.header, tool: trace.tool).rawValue)")
            Text(compactSummary)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(trace.status == .failed ? t.danger : t.text3)
                .lineLimit(1)
                .truncationMode(.middle)
                .runtimeTextSweep(isActive: trace.status == .running, highlightColor: t.text)
            Spacer(minLength: 0)
            if isCollapsible {
                Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(t.text3)
                    .timelineChevron(
                        isHighlighted: isFocused,
                        isExpanded: isExpanded,
                        reduceMotion: reduceMotion
                    )
            }
        }
        .frame(minHeight: 40)
        .padding(.horizontal, 6)
        .background {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(t.accent.opacity(isFocused || isExpanded ? 0.10 : 0))
        }
        .contentShape(.rect)
    }

    private var compactSummary: String {
        var summary = ConversationDesktopTimeline.summary([trace])
        if let display = trace.display, let headline = ToolDisplayText.headline(display) {
            summary += " · " + headline
        }
        return summary
    }

    // MARK: header

    private var headerRow: some View {
        HStack(alignment: .center, spacing: 7) {
            LXIcon(
                name: ToolDisplayText.icon(header: trace.header, tool: trace.tool),
                size: 18,
                color: trace.status == .failed ? t.danger : t.text3,
                stroke: 1.65
            )
            .accessibilityIdentifier(
                "conversation.tool-call.\(trace.id).icon.\(ToolDisplayText.icon(header: trace.header, tool: trace.tool).rawValue)"
            )
            titleText
                .runtimeTextSweep(isActive: trace.status == .running, highlightColor: t.text)
            Spacer(minLength: 4)
            if !compact || trace.status == .failed || trace.status == .cancelled {
                Text(trace.status.label)
                    .font(.system(size: 10.5, weight: .medium))
                    .foregroundStyle(statusColor)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 3)
                    .background(statusColor.opacity(0.12))
                    .clipShape(Capsule())
                    .accessibilityIdentifier("conversation.tool-call.\(trace.id).status")
            }
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
                    if isCollapsible && !compact { disclosure(display) }
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

    /// Tool details stay out of the transcript until the user asks for them.
    /// The engine's `collapsed` hint is still useful metadata, but the client
    /// keeps the interaction consistent for short and long outputs alike.
    private var isCollapsible: Bool {
        guard let display = trace.display else {
            return !(trace.outputSummary ?? "").isEmpty || !(trace.inputSummary ?? "").isEmpty
        }
        return display.diff != nil
            || !(display.body ?? "").isEmpty
            || display.bodyTruncated
    }

    private var showsDetail: Bool { isExpanded }

    private var statusColor: Color {
        switch trace.status {
        case .running: return t.text3
        case .completed: return t.ok
        case .failed: return t.danger
        case .cancelled, .unknown: return t.text3
        }
    }

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

    /// Maps the engine's stable tool verb to a compact action glyph. For older
    /// engines without a header, the raw tool name is used as a best-effort
    /// compatibility fallback.
    static func icon(header: ConversationToolHeader?, tool: String) -> LXIconName {
        if let header {
            return iconName(for: header.icon(for: tool))
        }

        let normalized = tool.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if normalized.contains("shell") || normalized.contains("bash") || normalized.contains("terminal") {
            return .terminal
        }
        if normalized.contains("fetch") || normalized.contains("web") || normalized.contains("url") || normalized.contains("browser") {
            return .globe
        }
        if normalized.contains("ls") || normalized.contains("list") {
            return .listFiles
        }
        if normalized.contains("read") || normalized.contains("file") {
            return .bookOpen
        }
        if normalized.contains("search") || normalized.contains("grep") || normalized.contains("glob") || normalized.contains("documentation") || normalized.contains("docs") {
            return .search
        }
        if normalized.contains("write") || normalized.contains("create") || normalized.contains("edit") || normalized.contains("update") {
            return .pencil
        }
        if normalized.contains("task") || normalized.contains("agent") || normalized.contains("workflow") {
            return .workflow
        }
        if normalized.contains("skill") {
            return .sparkles
        }
        if normalized.contains("todo") || normalized.contains("plan") {
            return .listChecks
        }
        if normalized.contains("mcp") || normalized.contains("plugin") {
            return .plug
        }
        if normalized.contains("kill") || normalized.contains("stop") || normalized.contains("cancel") {
            return .squareStop
        }
        return .wrench
    }

    private static func iconName(for icon: ConversationToolIcon) -> LXIconName {
        switch icon {
        case .read: return .bookOpen
        case .search: return .search
        case .list: return .listFiles
        case .edit: return .pencil
        case .terminal: return .terminal
        case .globe: return .globe
        case .workflow: return .workflow
        case .listChecks: return .listChecks
        case .sparkles: return .sparkles
        case .plug: return .plug
        case .output: return .message
        case .stop: return .squareStop
        case .wrench: return .wrench
        }
    }

    static func iconColor(header: ConversationToolHeader?, tool: String, palette: Palette) -> Color {
        palette.text3
    }

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
