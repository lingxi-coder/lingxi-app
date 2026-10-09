import SwiftUI

/// The interactive AskUserQuestion form shown in a native sheet. All questions
/// stay in view as an accordion: answered rows retain the question and answer,
/// while one row at a time shows its options and custom response field.
struct AskUserQuestionCard: View {
    @Environment(\.theme) private var t
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    let question: ConversationPendingQuestion
    /// Returns whether the answer command was submitted; false enables retry.
    let onSubmit: ([String: String]) async -> Bool
    let onCancel: () async -> Bool

    @State private var expandedQuestion: Int? = 0
    /// Selected option labels per question index, in tap order.
    @State private var selected: [Int: [String]] = [:]
    /// The free-text answer per question index.
    @State private var custom: [Int: String] = [:]
    @State private var submitting = false
    @FocusState private var focusedQuestionIndex: Int?

    private var questions: [ConversationAskQuestion] { question.questions }

    private var answeredCount: Int {
        questions.indices.filter { isAnswered(at: $0) }.count
    }

    private var canSubmit: Bool {
        Self.isComplete(questions: questions, selected: selected, custom: custom)
    }

    // MARK: answer logic

    /// Toggle an option. Single-select replaces the selection; multi-select
    /// toggles membership while preserving tap order.
    static func toggled(_ label: String, in current: [String], multiSelect: Bool) -> [String] {
        if let index = current.firstIndex(of: label) {
            var copy = current
            copy.remove(at: index)
            return copy
        }
        return multiSelect ? current + [label] : [label]
    }

    /// Build the wire answer map, joining selected options and custom text.
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

    /// Every question needs at least one selected option or custom answer.
    static func isComplete(
        questions: [ConversationAskQuestion],
        selected: [Int: [String]],
        custom: [Int: String]
    ) -> Bool {
        answers(questions: questions, selected: selected, custom: custom).count == questions.count
    }

    var body: some View {
        VStack(spacing: 0) {
            header
                .padding(.horizontal, 20)
                .padding(.top, 18)
                .padding(.bottom, 12)

            ScrollView {
                VStack(spacing: 10) {
                    ForEach(questions.indices, id: \.self) { index in
                        questionRow(questions[index], at: index)
                    }
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 8)
            }
            .scrollBounceBehavior(.basedOnSize)
            .scrollDismissesKeyboard(.interactively)

            Divider().overlay(t.border)
            submitAction
                .padding(.horizontal, 20)
                .padding(.vertical, 14)
                .background(t.surface)
        }
        .frame(maxWidth: 640, maxHeight: .infinity, alignment: .top)
        .background(t.surface)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("chat.ask.card.\(question.requestId)")
    }

    private var header: some View {
        HStack(spacing: 11) {
            Image(systemName: "questionmark.bubble")
                .font(.system(size: 18, weight: .medium))
                .foregroundStyle(t.text2)
                .frame(width: 34, height: 34)
                .background(t.surfaceActive, in: Circle())
                .accessibilityHidden(true)
            Text("chat_ask_user_question_title")
                .font(.headline)
                .foregroundStyle(t.text)
                .lineLimit(1)
                .minimumScaleFactor(0.8)
            Spacer(minLength: 0)
            if questions.count > 1 {
                Text("\(answeredCount)/\(questions.count)")
                    .font(.subheadline.monospacedDigit())
                    .foregroundStyle(t.text3)
                    .accessibilityLabel("\(answeredCount) of \(questions.count) answered")
            }
            Button {
                resolve { await onCancel() }
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 14, weight: .medium))
                    .foregroundStyle(t.text3)
                    .frame(width: 36, height: 36)
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .disabled(submitting)
            .accessibilityLabel(String(localized: "common_cancel"))
            .accessibilityIdentifier("chat.ask.cancel")
        }
    }

    private func questionRow(_ current: ConversationAskQuestion, at index: Int) -> some View {
        let answer = answerSummary(at: index)
        let expanded = expandedQuestion == index
        let rowShape = RoundedRectangle(cornerRadius: 18, style: .continuous)

        return VStack(alignment: .leading, spacing: 0) {
            Button {
                guard !submitting else { return }
                focusedQuestionIndex = nil
                withAnimation(reduceMotion ? nil : .spring(response: 0.32, dampingFraction: 0.88)) {
                    expandedQuestion = expanded ? nil : index
                }
            } label: {
                HStack(spacing: 12) {
                    Text("\(index + 1)")
                        .font(.subheadline.weight(.semibold).monospacedDigit())
                        .foregroundStyle(answer == nil ? t.text3 : t.text2)
                        .frame(width: 38, height: 38)
                        .background(t.surfaceActive, in: Circle())
                        .overlay(Circle().stroke(t.border, lineWidth: 0.8))
                    VStack(alignment: .leading, spacing: answer == nil ? 0 : 3) {
                        Text(current.question)
                            .font(.body.weight(expanded ? .medium : .regular))
                            .foregroundStyle(t.text)
                            .fixedSize(horizontal: false, vertical: true)
                        if let answer {
                            Text(answer)
                                .font(.subheadline)
                                .foregroundStyle(t.text3)
                                .lineLimit(2)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    Image(systemName: expanded ? "chevron.up" : "chevron.down")
                        .font(.system(size: 13, weight: .medium))
                        .foregroundStyle(t.text3)
                        .accessibilityHidden(true)
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 10)
                .contentShape(rowShape)
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("chat.ask.question.\(index)")

            if expanded {
                questionOptions(current, at: index)
                    .padding(.leading, 62)
                    .padding(.trailing, 12)
                    .padding(.bottom, 14)
                    .transition(.opacity.combined(with: .move(edge: .top)))
            }
        }
        .background(expanded ? t.surfaceActive.opacity(0.42) : t.surface, in: rowShape)
        .overlay { rowShape.stroke(t.border.opacity(expanded ? 0.95 : 0.65), lineWidth: 0.8) }
        .animation(reduceMotion ? nil : .spring(response: 0.32, dampingFraction: 0.88), value: expanded)
    }

    @ViewBuilder
    private func questionOptions(_ current: ConversationAskQuestion, at index: Int) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            if !current.header.isEmpty {
                Text(current.header)
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(t.text3)
                    .textCase(.uppercase)
            }
            ForEach(current.options.indices, id: \.self) { optionIndex in
                let option = current.options[optionIndex]
                let active = (selected[index] ?? []).contains(option.label)
                Button {
                    guard !submitting else { return }
                    let next = Self.toggled(
                        option.label,
                        in: selected[index] ?? [],
                        multiSelect: current.multiSelect
                    )
                    withAnimation(reduceMotion ? nil : .spring(response: 0.3, dampingFraction: 0.9)) {
                        selected[index] = next
                    }
                    if current.multiSelect {
                        expandedQuestion = index
                    } else if !next.isEmpty {
                        expandedQuestion = questions.indices.first(where: { candidate in
                            candidate != index && !isAnswered(at: candidate)
                        })
                    }
                } label: {
                    HStack(alignment: .top, spacing: 11) {
                        Image(systemName: current.multiSelect
                            ? (active ? "checkmark.square.fill" : "square")
                            : (active ? "checkmark.circle.fill" : "circle"))
                            .font(.title3)
                            .foregroundStyle(active ? t.accent : t.text3)
                            .accessibilityHidden(true)
                        VStack(alignment: .leading, spacing: 4) {
                            Text(option.label)
                                .font(.body.weight(.medium))
                                .foregroundStyle(t.text)
                            if !option.description.isEmpty {
                                Text(option.description)
                                    .font(.subheadline)
                                    .foregroundStyle(t.text3)
                            }
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .multilineTextAlignment(.leading)
                    .padding(13)
                    .frame(maxWidth: .infinity, minHeight: 52, alignment: .leading)
                    .background(active ? t.accent.opacity(0.1) : t.surface, in: .rect(cornerRadius: 15))
                    .overlay {
                        RoundedRectangle(cornerRadius: 15, style: .continuous)
                            .stroke(active ? t.accent : t.border, lineWidth: active ? 1.4 : 0.8)
                    }
                    .contentShape(.rect(cornerRadius: 15))
                }
                .buttonStyle(.plain)
                .disabled(submitting)
                .accessibilityAddTraits(active ? [.isSelected] : [])
                .accessibilityIdentifier("chat.ask.option.\(index).\(optionIndex)")
            }

            VStack(alignment: .leading, spacing: 7) {
                Text("chat_ask_other_option")
                    .font(.subheadline.weight(.medium))
                    .foregroundStyle(t.text3)
                TextField(
                    String(localized: "chat_ask_other_placeholder"),
                    text: Binding(
                        get: { custom[index] ?? "" },
                        set: { custom[index] = $0 }
                    ),
                    axis: .vertical
                )
                .font(.body)
                .textFieldStyle(.plain)
                .lineLimit(2 ... 5)
                .padding(14)
                .background(t.surface, in: .rect(cornerRadius: 15))
                .overlay {
                    RoundedRectangle(cornerRadius: 15, style: .continuous)
                        .stroke(focusedQuestionIndex == index ? t.accent : t.border, lineWidth: 0.8)
                }
                .focused($focusedQuestionIndex, equals: index)
                .disabled(submitting)
                .accessibilityIdentifier("chat.ask.other.\(index)")
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var submitAction: some View {
        Button {
            let answers = Self.answers(questions: questions, selected: selected, custom: custom)
            resolve { await onSubmit(answers) }
        } label: {
            HStack(spacing: 8) {
                if submitting { ProgressView().controlSize(.small) }
                Text("common_submit")
            }
            .frame(maxWidth: .infinity)
        }
        .buttonStyle(.borderedProminent)
        .controlSize(.large)
        .disabled(!canSubmit || submitting)
        .accessibilityIdentifier("chat.ask.submit")
    }

    private func answerSummary(at index: Int) -> String? {
        var labels = selected[index] ?? []
        let free = (custom[index] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if !free.isEmpty { labels.append(free) }
        guard !labels.isEmpty else { return nil }
        return labels.joined(separator: ", ")
    }

    private func isAnswered(at index: Int) -> Bool {
        answerSummary(at: index) != nil
    }

    private func resolve(_ action: @escaping () async -> Bool) {
        guard !submitting else { return }
        submitting = true
        Task {
            let accepted = await action()
            // The view disappears when the engine confirms. A failed submit
            // remains actionable so the user can retry.
            if !accepted { submitting = false }
        }
    }
}
