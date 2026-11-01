import SwiftUI
import UIKit

struct ProviderAccountsPage: View {
    let providerId: String
    let executionBindingId: String
    var executionBindingIds: [String] = []
    let allowsAPIKey: Bool
    let allowsOAuth: Bool
    @State private var repository = ProviderAccountRepository.shared
    private enum AccountSheet: String, Identifiable { case add, deviceCode; var id: String { rawValue } }
    @State private var sheet: AccountSheet?

    private var accounts: [ProviderAccount] { repository.accounts(providerId: providerId) }

    var body: some View {
        List {
            Section {
                ForEach(accounts) { account in
                    NavigationLink {
                        ProviderAccountDetailPage(accountId: account.accountId, allowsOAuth: allowsOAuth && repository.oauthSupported)
                    } label: {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(account.displayName)
                            Text(account.executionBindingId).font(.caption).foregroundStyle(.secondary)
                            Text(accountStatus(account)).font(.caption).foregroundStyle(.secondary)
                            if repository.wasLastUsed(account) {
                                Text("Last used").font(.caption).foregroundStyle(.secondary)
                            }
                        }
                    }
                    .accessibilityIdentifier("provider.account.\(account.accountId)")
                }
                .onMove(perform: reorder)
                Button("Add account", systemImage: "person.crop.circle.badge.plus") { sheet = .add }
                    .disabled(!repository.available || repository.snapshot.storeScope.isEmpty || (!allowsAPIKey && !(allowsOAuth && repository.oauthSupported)))
                    .accessibilityIdentifier("provider.account.add")
            } header: {
                Text("Account priority")
            } footer: {
                Text("Requests use the highest priority available account. Temporary failures do not change this order.")
                if !repository.oauthSupported {
                    Text("OAuth sign-in must be completed on the execution Host.")
                }
            }
            if providerId == "openai" {
                Section {
                    Toggle("Allow subscription fallback to metered API", isOn: Binding(
                        get: { repository.snapshot.providerPolicies?[providerId]?.allowSubscriptionToMetered ?? false },
                        set: { allowed in
                            Task {
                                await repository.perform(.init(action: "set_policy", providerId: providerId,
                                    allowSubscriptionToMetered: allowed))
                            }
                        }
                    ))
                } footer: {
                    Text("May incur API charges when subscription accounts are unavailable.")
                }
            }
            if let error = repository.errorMessage {
                Section { Text(error).foregroundStyle(.red) }
            }
        }
        .navigationTitle("Accounts")
        .toolbar { EditButton().disabled(repository.busy) }
        .disabled(repository.busy)
        .overlay { if repository.busy { ProgressView() } }
        .task {
            await repository.refresh()
            if repository.deviceFlow != nil { sheet = .deviceCode }
        }
        .refreshable { await repository.refresh() }
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
                        executionBindingIds: executionBindingIds, allowsAPIKey: allowsAPIKey, allowsOAuth: allowsOAuth && repository.oauthSupported)
                case .deviceCode:
                    ProviderDeviceCodePage()
                }
            }
        }
    }

    private func accountStatus(_ account: ProviderAccount) -> String {
        if !account.enabled { return String(localized: "Disabled") }
        if let health = repository.health(for: account) {
            let reason = health.displayReason.map { " · \($0)" } ?? ""
            return "\(account.authKind == "oauth" ? "OAuth" : "API key") · \(health.displayStatus)\(reason)"
        }
        return account.authKind == "oauth" ? "OAuth" : "API key"
    }

    private func reorder(from source: IndexSet, to destination: Int) {
        var ordered = accounts
        ordered.move(fromOffsets: source, toOffset: destination)
        Task {
            await repository.perform(.init(action: "reorder", providerId: providerId, accountIds: ordered.map(\.accountId)))
        }
    }
}

private struct ProviderDeviceCodePage: View {
    @State private var repository = ProviderAccountRepository.shared
    var body: some View {
        Form {
            if let flow = repository.deviceFlow {
                Section {
                    Text(flow.userCode).font(.title.monospaced()).textSelection(.enabled)
                        .accessibilityIdentifier("provider.account.device-code")
                    Button("Copy code", systemImage: "doc.on.doc") { UIPasteboard.general.string = flow.userCode }
                    Link(destination: flow.verificationURL) { Label("Open authorization page", systemImage: "safari") }
                        .accessibilityIdentifier("provider.account.verify")
                    Text("Choose the upstream account you want to add in the browser, then approve this code.")
                    LabeledContent("Code expires") { Text(flow.expiresAt, style: .relative) }
                    ProgressView("Waiting for authorization")
                }
            }
            if let error = repository.errorMessage { Text(error).foregroundStyle(.red) }
        }
        .navigationTitle("Sign in to ChatGPT")
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel") { repository.cancelDeviceOAuth() }
                    .accessibilityIdentifier("provider.account.oauth.cancel")
            }
        }
        .interactiveDismissDisabled()
    }
}

private struct ProviderAccountAddPage: View {
    @Environment(\.dismiss) private var dismiss
    let providerId: String
    let executionBindingId: String
    var executionBindingIds: [String] = []
    let allowsAPIKey: Bool
    let allowsOAuth: Bool
    @State private var repository = ProviderAccountRepository.shared
    @State private var name = ""
    @State private var selectedBinding = ""
    @State private var secret = ""
    @State private var oauth = false
    @State private var createdAccountID: String?
    @State private var submissionGeneration: UInt64?
    @State private var accountIdentityGeneration: UInt64?
    @State private var validationError: String?
    @State private var submitting = false

    var body: some View {
        Form {
            Section {
                TextField("Account name", text: $name)
                    .accessibilityIdentifier("provider.account.name")
                    .disabled(createdAccountID != nil)
                if !usesOAuth && executionBindingIds.count > 1 {
                    Picker("Connection", selection: $selectedBinding) {
                        ForEach(executionBindingIds, id: \.self) { Text($0).tag($0) }
                    }.disabled(createdAccountID != nil)
                }
                if allowsAPIKey && allowsOAuth { Toggle("OAuth", isOn: $oauth).disabled(createdAccountID != nil) }
                if allowsAPIKey && !usesOAuth {
                    SecureField("API key", text: $secret)
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                        .accessibilityIdentifier("provider.account.api-key")
                        .disabled(createdAccountID != nil)
                }
            } footer: {
                if usesOAuth { Text("Choose the upstream account in the browser before approving your device code.") }
            }
            if let error = validationError ?? repository.errorMessage {
                Section { Text(error).foregroundStyle(.red).accessibilityIdentifier("provider.account.add.error") }
            }
            if submitting { Section { ProgressView("Adding account") } }
        }
        .disabled(submitting || repository.busy)
        .navigationTitle("Add account")
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
                Button("Cancel") { dismiss() }.disabled(submitting || repository.busy)
                    .accessibilityIdentifier("provider.account.cancel")
            }
            ToolbarItem(placement: .confirmationAction) {
                Button(usesOAuth ? "Add and sign in" : "Add", action: add)
                    .disabled(submitting || repository.busy)
                    .accessibilityIdentifier("provider.account.submit")
            }
        }
        .onAppear { selectedBinding = executionBindingIds.first ?? executionBindingId }
        .onChange(of: repository.snapshot.storeScope) { _, _ in secret = ""; dismiss() }
        .interactiveDismissDisabled(submitting || repository.busy)
        .onDisappear { secret = "" }
    }

    private var usesOAuth: Bool { allowsOAuth && (!allowsAPIKey || oauth) }

    private func add() {
        let accountID = createdAccountID ?? UUID().uuidString.lowercased()
        let authenticatesWithOAuth = usesOAuth
        let kind = authenticatesWithOAuth ? "oauth" : "api_key"
        let submittedName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !submittedName.isEmpty else { validationError = String(localized: "Enter an account name."); return }
        guard authenticatesWithOAuth || !secret.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            validationError = String(localized: "Enter an API key."); return
        }
        validationError = nil
        guard !submitting else { return }
        submitting = true
        let submittedSecret = authenticatesWithOAuth ? nil : secret
        let binding = selectedBinding.isEmpty ? (executionBindingIds.first ?? executionBindingId) : selectedBinding
        let actualBinding = authenticatesWithOAuth && providerId == "openai" ? "openai-chatgpt" : binding
        let isRetry = createdAccountID != nil
        if !isRetry { createdAccountID = accountID; submissionGeneration = repository.generation }
        Task {
            defer { submitting = false }
            guard submissionGeneration == repository.generation else {
                validationError = String(localized: "The execution Host changed. Close this form and try again."); return
            }
            if isRetry {
                guard await repository.refresh(), submissionGeneration == repository.generation else { return }
            }
            if let existing = repository.snapshot.accounts.first(where: { $0.accountId == accountID }) {
                guard verifyIdentity(existing, binding: actualBinding, kind: kind, name: submittedName) else { return }
            } else {
                guard accountIdentityGeneration == nil else {
                    validationError = String(localized: "The account no longer matches this request. Close this form and try again."); return
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
                    validationError = String(localized: "The account could not be confirmed. Try again."); return
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
            validationError = String(localized: "The account no longer matches this request. Close this form and try again.")
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

    private var account: ProviderAccount? { repository.snapshot.accounts.first { $0.accountId == accountId } }

    var body: some View {
        Form {
            if let account {
                Section {
                    TextField("Account name", text: $name)
                    Button("Rename") { run(.init(action: "update", accountId: accountId, displayName: name)) }
                        .disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || name == account.displayName)
                    Toggle("Enabled", isOn: Binding(get: { account.enabled }, set: {
                        run(.init(action: "set_enabled", accountId: accountId, enabled: $0))
                    }))
                    LabeledContent("Authentication", value: account.authKind == "oauth" ? "OAuth" : "API key")
                    if let health = repository.health(for: account) {
                        LabeledContent("Status", value: health.displayStatus)
                        if let reason = health.displayReason { Text(reason).foregroundStyle(.secondary) }
                        if let until = health.cooldownUntilMs {
                            LabeledContent("Retry after") { Text(Date(timeIntervalSince1970: Double(until) / 1000), style: .relative) }
                        }
                    }
                }
                Section {
                    if account.authKind == "oauth" {
                        if allowsOAuth {
                            Button("Reconnect") { run(.init(action: "begin_oauth", accountId: accountId)) }
                        }
                        Button("Sign out", role: .destructive) { run(.init(action: "logout", accountId: accountId)) }
                    } else {
                        SecureField("Replacement API key", text: $secret)
                            .textInputAutocapitalization(.never).autocorrectionDisabled()
                        Button("Replace API key") {
                            Task {
                                if await repository.perform(.init(action: "set_credential", accountId: accountId), credential: secret) { secret = "" }
                            }
                        }.disabled(secret.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                    Button("Test account") { run(.init(action: "test", accountId: accountId)) }
                        .accessibilityIdentifier("provider.account.test")
                }
                if !repository.scopedHealth(for: account).isEmpty {
                    Section("Temporary unavailability") {
                        ForEach(repository.scopedHealth(for: account)) { health in
                            VStack(alignment: .leading, spacing: 4) {
                                Text(health.scopeKind == "model" ? String(localized: "Model") : String(localized: "Connection"))
                                    .font(.caption).foregroundStyle(.secondary)
                                Text(health.scopeId)
                                Text(health.reason.replacingOccurrences(of: "_", with: " "))
                                    .font(.caption).foregroundStyle(.secondary)
                                if let until = health.unavailableUntilMs {
                                    LabeledContent("Retry after") { Text(Date(timeIntervalSince1970: Double(until) / 1000), style: .relative) }
                                }
                            }
                        }
                    }
                }
                Section("Models") {
                    let entitlements = repository.modelEntitlements(for: account)
                    if entitlements.isEmpty { Text("Model access unverified").foregroundStyle(.secondary) }
                    ForEach(entitlements) { entitlement in
                        LabeledContent(entitlement.modelId, value: entitlement.displayStatus)
                        if let retry = entitlement.retryAtMs, entitlement.status == "forbidden" {
                            LabeledContent("Retry after") { Text(Date(timeIntervalSince1970: Double(retry) / 1000), style: .relative) }
                        }
                    }
                    Text("A provider catalog lists models; account access is confirmed only after a successful request.")
                        .font(.footnote).foregroundStyle(.secondary)
                }
                Section {
                    Button("Delete account", role: .destructive) { confirmingDelete = true }
                }
            }
            if let error = repository.errorMessage { Section { Text(error).foregroundStyle(.red) } }
        }
        .disabled(repository.busy)
        .navigationTitle(account?.displayName ?? "Account")
        .task { name = account?.displayName ?? "" }
        .onChange(of: repository.snapshot.storeScope) { _, _ in secret = ""; name = ""; dismiss() }
        .onDisappear { secret = "" }
        .confirmationDialog("Delete this account and its credentials?", isPresented: $confirmingDelete, titleVisibility: .visible) {
            Button("Delete account", role: .destructive) {
                Task { if await repository.perform(.init(action: "remove", accountId: accountId)) { dismiss() } }
            }
        }
    }

    private func run(_ command: ProviderAccountCommand) { Task { await repository.perform(command) } }
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
