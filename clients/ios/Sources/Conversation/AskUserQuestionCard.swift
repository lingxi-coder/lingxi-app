import SwiftUI

/// The interactive `AskUserQuestion` questionnaire rendered in a native sheet
/// while a request is pending. 1–4 questions behind a
/// stepper, option chips per question (single or multi select), an automatic
/// 「其他」free-text row, 提交/取消.
///
/// Stores/sources reach this view as plain properties — never via
/// `@Environment(SomeType.self)` (only AppState/LocalizationManager are
/// injected; anything else force-unwraps and traps).
struct AskUserQuestionCard: View {
    @Environment(\.theme) private var t

    let question: ConversationPendingQuestion
    /// Returns whether the answer command was submitted; `false` re-enables
    /// the card for a retry.
    let onSubmit: ([String: String]) async -> Bool
    let onCancel: () async -> Bool

    @State private var step = 0
    /// Selected option labels per question index, in tap order.
    @State private var selected: [Int: [String]] = [:]
    /// The 「其他」free-text answer per question index.
    @State private var custom: [Int: String] = [:]
    @State private var submitting = false

    // MARK: pure answer logic (unit-tested)

    /// Toggle `label` in the running selection. Single-select replaces the
    /// selection; multi-select toggles membership preserving tap order.
    static func toggled(_ label: String, in current: [String], multiSelect: Bool) -> [String] {
        if let index = current.firstIndex(of: label) {
            var copy = current
            copy.remove(at: index)
            return copy
        }
        return multiSelect ? current + [label] : [label]
    }

    /// The wire answer map: question text → chosen label(s) comma-joined,
    /// with a non-empty free-text answer appended as one more value.
    /// Unanswered questions are omitted.
    static func answers(
        questions: [ConversationAskQuestion],
        selected: [Int: [String]],
        custom: [Int: String]
    ) -> [String: String] {
        var out: [String: String] = [:]
        for (index, question) in questions.enumerated() {
            var labels = selected[index] ?? []
            let free = (custom[index] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            if !free.isEmpty { labels.append(free) }
            guard !labels.isEmpty else { continue }
            out[question.question] = labels.joined(separator: ", ")
        }
        return out
    }

    /// Every question needs an answer (a chip or free text) before 提交.
    static func isComplete(
        questions: [ConversationAskQuestion],
        selected: [Int: [String]],
        custom: [Int: String]
    ) -> Bool {
        answers(questions: questions, selected: selected, custom: custom).count == questions.count
    }

    private var questions: [ConversationAskQuestion] { question.questions }
    private var currentQuestion: ConversationAskQuestion? {
        questions.indices.contains(step) ? questions[step] : nil
    }
    private var canSubmit: Bool {
        Self.isComplete(questions: questions, selected: selected, custom: custom)
    }

    var body: some View {
        // The presenting sheet IS the container: the questionnaire draws no
        // card of its own. It used to carry the surface/stroke/rounded-corner
        // chrome it needed when it lived at the tail of the transcript, which
        // read as a card nested inside the sheet once it moved into one.
        //
        // Header and actions are pinned outside the scroll area so a
        // wire-maximum request (4 questions x 4 described options) can never
        // push 取消/下一题/提交 below the sheet's visible height.
        VStack(alignment: .leading, spacing: 0) {
            header
                .padding(.horizontal, 18)
                .padding(.top, 18)
                .padding(.bottom, 12)

            ScrollView {
                if let current = currentQuestion {
                    questionBody(current)
                        .padding(.horizontal, 18)
                        .padding(.bottom, 16)
                }
            }
            .scrollBounceBehavior(.basedOnSize)

            Divider().overlay(t.border)

            actions
                .padding(.horizontal, 18)
                .padding(.vertical, 12)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(t.surface)
        // `.accessibilityIdentifier` on a container REPLACES every descendant's
        // identifier unless the container is declared a containing element, so
        // without `children: .contain` the card id swallowed
        // `chat.ask.cancel` / `chat.ask.submit` / `chat.ask.other.N` and
        // nothing inside the questionnaire was addressable.
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("chat.ask.card.\(question.requestId)")
    }

    private var header: some View {
        HStack {
            Label("chat_ask_user_question_title", systemImage: "questionmark.bubble")
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(t.accent)
            Spacer()
            if questions.count > 1 {
                Text("chat_ask_progress \(step + 1) \(questions.count)")
                    .font(.caption)
                    .foregroundColor(t.text4)
            }
        }
    }

    private func questionBody(_ current: ConversationAskQuestion) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            if !current.header.isEmpty {
                Text(current.header)
                    .font(.caption.weight(.semibold))
                    .foregroundColor(t.text3)
                    .textCase(.uppercase)
            }
            Text(current.question)
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(t.text)
                .fixedSize(horizontal: false, vertical: true)

            optionChips(for: current)

            // The automatic 「其他」 free-text row — every question
            // offers it, mirroring the oracle client's synthesized
            // Other row.
            HStack(spacing: 8) {
                Text("chat_ask_other_option")
                    .font(.system(size: 12.5))
                    .foregroundColor(t.text3)
                TextField(
                    String(localized: "chat_ask_other_placeholder"),
                    text: Binding(
                        get: { custom[step] ?? "" },
                        set: { custom[step] = $0 }
                    ),
                    axis: .vertical
                )
                .font(.system(size: 13))
                .textFieldStyle(.roundedBorder)
                .lineLimit(1 ... 3)
                .disabled(submitting)
                .accessibilityIdentifier("chat.ask.other.\(step)")
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var actions: some View {
        HStack(spacing: 8) {
            Button("common_cancel") {
                resolve { await onCancel() }
            }
            .buttonStyle(.bordered)
            .accessibilityIdentifier("chat.ask.cancel")
            Spacer()
            if step > 0 {
                Button("chat_ask_prev") { step -= 1 }
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("chat.ask.prev")
            }
            if step < questions.count - 1 {
                Button("chat_ask_next") { step += 1 }
                    .buttonStyle(.borderedProminent)
                    .accessibilityIdentifier("chat.ask.next")
            } else {
                Button {
                    let answers = Self.answers(questions: questions, selected: selected, custom: custom)
                    resolve { await onSubmit(answers) }
                } label: {
                    if submitting {
                        ProgressView().controlSize(.small)
                    } else {
                        Text("local_apps_submit")
                    }
                }
                .buttonStyle(.borderedProminent)
                .disabled(!canSubmit || submitting)
                .accessibilityIdentifier("chat.ask.submit")
            }
        }
        .disabled(submitting)
    }

    @ViewBuilder
    private func optionChips(for current: ConversationAskQuestion) -> some View {
        let currentSelection = selected[step] ?? []
        FlowChips(options: current.options, isSelected: { currentSelection.contains($0.label) }) { option in
            guard !submitting else { return }
            selected[step] = Self.toggled(
                option.label,
                in: currentSelection,
                multiSelect: current.multiSelect
            )
        }
    }

    private func resolve(_ action: @escaping () async -> Bool) {
        guard !submitting else { return }
        submitting = true
        Task {
            let accepted = await action()
            // The card itself disappears when the engine confirms with
            // `askUserQuestionResolved`; on a failed submit re-enable for a
            // retry instead of leaving a dead card.
            if !accepted { submitting = false }
        }
    }
}

/// A simple wrapping chip row for the answer options.
private struct FlowChips: View {
    @Environment(\.theme) private var t
    let options: [ConversationAskOption]
    let isSelected: (ConversationAskOption) -> Bool
    let onTap: (ConversationAskOption) -> Void

    var body: some View {
        // A vertical list of chips: option descriptions matter more than
        // density here, and questions carry at most a handful of options.
        VStack(alignment: .leading, spacing: 6) {
            ForEach(options.indices, id: \.self) { index in
                let option = options[index]
                let active = isSelected(option)
                Button {
                    onTap(option)
                } label: {
                    HStack(alignment: .top, spacing: 8) {
                        Image(systemName: active ? "checkmark.circle.fill" : "circle")
                            .font(.system(size: 14))
                            .foregroundColor(active ? t.accent : t.text4)
                            .padding(.top, 1)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(option.label)
                                .font(.system(size: 13.5, weight: active ? .semibold : .regular))
                                .foregroundColor(t.text)
                                .multilineTextAlignment(.leading)
                            if !option.description.isEmpty {
                                Text(option.description)
                                    .font(.caption)
                                    .foregroundColor(t.text4)
                                    .multilineTextAlignment(.leading)
                            }
                        }
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, 10)
                    .padding(.vertical, 7)
                    .background(active ? t.accent.opacity(0.12) : t.surfaceActive.opacity(0.6))
                    .clipShape(RoundedRectangle(cornerRadius: 10))
                    .overlay(
                        RoundedRectangle(cornerRadius: 10)
                            .stroke(active ? t.accent.opacity(0.5) : t.border, lineWidth: 0.6)
                    )
                }
                .buttonStyle(.plain)
            }
        }
    }
}
