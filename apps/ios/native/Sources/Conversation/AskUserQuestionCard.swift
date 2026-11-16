import SwiftUI
import harness_runtimeFFI

/// The interactive AskUserQuestion form shown in a native sheet. All questions
/// stay in view as an accordion: answered rows retain the question and answer,
/// while one row at a time shows its options above a shared reply composer.
struct AskUserQuestionCard: View {
    @Environment(\.theme) private var t
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    let question: ConversationPendingQuestion
    /// Returns whether the answer command was submitted; false enables retry.
    let onSubmit: (AskUserQuestionAnswersDto) async -> Bool
    let onCancel: () async -> Bool

    @State private var expandedQuestion: Int? = 0
    /// Selected option indices per question index, in tap order.
    @State private var selected: [Int: [Int]] = [:]
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
    static func toggled(_ optionIndex: Int, in current: [Int], multiSelect: Bool) -> [Int] {
        if let index = current.firstIndex(of: optionIndex) {
            var copy = current
            copy.remove(at: index)
            return copy
        }
        return multiSelect ? current + [optionIndex] : [optionIndex]
    }

    /// Build the wire answer map, joining selected options and custom text.
    static func answers(
        questions: [ConversationAskQuestion],
        selected: [Int: [Int]],
        custom: [Int: String]
    ) -> AskUserQuestionAnswersDto {
        var pairs: [AskUserQuestionAnswerDto] = []
        for (index, question) in questions.enumerated() {
            var parts = (selected[index] ?? []).compactMap { optionIndex -> [UInt16]? in
                guard question.options.indices.contains(optionIndex) else { return nil }
                let option = question.options[optionIndex]
                return option.sourceLabelCodeUnits ?? Array(option.label.utf16)
            }
            let free = (custom[index] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            if !free.isEmpty { parts.append(Array(free.utf16)) }
            guard !parts.isEmpty else { continue }
            var answerUnits: [UInt16] = []
            for (partIndex, units) in parts.enumerated() {
                if partIndex > 0 { answerUnits.append(contentsOf: [44, 32]) }
                answerUnits.append(contentsOf: units)
            }
            pairs.append(AskUserQuestionAnswerDto(
                question: ClientTextDto(codeUnits: question.sourceQuestionCodeUnits ?? Array(question.question.utf16)),
                answer: ClientTextDto(codeUnits: answerUnits)
            ))
        }
        return AskUserQuestionAnswersDto(pairs: pairs)
    }

    /// Every question needs at least one selected option or custom answer.
    static func isComplete(
        questions: [ConversationAskQuestion],
        selected: [Int: [Int]],
        custom: [Int: String]
    ) -> Bool {
        answers(questions: questions, selected: selected, custom: custom).pairs.count == questions.count
    }

    static func answerSummary(
        question: ConversationAskQuestion,
        selected: [Int],
        custom: String
    ) -> String? {
        var labels = selected.compactMap { optionIndex -> String? in
            guard question.options.indices.contains(optionIndex) else { return nil }
            return question.options[optionIndex].label
        }
        let free = custom.trimmingCharacters(in: .whitespacesAndNewlines)
        if !free.isEmpty { labels.append(free) }
        return labels.isEmpty ? nil : labels.joined(separator: ", ")
    }

    var body: some View {
        VStack(spacing: 0) {
            header
                .padding(.horizontal, 20)
                .padding(.top, 12)
                .padding(.bottom, 6)

            ScrollView {
                VStack(spacing: 12) {
                    ForEach(questions.indices, id: \.self) { index in
                        questionRow(questions[index], at: index)
                    }
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 8)
            }
            .scrollBounceBehavior(.basedOnSize)
            .scrollDismissesKeyboard(.interactively)

            submitAction
                .padding(.horizontal, 20)
                .padding(.vertical, 14)
                .background(t.surface)
        }
        .frame(maxWidth: 640, maxHeight: .infinity, alignment: .top)
        .background(t.surface, in: .rect(cornerRadius: 24))
        .overlay { RoundedRectangle(cornerRadius: 24).stroke(t.border, lineWidth: 1) }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("chat.ask.card.\(question.requestId)")
    }

    private var header: some View {
        HStack(spacing: 8) {
            Image("CodexQuestion")
                .resizable()
                .renderingMode(.template)
                .foregroundStyle(t.text2)
                .frame(width: 20, height: 20)
                .accessibilityHidden(true)
            Text("chat_ask_user_question_title")
                .font(.subheadline)
                .foregroundStyle(t.text2)
                .lineLimit(1)
                .minimumScaleFactor(0.8)
            Spacer(minLength: 0)
            if questions.count > 1 {
                Text("\(answeredCount)/\(questions.count)")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(t.text2)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 5)
                    .background(t.text.opacity(0.05), in: Capsule())
                    .accessibilityLabel("\(answeredCount) of \(questions.count) answered")
            }
            Button {
                resolve { await onCancel() }
            } label: {
                Image(systemName: "xmark")
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(t.text2)
                    .frame(width: 44, height: 44)
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
                withAnimation(reduceMotion ? nil : .spring(response: 0.32, dampingFraction: 1)) {
                    expandedQuestion = expanded && questions.count > 1 ? nil : index
                }
            } label: {
                HStack(spacing: 12) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(current.question)
                            .font(.headline)
                            .foregroundStyle(t.text)
                            .fixedSize(horizontal: false, vertical: true)
                        if !expanded, let answer {
                            Text(answer)
                                .font(.subheadline)
                                .foregroundStyle(t.text2)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    if questions.count > 1 {
                        Image(systemName: expanded ? "chevron.up" : "chevron.down")
                            .font(.footnote.weight(.medium))
                            .foregroundStyle(t.text3)
                            .accessibilityHidden(true)
                    }
                }
                .padding(.vertical, 10)
                .contentShape(rowShape)
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("chat.ask.question.\(index)")

            if expanded {
                questionOptions(current, at: index)
                    .padding(.bottom, 6)
                    .transition(.opacity)
            }
        }
        .animation(reduceMotion ? nil : .spring(response: 0.32, dampingFraction: 1), value: expanded)
    }

    @ViewBuilder
    private func questionOptions(_ current: ConversationAskQuestion, at index: Int) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(current.options.indices, id: \.self) { optionIndex in
                let option = current.options[optionIndex]
                let active = (selected[index] ?? []).contains(optionIndex)
                Button {
                    guard !submitting else { return }
                    let next = Self.toggled(
                        optionIndex,
                        in: selected[index] ?? [],
                        multiSelect: current.multiSelect
                    )
                    withAnimation(reduceMotion ? nil : .spring(response: 0.3, dampingFraction: 1)) {
                        selected[index] = next
                    }
                    if current.multiSelect {
                        expandedQuestion = index
                    } else if questions.count > 1 && !next.isEmpty {
                        expandedQuestion = questions.indices.first(where: { candidate in
                            candidate != index && !isAnswered(at: candidate)
                        })
                    }
                } label: {
                    HStack(alignment: .center, spacing: 12) {
                        Text(current.multiSelect && active ? "✓" : "\(optionIndex + 1)")
                            .font(.subheadline.monospacedDigit())
                            .foregroundStyle(t.text2)
                            .frame(width: 26, height: 26)
                            .background(t.text.opacity(0.04), in: .rect(cornerRadius: 6))
                            .overlay { RoundedRectangle(cornerRadius: 6).stroke(t.border, lineWidth: 1) }
                            .accessibilityHidden(true)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(option.label)
                                .font(.subheadline.weight(.semibold))
                                .foregroundStyle(t.text)
                            if !option.description.isEmpty {
                                Text(option.description)
                                    .font(.subheadline)
                                    .foregroundStyle(t.text2)
                            }
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        if active {
                            Image(systemName: "arrow.right")
                                .foregroundStyle(t.text3)
                                .accessibilityHidden(true)
                        }
                    }
                    .multilineTextAlignment(.leading)
                    .padding(.horizontal, 13)
                    .padding(.vertical, 11)
                    .frame(maxWidth: .infinity, minHeight: 56, alignment: .leading)
                    .background(active ? t.text.opacity(0.05) : Color.clear, in: .rect(cornerRadius: 15))
                    .contentShape(.rect(cornerRadius: 12))
                }
                .buttonStyle(AskQuestionOptionButtonStyle())
                .disabled(submitting)
                .accessibilityAddTraits(active ? [.isSelected] : [])
                .accessibilityIdentifier("chat.ask.option.\(index).\(optionIndex)")
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var submitAction: some View {
        let index = expandedQuestion ?? 0
        return VStack(spacing: 6) {
            if expandedQuestion != nil {
                HStack(alignment: .center, spacing: 10) {
                    Image(systemName: "pencil")
                        .foregroundStyle(t.text3)
                        .accessibilityHidden(true)
                    TextField(
                        String(localized: "chat_ask_other_placeholder"),
                        text: Binding(get: { custom[index] ?? "" }, set: { custom[index] = $0 }),
                        axis: .vertical
                    )
                    .font(.body)
                    .textFieldStyle(.plain)
                    .lineLimit(1 ... 4)
                    .frame(minHeight: 44)
                    .focused($focusedQuestionIndex, equals: index)
                    .disabled(submitting)
                    .accessibilityLabel(String(localized: "chat_ask_other_option"))
                    .accessibilityIdentifier("chat.ask.other.\(index)")
                }
            }
            HStack(spacing: 8) {
                Spacer(minLength: 0)
                Button {
                    resolve { await onCancel() }
                } label: {
                    Text("chat_question_skip")
                        .foregroundStyle(t.text)
                        .padding(.horizontal, 16)
                        .frame(minHeight: 44)
                        .overlay { Capsule().stroke(t.border, lineWidth: 1) }
                }
                .buttonStyle(AskQuestionOptionButtonStyle())
                .disabled(submitting)
                Button {
                    let answers = Self.answers(questions: questions, selected: selected, custom: custom)
                    resolve { await onSubmit(answers) }
                } label: {
                    HStack(spacing: 8) {
                        if submitting { ProgressView().controlSize(.small) }
                        Text("composer_send")
                    }
                    .padding(.horizontal, 16)
                    .frame(minHeight: 44)
                    .foregroundStyle(t.surface)
                    .background(t.text, in: Capsule())
                    .opacity(canSubmit ? 1 : 0.4)
                }
                .buttonStyle(AskQuestionOptionButtonStyle())
                .disabled(!canSubmit || submitting)
                .accessibilityIdentifier("chat.ask.submit")
            }
        }
        .padding(8)
        .padding(.leading, 6)
        .overlay {
            RoundedRectangle(cornerRadius: 18, style: .continuous)
                .stroke(focusedQuestionIndex == index ? t.text : t.border, lineWidth: 1)
        }
    }

    private func answerSummary(at index: Int) -> String? {
        Self.answerSummary(
            question: questions[index],
            selected: selected[index] ?? [],
            custom: custom[index] ?? ""
        )
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

private struct AskQuestionOptionButtonStyle: ButtonStyle {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .opacity(configuration.isPressed ? 0.75 : 1)
            .scaleEffect(configuration.isPressed && !reduceMotion ? 0.995 : 1)
    }
}

/// A completed request stays expanded by default; its owner retains disclosure state.
struct AnsweredQuestionsView: View {
    @Environment(\.theme) private var t
    @Environment(\.colorScheme) private var colorScheme
    let rows: [ConversationAnsweredQuestion]
    let isExpanded: Bool
    let onToggle: () -> Void

    private var summaryColor: Color { colorScheme == .dark ? t.text2 : t.text3 }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Button(action: onToggle) {
                HStack(spacing: 8) {
                    Image("CodexQuestion")
                        .resizable().renderingMode(.template)
                        .frame(width: 18, height: 18)
                        .accessibilityHidden(true)
                    Text("chat_asked_questions \(rows.count)")
                    Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                        .font(.caption.weight(.regular))
                        .accessibilityHidden(true)
                }
                .font(.subheadline)
                .foregroundStyle(summaryColor)
                .frame(minHeight: 44)
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .accessibilityValue(String(localized: isExpanded ? "chat_question_expanded" : "chat_question_collapsed"))
            if isExpanded {
                ForEach(rows.indices, id: \.self) { index in
                    VStack(alignment: .leading, spacing: 4) {
                        Text(rows[index].question).foregroundStyle(summaryColor.opacity(0.85))
                        Text(rows[index].answer ?? String(localized: "chat_question_no_answer")).foregroundStyle(summaryColor.opacity(0.5))
                    }
                    .font(.subheadline)
                    .lineSpacing(4)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
        .padding(.vertical, 2)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
