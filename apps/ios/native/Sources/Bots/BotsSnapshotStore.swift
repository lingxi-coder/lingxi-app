import CryptoKit
import Foundation
import Observation

/// Disk IO is serialized off the main actor. A uid switch invalidates all outstanding writes.
actor BotsSnapshotFiles {
    private let directory: URL
    private var uid: String?
    private var generation = 0

    init(directory: URL? = nil) {
        self.directory = directory ?? URL.applicationSupportDirectory.appendingPathComponent("BotsSnapshots", isDirectory: true)
    }

    func configure(uid: String?, generation: Int) throws -> [String: BotsSnapshot] {
        guard generation >= self.generation else { return [:] }
        self.uid = uid
        self.generation = generation
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        var folder = directory
        var values = URLResourceValues(); values.isExcludedFromBackup = true
        try folder.setResourceValues(values)
        var loaded: [BotsSnapshot] = []
        for file in try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil) where file.pathExtension == "json" {
            if let data = try? Data(contentsOf: file), let snapshot = try? JSONDecoder().decode(BotsSnapshot.self, from: data),
               snapshot.uid == uid, file.lastPathComponent == name(uid: snapshot.uid, hostID: snapshot.hostID) {
                loaded.append(snapshot)
            } else {
                try FileManager.default.removeItem(at: file)
            }
        }
        let kept = BotsLogic.evictHosts(loaded)
        for value in loaded where !kept.contains(where: { $0.hostID == value.hostID }) {
            try FileManager.default.removeItem(at: url(uid: value.uid, hostID: value.hostID))
        }
        return Dictionary(kept.map { ($0.hostID, $0) }, uniquingKeysWith: { _, b in b })
    }

    func save(_ snapshots: [BotsSnapshot], uid: String, generation: Int) throws {
        guard self.uid == uid, self.generation == generation else { return }
        let kept = BotsLogic.evictHosts(snapshots.filter { $0.uid == uid })
        let names = Set(kept.map { name(uid: uid, hostID: $0.hostID) })
        for file in try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
            where file.pathExtension == "json" && !names.contains(file.lastPathComponent) {
            try FileManager.default.removeItem(at: file)
        }
        for snapshot in kept {
            let file = url(uid: uid, hostID: snapshot.hostID)
            #if os(iOS)
            try JSONEncoder().encode(snapshot).write(to: file, options: [.atomic, .completeFileProtectionUnlessOpen])
            #else
            try JSONEncoder().encode(snapshot).write(to: file, options: .atomic)
            #endif
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
        }
    }

    private func name(uid: String, hostID: String) -> String {
        SHA256.hash(data: (try? JSONEncoder().encode([uid, hostID])) ?? Data()).map { String(format: "%02x", $0) }.joined() + ".json"
    }
    private func url(uid: String, hostID: String) -> URL { directory.appendingPathComponent(name(uid: uid, hostID: hostID)) }
}

@MainActor
@Observable
final class BotsSnapshotStore {
    private(set) var snapshots: [String: BotsSnapshot] = [:]
    private(set) var collapsedHosts: Set<String> = []
    private(set) var expandedBots: Set<String> = []
    private(set) var lastHostID: String?
    private(set) var hasSavedExpansion = false
    private(set) var storageError = false
    @ObservationIgnored private let files: BotsSnapshotFiles
    @ObservationIgnored private let defaults: UserDefaults
    @ObservationIgnored private var uid: String?
    @ObservationIgnored private var generation = 0
    @ObservationIgnored private var pending: Task<Void, Never>?

    init(files: BotsSnapshotFiles = BotsSnapshotFiles(), defaults: UserDefaults = .standard) {
        self.files = files; self.defaults = defaults
    }

    func configure(uid: String?) async {
        generation += 1
        let mine = generation
        pending?.cancel(); pending = nil
        // These are only this feature's preferences; account changes discard prior users' tree state.
        for key in defaults.dictionaryRepresentation().keys where
            ["bots.collapsedHosts.", "bots.expandedBots.", "bots.lastHost."].contains(where: { key.hasPrefix($0) }) &&
            !["bots.collapsedHosts.", "bots.expandedBots.", "bots.lastHost."].contains(where: { key == $0 + (uid ?? "") }) {
            defaults.removeObject(forKey: key)
        }
        self.uid = uid
        snapshots = [:]; collapsedHosts = []; expandedBots = []; lastHostID = nil; hasSavedExpansion = false
        if let uid {
            collapsedHosts = Set(defaults.stringArray(forKey: "bots.collapsedHosts." + uid) ?? [])
            expandedBots = Set(defaults.stringArray(forKey: "bots.expandedBots." + uid) ?? [])
            hasSavedExpansion = defaults.object(forKey: "bots.expandedBots." + uid) != nil
            lastHostID = defaults.string(forKey: "bots.lastHost." + uid)
        }
        do {
            let loaded = try await files.configure(uid: uid, generation: mine)
            guard generation == mine else { return }
            snapshots = loaded; storageError = false
        } catch { if generation == mine { storageError = true } }
    }

    func receive(bots: [BotsItem], hostID: String, nowMs: UInt64) {
        guard let uid else { return }
        snapshots[hostID] = BotsLogic.applyBots(prior: snapshots[hostID], uid: uid, hostID: hostID, bots: bots, nowMs: nowMs)
        snapshots = Dictionary(BotsLogic.evictHosts(Array(snapshots.values)).map { ($0.hostID, $0) }, uniquingKeysWith: { _, b in b })
        schedule()
    }
    func receive(chats: [BotsChat], hostID: String, botID: String, nowMs: UInt64) {
        guard uid != nil else { return }
        snapshots[hostID] = BotsLogic.applyChats(prior: snapshots[hostID], botID: botID, chats: chats, nowMs: nowMs)
        schedule()
    }
    func forget(_ hostID: String) async {
        snapshots.removeValue(forKey: hostID)
        collapsedHosts.remove(hostID)
        expandedBots = expandedBots.filter { !$0.hasPrefix(hostID + "/") }
        if lastHostID == hostID { lastHostID = nil }
        persistPreferences()
        await flush()
    }
    func connected(_ hostID: String) { lastHostID = hostID; persistPreferences() }
    func toggleHost(_ hostID: String) {
        if !collapsedHosts.insert(hostID).inserted { collapsedHosts.remove(hostID) }
        persistPreferences()
    }
    func toggleBot(_ key: String) {
        hasSavedExpansion = true
        if !expandedBots.insert(key).inserted { expandedBots.remove(key) }
        persistPreferences()
    }
    func expandDefault(_ key: String) {
        guard !hasSavedExpansion else { return }
        hasSavedExpansion = true; expandedBots.insert(key); persistPreferences()
    }
    private func persistPreferences() {
        guard let uid else { return }
        defaults.set(Array(collapsedHosts), forKey: "bots.collapsedHosts." + uid)
        if hasSavedExpansion { defaults.set(Array(expandedBots), forKey: "bots.expandedBots." + uid) }
        defaults.set(lastHostID, forKey: "bots.lastHost." + uid)
    }
    private func schedule() {
        pending?.cancel()
        pending = Task { [weak self] in
            do { try await Task.sleep(for: .seconds(1)) } catch { return }
            await self?.flush()
        }
    }
    func flush() async {
        pending?.cancel(); pending = nil
        guard let uid else { return }
        let mine = generation, snapshots = Array(snapshots.values)
        do {
            try await files.save(snapshots, uid: uid, generation: mine)
            if generation == mine { storageError = false }
        } catch { if generation == mine { storageError = true } }
    }
}
