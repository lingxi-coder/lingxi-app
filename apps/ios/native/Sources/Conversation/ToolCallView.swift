// ToolCallView.swift — one tool call, rendered from the engine's derivation.
//
// The engine ships a `ToolHeaderDto` on the call and a `ToolResultDisplayDto` on
// the result. This view renders THOSE. It must never re-parse `input_json` /
// `result_json` to rebuild a header — four clients each re-deriving the same
// presentation is the drift this whole change exists to delete.
//
// Headers share the transcript's leading edge. Expanded command, result and
// status content lives in a separate rounded detail surface beneath the header.
//
// COLLAPSE STATE IS NOT HELD HERE. Every list this view lands in recycles its
// rows, so row-local `@State` is lost on scroll and reappears on the wrong row.
// The owner passes `isExpanded` from `ConversationModel.expandedToolCalls`,
// keyed by tool-use id, and gets a toggle callback back.

import SwiftUI
import UIKit

struct ToolCallView: View {
    @Environment(\.theme) private var t
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @FocusState private var isFocused: Bool
    @State private var isHovering = false
    let trace: ConversationToolTrace
    var isExpanded: Bool = false
    /// Execution rows also retain their elapsed-time metadata.
    var compact: Bool = false
    var onToggle: () -> Void = {}

    private var isSkill: Bool {
        trace.header?.verb == .skill || trace.tool.caseInsensitiveCompare("Skill") == .orderedSame
    }

    private var denial: ToolDenialPresentation? {
        ToolDenialPresentation(failed: trace.status == .failed, headline: trace.display?.headline, body: trace.display?.body)
    }

    var body: some View {
        if let rows = trace.questionAnswers, trace.status == .completed {
            AnsweredQuestionsView(rows: rows, isExpanded: !isExpanded, onToggle: onToggle)
                .accessibilityIdentifier("conversation.tool-call.\(trace.id)")
        } else if let app = trace.localApp, trace.status == .completed {
            LocalAppCard(app: app)
        } else if let document = trace.planDocument {
            PlanDocumentCard(document: document)
        } else {
            toolContent
        }
    }

    private var toolContent: some View {
        VStack(alignment: .leading, spacing: 5) {
            if isCollapsible {
                Button(action: onToggle) { compactHeader }
                    .buttonStyle(.plain)
                    .focused($isFocused)
                    .onHover { isHovering = $0 }
                    .accessibilityLabel(compactSummary)
                    .accessibilityValue(isExpanded ? String(localized: "chat_agent_details_collapse") : String(localized: "chat_agent_details_expand"))
                    .accessibilityIdentifier("conversation.tool-call.\(trace.id).toggle")
            } else {
                compactHeader
            }
            if showsDetail { resultBlock }
        }
        .padding(.vertical, 2)
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .contain)
        .accessibilityValue(trace.status.label)
        .accessibilityIdentifier("conversation.tool-call.\(trace.id)")
    }

    private var compactHeader: some View {
        HStack(spacing: 8) {
            LXIcon(name: denial != nil ? .shieldTerminal : ToolDisplayText.icon(header: trace.header, tool: trace.tool), size: 20,
                   color: denial != nil ? (t.isDark ? Color(red: 0.91, green: 0.65, blue: 0.42) : Color(red: 0.65, green: 0.26, blue: 0.07)) : trace.status == .failed ? t.danger : isExpanded ? t.text : t.text3, stroke: 1.8)
                .accessibilityIdentifier("conversation.tool-call.\(trace.id).icon.\((denial != nil ? LXIconName.shieldTerminal : ToolDisplayText.icon(header: trace.header, tool: trace.tool)).rawValue)")
            Text(compactSummary)
                .font(.footnote)
                .foregroundStyle(isExpanded || isFocused || isHovering ? t.text : t.text3)
                .lineLimit(showsRequiresCodeMessage ? nil : 1)
                .truncationMode(.tail)
                .fixedSize(horizontal: false, vertical: showsRequiresCodeMessage)
                .layoutPriority(showsRequiresCodeMessage ? 1 : 0)
                .runtimeTextSweep(isActive: trace.status == .running, highlightColor: t.text)
            if trace.status == .failed && denial == nil {
                Text(trace.status.label).font(.caption).foregroundStyle(t.danger).lineLimit(1).layoutPriority(1)
            }
            if !compact, let elapsed = ConversationExecutionParsing.formatDuration(trace.elapsedMs) {
                Text(elapsed).font(.caption).foregroundStyle(t.text4)
            }
            if isCollapsible {
                Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(t.text3)
                    .timelineChevron(
                        isHighlighted: true,
                        isExpanded: isExpanded,
                        reduceMotion: reduceMotion
                    )
            }
        }
        .frame(minHeight: 44)
        .contentShape(.rect)
    }

    private var compactSummary: String {
        if let denial { return denial.title }
        var summary = ConversationDesktopTimeline.summary([trace])
        if let display = trace.display, let headline = ToolDisplayText.headline(display, trace: trace) {
            let headlineInTitle = trace.status != .failed
                && (trace.header?.primary == nil || trace.header?.primary == headline)
            if trace.tool == "LocalApp" && !showsRequiresCodeMessage && (isCollapsible || headlineInTitle) { return summary }
            summary += " · " + headline
        }
        return summary
    }

    private var showsRequiresCodeMessage: Bool {
        guard let display = trace.display else { return false }
        return LocalAppToolFailurePresentation.requiresCode(tool: trace.tool, failed: trace.status == .failed,
                                                             headline: display.headline, body: display.body)
    }

    @ViewBuilder
    private var resultBlock: some View {
        if let denial {
            ToolDetailPanel(title: detailTitle, footerText: trace.header?.verb == .shell ? String(localized: "tool_denied_exit_unknown") : nil) {
                if let sub = trace.header?.subLine {
                    HStack(alignment: .top, spacing: 8) {
                        Text("\(sub.prefix) \(sub.text)")
                            .font(.system(.footnote, design: .monospaced))
                            .foregroundStyle(t.text2).textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        Button { UIPasteboard.general.string = sub.text } label: {
                            Image(systemName: "doc.on.doc").font(.caption).foregroundStyle(t.text3)
                                .frame(minWidth: 44, minHeight: 44)
                        }.buttonStyle(.plain).accessibilityLabel(String(localized: "chat_copy_code"))
                    }
                } else if let primary = trace.header?.primary {
                    Text(primary).font(.system(.footnote, design: .monospaced))
                        .foregroundStyle(t.text2).textSelection(.enabled)
                }
                Text("tool_denied_no_output").font(.system(.footnote, design: .monospaced)).foregroundStyle(t.text2)
            }
            VStack(alignment: .leading, spacing: 8) {
                Text("tool_denied_reason").font(.footnote.weight(.medium)).foregroundStyle(t.text2)
                Text(denial.reason).font(.footnote).foregroundStyle(t.text3)
                    .fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
                if trace.display?.bodyTruncated == true {
                    Text("chat_tool_body_truncated").font(.caption).foregroundStyle(t.text4)
                }
            }
            .padding(.bottom, 8)
        } else {
            ToolDetailPanel(title: detailTitle, status: trace.status) {
                if let sub = trace.header?.subLine, !sub.text.isEmpty {
                    Text("\(sub.prefix) \(sub.text)")
                        .font(.system(.footnote, design: .monospaced))
                        .foregroundStyle(t.text2)
                        .textSelection(.enabled)
                } else if trace.header == nil, let input = trace.inputSummary, !input.isEmpty {
                    Text(input)
                        .font(.system(.footnote, design: .monospaced))
                        .foregroundStyle(t.text2)
                        .textSelection(.enabled)
                }
                if let display = trace.display {
                    if let headline = ToolDisplayText.headline(display, trace: trace) {
                        Text(headline)
                            .font(.footnote)
                            .foregroundStyle(trace.status == .failed ? t.danger : t.text3)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    if let diff = display.diff { DiffView(diff: diff) }
                    if let body = ToolDisplayText.body(display, trace: trace), !body.isEmpty {
                        if isSkill {
                            AIText(markdown: body, compact: true)
                        } else {
                            Text(body)
                                .font(.system(.footnote, design: .monospaced))
                                .foregroundStyle(t.text2)
                                .fixedSize(horizontal: false, vertical: true)
                                .textSelection(.enabled)
                        }
                    }
                    if display.bodyTruncated {
                        Text("chat_tool_body_truncated").font(.caption).foregroundStyle(t.text4)
                    }
                }
            }
            .accessibilityIdentifier("conversation.tool-detail.\(trace.id)")
        }
    }

    private var detailTitle: String {
        if trace.header?.verb == .shell || trace.tool.caseInsensitiveCompare("Shell") == .orderedSame { return "Shell" }
        if isSkill { return String(localized: "chat_tool_verb_skill") }
        return trace.header?.label ?? trace.tool
    }

    /// Tool details stay out of the transcript until the user asks for them.
    /// The engine's `collapsed` hint is still useful metadata, but the client
    /// keeps the interaction consistent for short and long outputs alike.
    private var isCollapsible: Bool {
        if denial != nil { return true }
        guard let display = trace.display else { return !(trace.inputSummary ?? "").isEmpty }
        return display.hasExpandableContent
    }

    private var showsDetail: Bool { isExpanded && isCollapsible }

}

/// A leading-aligned surface shared by tool results and Skill instructions.
struct ToolDetailPanel<Content: View>: View {
    @Environment(\.theme) private var t
    @ScaledMetric(relativeTo: .footnote) private var detailHeight: CGFloat = 384
    let title: String
    var status: ConversationToolStatus? = nil
    var footerText: String? = nil
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(title).font(.footnote).foregroundStyle(t.text2)
                .padding(.horizontal, 14).padding(.top, 10)
            ViewThatFits(in: .vertical) {
                detailContent.fixedSize(horizontal: false, vertical: true)
                ScrollView { detailContent }
                    .scrollBounceBehavior(.basedOnSize)
            }
            .frame(maxHeight: detailHeight, alignment: .topLeading)
            if let footerText {
                Text(footerText).font(.caption).foregroundStyle(t.text3)
                    .frame(maxWidth: .infinity, alignment: .trailing)
                    .padding(.horizontal, 14).padding(.bottom, 10)
            } else if let status {
                HStack(spacing: 6) {
                    Spacer(minLength: 0)
                    Image(systemName: status == .completed ? "checkmark" : status == .failed || status == .cancelled ? "xmark" : "clock")
                        .accessibilityHidden(true)
                    Text(status.label)
                }
                .font(.caption).foregroundStyle(t.text3)
                .padding(.horizontal, 14).padding(.bottom, 10)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(t.surfaceHover, in: .rect(cornerRadius: 12))
        .overlay { RoundedRectangle(cornerRadius: 12).stroke(t.border, lineWidth: 0.5) }
        .padding(.top, 6).padding(.bottom, 8)
        .accessibilityElement(children: .contain)
    }

    private var detailContent: some View {
        VStack(alignment: .leading, spacing: 12) { content }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 14).padding(.vertical, 12)
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
    /// This stable host error is localized only for the failed LocalApp tool.
    /// The source trace retains the original wire text for replay/diagnostics.
    static func headline(_ display: ConversationToolResultDisplay, trace: ConversationToolTrace) -> String? {
        LocalAppToolFailurePresentation.visibleText(tool: trace.tool, failed: trace.status == .failed,
            headline: display.headline, body: display.body, fallback: headline(display))
    }

    static func body(_ display: ConversationToolResultDisplay, trace: ConversationToolTrace) -> String? {
        guard let body = display.body, !body.isEmpty else { return display.body }
        return LocalAppToolFailurePresentation.visibleText(tool: trace.tool, failed: trace.status == .failed,
            headline: display.headline, body: display.body, fallback: body)
    }

    /// Maps the engine's stable tool verb to a compact action glyph. An orphan
    /// or synthetic row without a start header uses the raw tool name.
    static func icon(header: ConversationToolHeader?, tool: String) -> LXIconName {
        if tool.lowercased() == "computer" { return .computer }
        if tool == "LocalApp" { return .localApp }
        if ConversationToolTrace.isTaskProgressTool(tool) { return .listChecks }
        if ["sendmessage", "send_message"].contains(tool.lowercased()) || tool.lowercased().hasSuffix("__send_message") { return .send }
        if tool == "ToolSearch" { return .search }
        if tool.lowercased().hasPrefix("mcp__"),
           (header == nil || header?.verb == .generic),
           (header?.icon == nil || header?.icon == .wrench) { return .mcpFallback }
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

/// Historical tool results retain only the gate's display text. Restrict matching
/// to refusal prefixes so failed shell commands containing EACCES stay ordinary failures.
struct ToolDenialPresentation {
    let reason: String
    let autoReview: Bool
    init?(failed: Bool, headline: String?, body: String?) {
        guard failed else { return nil }
        let body = body?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let reason = body.isEmpty ? (headline?.trimmingCharacters(in: .whitespacesAndNewlines) ?? "") : body
        let text = reason.lowercased()
        let prefixes = ["permission to use ", "permission denied:", "permission denied by ", "tool use denied", "user denied ", "automatic approval review failed", "command declined by auto-review"]
        guard text == "permission denied" || prefixes.contains(where: { text.hasPrefix($0) }) else { return nil }
        self.reason = reason
        autoReview = text.contains("auto-review") || text.contains("auto review") || text.contains("automatic approval review")
    }
    var title: String {
        autoReview ? String(localized: "tool_denied_auto_review") : String(localized: "tool_denied")
    }
}
