import SwiftUI

/// The live view of a generation in flight: what the model is saying right
/// now, and a place to say something back.
///
/// Everything visible here is borrowed from the conversation screen —
/// `TranscriptScroll` for the scrolling and the stay-pinned rule,
/// `MessageBubble` for the blocks, and through it `AIText` for markdown and
/// code. Generating an app IS a conversation with a model; rendering it with a
/// second, lesser set of views would mean two things to maintain and two
/// places for code blocks to look wrong.
///
/// The pipeline stage stays visible underneath the transcript: the words say
/// what the model is thinking, the stage says how far along the job is, and
/// neither answers the other's question.
struct LocalAppGenerationTranscriptView: View {
    /// Passed in, never read from the environment. Nothing injects a
    /// `LocalAppsStore` into the environment — the app root installs only
    /// `AppState` and `LocalizationManager` — so an
    /// `@Environment(LocalAppsStore.self)` here force-unwrapped a missing
    /// value and trapped the moment this view laid out. Every other view in
    /// this module takes the store as a property; so does this one.
    @Bindable var store: LocalAppsStore
    @Environment(\.theme) private var theme

    let appID: String
    /// Workflow label shown beside the stage — the caller already resolved it.
    let workflowLabel: String
    /// Whether the composer accepts input right now. Owned by the caller
    /// because "may I revise this app" is a workflow question, not a view one.
    let acceptsInput: Bool

    @State private var draft = ""
    @State private var isSubmitting = false
    @State private var followsLatest = true
    @FocusState private var inputFocused: Bool

    private var blocks: [LocalAppTranscriptBlock] {
        store.generationTranscript[appID] ?? []
    }

    private var progress: LocalAppGenerationProgress? {
        store.generationProgress[appID]
    }

    var body: some View {
        VStack(spacing: 0) {
            transcript
            Divider()
            statusLine
            if acceptsInput {
                composer
            }
        }
    }

    private var transcript: some View {
        TranscriptScroll(
            // The trailing block GROWS as chunks arrive, so a count alone
            // would not change and the view would stop following mid-answer.
            follow: FollowSignal(blockCount: blocks.count, tailLength: blocks.last?.text.count ?? 0),
            followsLatest: $followsLatest,
            focused: inputFocused,
            accessibilityIdentifier: "local-apps.generation.transcript"
        ) {
            if blocks.isEmpty {
                waitingRow
            }
            ForEach(blocks) { block in
                MessageBubble(
                    message: Message(role: .ai, text: block.text),
                    detail: ConversationMessageDetail(blocks: [block.renderBlock])
                )
                .equatable()
            }
        }
        .frame(maxHeight: .infinity)
    }

    /// Before the first chunk lands there is genuinely nothing to show — the
    /// request is open and the model has not spoken. Say that, rather than
    /// leaving an empty rectangle that reads as a failure.
    private var waitingRow: some View {
        HStack(spacing: 8) {
            ProgressView()
            Text("local_apps_generation_waiting_for_model")
                .font(.footnote)
                .foregroundStyle(theme.text4)
        }
        .padding(.vertical, 12)
    }

    private var statusLine: some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text(workflowLabel)
                .font(.footnote)
            if let percent = progress?.percent {
                Text("\(percent)%")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(theme.text4)
            }
            Spacer()
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .accessibilityIdentifier("local-apps.generation.status")
    }

    private var composer: some View {
        // Argument ORDER matters: `Composer`'s memberwise init is positional
        // past the labels, and its model/picker parameters come first even
        // though this screen has no model picker to offer.
        Composer(
            model: .constant(Self.unusedModel),
            draft: $draft,
            onSend: { text in Task { await submit(text) } },
            streaming: isSubmitting,
            sendEnabled: !isSubmitting,
            inputFocused: $inputFocused
        )
    }

    /// Guards against a double-tap firing two revision round trips — the same
    /// defect the detail screen's revision bar had to close.
    private func submit(_ text: String) async {
        guard !isSubmitting else { return }
        let feedback = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !feedback.isEmpty else { return }
        isSubmitting = true
        let succeeded = await store.requestRevision(appID: appID, feedback: feedback)
        isSubmitting = false
        if succeeded {
            // A new run's output must not read as a continuation of the last
            // one's. Cleared on START rather than on finish, so a failed run's
            // last words stay readable until the user asks for another.
            store.clearTranscript(appID: appID)
        }
    }

    /// Block count plus the length of the growing tail — the pair that changes
    /// on every arriving chunk, including ones that only extend the last block.
    private struct FollowSignal: Equatable {
        let blockCount: Int
        let tailLength: Int
    }

    /// `Composer` requires a bound `ModelOption` for its picker chip. This
    /// screen has no model to switch — the generation follows whatever the
    /// session already selected — so it binds an inert placeholder rather than
    /// showing a picker that cannot mean anything here.
    private static let unusedModel = ModelOption(
        id: "",
        name: "",
        desc: "",
        tag: "",
        color: .clear
    )
}

extension LocalAppTranscriptBlock {
    /// Project onto the conversation's render block, so the shared
    /// `MessageBubble` styles thinking and text exactly as it does in chat.
    var renderBlock: ConversationMessageBlock {
        switch kind {
        case .thinking: .thinking(text: text, signature: nil)
        case .text: .text(text)
        }
    }
}
