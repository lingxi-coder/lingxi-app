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
            header
            Divider()
            transcript
            Divider()
            toolbar
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
                .navigationTitle("终端")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        Button("关闭") {
                            onDismiss?()
                        }
                    }
                    ToolbarItemGroup(placement: .topBarTrailing) {
                        actionToolbar
                    }
                }
        #else
            content
                .navigationTitle("终端")
                .toolbar {
                    ToolbarItem {
                        Button("关闭") {
                            onDismiss?()
                        }
                    }
                    ToolbarItemGroup {
                        actionToolbar
                    }
                }
        #endif
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(model.descriptor.workspace.displayName)
                .font(.headline)
                .foregroundStyle(.white)
            Text(model.descriptor.workspace.guestPath)
                .font(.caption.monospaced())
                .foregroundStyle(.white.opacity(0.75))
            if let requestedCwd = model.descriptor.requestedCwdDisplay {
                Text("请求 cwd · \(requestedCwd)")
                    .font(.caption.monospaced())
                    .foregroundStyle(.white.opacity(0.75))
            }
            Text("实际 cwd · \(model.descriptor.launchCwd.flatMap { $0.isEmpty ? nil : $0 } ?? "未解析")")
                .font(.caption.monospaced())
                .foregroundStyle(.white.opacity(0.75))
            if let initialCommand = model.descriptor.initialCommand {
                Text("初始命令 · \(initialCommand)")
                    .font(.caption.monospaced())
                    .foregroundStyle(.white.opacity(0.75))
                    .lineLimit(2)
            }
            if let availabilityMessage = model.availability.message {
                Text(availabilityMessage)
                    .font(.caption)
                    .foregroundStyle(.orange)
                repairActions
            } else if let exit = model.lastExitStatus {
                Text(exit.timedOut ? "会话已超时结束" : "会话已结束 · exit \(exit.code ?? 0)")
                    .font(.caption)
                    .foregroundStyle(.white.opacity(0.75))
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding()
    }

    @ViewBuilder
    private var repairActions: some View {
        switch model.availability {
        case .unavailable, .workspaceUnavailable:
            if let onOpenRuntimeSettings {
                Button("打开运行时设置") { onOpenRuntimeSettings() }
                    .buttonStyle(.borderedProminent)
                    .tint(.orange)
            }
        case .integrityFailure:
            HStack(spacing: 10) {
                if let onRepairRuntime {
                    Button("尝试修复") { onRepairRuntime() }
                        .buttonStyle(.borderedProminent)
                        .tint(.orange)
                }
                if let onOpenRuntimeSettings {
                    Button("查看运行时") { onOpenRuntimeSettings() }
                        .buttonStyle(.bordered)
                }
            }
        default:
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

    private var toolbar: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 12) {
                ForEach(TerminalControlKey.allCases) { key in
                    Button(key.rawValue) {
                        Task { await model.sendControl(key) }
                    }
                    .buttonStyle(.bordered)
                    .tint(.white)
                }

                ForEach(model.tasks) { task in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(task.title)
                            .font(.caption.weight(.semibold))
                        Text(task.detail ?? statusLabel(task.state))
                            .font(.caption2)
                            .foregroundStyle(.white.opacity(0.7))
                    }
                    .padding(.horizontal, 10)
                    .padding(.vertical, 8)
                    .background(Color.white.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
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

                TextField("输入命令", text: $model.inputText, axis: .vertical)
                    .font(.system(.body, design: .monospaced))
                    .textFieldStyle(.roundedBorder)
                    .disabled(!model.availability.canInteract)

                Button("发送") {
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
        Button("复制") { model.copyTranscript() }
        Button("刷新任务") {
            Task { await model.refreshTasks() }
        }
    }

    private func statusLabel(_ state: TerminalTaskState) -> String {
        switch state {
        case .running: return "运行中"
        case .completed: return "完成"
        case .failed: return "失败"
        case .cancelled: return "已取消"
        case .unavailable: return "不可用"
        }
    }
}
