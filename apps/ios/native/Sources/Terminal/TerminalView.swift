import SwiftUI

/// A terminal is its transcript.
///
/// Everything that is not the shell was removed: the screen title, the framed
/// error banner, the standing Ctrl-key row, the history chevrons, the bordered
/// text box and its Send button, and every divider between them. What is left
/// is one black, monospaced, edge-to-edge surface whose caret sits directly
/// after the shell's own prompt — Return runs the line, the shell echoes it,
/// and the keys an iOS keyboard does not have live above the keyboard, where
/// they cost nothing while you are reading output.
struct TerminalView: View {
    @State private var model: TerminalSessionModel
    @State private var pendingClose: Task<Void, Never>?
    @FocusState private var inputFocused: Bool
    var onDismiss: (() -> Void)?
    var onOpenRuntimeSettings: (() -> Void)?
    // No onRepairRuntime callback exists anymore, deliberately: the only
    // implementation anyone ever wired was byte-identical to
    // `onOpenRuntimeSettings` — it popped the terminal (destroying the
    // session) and opened Settings without calling repair anywhere. Repair
    // lives on the runtime page; the terminal LINKS there
    // (`terminal_view_runtime`) instead of claiming to do it. Removing the
    // parameter makes the harmful re-wiring impossible rather than
    // comment-discouraged.

    /// Stable identity for the input row so the caret keeps focus while the
    /// transcript grows underneath it, and a scroll anchor that always names
    /// the bottom of the session.
    private let promptAnchor = "terminal.prompt"

    init(
        model: TerminalSessionModel,
        onDismiss: (() -> Void)? = nil,
        onOpenRuntimeSettings: (() -> Void)? = nil
    ) {
        _model = State(initialValue: model)
        self.onDismiss = onDismiss
        self.onOpenRuntimeSettings = onOpenRuntimeSettings
    }

    init(
        descriptor: TerminalRuntimeDescriptor,
        client: TerminalRuntimeClient,
        onDismiss: (() -> Void)? = nil,
        onOpenRuntimeSettings: (() -> Void)? = nil
    ) {
        _model = State(initialValue: TerminalSessionModel(descriptor: descriptor, client: client))
        self.onDismiss = onDismiss
        self.onOpenRuntimeSettings = onOpenRuntimeSettings
    }

    var body: some View {
        let content = transcript
            .background(Color.black)
            // `.preferredColorScheme` is presentation-scoped, not
            // subtree-scoped: it walks up to the nearest presentation, which
            // for a `navigationDestination` push is the window. Setting it here
            // flipped the WHOLE app dark for a Light-theme user — visible on
            // the parent during the push animation and the back-swipe, and on
            // the settings sheet this screen can open — while `app.palette`
            // stayed light. The environment key is scoped to this subtree.
            .environment(\.colorScheme, .dark)
            .task {
                await model.startIfNeeded()
                // Second attempt, after the session has settled. Setting
                // `@FocusState` while the view hierarchy is still being
                // installed is silently dropped, and the `onChange` below only
                // fires if `canInteract` actually transitions while this view
                // is observing — a session that is already `.ready` when the
                // screen appears never transitions at all, so that path alone
                // left the terminal with no keyboard and no caret.
                guard model.availability.canInteract else { return }
                try? await Task.sleep(for: .milliseconds(150))
                inputFocused = true
            }
            .onAppear {
                // A disappear that is followed by an appear was a transition,
                // not a dismissal. Cancel the pending teardown.
                pendingClose?.cancel()
                pendingClose = nil
            }
            .onDisappear {
                // Do NOT close the shell here directly. `onDisappear` fires for
                // transient reasons — a sheet presented over this screen, a
                // navigation transition — and `close()` writes `exit\n` into
                // the guest and emits `pty_closed`, which is the ONLY producer
                // of an exit event in the whole bridge (the iSH kernel never
                // reports a guest process exit). So a transient disappear read
                // on screen as "[process exited with code 0]" with an empty
                // transcript, and the user's shell was genuinely gone. A real
                // terminal survives you switching screens; deferring the
                // teardown lets a re-appear cancel it while a real pop still
                // releases the PTY.
                pendingClose?.cancel()
                pendingClose = Task { @MainActor in
                    try? await Task.sleep(for: .milliseconds(700))
                    guard !Task.isCancelled else { return }
                    await model.close()
                }
            }
            .onChange(of: model.availability.canInteract) { _, canInteract in
                // A shell that is ready wants the caret, exactly as opening a
                // terminal on a desktop puts you at the prompt. The immediate
                // set can land in the same transaction that re-enables the
                // field — the silent-drop class the `.task` above retries for
                // — so retry once here too after the update settles, or the
                // keyboard intermittently fails to return after 重新启动 shell.
                if canInteract {
                    inputFocused = true
                    Task { @MainActor in
                        try? await Task.sleep(for: .milliseconds(150))
                        if model.availability.canInteract { inputFocused = true }
                    }
                }
            }
            .toolbar {
                ToolbarItemGroup(placement: .keyboard) {
                    keyboardKeys
                }
            }
            // An identifier is not a label: with the navigation title gone,
            // this is the only thing that names the screen to VoiceOver.
            .accessibilityLabel("settings_linux_section_terminal")
            .accessibilityIdentifier("terminal.root")
        #if os(iOS) || os(tvOS)
            content
                .navigationTitle("")
                .navigationBarTitleDisplayMode(.inline)
                .toolbarBackground(Color.black, for: .navigationBar)
                .toolbarBackground(.visible, for: .navigationBar)
                // The window keeps the user's Light scheme (the dark
                // environment above is subtree-scoped by design), so without
                // this the status bar keeps its black glyphs — clock, battery
                // and Wi-Fi invisible on the black bar for a Light-theme user.
                .toolbarColorScheme(.dark, for: .navigationBar)
                .toolbar {
                    ToolbarItem(placement: .topBarTrailing) {
                        Button {
                            model.copyTranscript()
                        } label: {
                            Image(systemName: "doc.on.doc")
                        }
                        .accessibilityLabel("terminal_copy_button")
                    }
                }
        #else
            content
                .navigationTitle("")
                .toolbar {
                    ToolbarItem {
                        Button("common_close") {
                            onDismiss?()
                        }
                    }
                    ToolbarItem {
                        Button("terminal_copy_button") { model.copyTranscript() }
                    }
                }
        #endif
    }

    private var transcript: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    // Everything above the cursor, then the cursor's own line
                    // with the caret on it, then everything below. The caret
                    // belongs on `cursorRow`, not on the last line: the parser
                    // implements cursor motion, and `ensureCursorVisible` only
                    // ever grows `lines` — `CSI J` empties rows without
                    // removing them. After a `\e[H\e[J` clear, `cursorRow` is 0
                    // with 200 blank rows still below it, and pinning the caret
                    // to `lines.last` would strand the shell's live prompt in
                    // the middle of the buffer with the viewport scrolled to a
                    // blank line far below it.
                    ForEach(model.buffer.lines.prefix(cursorRow)) { line in
                        transcriptRow(line)
                    }
                    promptRow
                    ForEach(model.buffer.lines.dropFirst(cursorRow + 1)) { line in
                        transcriptRow(line)
                    }
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 6)
            }
            .scrollDismissesKeyboard(.interactively)
            .contentShape(Rectangle())
            .onTapGesture {
                // Unconditional: a disabled field simply refuses focus, and
                // gating the gesture meant that if auto-focus ever missed, the
                // user had no way at all to summon the keyboard.
                inputFocused = true
            }
            .onChange(of: scrollKey) {
                withAnimation(.easeOut(duration: 0.12)) {
                    proxy.scrollTo(promptAnchor, anchor: .bottom)
                }
            }
        }
    }

    /// What changed on the anchored cluster, in O(1).
    ///
    /// `lines.count` looks like the obvious trigger and is the one thing that
    /// cannot work: `trimScrollbackIfNeeded` drops a line for every line it
    /// appends past `maxScrollback`, so past 2000 lines the count is pinned and
    /// `onChange` never fires again — the terminal stops following output for
    /// the rest of the session, in exactly the long builds this screen exists
    /// for. The last line's id is monotonic, and its cell count moves when the
    /// shell rewrites the open line (a `\r` progress bar, a `read -p` prompt),
    /// which the id alone would miss. The notice fields are here because the
    /// buffer does not change when the SHELL does: an exit or failure grows
    /// the prompt cluster without touching a single line, and without a
    /// re-pin the new rows sit just below the fold.
    private struct ScrollKey: Equatable {
        var lineCount: Int
        var cursorRow: Int
        var cursorLineID: Int
        var cursorCellCount: Int
        var showsExitNotice: Bool
        var availabilityMessage: String?
        var canRestart: Bool
        var isStarting: Bool
        var selectionSummary: String?
    }

    private var scrollKey: ScrollKey {
        let cursor = cursorLine
        return ScrollKey(
            lineCount: model.buffer.lines.count,
            cursorRow: cursorRow,
            cursorLineID: cursor?.id ?? -1,
            cursorCellCount: cursor?.cells.count ?? 0,
            showsExitNotice: model.exitNotice != nil,
            availabilityMessage: model.availability.message,
            canRestart: model.canRestart,
            isStarting: model.isStarting,
            selectionSummary: model.selectionSummary
        )
    }

    /// Clamped, because the view reads two independently-published values and
    /// a body evaluation can land between the buffer shrinking and `cursorRow`
    /// catching up. An out-of-range `prefix`/`dropFirst` would trap.
    private var cursorRow: Int {
        min(max(model.buffer.cursorRow, 0), max(model.buffer.lines.count - 1, 0))
    }

    private var cursorLine: TerminalBufferLine? {
        let lines = model.buffer.lines
        // The clamp in `cursorRow` already bounds the index whenever `lines`
        // is non-empty; the only case left is the empty buffer itself.
        return lines.isEmpty ? nil : lines[cursorRow]
    }

    private func transcriptRow(_ line: TerminalBufferLine) -> some View {
        Text(line.renderedString(defaultForeground: .white))
            .font(TerminalTypography.font)
            .frame(maxWidth: .infinity, alignment: .leading)
            .textSelection(.enabled)
            .id(line.id)
    }

    /// A terminal says so when its shell goes away. Without this the screen is
    /// a black rectangle that silently stopped accepting input: `.closed`
    /// carries no message, and the rewrite removed every control that used to
    /// render its disabled state (the bordered field, the prominent Send
    /// button, the standing key row), so nothing was left to look wrong.
    private func exitNoticeRow(_ notice: String) -> some View {
        Text(verbatim: notice)
            .font(TerminalTypography.font)
            .foregroundStyle(.white.opacity(0.55))
            .frame(maxWidth: .infinity, alignment: .leading)
            .textSelection(.enabled)
            .padding(.top, 4)
    }

    private var restartLink: some View {
        terminalLink("terminal_restart_session") {
            Task { await model.restart() }
        }
        .accessibilityIdentifier("terminal.restart")
        .padding(.top, 4)
    }

    /// The shell's last (still open) line and the caret, on one row — what a
    /// terminal looks like when you are typing at the prompt. The prompt text
    /// truncates from the head rather than pushing the caret off screen, and
    /// the row's identity never changes, so focus survives every command.
    ///
    /// The session's status lines live INSIDE this scroll anchor, not as
    /// trailing rows of the LazyVStack: the auto-scroll pins this cluster's
    /// bottom edge to the viewport, so a notice that appears is on screen by
    /// construction. As trailing rows they sat below the below-cursor tail —
    /// after a `CSI J` clear that is hundreds of emptied rows — and nothing
    /// ever scrolled to them, because the buffer does not change when the
    /// shell dies: the exit line, the failure message and the ONLY way back
    /// (重新启动 shell) all rendered below the fold of a screen that looked
    /// simply dead. A shell prints its errors where the cursor is; this is
    /// where the cursor is.
    private var promptRow: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline, spacing: 0) {
                if let tail = cursorLine {
                    Text(tail.renderedString(defaultForeground: .white))
                        .font(TerminalTypography.font)
                        .lineLimit(1)
                        .truncationMode(.head)
                        .textSelection(.enabled)
                }
                inputField
                    .frame(minWidth: 120, alignment: .leading)
                    // A TextField draws its caret only while it holds focus,
                    // and this one is borderless with an empty placeholder —
                    // so an unfocused terminal had no cursor anywhere on
                    // screen, which reads as a dead app rather than as "tap to
                    // type". A real terminal keeps a block where the cursor is
                    // and hollows it when the window loses focus; this is that
                    // block.
                    .overlay(alignment: .leading) {
                        if !inputFocused && model.inputText.isEmpty {
                            Text(verbatim: "▏")
                                .font(TerminalTypography.font)
                                .foregroundStyle(.green.opacity(model.availability.canInteract ? 0.85 : 0.3))
                                .allowsHitTesting(false)
                                .accessibilityHidden(true)
                        }
                    }
            }
            if let notice = model.exitNotice {
                exitNoticeRow(notice)
            }
            if let message = model.availability.message {
                unavailableNotice(message)
            }
            // `.idle` and `.opening` carry no message, offer no restart and
            // disable the field — and a disabled, empty, borderless TextField
            // draws nothing at all. Without this line the screen while the
            // runtime boots is an entirely silent black rectangle: no text,
            // no caret, no keyboard, and nothing to distinguish "still
            // connecting" from "broken".
            if model.isStarting {
                Text("terminal_status_connecting")
                    .font(TerminalTypography.font)
                    .foregroundStyle(.white.opacity(0.45))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .accessibilityIdentifier("terminal.status.connecting")
            }
            // THE remedy render — one decision table (`TerminalRecovery`),
            // one renderer. Covers `.closed` (exit notice above, no
            // availability message) as well as the message-carrying states,
            // so it lives here rather than inside `unavailableNotice`.
            repairActions
                .font(TerminalTypography.font)
            if let summary = model.selectionSummary {
                Text(summary)
                    .font(TerminalTypography.font)
                    .foregroundStyle(.green.opacity(0.8))
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .id(promptAnchor)
    }

    private var inputField: some View {
        let field = TextField("", text: $model.inputText)
            .font(TerminalTypography.font)
            .foregroundStyle(.white)
            .tint(.green)
            .textFieldStyle(.plain)
            .autocorrectionDisabled()
            .focused($inputFocused)
            .submitLabel(.return)
            .onSubmit {
                // Keep the caret. Ctrl-C now lives only above the keyboard, so
                // letting Return resign first responder would take the
                // interrupt key away at the exact moment a command starts
                // running — when interrupting is the thing you need.
                inputFocused = true
                Task { await model.submitInput() }
            }
            // `canAcceptInput`, not `canInteract`: `.failed` keeps a live
            // handle on purpose, and a disabled field refuses focus — which
            // dismisses the keyboard and takes the ^C above it away at the
            // exact moment a runaway command needs interrupting.
            .disabled(!model.canAcceptInput)
            .accessibilityLabel("terminal_input_placeholder")
            .accessibilityIdentifier("terminal.input")
        // Each platform branch compiles alone, so `some View` resolves without
        // erasing the field — AnyView here discarded the structural identity
        // that keeps the focused TextField alive across body re-evaluations.
        #if os(iOS) || os(tvOS)
            // A shell takes ASCII. Autocapitalisation and smart punctuation
            // silently corrupt commands, so the terminal asks for the plain
            // keyboard instead of correcting after the fact.
            return field
                .textInputAutocapitalization(.never)
                .keyboardType(.asciiCapable)
        #else
            return field
        #endif
    }

    /// Ctrl-C/D/Z and Esc have no keys on an iOS keyboard, and neither does
    /// shell history here. They ride above the keyboard, so they exist only
    /// while you are typing.
    @ViewBuilder
    private var keyboardKeys: some View {
        ForEach(TerminalControlKey.allCases) { key in
            Button(key.compactLabel) {
                Task { await model.sendControl(key) }
            }
            .font(TerminalTypography.font)
        }
        // Icon-only buttons announce their SF Symbol name to VoiceOver without
        // an explicit label — "arrow.up" instead of 上一条命令.
        Button {
            model.previousHistory()
        } label: {
            Image(systemName: "arrow.up")
        }
        .accessibilityLabel("terminal_history_previous")
        Button {
            model.nextHistory()
        } label: {
            Image(systemName: "arrow.down")
        }
        .accessibilityLabel("terminal_history_next")
        Spacer()
        Button {
            inputFocused = false
        } label: {
            Image(systemName: "keyboard.chevron.compact.down")
        }
        .accessibilityLabel("terminal_dismiss_keyboard")
        .accessibilityIdentifier("terminal.keyboard.dismiss")
    }

    /// A shell that cannot start says so the way a shell does — as a line of
    /// text at the top of the session, not as a framed banner. The recovery
    /// actions read as terminal links rather than filled buttons.
    private func unavailableNotice(_ message: String) -> some View {
        // A `nil` title (`.restartSession` / `.none`) renders the message
        // BARE: prefixing "终端不可用" one row above a working restart link
        // told the user the terminal cannot be used, and they popped the
        // screen instead. The remedy links render once, from the shared
        // `repairActions` table in the prompt group — not here.
        Text(verbatim: recovery.title.map { "\($0): \(message)" } ?? message)
            .foregroundStyle(Color(red: 1.0, green: 0.45, blue: 0.4))
            .frame(maxWidth: .infinity, alignment: .leading)
            .textSelection(.enabled)
            .font(TerminalTypography.font)
            .padding(.bottom, 6)
    }

    private var recovery: TerminalRecovery {
        TerminalRecovery.forState(model.availability)
    }

    @ViewBuilder
    private var repairActions: some View {
        switch recovery {
        case .workspaceNotMounted, .runtimeUnavailable:
            if let onOpenRuntimeSettings {
                terminalLink("terminal_open_runtime_settings", action: onOpenRuntimeSettings)
                    .accessibilityIdentifier("terminal.repair.runtimeSettings")
            }
        case .repairRuntime:
            // The remedy IS the runtime page (install/repair/reset live
            // there); an in-terminal "try repair" link was removed — see the
            // note on the stored callbacks above.
            if let onOpenRuntimeSettings {
                terminalLink("terminal_view_runtime", action: onOpenRuntimeSettings)
            }
        case .restartSession:
            // Both stopped states get the way back in place. `.failed` keeps
            // its live handle (a polling error is not proof the process
            // died); `.closed` restarts fresh.
            restartLink
        case .none:
            EmptyView()
        }
    }

    private func terminalLink(_ key: LocalizedStringKey, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(key)
                .font(TerminalTypography.font)
                .underline()
                .foregroundStyle(.cyan)
        }
        .buttonStyle(.plain)
    }
}
