// PlanTasksPanel.swift — the model-managed working plan, pinned above the composer.
//
// The mobile analog of the terminal's task block: while the model keeps a
// TodoWrite / Task checklist, the chat pins it immediately above the composer.
// Fed by `ConversationModel.planTasks`, which the reducer replaces WHOLESALE on
// every `PlanUpdated` (the engine emits the full list; an empty list clears it).
//
// This is a SEPARATE panel from `TasksStatusPanel`, deliberately. That one lists
// engine BACKGROUND TASKS (Workflow builds, background bash jobs); this one is
// the model's own plan. They stack — background tasks above, plan closest to the
// composer, matching the terminal's ordering — and neither may swallow the
// other's rows.
//
// Row cap, glyphs and overflow ordering match
// `tui-core/src/tool_display/plan.rs` (`MAX_VISIBLE_TASKS`, `glyph`,
// `overflow_summary`): first five rows, then one compact
// "in progress / pending / completed" tail counting only the HIDDEN remainder.

import SwiftUI

struct PlanTasksPanel: View {
    @Environment(\.theme) private var theme
    /// The in-app language override lives in the environment: `LocalizationManager`
    /// swizzles `Bundle.localizedString` for the string table and `RootView` injects
    /// `localization.effectiveLocale()` here. It never touches `Locale.current`, so
    /// every locale-dependent decision in this panel must read THIS locale.
    @Environment(\.locale) private var locale
    let tasks: [ConversationPlanTask]
    let showsContainer: Bool
    @State private var collapsed = false

    init(tasks: [ConversationPlanTask], showsContainer: Bool = true) {
        self.tasks = tasks
        self.showsContainer = showsContainer
    }

    /// `plan::MAX_VISIBLE_TASKS`. A phone has a scrolling viewport, so the
    /// terminal's height-dependent cap does not apply — the flat 5 does.
    private static let visibleLimit = 5

    private var visible: [ConversationPlanTask] { Array(tasks.prefix(Self.visibleLimit)) }
    private var hidden: [ConversationPlanTask] { Array(tasks.dropFirst(Self.visibleLimit)) }

    @ViewBuilder
    private var panelContent: some View {
        VStack(alignment: .leading, spacing: 5) {
            header
            if !collapsed {
                taskRows
                if let overflow = overflowText {
                    Text("chat_plan_overflow_prefix \(overflow)")
                        .font(.caption2)
                        .foregroundStyle(theme.text4)
                        .padding(.leading, 22)
                }
            }
        }
    }

    var body: some View {
        if showsContainer {
            panelContent
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .fill(theme.surface)
                        .overlay(
                            RoundedRectangle(cornerRadius: 12, style: .continuous)
                                .stroke(theme.border, lineWidth: 1)
                        )
                )
                .accessibilityIdentifier("chat.plan-panel")
        } else {
            panelContent
        }
    }

    private var header: some View {
        Button {
            withAnimation(.easeInOut(duration: 0.15)) { collapsed.toggle() }
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "list.bullet.rectangle")
                    .font(.caption)
                    .foregroundStyle(theme.text3)
                Text("chat_plan_title")
                    .font(.caption)
                    .foregroundStyle(theme.text2)
                Spacer(minLength: 0)
                // Points the way the tap goes: down-chevron opens the list,
                // up-chevron folds it away — the same convention as
                // `TasksStatusPanel` (stacked directly above) and the
                // transcript's own `ToolCallView.disclosure`.
                Image(systemName: collapsed ? "chevron.down" : "chevron.up")
                    .font(.caption2)
                    .foregroundStyle(theme.text4)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("chat.plan-panel.toggle")
        .accessibilityLabel(collapsed
            ? String(localized: "chat_plan_expand_hint")
            : String(localized: "chat_plan_collapse_hint"))
    }

    /// One rendered row plus the identity `ForEach` keys it by.
    struct Row: Identifiable {
        let id: String
        let task: ConversationPlanTask
    }

    /// Rows keyed by the engine's stable task id when there is one, and by an
    /// INDEX-QUALIFIED subject when there is not — the same guard Electron's
    /// plan block uses (``key={task.id ?? `${i}:${task.subject}`}``).
    ///
    /// The fallback is not cosmetic. `plan_tasks_from_todowrite_input`
    /// (`tui-core/src/tool_display/plan.rs`) sets `id: None` for EVERY TodoWrite
    /// V1 item, and `validate_todos` (`tools/task/src/todo_write.rs`) enforces
    /// no uniqueness on `content` — so two todos carrying the same text collapse
    /// onto one `ConversationPlanTask.id` (`taskId ?? subject`) and hand
    /// `ForEach` a DUPLICATE identifier: undefined row identity, wrong move
    /// animations, and a SwiftUI runtime diagnostic.
    ///
    /// Keying by the id whenever the engine supplies one is what keeps the
    /// reorder a MOVE: `PlanUpdated` replaces the list wholesale, so a purely
    /// positional key would animate a reordered plan as every row mutating its
    /// content instead of the rows swapping places.
    static func rows(for tasks: [ConversationPlanTask]) -> [Row] {
        tasks.enumerated().map { index, task in
            Row(id: task.taskId ?? "\(index):\(task.subject)", task: task)
        }
    }

    var taskRows: some View {
        ForEach(Self.rows(for: visible)) { entry in
            row(for: entry.task)
        }
    }

    private func row(for task: ConversationPlanTask) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Text(task.state.glyph)
                .font(.caption)
                .foregroundStyle(glyphColor(task.state))
                .frame(width: 14, alignment: .leading)
            Text(task.subject)
                .font(.caption)
                .fontWeight(task.state == .inProgress ? .semibold : .regular)
                .strikethrough(task.state == .completed)
                .foregroundStyle(task.state == .completed ? theme.text4 : theme.text)
                .lineLimit(2)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(task.state.label) \(task.subject)")
    }

    private func glyphColor(_ state: ConversationPlanTaskState) -> Color {
        switch state {
        case .pending: return theme.text4
        case .inProgress: return theme.accent
        case .completed: return theme.ok
        }
    }

    private var overflowText: String? { Self.overflowSummary(hidden: hidden, locale: locale) }

    /// `plan::overflow_summary`: only non-zero clauses, ordered in progress →
    /// pending → completed, counting ONLY the hidden remainder.
    ///
    /// Takes the locale explicitly because the clause separator is script-dependent
    /// and the app's language is an environment value, not the process locale.
    static func overflowSummary(hidden: [ConversationPlanTask], locale: Locale) -> String? {
        guard !hidden.isEmpty else { return nil }
        func count(_ state: ConversationPlanTaskState) -> Int {
            hidden.filter { $0.state == state }.count
        }
        var clauses: [String] = []
        let inProgress = count(.inProgress)
        let pending = count(.pending)
        let completed = count(.completed)
        if inProgress > 0 {
            clauses.append(String(localized: "chat_plan_overflow_in_progress \(inProgress)"))
        }
        if pending > 0 {
            clauses.append(String(localized: "chat_plan_overflow_pending \(pending)"))
        }
        if completed > 0 {
            clauses.append(String(localized: "chat_plan_overflow_completed \(completed)"))
        }
        guard !clauses.isEmpty else { return nil }
        return clauses.joined(separator: clauseSeparator(for: locale))
    }

    /// CJK enumerates with `、`; the Latin locales use `, `. There is no string
    /// key for a separator, so it is derived from the active language rather
    /// than hardcoded to one script.
    ///
    /// ⚠️ Resolve this from the ENVIRONMENT locale (`@Environment(\.locale)`), never
    /// from `Locale.current`: the in-app language picker does not move the process
    /// locale, so `Locale.current` still reports the device language and an English
    /// phone switched to 简体中文 would join Chinese clauses with ", ".
    static func clauseSeparator(for locale: Locale) -> String {
        switch locale.language.languageCode?.identifier {
        case "zh", "ja": return "、"
        default: return ", "
        }
    }
}
