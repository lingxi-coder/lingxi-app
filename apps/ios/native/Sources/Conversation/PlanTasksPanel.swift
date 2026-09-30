// The model-managed todo list pinned above the composer.

import SwiftUI

struct PlanTasksPanel: View {
    @Environment(\.theme) private var theme

    private static let maximumVisibleRows = 6
    private static let maxListHeight: CGFloat = 220

    let tasks: [ConversationPlanTask]
    let showsContainer: Bool

    init(tasks: [ConversationPlanTask], showsContainer: Bool = true) {
        self.tasks = tasks
        self.showsContainer = showsContainer
    }

    @ViewBuilder
    private var content: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 6) {
                Image(systemName: "list.bullet.rectangle")
                    .font(.caption)
                    .foregroundStyle(theme.text3)
                Text("chat_plan_title")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(theme.text2)
                Spacer(minLength: 0)
            }

            boundedTaskRows
        }
        .accessibilityIdentifier("chat.plan-panel")
    }

    var body: some View {
        if showsContainer {
            content
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
        } else {
            content
        }
    }

    /// One rendered row plus the stable identity used by SwiftUI's ForEach.
    struct Row: Identifiable {
        let id: String
        let task: ConversationPlanTask
    }

    static func rows(for tasks: [ConversationPlanTask]) -> [Row] {
        tasks.enumerated().map { index, task in
            Row(id: task.taskId ?? "\(index):\(task.subject)", task: task)
        }
    }

    var taskRows: some View {
        ForEach(Self.rows(for: tasks)) { entry in
            row(for: entry.task)
        }
    }

    @ViewBuilder
    private var boundedTaskRows: some View {
        if tasks.count > Self.maximumVisibleRows {
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    taskRows
                }
            }
            .frame(maxHeight: Self.maxListHeight)
            .scrollBounceBehavior(.basedOnSize)
        } else {
            taskRows
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
        .padding(.vertical, 2)
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

    // Kept as a pure compatibility helper for terminal parity tests. The chat
    // surface no longer truncates or renders an overflow row.
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

    static func clauseSeparator(for locale: Locale) -> String {
        switch locale.language.languageCode?.identifier {
        case "zh", "ja": return "、"
        default: return ", "
        }
    }
}
