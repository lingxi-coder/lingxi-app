import SwiftUI

/// A shared document (Desktop `DocumentDialog`): read it, edit it, and look through its history. Reading always checks the
/// current Team membership. The body is Markdown rendered by the same `AIText` as chat messages; Markdown images stay inert
/// (alt text only, nothing is fetched) and links open only when they are http(s).
///
/// An edit is saved only on top of the revision it was made from. When someone saved first, nothing is saved: the text
/// being edited stays in the editor above the newer version, and saving over it is a decision the person makes.
struct TeamArtifactViewer: View {
    @Environment(\.theme) private var theme
    let model: TeamMessagingModel?
    let members: [TeamMember]
    let onClose: () -> Void
    @State private var shown: TeamArtifact
    @State private var draft: TeamDocumentDraft?
    /// The newer revision that refused the draft; while set, the draft is saved on top of it.
    @State private var newer: TeamArtifact?
    @State private var history: TeamArtifactRevisions?
    @State private var busy = false
    @State private var error = ""
    @State private var leaving = false
    @State private var operation: TeamDocumentOperation?

    init(artifact: TeamArtifact, model: TeamMessagingModel?, members: [TeamMember], onClose: @escaping () -> Void) {
        self.model = model; self.members = members; self.onClose = onClose
        _shown = State(initialValue: artifact)
    }

    private var base: TeamArtifact { newer ?? shown }
    private var changed: Bool { draft?.changed(from: base.document) ?? false }

    var body: some View {
        NavigationStack {
            Group {
                if draft != nil { editor } else if let history { historyList(history) } else { reader }
            }
            .background(theme.windowBg)
            .navigationBarTitleDisplayMode(.inline)
        }
        // Unsaved text is never lost to a swipe: leaving asks first.
        .interactiveDismissDisabled(changed)
        .confirmationDialog(Text("teams_doc_unsaved"), isPresented: $leaving, titleVisibility: .visible) {
            Button("teams_doc_discard", role: .destructive, action: stopEditing)
            Button("teams_doc_keep_editing", role: .cancel) {}
        }
        .accessibilityIdentifier("teams.artifact.\(shown.artifactId)")
    }

    // MARK: Reading

    private var reader: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                Text(shown.document.title).font(.title2.weight(.semibold)).foregroundStyle(theme.text)
                Text(shown.old ? String(localized: "teams_doc_revision_old \(shown.revision) \(shown.currentRevision)") : String(localized: "teams_doc_revision \(shown.revision)"))
                    .font(.caption).foregroundStyle(.secondary)
                    .accessibilityIdentifier("teams.artifact.revision")
                AIText(markdown: shown.document.body)
                    .textSelection(.enabled)
                    .accessibilityIdentifier("teams.artifact.body")
                failure
            }.frame(maxWidth: .infinity, alignment: .leading).padding()
        }
        .teamBotLinkPolicy()
        .navigationTitle("teams_artifact_title")
        .toolbar {
            ToolbarItem(placement: .confirmationAction) { Button("common_close", action: onClose) }
            ToolbarItemGroup(placement: .bottomBar) {
                if shown.old {
                    Button("teams_doc_show_latest") { attempt { shown = try await read() } }
                        .accessibilityIdentifier("teams.artifact.latest")
                } else {
                    Button("teams_doc_edit", systemImage: "pencil") { error = ""; draft = TeamDocumentDraft(shown.document) }
                        .labelStyle(.titleAndIcon)
                        .accessibilityIdentifier("teams.artifact.edit")
                }
                Spacer()
                Button("teams_doc_history", systemImage: "clock.arrow.circlepath") {
                    attempt { history = try await service().listArtifactRevisions(shown.artifactId) }
                }
                .labelStyle(.titleAndIcon)
                .accessibilityIdentifier("teams.artifact.history")
            }
        }
        .disabled(busy)
    }

    // MARK: Editing

    private var editor: some View {
        Form {
            Section("teams_doc_field_title") {
                TextField("teams_doc_field_title", text: Binding(get: { draft?.title ?? "" }, set: { draft?.title = $0 }))
                    .accessibilityIdentifier("teams.artifact.edit.title")
            }
            Section("teams_doc_field_body") {
                TextEditor(text: Binding(get: { draft?.body ?? "" }, set: { draft?.body = $0 }))
                    .font(.body.monospaced()).frame(minHeight: 260)
                    .accessibilityIdentifier("teams.artifact.edit.body")
                if draft?.tooLong == true { Text("teams_doc_too_long").font(.footnote).foregroundStyle(.red) }
            }
            if let newer {
                Section("teams_doc_newer") {
                    Label("teams_doc_conflict \(newer.revision)", systemImage: "exclamationmark.triangle")
                        .font(.footnote).foregroundStyle(.orange)
                        .accessibilityIdentifier("teams.artifact.conflict")
                    Text(newer.document.title).font(.headline)
                    AIText(markdown: newer.document.body).textSelection(.enabled)
                }
            }
            if !error.isEmpty { Section { failure } }
        }
        .teamBotLinkPolicy()
        .navigationTitle("teams_doc_edit_title")
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button(newer != nil ? "teams_doc_discard_mine" : "common_cancel") { if changed { leaving = true } else { stopEditing() } }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button(busy ? LocalizedStringKey("teams_doc_saving") : newer.map { "teams_doc_save_over \($0.revision)" } ?? "common_save", action: save)
                    .disabled(busy || !changed || draft?.valid != true)
                    .accessibilityIdentifier("teams.artifact.save")
            }
        }
    }

    // MARK: History

    private func historyList(_ history: TeamArtifactRevisions) -> some View {
        List {
            ForEach(history.revisions) { row in
                Button {
                    attempt { shown = try await read(row.revision); self.history = nil }
                } label: {
                    VStack(alignment: .leading, spacing: 2) {
                        HStack(spacing: 6) {
                            Text("teams_doc_revision \(row.revision)").font(.body.weight(.semibold))
                            if row.revision == history.currentRevision { Text("teams_doc_current").font(.caption).foregroundStyle(.secondary) }
                        }
                        Text(row.title).font(.subheadline).foregroundStyle(theme.text)
                        Text(verbatim: "\(TeamDocumentRules.authorName(row.author, members: members, bots: model?.bots ?? [], strings: .current)) · \(Date(timeIntervalSince1970: row.createdAt / 1000).formatted(date: .abbreviated, time: .shortened))")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .disabled(busy)
                .accessibilityIdentifier("teams.artifact.revision.\(row.revision)")
            }
            if !error.isEmpty { Section { failure } }
        }
        .navigationTitle("teams_doc_history_title")
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button("remote_back") { self.history = nil; error = "" } }
            ToolbarItem(placement: .confirmationAction) { Button("common_close", action: onClose) }
        }
    }

    @ViewBuilder
    private var failure: some View {
        if !error.isEmpty {
            Text(LocalizedStringKey(TeamsErrors.key(error))).font(.footnote).foregroundStyle(.red).accessibilityIdentifier("teams.artifact.error")
        }
    }

    // MARK: Requests

    private func service() throws -> TeamMessagingModel {
        guard let model else { throw TeamMessageFailure(code: "INVALID_SESSION") }
        return model
    }

    private func read(_ revision: Int? = nil) async throws -> TeamArtifact {
        try await service().readArtifact(shown.artifactId, revision: revision)
    }

    /// One request at a time; a failure is shown and never clears what was typed.
    private func attempt(_ work: @escaping @MainActor () async throws -> Void) {
        guard !busy else { return }
        busy = true; error = ""
        Task { @MainActor in
            do { try await work() } catch let failure as TeamMessageFailure { error = failure.code } catch { error = "SERVICE_UNAVAILABLE" }
            busy = false
        }
    }

    private func save() {
        guard let draft, changed, draft.valid else { return }
        let target = base
        attempt {
            let current = TeamDocumentOperation.of(operation, revision: target.revision, draft: draft)
            operation = current
            do {
                try await service().reviseArtifact(target.artifactId, expectedRevision: target.revision, operationId: current.id, draft: draft)
            } catch let failure as TeamMessageFailure where failure.code == "ARTIFACT_REVISION_CONFLICT" {
                newer = try await read(); return
            }
            shown = try await read(); self.draft = nil; newer = nil
        }
    }

    /// Leaves the editor without saving. After a refused save the reader shows the newer revision, not the one the edit started from.
    private func stopEditing() {
        draft = nil; error = ""; leaving = false
        if let newer { shown = newer; self.newer = nil }
    }
}
