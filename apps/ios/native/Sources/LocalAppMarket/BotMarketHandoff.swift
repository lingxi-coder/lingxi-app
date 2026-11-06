import Foundation
import SwiftUI
import FirebaseAuth

struct BotInstallReceipt: Decodable, Sendable {
    let id: String
    let listingId: String
    let versionId: String
    let releaseDigest: String
    let status: String
    let instanceId: String?
}

/// Public identities and an authenticated receipt; never a mobile worker or model binding.
struct BotMarketHandoff: Codable, Equatable {
    let listingID: String
    var versionID: String?
    var intentID: String? = nil
    var stage: String
    var requestID: String = UUID().uuidString
    var instanceID: String? = nil
    static func validID(_ value: String) -> Bool { value.range(of: "^[A-Za-z0-9_-]{1,128}$", options: .regularExpression) != nil }
    var desktopURL: URL? {
        guard Self.validID(listingID), versionID.map(Self.validID) ?? true, intentID.map(Self.validID) ?? true else { return nil }
        var parts = URLComponents(string: "https://lingxios.net/open/market/bot/\(listingID)")!
        var query: [URLQueryItem] = []
        if let versionID { query.append(.init(name: "versionId", value: versionID)) }
        if let intentID { query.append(.init(name: "intentId", value: intentID)) }
        parts.queryItems = query.isEmpty ? nil : query
        return parts.url
    }
    static func key() -> String { "bot-market-handoff:" + (Auth.auth().currentUser?.uid ?? "anonymous") }
    func save() { if let data = try? JSONEncoder().encode(self) { UserDefaults.standard.set(data, forKey: Self.key()) } }
    static func pending() -> Self? {
        guard let data = UserDefaults.standard.data(forKey: key()), let value = try? JSONDecoder().decode(Self.self, from: data), value.desktopURL != nil else { return nil }
        return value
    }
    func resolve(marketURL: URL?, create: Bool) async throws -> Self {
        if intentID == nil && !create { return self }
        guard let user = Auth.auth().currentUser else { throw MarketFailure("SIGN_IN_REQUIRED", "Sign in to track this Desktop download.") }
        let owner = user.uid
        let token = try await user.getIDToken()
        guard Auth.auth().currentUser?.uid == owner else { throw MarketFailure("ACCOUNT_CHANGED") }
        guard let marketURL, var parts = URLComponents(url: marketURL, resolvingAgainstBaseURL: false) else { throw MarketFailure("MARKET_NOT_CONFIGURED") }
        parts.path = "/api/v3"; parts.query = nil; parts.fragment = nil
        guard let endpoint = parts.url else { throw MarketFailure("MARKET_NOT_CONFIGURED") }
        let api = try MarketAPI(baseURL: endpoint)
        var captured = Self.pending().flatMap { $0.requestID == requestID ? $0 : nil } ?? self
        let receipt: BotInstallReceipt
        if let intentID = captured.intentID {
            receipt = try await api.authenticatedValue("bot-install-intents/\(intentID)", bearer: token)
        } else {
            if captured.versionID == nil {
                struct Detail: Decodable { struct Version: Decodable { let id: String }; let currentVersion: Version? }
                let detail: Detail = try await api.authenticatedValue("bots/\(listingID)", bearer: token)
                captured.versionID = detail.currentVersion?.id
            }
            guard let versionID = captured.versionID, Self.validID(versionID) else { throw MarketFailure("NO_RELEASE") }
            guard Auth.auth().currentUser?.uid == owner else { throw MarketFailure("ACCOUNT_CHANGED") }
            captured.save() // Freeze command identity and release before POST, including ACK loss.
            receipt = try await api.authenticatedValue("bot-install-intents", bearer: token, body: ["listingId": .string(listingID), "versionId": .string(versionID)], idempotencyKey: captured.requestID)
        }
        guard Auth.auth().currentUser?.uid == owner else { throw MarketFailure("ACCOUNT_CHANGED") }
        guard receipt.listingId == listingID, captured.versionID == nil || captured.versionID == receipt.versionId,
              Self.validID(receipt.id), Self.validID(receipt.versionId) else { throw MarketFailure("HANDOFF_CHANGED") }
        guard captured.intentID == nil || captured.intentID == receipt.id,
              ["awaiting_desktop", "installed"].contains(receipt.status),
              receipt.status != "installed" || receipt.instanceId?.range(of: "^[A-Za-z0-9_-]{22}$", options: .regularExpression) != nil else { throw MarketFailure("INVALID_HANDOFF_RECEIPT") }
        captured.versionID = receipt.versionId; captured.intentID = receipt.id; captured.stage = receipt.status; captured.instanceID = receipt.instanceId
        captured.save()
        return captured
    }
}

struct BotMarketHandoffPage: View {
    let listingID: String
    let versionID: String?
    let intentID: String?
    let marketURL: URL?
    @Environment(\.openURL) private var openURL
    @Environment(\.scenePhase) private var scenePhase
    @State private var handoff: BotMarketHandoff
    @State private var error: String?
    @State private var busy = false
    init(listingID: String, versionID: String?, intentID: String?, marketURL: URL?) {
        self.listingID = listingID; self.versionID = versionID; self.intentID = intentID; self.marketURL = marketURL
        let pending = BotMarketHandoff.pending().flatMap { value in value.listingID == listingID && (versionID == nil || value.versionID == versionID) && (intentID == nil || value.intentID == intentID) ? value : nil }
        _handoff = State(initialValue: pending ?? .init(listingID: listingID, versionID: versionID, intentID: intentID, stage: "awaiting_desktop"))
    }
    private var detailURL: URL? {
        guard let marketURL, BotMarketHandoff.validID(listingID), var parts = URLComponents(url: marketURL, resolvingAgainstBaseURL: false) else { return nil }
        parts.path = "/bots/\(listingID)"; parts.queryItems = [.init(name: "platform", value: "macos")]; parts.fragment = nil
        return parts.url
    }
    var body: some View {
        List {
            Section("Bot download") {
                Text(handoff.stage == "installed" ? "Downloaded on Desktop" : "Continue on Desktop").font(.headline)
                Text("Choose your own model and authorize your resources on Desktop before sharing this Bot with a Team.")
                Text("Listing: \(listingID)").textSelection(.enabled)
                if let versionID = handoff.versionID { Text("Release: \(versionID)").textSelection(.enabled) }
            }
            Section {
                if let detailURL { Button("View in market") { handoff.save(); openURL(detailURL) { accepted in if !accepted { error = "The system browser could not open the market." } } } }
                Button("Prepare Desktop handoff") { Task { await refresh(create: true) } }.disabled(busy)
                if handoff.intentID != nil, let desktopURL = handoff.desktopURL {
                    ShareLink(item: desktopURL) { Label("Send to Desktop", systemImage: "square.and.arrow.up") }
                    Button("Check Desktop download") { Task { await refresh(create: false) } }.disabled(busy)
                    Text(desktopURL.absoluteString).font(.caption).textSelection(.enabled)
                }
                Text(handoff.stage == "installed" ? "Desktop confirmed the personal Bot was created. Enable and share it there. No worker runs on this phone." : "Handoff saved. This page checks the authenticated Desktop receipt when you return. No Bot is installed on this phone.").foregroundStyle(.secondary)
            }
            if let error { Text(error).foregroundStyle(.red) }
        }
        .navigationTitle("Bot Market")
        .task(id: scenePhase) { handoff.save(); if scenePhase == .active, handoff.intentID != nil { await refresh(create: false) } }
    }
    private func refresh(create: Bool) async {
        guard !busy else { return }
        busy = true; defer { busy = false }
        do { handoff = try await handoff.resolve(marketURL: marketURL, create: create); error = nil }
        catch { self.error = error.localizedDescription }
    }
}
