import SwiftUI
import UIKit

struct ProviderAccountsPage: View {
    let providerId: String
    let executionBindingId: String
    var providerName: String = ""
    var executionBindingIds: [String] = []
    let allowsAPIKey: Bool
    let allowsOAuth: Bool
    @State private var repository = ProviderAccountRepository.shared
    private enum AccountSheet: String, Identifiable { case add, deviceCode; var id: String { rawValue } }
    @State private var sheet: AccountSheet?
    @State private var confirmMetered = false

    private var accounts: [ProviderAccount] { repository.accounts(providerId: providerId) }

    @State private var feedback: String?

    private var guidance: String {
        [
            accounts.count > 1 ? String(localized: "provider_accounts_order_hint") : nil,
            providerId == "openai" && !repository.oauthSupported ? String(localized: "provider_account_remote_oauth_hint") : nil,
        ].compactMap { $0 }.joined(separator: "\n\n")
    }

    var body: some View {
        List {
            Section {
                ForEach(accounts) { account in
                    NavigationLink {
                        ProviderAccountDetailPage(accountId: account.accountId, allowsOAuth: allowsOAuth && repository.oauthSupported)
                    } label: {
                        HStack(spacing: 10) {
                            ProviderBrandMark(providerID: providerId, name: account.displayName)
                            VStack(alignment: .leading, spacing: 4) {
                                Text(account.displayName)
                                Text(account.authKind == "oauth" ? String(localized: "provider_account_auth_chatgpt") : String(localized: "provider_account_auth_api_key")).font(.caption).foregroundStyle(.secondary)
                                Text(accountStatus(account)).font(.caption).foregroundStyle(.secondary)
                                if repository.wasLastUsed(account) {
                                    Text("provider_account_last_selected").font(.caption).foregroundStyle(.secondary)
                                }
                            }
                        }
                    }
                    .accessibilityIdentifier("provider.account.\(account.accountId)")
                }
                .onMove(perform: reorder)
                Button("provider_account_add_title", systemImage: "person.crop.circle.badge.plus") { sheet = .add }
                    .disabled(!repository.available || repository.snapshot.storeScope.isEmpty || (!allowsAPIKey && !(allowsOAuth && repository.oauthSupported)))
                    .accessibilityIdentifier("provider.account.add")
            } header: {
                if accounts.count > 1 { Text("provider_accounts_priority") }
            }
            if providerId == "openai" {
                Section {
                    HStack {
                        Toggle("provider_account_allow_metered", isOn: Binding(
                            get: { repository.snapshot.providerPolicies?[providerId]?.allowSubscriptionToMetered ?? false },
                            set: { allowed in
                                if allowed { confirmMetered = true }
                                else {
                                    Task { await perform(.init(action: "set_policy", providerId: providerId, allowSubscriptionToMetered: false)) }
                                }
                            }
                        ))
                        ProviderInfoButton(message: String(localized: "provider_account_metered_fee_hint"))
                    }
                }
            }
        }
        .confirmationDialog("provider_account_allow_metered", isPresented: $confirmMetered, titleVisibility: .visible) {
            Button("provider_account_allow_api_charges") {
                Task { await perform(.init(action: "set_policy", providerId: providerId, allowSubscriptionToMetered: true)) }
            }
            Button("common_cancel", role: .cancel) {}
        } message: {
            Text("provider_account_metered_consent")
        }
        .navigationTitle("provider_accounts_title")
        .toolbar {
            if !guidance.isEmpty { ProviderInfoButton(message: guidance) }
            EditButton().disabled(repository.busy)
        }
        .providerMessage($feedback)
        .disabled(repository.busy)
        .overlay { if repository.busy { ProgressView() } }
        .task {
            await refresh()
            if repository.deviceFlow != nil { sheet = .deviceCode }
        }
        .refreshable { await refresh() }
        .onChange(of: repository.deviceFlow?.id) { _, id in
            if id != nil, sheet == nil { sheet = .deviceCode }
            else if id == nil, sheet == .deviceCode { sheet = nil }
        }
        .sheet(item: $sheet, onDismiss: {
            if repository.deviceFlow != nil { sheet = .deviceCode }
        }) { page in
            NavigationStack {
                switch page {
                case .add:
                    ProviderAccountAddPage(providerId: providerId, executionBindingId: executionBindingId,
                        providerName: providerName, executionBindingIds: executionBindingIds, allowsAPIKey: allowsAPIKey, allowsOAuth: allowsOAuth && repository.oauthSupported)
                case .deviceCode:
                    ProviderDeviceCodePage()
                }
            }
            .presentationDetents(page == .add ? [.medium, .large] : [.large])
            .presentationDragIndicator(.visible)
        }
    }

    private func refresh() async {
        if !(await repository.refresh()) { feedback = repository.errorMessage }
    }

    private func perform(_ command: ProviderAccountCommand) async {
        if !(await repository.perform(command)) { feedback = repository.errorMessage }
    }

    private func accountStatus(_ account: ProviderAccount) -> String {
        if !account.enabled { return String(localized: "provider_account_disabled") }
        if let health = repository.health(for: account) {
            let reason = health.displayReason.map { " · \($0)" } ?? ""
            return "\(account.authKind == "oauth" ? "OAuth" : String(localized: "provider_account_auth_api_key")) · \(health.displayStatus)\(reason)"
        }
        return account.authKind == "oauth" ? "OAuth" : String(localized: "provider_account_auth_api_key")
    }

    private func reorder(from source: IndexSet, to destination: Int) {
        var ordered = accounts
        ordered.move(fromOffsets: source, toOffset: destination)
        Task {
            await perform(.init(action: "reorder", providerId: providerId, accountIds: ordered.map(\.accountId)))
        }
    }
}

private struct ProviderDeviceCodePage: View {
    @State private var repository = ProviderAccountRepository.shared
    @State private var feedback: String?
    var body: some View {
        Form {
            if let flow = repository.deviceFlow {
                Section {
                    Text(flow.userCode).font(.title.monospaced()).textSelection(.enabled)
                        .accessibilityIdentifier("provider.account.device-code")
                    Button("provider_account_device_copy", systemImage: "doc.on.doc") { UIPasteboard.general.string = flow.userCode }
                    Link(destination: flow.verificationURL) { Label("provider_account_device_open_browser", systemImage: "safari") }
                        .accessibilityIdentifier("provider.account.verify")
                    Text("provider_account_device_choose_account")
                    LabeledContent("provider_account_device_expires") { Text(flow.expiresAt, style: .relative) }
                    ProgressView("provider_account_device_waiting")
                }
            }
        }
        .onChange(of: repository.errorMessage, initial: true) { _, error in feedback = error }
        .providerMessage($feedback)
        .navigationTitle("provider_account_device_title")
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("common_cancel") { repository.cancelDeviceOAuth() }
                    .accessibilityIdentifier("provider.account.oauth.cancel")
            }
        }
        .interactiveDismissDisabled()
    }
}

// Official credential pages; custom providers have no inferred destination.
private func providerCredentialManagementURL(_ providerID: String) -> URL? {
    let address: String
    switch providerID {
    case "anthropic": address = "https://console.anthropic.com/settings/keys"
    case "openai": address = "https://platform.openai.com/api-keys"
    case "xai": address = "https://console.x.ai"
    case "deepseek": address = "https://platform.deepseek.com/api_keys"
    case "kimi": address = "https://platform.kimi.com/console/api-keys"
    case "kimi-code": address = "https://www.kimi.com/code/console"
    case "gemini": address = "https://aistudio.google.com/app/apikey"
    case "openrouter": address = "https://openrouter.ai/settings/keys"
    case "zai": address = "https://z.ai/manage-apikey/apikey-list"
    case "glm-coding": address = "https://bigmodel.cn/usercenter/proj-mgmt/apikeys"
    case "github-copilot": address = "https://github.com/settings/tokens"
    default: return nil
    }
    return URL(string: address)
}

private struct ProviderAccountAddPage: View {
    @Environment(\.dismiss) private var dismiss
    let providerId: String
    let executionBindingId: String
    var providerName: String = ""
    var executionBindingIds: [String] = []
    let allowsAPIKey: Bool
    let allowsOAuth: Bool
    @State private var repository = ProviderAccountRepository.shared
    @State private var name = ""
    @State private var selectedBinding = ""
    @State private var secret = ""
    @State private var oauth = false
    @State private var showAccountOptions = false
    @State private var createdAccountID: String?
    @State private var submissionGeneration: UInt64?
    @State private var accountIdentityGeneration: UInt64?
    @State private var validationError: String?
    @State private var submitting = false

    var body: some View {
        Form {
            Section {
                if !usesOAuth && executionBindingIds.count > 1 {
                    Picker("provider_account_scope_connection", selection: $selectedBinding) {
                        ForEach(executionBindingIds, id: \.self) { Text($0).tag($0) }
                    }.disabled(createdAccountID != nil)
                }
                if allowsAPIKey && allowsOAuth {
                    Picker("provider_account_auth_method", selection: $oauth) {
                        Text("provider_account_auth_api_key").tag(false)
                        Text(verbatim: "ChatGPT").tag(true)
                    }
                    .pickerStyle(.segmented)
                    .disabled(createdAccountID != nil)
                }
                if allowsAPIKey && !usesOAuth {
                    SecureField("provider_account_auth_api_key", text: $secret)
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                        .accessibilityIdentifier("provider.account.api-key")
                        .disabled(createdAccountID != nil)
                    if let url = providerCredentialManagementURL(selectedBinding.isEmpty ? executionBindingId : selectedBinding) {
                        Link("provider_account_manage_api_key", destination: url)
                            .accessibilityIdentifier("provider.account.manage-api-key")
                    }
                }
            } header: {
                HStack {
                    ProviderBrandMark(providerID: providerId, name: defaultAccountName)
                    Text(defaultAccountName)
                    if usesOAuth { ProviderInfoButton(message: String(localized: "provider_account_device_choose_account")) }
                }
            }
            Section {
                Button { showAccountOptions.toggle() } label: {
                    HStack {
                        Text("settings_section_advanced")
                        Spacer()
                        Image(systemName: showAccountOptions ? "chevron.up" : "chevron.down")
                            .font(.caption.weight(.semibold))
                    }
                }
                .accessibilityIdentifier("provider.account.options")
                if showAccountOptions {
                    TextField("provider_account_optional_name", text: $name, prompt: Text(defaultAccountName))
                        .accessibilityIdentifier("provider.account.name")
                        .disabled(createdAccountID != nil)
                }
            }
            if submitting { Section { ProgressView("provider_account_adding") } }
        }
        .disabled(submitting || repository.busy)
        .navigationTitle("provider_account_add_title")
        .providerMessage($validationError, identifier: "provider.account.add.error")
        .toolbar {
            #if DEBUG
            if ProcessInfo.processInfo.environment["LINGXI_PROVIDER_ACCOUNT_COMPONENT_TEST"] == "1" {
                ToolbarItem(placement: .bottomBar) {
                    Button("Release fixture request") { ProviderAccountFixtureGate.release() }
                        .accessibilityIdentifier("provider.account.fixture.release")
                }
            }
            #endif
            ToolbarItem(placement: .cancellationAction) {
                Button("common_cancel") { dismiss() }.disabled(submitting || repository.busy)
                    .accessibilityIdentifier("provider.account.cancel")
            }
            ToolbarItem(placement: .confirmationAction) {
                Button(usesOAuth ? "provider_account_add_and_sign_in" : "workspace_confirm_add", action: add)
                    .disabled(submitting || repository.busy)
                    .accessibilityIdentifier("provider.account.submit")
            }
        }
        .onAppear { selectedBinding = executionBindingIds.first ?? executionBindingId }
        .onChange(of: repository.snapshot.storeScope) { _, _ in secret = ""; dismiss() }
        .interactiveDismissDisabled(submitting || repository.busy)
        .onDisappear { secret = "" }
    }

    private var defaultAccountName: String { providerName.isEmpty ? providerId : providerName }

    private var usesOAuth: Bool { allowsOAuth && (!allowsAPIKey || oauth) }

    private func add() {
        let accountID = createdAccountID ?? UUID().uuidString.lowercased()
        let authenticatesWithOAuth = usesOAuth
        let kind = authenticatesWithOAuth ? "oauth" : "api_key"
        let enteredName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let submittedName = enteredName.isEmpty ? defaultAccountName : enteredName
        guard authenticatesWithOAuth || !secret.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            validationError = String(localized: "provider_account_api_key_required"); return
        }
        validationError = nil
        guard !submitting else { return }
        submitting = true
        let submittedSecret = authenticatesWithOAuth ? nil : secret
        let binding = selectedBinding.isEmpty ? (executionBindingIds.first ?? executionBindingId) : selectedBinding
        let actualBinding = authenticatesWithOAuth && providerId == "openai" ? "openai-chatgpt" : binding
        let isRetry = createdAccountID != nil
        if !isRetry { name = submittedName; createdAccountID = accountID; submissionGeneration = repository.generation }
        Task {
            defer {
                submitting = false
                if validationError == nil { validationError = repository.errorMessage }
            }
            guard submissionGeneration == repository.generation else {
                validationError = String(localized: "provider_account_host_changed"); return
            }
            if isRetry {
                guard await repository.refresh(), submissionGeneration == repository.generation else { return }
            }
            if let existing = repository.snapshot.accounts.first(where: { $0.accountId == accountID }) {
                guard verifyIdentity(existing, binding: actualBinding, kind: kind, name: submittedName) else { return }
            } else {
                guard accountIdentityGeneration == nil else {
                    validationError = String(localized: "provider_account_identity_changed"); return
                }
                let added = await repository.perform(.init(action: "add", providerId: providerId,
                    accountId: accountID, displayName: submittedName,
                    executionBindingId: actualBinding, authKind: kind), credential: submittedSecret)
                guard submissionGeneration == repository.generation else { return }
                if let existing = repository.snapshot.accounts.first(where: { $0.accountId == accountID }) {
                    guard verifyIdentity(existing, binding: actualBinding, kind: kind, name: submittedName) else { return }
                }
                guard added else { return }
                guard accountIdentityGeneration != nil else {
                    validationError = String(localized: "provider_account_unconfirmed"); return
                }
            }
            guard submissionGeneration == repository.generation else { return }
            secret = ""
            if authenticatesWithOAuth {
                guard await repository.perform(.init(action: "begin_oauth", accountId: accountID)) else { return }
            }
            dismiss()
        }
    }

    private func verifyIdentity(_ account: ProviderAccount, binding: String, kind: String, name: String) -> Bool {
        guard account.providerId == providerId, account.executionBindingId == binding,
              account.authKind == kind, account.displayName == name,
              accountIdentityGeneration == nil || accountIdentityGeneration == account.identityGeneration else {
            validationError = String(localized: "provider_account_identity_changed")
            return false
        }
        accountIdentityGeneration = account.identityGeneration
        return true
    }

}

private struct ProviderAccountDetailPage: View {
    @Environment(\.dismiss) private var dismiss
    let accountId: String
    let allowsOAuth: Bool
    @State private var repository = ProviderAccountRepository.shared
    @State private var name = ""
    @State private var secret = ""
    @State private var confirmingDelete = false
    @State private var feedback: String?

    private var account: ProviderAccount? { repository.snapshot.accounts.first { $0.accountId == accountId } }

    var body: some View {
        Form {
            if let account {
                Section {
                    TextField("provider_account_name", text: $name)
                    Button("provider_account_rename") { run(.init(action: "update", accountId: accountId, displayName: name)) }
                        .disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || name == account.displayName)
                    Toggle("plugins_enabled", isOn: Binding(get: { account.enabled }, set: {
                        run(.init(action: "set_enabled", accountId: accountId, enabled: $0))
                    }))
                    LabeledContent("provider_account_auth_method", value: account.authKind == "oauth" ? "OAuth" : String(localized: "provider_account_auth_api_key"))
                    if let health = repository.health(for: account) {
                        LabeledContent("session_details_status", value: health.displayStatus)
                        if let reason = health.displayReason { Text(reason).foregroundStyle(.secondary) }
                        if let until = health.cooldownUntilMs {
                            LabeledContent("provider_account_retry_after") { Text(Date(timeIntervalSince1970: Double(until) / 1000), style: .relative) }
                        }
                    }
                }
                Section {
                    if account.authKind == "oauth" {
                        if allowsOAuth {
                            Button("mcp_reconnect") { run(.init(action: "begin_oauth", accountId: accountId)) }
                        }
                        Button("provider_account_logout", role: .destructive) { run(.init(action: "logout", accountId: accountId)) }
                    } else {
                        SecureField("provider_account_replacement_key", text: $secret)
                            .textInputAutocapitalization(.never).autocorrectionDisabled()
                        if let url = providerCredentialManagementURL(account.executionBindingId) {
                            Link("provider_account_manage_api_key", destination: url)
                        }
                        Button("provider_account_replace_key") {
                            Task {
                                if await repository.perform(.init(action: "set_credential", accountId: accountId), credential: secret) { secret = "" }
                                else { feedback = repository.errorMessage }
                            }
                        }.disabled(secret.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                    Button("provider_account_test") { run(.init(action: "test", accountId: accountId)) }
                        .accessibilityIdentifier("provider.account.test")
                }
                if !repository.scopedHealth(for: account).isEmpty {
                    Section("provider_account_temporarily_unavailable") {
                        ForEach(repository.scopedHealth(for: account)) { health in
                            VStack(alignment: .leading, spacing: 4) {
                                Text(health.scopeKind == "model" ? String(localized: "session_details_model") : String(localized: "provider_account_scope_connection"))
                                    .font(.caption).foregroundStyle(.secondary)
                                Text(health.scopeId)
                                Text(health.reason.replacingOccurrences(of: "_", with: " "))
                                    .font(.caption).foregroundStyle(.secondary)
                                if let until = health.unavailableUntilMs {
                                    LabeledContent("provider_account_retry_after") { Text(Date(timeIntervalSince1970: Double(until) / 1000), style: .relative) }
                                }
                            }
                        }
                    }
                }
                Section {
                    Button("provider_account_remove", role: .destructive) { confirmingDelete = true }
                }
            }
        }
        .providerMessage($feedback)
        .disabled(repository.busy)
        .navigationTitle(account?.displayName ?? String(localized: "provider_accounts_title"))
        .task { name = account?.displayName ?? "" }
        .onChange(of: repository.snapshot.storeScope) { _, _ in secret = ""; name = ""; dismiss() }
        .onDisappear { secret = "" }
        .confirmationDialog("provider_account_remove_confirm_title", isPresented: $confirmingDelete, titleVisibility: .visible) {
            Button("provider_account_remove", role: .destructive) {
                Task {
                    if await repository.perform(.init(action: "remove", accountId: accountId)) { dismiss() }
                    else { feedback = repository.errorMessage }
                }
            }
        }
    }

    private func run(_ command: ProviderAccountCommand) {
        Task {
            if await repository.perform(command) {
                if command.action == "test" { feedback = String(localized: "provider_account_test_success") }
            } else {
                feedback = repository.errorMessage
            }
        }
    }
}

#if DEBUG
@MainActor
enum ProviderAccountFixtureGate {
    static var pending: CheckedContinuation<Void, Never>?
    static func release() { pending?.resume(); pending = nil }
}

/// Isolated UI fixture. Enabled only by the explicit component-test launch flag.
@MainActor
struct ProviderAccountComponentTestFixture: View {
    @State private var ready = false
    @State private var accountID: String?
    @State private var accountName = ""
    @State private var beginAttempts = 0
    @State private var identityGeneration = 1

    var body: some View {
        NavigationStack {
            if ready {
                ProviderAccountsPage(providerId: "openai", executionBindingId: "openai-chatgpt",
                    allowsAPIKey: false, allowsOAuth: true)
            } else {
                ProgressView()
            }
        }
        .task {
            let repository = ProviderAccountRepository.shared
            repository.configure(load: { try snapshot() }, command: { raw, _ in
                let command = try JSONSerialization.jsonObject(with: Data(raw.utf8)) as? [String: Any]
                switch command?["action"] as? String {
                case "add":
                    guard accountID == nil, let id = command?["accountId"] as? String,
                          command?["providerId"] as? String == "openai",
                          command?["authKind"] as? String == "oauth",
                          command?["executionBindingId"] as? String == "openai-chatgpt" else {
                        throw NSError(domain: "fixture", code: 1, userInfo: [NSLocalizedDescriptionKey: "Duplicate or invalid account"])
                    }
                    await withCheckedContinuation { ProviderAccountFixtureGate.pending = $0 }
                    accountID = id
                    accountName = command?["displayName"] as? String ?? ""
                    throw NSError(domain: "fixture", code: 4, userInfo: [NSLocalizedDescriptionKey: "Synthetic lost add reply"])
                case "begin_oauth":
                    guard command?["accountId"] as? String == accountID else {
                        throw NSError(domain: "fixture", code: 2, userInfo: [NSLocalizedDescriptionKey: "Wrong account"])
                    }
                    beginAttempts += 1
                    if beginAttempts == 1 {
                        if ProcessInfo.processInfo.environment["LINGXI_PROVIDER_ACCOUNT_IDENTITY_CHANGE"] == "1" { identityGeneration = 2 }
                        throw NSError(domain: "fixture", code: 3, userInfo: [NSLocalizedDescriptionKey: "Synthetic authorization failure"])
                    }
                    return try json(["mode": "device_code", "flow_id": "fixture-flow", "account_id": accountID!,
                        "user_code": "TEST-CODE", "verification_url": "https://example.com/authorize",
                        "interval_seconds": 60, "expires_in_seconds": 900])
                case "cancel_oauth": return try snapshot()
                default: throw ProviderAccountError.invalidResponse
                }
            })
            await repository.refresh()
            ready = true
        }
    }

    private func snapshot() throws -> String {
        var rows: [[String: Any]] = []
        if let accountID {
            rows = [["accountId": accountID, "providerId": "openai", "displayName": accountName,
                     "authKind": "oauth", "executionBindingId": "openai-chatgpt", "enabled": true,
                     "userPriority": 0, "configRevision": 2, "identityGeneration": identityGeneration]]
        }
        return try json(["storeScope": "component-test", "configRevision": accountID == nil ? 1 : 2,
                         "accounts": rows, "health": [], "modelEntitlements": [], "modelAvailability": [], "scopedHealth": []])
    }

    private func json(_ value: [String: Any]) throws -> String {
        String(decoding: try JSONSerialization.data(withJSONObject: value), as: UTF8.self)
    }
}
#endif
