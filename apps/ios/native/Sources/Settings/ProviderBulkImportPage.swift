import SwiftUI
import UniformTypeIdentifiers

struct ProviderBulkImportPage: View {
    let layer: DesktopSettingsLayer
    @State private var repository = DesktopSettingsRepository.shared
    @Environment(\.dismiss) private var dismiss
    @State private var input = ""
    @State private var rejectedInput: String?
    @State private var document: ProviderImportDocument?
    @State private var showingFile = false
    @State private var importing = false
    @State private var importTask: Task<Void, Never>?
    @State private var errorMessage: String?
    @State private var status: String?
    @State private var reviewedProviders = ""
    private var own: [String: Any] { repository.ownValue(key: "providers", layer: layer) as? [String: Any] ?? [:] }
    private var writable: Bool { repository.canEdit(key: "providers", layer: layer) && !importing }
    private var configured: Set<String> { Set(repository.credentialStates.filter { $0.value }.map(\.key)) }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Text("settings_parity_import_intro")
                        .font(.footnote).foregroundStyle(.secondary)
                    LabeledContent("Settings layer", value: layer.title)
                    if let errorMessage { Text(errorMessage).foregroundStyle(.red) }
                    if let status { Text(status).foregroundStyle(.secondary) }
                    if let document {
                        ForEach(document.errors, id: \.self) { Text($0).foregroundStyle(.red) }
                        ForEach(Array(document.warnings.enumerated()), id: \.offset) { _, warning in Text(warning).font(.caption).foregroundStyle(.secondary) }
                        ForEach(document.entries) { entry in
                            reviewRow(entry)
                        }
                        Button("settings_parity_import_apply") { startImport() }
                            .disabled(!writable || !document.errors.isEmpty || !document.entries.contains(where: \.selected))
                        Button("settings_parity_import_edit_source") { input = rejectedInput ?? ""; rejectedInput = nil; self.document = nil; status = nil; errorMessage = nil }
                            .disabled(importing)
                    } else {
                        TextEditor(text: $input).font(.system(.body, design: .monospaced)).frame(minHeight: 230)
                            .autocorrectionDisabled().textInputAutocapitalization(.never)
                            .accessibilityLabel(String(localized: "settings_parity_import_json"))
                        HStack {
                            PasteButton(payloadType: String.self) { values in
                                guard let value = values.first else { return }
                                if value.utf8.count > ProviderBulkImport.maximumBytes { errorMessage = String(localized: "settings_parity_import_too_large") }
                                else { input = value }
                            }
                            Button("settings_parity_import_choose_file") { showingFile = true }
                            Button("settings_parity_import_preview") { preview() }.disabled(input.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        }.disabled(importing)
                    }
                    if importing { ProgressView("Waiting for engine confirmation…") }
                }.padding()
            }
            .navigationTitle("settings_parity_import_providers")
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("common_cancel") { dismiss() }.disabled(importing) } }
            .interactiveDismissDisabled(importing)
            .fileImporter(isPresented: $showingFile, allowedContentTypes: [.json, .plainText], allowsMultipleSelection: false) { result in
                loadFile(result)
            }
        }
        .onChange(of: repository.sourceGeneration) { _, _ in
            importTask?.cancel(); input = ""; rejectedInput = nil; document = nil; importing = false
            errorMessage = String(localized: "settings_parity_import_source_changed")
        }
        .onDisappear { importTask?.cancel(); input = ""; rejectedInput = nil; document = nil }
        .task { await repository.refreshCredentials() }
    }

    private func reviewRow(_ entry: ProviderImportEntry) -> some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 10) {
                Toggle(isOn: Binding(get: { current(entry.id)?.selected ?? false }, set: { value in mutate(entry.id) { $0.selected = value } })) {
                    Text(entry.name)
                }
                if own[entry.name] != nil { Text("settings_parity_import_conflict").font(.caption).foregroundStyle(.orange) }
                TextField("Profile ID", text: string(entry.id, field: "name"))
                    .textInputAutocapitalization(.never).autocorrectionDisabled()
                Picker("Protocol", selection: string(entry.id, field: "type")) {
                    Text("Choose protocol").tag("")
                    ForEach(ProviderBulkImport.supportedTypes, id: \.self) { Text($0).tag($0) }
                }
                TextField("Base URL", text: string(entry.id, field: "baseUrl")).textInputAutocapitalization(.never).autocorrectionDisabled()
                TextField("API key environment variable", text: string(entry.id, field: "apiKeyEnv")).textInputAutocapitalization(.never).autocorrectionDisabled()
                SecureField("API key", text: string(entry.id, field: "credential")).textInputAutocapitalization(.never).autocorrectionDisabled()
                if entry.credential?.isEmpty == false { Text("settings_parity_import_secret_detected").font(.caption).foregroundStyle(.secondary) }
                if let issue = ProviderBulkImport.validate(entry, credentialConfigured: configured.contains(entry.name)) { Text(issue).font(.caption).foregroundStyle(.red) }
                ForEach(Array(entry.warnings.enumerated()), id: \.offset) { _, warning in Text(warning).font(.caption).foregroundStyle(.secondary) }
                DisclosureGroup("Sanitized provider settings") {
                    Text(DesktopSettingsRepository.json(entry.draft)).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                }
            }.textFieldStyle(.roundedBorder).disabled(importing)
        }
    }

    private func current(_ id: UUID) -> ProviderImportEntry? { document?.entries.first { $0.id == id } }
    private func mutate(_ id: UUID, _ action: (inout ProviderImportEntry) -> Void) {
        guard let index = document?.entries.firstIndex(where: { $0.id == id }) else { return }
        action(&document!.entries[index])
    }
    private func string(_ id: UUID, field: String) -> Binding<String> {
        Binding(get: {
            guard let entry = current(id) else { return "" }
            if field == "name" { return entry.name }
            if field == "credential" { return entry.credential ?? "" }
            return entry.draft[field] as? String ?? ""
        }, set: { value in
            mutate(id) { entry in
                if field == "name" { entry.name = value; if own[value] != nil { entry.selected = false } }
                else if field == "credential" { entry.credential = value.isEmpty ? nil : value }
                else { entry.draft[field] = value.isEmpty ? nil : value }
            }
        })
    }
    private func preview() {
        let parsed = ProviderBulkImport.parse(input, existing: own)
        rejectedInput = !parsed.errors.isEmpty || parsed.entries.contains { ProviderBulkImport.validate($0, credentialConfigured: configured.contains($0.name)) != nil } ? input : nil
        document = parsed
        input = ""
        reviewedProviders = DesktopSettingsRepository.json(own)
        errorMessage = nil; status = nil
        let ids = document?.entries.map(\.name) ?? []
        Task { await repository.refreshCredentials(providerIDs: ids) }
    }
    private func loadFile(_ result: Result<[URL], Error>) {
        do {
            guard let url = try result.get().first else { return }
            let accessed = url.startAccessingSecurityScopedResource()
            defer { if accessed { url.stopAccessingSecurityScopedResource() } }
            let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
            guard size <= ProviderBulkImport.maximumBytes else { throw ProviderBulkImport.ImportError(String(localized: "settings_parity_import_too_large")) }
            let handle = try FileHandle(forReadingFrom: url)
            defer { try? handle.close() }
            let data = try handle.read(upToCount: ProviderBulkImport.maximumBytes + 1) ?? Data()
            guard data.count <= ProviderBulkImport.maximumBytes, let text = String(data: data, encoding: .utf8) else { throw ProviderBulkImport.ImportError(String(localized: "settings_parity_import_file_failed")) }
            input = text; preview()
        } catch { errorMessage = String(localized: "settings_parity_import_file_failed") }
    }
    private func startImport() {
        guard let document, writable else { return }
        let generation = repository.sourceGeneration
        let reviewed = reviewedProviders
        let entries = document.entries
        let original = own
        importing = true; errorMessage = nil; status = nil; rejectedInput = nil
        importTask = Task { @MainActor in
            var savedCredentials = false
            do {
                let merged = try ProviderBulkImport.merge(entries, into: original, configured: configured)
                for entry in entries where entry.selected {
                    try validateScope(generation: generation, reviewed: reviewed)
                    if let secret = entry.credential, !secret.isEmpty {
                        await repository.saveCredential(providerID: entry.name, secret: secret)
                        try await repository.waitForConfirmation(generation: generation)
                        savedCredentials = true
                    }
                }
                try validateScope(generation: generation, reviewed: reviewed)
                await repository.save(key: "providers", json: DesktopSettingsRepository.json(merged), layer: layer)
                try await repository.waitForConfirmation(generation: generation)
                status = "Selected providers were saved and confirmed by the engine. Reconnect to apply provider changes."
                self.document = nil
            } catch {
                guard generation == repository.sourceGeneration else { return }
                errorMessage = error.localizedDescription + (savedCredentials ? " Some credentials were already saved securely; provider settings were not confirmed." : "")
                // Keep only sanitized configuration after any attempted write.
                if var pending = self.document {
                    for i in pending.entries.indices { pending.entries[i].credential = nil }
                    self.document = pending
                }
            }
            importing = false
        }
    }
    private func validateScope(generation: Int, reviewed: String) throws {
        try Task.checkCancellation()
        guard generation == repository.sourceGeneration, DesktopSettingsRepository.json(own) == reviewed else {
            throw ProviderBulkImport.ImportError(String(localized: "settings_parity_import_layer_changed"))
        }
    }
}
