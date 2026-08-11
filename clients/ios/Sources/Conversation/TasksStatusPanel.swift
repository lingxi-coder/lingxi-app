// TasksStatusPanel.swift — the pinned background-tasks widget.
//
// The mobile analog of the CLI's tasks footer: while a Workflow build (or any
// background task) runs, the chat pins a compact status card between the
// message list and the composer — header with counts, active rows live,
// finished rows struck through, older finished rows collapsed into a "+N"
// line. Fed by `ConversationModel.backgroundTasks` (TaskRow / TaskStatusChanged
// events); hidden entirely when no task was ever announced.

import SwiftUI

struct TasksStatusPanel: View {
    @Environment(\.theme) private var theme
    let tasks: [BackgroundTaskSnapshot]
    @State private var collapsed = false

    /// Finished rows shown before the "+N" overflow line.
    private static let visibleFinishedLimit = 3
    /// Ceiling on the whole panel. It sits between the transcript and the
    /// composer in a plain VStack, so an unbounded row list would squeeze the
    /// transcript to nothing and push the composer past the safe area —
    /// exactly when a fan-out run is what the user needs to stop.
    private static let maxPanelHeight: CGFloat = 148

    private var active: [BackgroundTaskSnapshot] { tasks.filter { !$0.status.isTerminal } }
    private var finished: [BackgroundTaskSnapshot] { tasks.filter { $0.status.isTerminal } }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            header
            if !collapsed {
                ScrollView {
                    VStack(alignment: .leading, spacing: 6) {
                        ForEach(active) { row(for: $0) }
                        ForEach(finished.suffix(Self.visibleFinishedLimit)) { row(for: $0) }
                        if finished.count > Self.visibleFinishedLimit {
                            Text("chat_tasks_more_completed \(finished.count - Self.visibleFinishedLimit)")
                                .font(.caption2)
                                .foregroundStyle(theme.text4)
                                .padding(.leading, 22)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: Self.maxPanelHeight)
                .scrollBounceBehavior(.basedOnSize)
            }
        }
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
        .accessibilityIdentifier("chat.tasks-panel")
    }

    private var header: some View {
        Button {
            withAnimation(.easeInOut(duration: 0.15)) { collapsed.toggle() }
        } label: {
            HStack(spacing: 6) {
                if active.contains(where: { $0.status == .running }) {
                    ProgressView().controlSize(.mini)
                } else {
                    Image(systemName: "checklist")
                        .font(.caption)
                        .foregroundStyle(theme.text3)
                }
                Text(
                    "chat_tasks_summary \(tasks.count) \(finished.count) \(active.filter { $0.status == .running }.count) \(active.filter { $0.status == .pending }.count)"
                )
                .font(.caption)
                .foregroundStyle(theme.text2)
                Spacer(minLength: 0)
                // Points the way the tap goes: down-chevron opens the list,
                // up-chevron folds it away. Matches the transcript's own
                // disclosure toggle (`ToolCallView.disclosure`, whose
                // `LXIcon(.chevron)` points down at rest and flips 180° once
                // expanded) and `PlanTasksPanel`, the panel stacked directly
                // below this one.
                Image(systemName: collapsed ? "chevron.down" : "chevron.up")
                    .font(.caption2)
                    .foregroundStyle(theme.text4)
            }
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("chat.tasks-panel.toggle")
        .accessibilityHint(Text(collapsed ? "chat_tasks_expand_hint" : "chat_tasks_collapse_hint"))
    }

    private func row(for task: BackgroundTaskSnapshot) -> some View {
        HStack(spacing: 8) {
            statusIcon(task.status)
                .frame(width: 14)
            Text(task.descriptionText.isEmpty ? task.id : task.descriptionText)
                .font(.caption)
                .lineLimit(1)
                .strikethrough(task.status == .completed)
                .foregroundStyle(task.status.isTerminal ? theme.text4 : theme.text)
            Spacer(minLength: 0)
        }
    }

    @ViewBuilder
    private func statusIcon(_ status: BackgroundTaskSnapshot.Status) -> some View {
        switch status {
        case .running:
            ProgressView().controlSize(.mini)
        case .pending:
            Image(systemName: "clock")
                .font(.caption2)
                .foregroundStyle(theme.text3)
        case .completed:
            Image(systemName: "checkmark")
                .font(.caption2)
                .foregroundStyle(theme.ok)
        case .failed:
            Image(systemName: "xmark")
                .font(.caption2)
                .foregroundStyle(theme.danger)
        case .cancelled:
            Image(systemName: "minus.circle")
                .font(.caption2)
                .foregroundStyle(theme.text4)
        }
    }
}
