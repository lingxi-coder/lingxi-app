import SwiftUI

/// Bot services (Bots offered outside Teams) as a sheet of the Teams tab: the services the person uses, a service
/// looked up by its link with its terms, and an open service's sessions. Nothing is offered from a phone.
struct HostedServicesView: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.scenePhase) private var scenePhase
    @State private var model = HostedServicesModel()
    @State private var link = ""
    @State private var path: [String] = []
    /// A link the person followed into the app; looked up once, never entered by itself.
    var initialLink: String?

    var body: some View {
        NavigationStack(path: $path) {
            List {
                Section {
                    Text("services_intro").font(.footnote).foregroundStyle(.secondary)
                }
                Section("services_lookup_label") {
                    TextField(text: $link, prompt: Text(verbatim: "https://…/services/s_…")) { Text("services_lookup_label") }
                        .textInputAutocapitalization(.never).autocorrectionDisabled().keyboardType(.URL).submitLabel(.search)
                        .onSubmit(lookup)
                        .accessibilityIdentifier("services.link")
                    Button("services_lookup_action", action: lookup)
                        .disabled(model.busy || link.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        .accessibilityIdentifier("services.lookup")
                }
                if !model.error.isEmpty && model.open == nil {
                    Section { Text(LocalizedStringKey(HostedService.errorKey(model.error))).foregroundStyle(.red).accessibilityIdentifier("services.error") }
                }
                Section("services_used") {
                    if model.listed && model.entered.isEmpty { Text("services_none").foregroundStyle(.secondary) }
                    ForEach(model.entered) { service in
                        Button { model.show(service); path = [service.serviceId] } label: { HostedServiceRow(service: service) }
                            .accessibilityIdentifier("services.entered.\(service.serviceId)")
                    }
                }
            }
            .navigationTitle("services_title")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("common_done") { dismiss() } } }
            .refreshable { try? await model.refresh() }
            .navigationDestination(for: String.self) { _ in HostedServiceSessionView(model: model) }
            .sheet(item: $model.candidate) { service in
                HostedServiceTermsSheet(service: service, busy: model.busy) {
                    Task { await model.enter(); if model.open != nil { path = [service.serviceId] } }
                }
            }
        }
        .onAppear { model.start(link: initialLink) }
        .onDisappear { model.stop() }
        .onChange(of: scenePhase) { _, phase in if phase == .active { model.start() } else { model.pause() } }
        .onChange(of: path) { _, value in if value.isEmpty { model.closeService() } }
        .onChange(of: model.open?.serviceId) { _, id in if id == nil, !path.isEmpty { path = [] } }
    }

    private func lookup() {
        let value = link
        Task { await model.lookup(value); if model.open != nil, let id = model.open?.serviceId { path = [id] } }
    }
}

private struct HostedServiceRow: View {
    let service: HostedServiceView
    var body: some View {
        HStack(spacing: 12) {
            AgentAvatar(agentID: AvatarIdentity.teamBotAvatarId(service.sharedBotId), size: 36)
            VStack(alignment: .leading, spacing: 2) {
                Text(service.name).font(.body.weight(.semibold)).foregroundStyle(.primary).lineLimit(1)
                Text("services_offered_by \(service.publisherName)").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                Text(LocalizedStringKey(HostedService.availabilityKey(service.availability))).font(.caption2)
                    .foregroundStyle(service.available ? Color.green : Color.secondary)
            }
            Spacer(minLength: 8)
            Image(systemName: "chevron.right").font(.caption).foregroundStyle(.tertiary)
        }
        .contentShape(Rectangle())
    }
}

/// What a person is told before they enter: who offers the Bot, who pays, who can read the session, what is kept, how
/// much they may ask. "Agree and open" is the acknowledgement the service records.
private struct HostedServiceTermsSheet: View {
    @Environment(\.dismiss) private var dismiss
    let service: HostedServiceView
    let busy: Bool
    let onAgree: () -> Void

    var body: some View {
        NavigationStack {
            List {
                Section {
                    HStack(spacing: 12) {
                        AgentAvatar(agentID: AvatarIdentity.teamBotAvatarId(service.sharedBotId), size: 44)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(service.name).font(.headline)
                            Text(LocalizedStringKey(HostedService.availabilityKey(service.availability))).font(.caption).foregroundStyle(.secondary)
                        }
                    }
                    if !service.description.isEmpty { Text(service.description) }
                    Text("services_terms_scope \(service.publisherName)").font(.subheadline)
                }
                Section {
                    Label { Text("services_terms_readers \(service.publisherName)") } icon: { Image(systemName: "eye") }
                    Label { Text("services_terms_cost \(service.publisherName)") } icon: { Image(systemName: "creditcard") }
                    Label { VStack(alignment: .leading, spacing: 2) { Text("services_terms_retention").font(.caption).foregroundStyle(.secondary); Text(service.retention) } }
                        icon: { Image(systemName: "archivebox") }
                    Label {
                        if let daily = service.limits.userDaily { Text("services_terms_daily \(daily) \(service.limits.userConcurrent)") }
                        else { Text("services_terms_concurrent \(service.limits.userConcurrent)") }
                    } icon: { Image(systemName: "gauge.with.dots.needle.33percent") }
                    Label { Text("services_terms_reach") } icon: { Image(systemName: "flag") }
                }
                .font(.subheadline)
            }
            .navigationTitle(service.name)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("services_terms_decline") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("services_terms_agree") { dismiss(); onAgree() }
                        .disabled(busy || service.withdrawn)
                        .accessibilityIdentifier("services.agree")
                }
            }
        }
        .presentationDetents([.large])
    }
}

/// An open service: its sessions, the open one's messages and requests, and the box to ask or answer.
private struct HostedServiceSessionView: View {
    @Bindable var model: HostedServicesModel
    @State private var text = ""
    @State private var reporting = false
    @State private var leaving = false
    @FocusState private var focused: Bool

    var body: some View {
        if let service = model.open {
            VStack(spacing: 0) {
                if !service.available {
                    TeamsBanner(kind: .offline, title: LocalizedStringKey(HostedService.availabilityKey(service.availability)),
                                detail: service.withdrawn ? nil : "services_cannot_answer")
                }
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 0) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text("services_note \(service.publisherName)")
                                if let left = service.limits.userDailyLeft { Text("services_left_today \(left)") }
                            }
                            .font(.footnote).foregroundStyle(.secondary).padding(.horizontal, 16).padding(.vertical, 8)
                            if model.messages.isEmpty && model.tasks.isEmpty {
                                Text("services_empty").font(.subheadline).foregroundStyle(.secondary).padding(16)
                            }
                            ForEach(model.messages) { message in HostedServiceMessageCell(message: message, service: service, uid: model.viewer) }
                            ForEach(model.tasks.filter { $0.state != "completed" }) { task in taskLine(task) }
                            Color.clear.frame(height: 1).id("end")
                        }
                    }
                    .onChange(of: model.messages.count + model.tasks.count) { _, _ in withAnimation { proxy.scrollTo("end", anchor: .bottom) } }
                }
                if !model.error.isEmpty {
                    Text(LocalizedStringKey(HostedService.errorKey(model.error))).font(.footnote).foregroundStyle(.red)
                        .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 16).padding(.top, 6)
                        .accessibilityIdentifier("services.session.error")
                }
                composer(service)
            }
            .navigationTitle(service.name)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .primaryAction) {
                    Menu {
                        if !model.sessions.isEmpty {
                            Picker("services_sessions", selection: Binding(get: { model.session ?? "" }, set: { model.select($0) })) {
                                ForEach(model.sessions) { Text($0.title).tag($0.id) }
                            }
                        }
                        Button("services_new_session", systemImage: "plus") {
                            Task { _ = await model.newSession(title: Date().formatted(date: .abbreviated, time: .shortened)) }
                        }
                        .disabled(model.busy)
                        Divider()
                        Button("services_report", systemImage: "flag") { reporting = true }
                        Button("services_leave", systemImage: "rectangle.portrait.and.arrow.right", role: .destructive) { leaving = true }
                    } label: { Image(systemName: "ellipsis.circle") }
                    .accessibilityIdentifier("services.menu")
                }
            }
            .sheet(isPresented: $reporting) { HostedServiceReportSheet(model: model) }
            .alert(Text("services_leave_title \(service.name)"), isPresented: $leaving) {
                Button("common_cancel", role: .cancel) {}
                Button("services_leave", role: .destructive) { Task { _ = await model.leave() } }
            } message: { Text("services_leave_body") }
        }
    }

    @ViewBuilder
    private func taskLine(_ task: TeamTask) -> some View {
        if let key = HostedService.taskKey(task) {
            VStack(alignment: .leading, spacing: 6) {
                Label { Text(LocalizedStringKey(key)) } icon: {
                    if task.busy { ProgressView().controlSize(.mini) } else { Image(systemName: task.terminal ? "exclamationmark.circle" : "questionmark.bubble") }
                }
                .font(.footnote).foregroundStyle(.secondary)
                HStack(spacing: 12) {
                    if model.approval?.taskId == task.taskId {
                        Button("teams_approval_approve") { Task { await model.decide(approve: true) } }.buttonStyle(.borderedProminent)
                        Button("teams_approval_decline") { Task { await model.decide(approve: false) } }.buttonStyle(.bordered)
                    }
                    if !task.terminal {
                        Button("services_task_cancel") { Task { await model.cancel(task.taskId) } }
                            .buttonStyle(.borderless).disabled(task.cancelling)
                    }
                }
                .controlSize(.small).disabled(model.busy)
            }
            .padding(.horizontal, 58).padding(.vertical, 6)
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("services.task.\(task.taskId)")
        }
    }

    private func composer(_ service: HostedServiceView) -> some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField(text: $text, prompt: model.answering ? Text("services_answer_hint") : Text("services_ask_hint \(service.name)"), axis: .vertical) { Text("services_ask_hint \(service.name)") }
                .lineLimit(1...6)
                .focused($focused)
                .padding(.horizontal, 12).padding(.vertical, 8)
                .background(Color(uiColor: .secondarySystemBackground), in: RoundedRectangle(cornerRadius: 18))
                .disabled(service.withdrawn)
                .accessibilityIdentifier("services.composer")
            Button {
                let sent = text
                Task { if await model.send(sent), text == sent { text = "" } }
            } label: { Image(systemName: "arrow.up.circle.fill").font(.title) }
            .disabled(model.busy || service.withdrawn || text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                      || TeamsComposeLogic.byteLength(text) > TeamsComposeLogic.messageBytes)
            .accessibilityLabel(Text("teams_message_send"))
            .accessibilityIdentifier("services.send")
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
        .background(.bar)
    }
}

private struct HostedServiceMessageCell: View {
    @Environment(\.locale) private var locale
    let message: TeamMessage
    let service: HostedServiceView
    let uid: String

    var body: some View {
        let bot = message.author.kind != "user", recalled = message.recalled == true
        HStack(alignment: .top, spacing: 10) {
            if bot { AgentAvatar(agentID: AvatarIdentity.teamBotAvatarId(service.sharedBotId), size: 32) } else { PersonAvatar(id: uid, name: "", size: 32) }
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    if bot { Text(service.name).font(.subheadline.weight(.semibold)).lineLimit(1) } else { Text("teams_preview_you").font(.subheadline.weight(.semibold)) }
                    if bot && message.waitId != nil {
                        Text("services_question").font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                            .padding(.horizontal, 5).padding(.vertical, 1).background(Color(uiColor: .tertiarySystemFill), in: Capsule())
                    }
                    let clock = TeamsStreamLogic.formatClock(atMs: message.createdAt.flatMap(Self.milliseconds), locale: locale)
                    if !clock.isEmpty { Text(clock).font(.caption).foregroundStyle(.secondary) }
                }
                if recalled {
                    Text("teams_message_recalled").italic().foregroundStyle(.secondary)
                } else if TeamsBotText.rendersMarkdown(authorKind: message.author.kind, recalled: recalled) {
                    // The Bot writes Markdown: the chat's renderer, web links only.
                    AIText(markdown: message.text).teamBotLinkPolicy()
                } else {
                    Text(message.text).textSelection(.enabled)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.horizontal, 16).padding(.top, 10).padding(.bottom, 1)
        .contextMenu { if !recalled && !message.text.isEmpty { Button("chat_message_copy", systemImage: "doc.on.doc") { UIPasteboard.general.string = message.text } } }
        .accessibilityIdentifier("services.message.\(message.id)")
    }

    private static func milliseconds(_ iso: String) -> Int64? {
        let parser = ISO8601DateFormatter(); parser.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return (parser.date(from: iso) ?? ISO8601DateFormatter().date(from: iso)).map { Int64($0.timeIntervalSince1970 * 1000) }
    }
}

/// A report goes to LingXi, not to the publisher.
private struct HostedServiceReportSheet: View {
    @Environment(\.dismiss) private var dismiss
    let model: HostedServicesModel
    @State private var reason = "harmful"
    @State private var text = ""
    @State private var withSession = false
    @State private var sent = false
    /// One id per report, so a report sent again after a lost answer is the same report.
    @State private var reportId = UUID().uuidString

    var body: some View {
        NavigationStack {
            Form {
                if sent {
                    Text("services_report_sent").accessibilityIdentifier("services.report.sent")
                } else {
                    Section { Text("services_report_hint").font(.footnote).foregroundStyle(.secondary) }
                    Picker("services_report_reason", selection: $reason) {
                        ForEach(HostedService.reportReasons, id: \.self) { Text(LocalizedStringKey("services_report_\($0)")).tag($0) }
                    }
                    Section("services_report_text") {
                        TextField(text: $text, axis: .vertical) { Text("services_report_text") }.lineLimit(3...8)
                            .onChange(of: text) { _, value in if value.count > 1000 { text = String(value.prefix(1000)) } }
                    }
                    if model.session != nil { Toggle("services_report_session", isOn: $withSession) }
                    if !model.error.isEmpty { Text(LocalizedStringKey(HostedService.errorKey(model.error))).foregroundStyle(.red) }
                }
            }
            .navigationTitle("services_report_title")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                if sent {
                    ToolbarItem(placement: .confirmationAction) { Button("common_close") { dismiss() } }
                } else {
                    ToolbarItem(placement: .cancellationAction) { Button("common_cancel") { dismiss() } }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("services_report_send") { Task { sent = await model.report(reason: reason, text: text, withSession: withSession, reportId: reportId) } }
                            .disabled(model.busy)
                            .accessibilityIdentifier("services.report.send")
                    }
                }
            }
        }
    }
}
