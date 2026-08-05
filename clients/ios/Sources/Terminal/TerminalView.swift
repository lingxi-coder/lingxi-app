import SwiftUI

struct TerminalView: View {
    @State private var model: TerminalSessionModel
    var onDismiss: (() -> Void)?
    var onOpenRuntimeSettings: (() -> Void)?
    var onRepairRuntime: (() -> Void)?

    init(
        model: TerminalSessionModel,
        onDismiss: (() -> Void)? = nil,
        onOpenRuntimeSettings: (() -> Void)? = nil,
        onRepairRuntime: (() -> Void)? = nil
    ) {
        _model = State(initialValue: model)
        self.onDismiss = onDismiss
        self.onOpenRuntimeSettings = onOpenRuntimeSettings
        self.onRepairRuntime = onRepairRuntime
    }

    init(
        descriptor: TerminalRuntimeDescriptor,
        client: TerminalRuntimeClient,
        onDismiss: (() -> Void)? = nil,
        onOpenRuntimeSettings: (() -> Void)? = nil,
        onRepairRuntime: (() -> Void)? = nil
    ) {
        _model = State(initialValue: TerminalSessionModel(descriptor: descriptor, client: client))
        self.onDismiss = onDismiss
        self.onOpenRuntimeSettings = onOpenRuntimeSettings
        self.onRepairRuntime = onRepairRuntime
    }

    var body: some View {
        let content = VStack(spacing: 0) {
            // A terminal is its transcript. Chrome appears only when the shell
            // cannot run — otherwise the screen is the shell, as on a desktop.
            if model.availability.message != nil {
                header
                Divider()
            }
            transcript
            Divider()
            controlKeys
            composer
        }
        .background(Color.black.opacity(0.96))
        .task {
            await model.startIfNeeded()
        }
        .onDisappear {
            Task { await model.close() }
        }
        .accessibilityIdentifier("terminal.root")
        #if os(iOS) || os(tvOS)
            content
                .navigationTitle("settings_linux_section_terminal")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItemGroup(placement: .topBarTrailing) {
                        actionToolbar
                    }
                }
        #else
            content
                .navigationTitle("settings_linux_section_terminal")
                .toolbar {
                    ToolbarItem {
                        Button("common_close") {
                            onDismiss?()
                        }
                    }
                    ToolbarItemGroup {
                        actionToolbar
                    }
                }
        #endif
    }

    /// Shown only when the shell cannot run. Everything that used to live
    /// here — project name, workspace path, requested and actual cwd, initial
    /// command — is diagnostic detail a working terminal does not need on
    /// screen; the shell's own prompt reports the directory.
    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label(recovery.title, systemImage: "exclamationmark.triangle.fill")
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(.orange)
            if let message = model.availability.message {
                Text(message)
                    .font(.caption)
                    .foregroundStyle(.white.opacity(0.9))
            }
            if let cwd = model.descriptor.launchCwd, !cwd.isEmpty {
                Text(cwd)
                    .font(.caption.monospaced())
                    .foregroundStyle(.white.opacity(0.5))
            }
            repairActions
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding()
    }

    private var recovery: TerminalRecovery {
        TerminalRecovery.forState(model.availability)
    }

    @ViewBuilder
    private var repairActions: some View {
        switch recovery {
        case .workspaceNotMounted, .runtimeUnavailable:
            if let onOpenRuntimeSettings {
                Button("terminal_open_runtime_settings") { onOpenRuntimeSettings() }
                    .buttonStyle(.borderedProminent)
                    .tint(.orange)
                    .accessibilityIdentifier("terminal.repair.runtimeSettings")
            }
        case .repairRuntime:
            HStack(spacing: 10) {
                if let onRepairRuntime {
                    Button("terminal_try_repair") { onRepairRuntime() }
                        .buttonStyle(.borderedProminent)
                        .tint(.orange)
                }
                if let onOpenRuntimeSettings {
                    Button("terminal_view_runtime") { onOpenRuntimeSettings() }
                        .buttonStyle(.bordered)
                }
            }
        case .none:
            EmptyView()
        }
    }

    private var transcript: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 2) {
                    ForEach(model.buffer.lines) { line in
                        Text(line.renderedString(defaultForeground: .white))
                            .font(.system(.body, design: .monospaced))
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .textSelection(.enabled)
                            .id(line.id)
                    }
                }
                .padding()
            }
            .background(Color.black)
            .onChange(of: model.buffer.lines.count) {
                if let lastID = model.buffer.lines.last?.id {
                    withAnimation(.easeOut(duration: 0.16)) {
                        proxy.scrollTo(lastID, anchor: .bottom)
                    }
                }
            }
        }
    }

    /// Ctrl-C/D/Z and Esc have no keys on an iOS keyboard, so these stay —
    /// they are the shell's own controls, not extra UI.
    private var controlKeys: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 12) {
                ForEach(TerminalControlKey.allCases) { key in
                    Button(key.rawValue) {
                        Task { await model.sendControl(key) }
                    }
                    .buttonStyle(.bordered)
                    .tint(.white)
                }
            }
            .padding(.horizontal)
            .padding(.vertical, 10)
        }
        .scrollIndicators(.hidden)
    }

    private var composer: some View {
        VStack(spacing: 8) {
            HStack(spacing: 8) {
                Button {
                    model.previousHistory()
                } label: {
                    Image(systemName: "chevron.up")
                }
                .buttonStyle(.bordered)

                Button {
                    model.nextHistory()
                } label: {
                    Image(systemName: "chevron.down")
                }
                .buttonStyle(.bordered)

                TextField("terminal_input_placeholder", text: $model.inputText, axis: .vertical)
                    .font(.system(.body, design: .monospaced))
                    .textFieldStyle(.roundedBorder)
                    .disabled(!model.availability.canInteract)

                Button("composer_send") {
                    Task { await model.submitInput() }
                }
                .buttonStyle(.borderedProminent)
                .disabled(!model.availability.canInteract || model.inputText.isEmpty)
            }

            if let summary = model.selectionSummary {
                Text(summary)
                    .font(.caption)
                    .foregroundStyle(.white.opacity(0.7))
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .padding()
        .background(Color.black)
        .foregroundStyle(.white)
    }

    @ViewBuilder
    private var actionToolbar: some View {
        Button("terminal_copy_button") { model.copyTranscript() }
    }
}
