import Foundation

@main struct CheckTeamMessages {
    struct Fixture: Decodable { let pages: [TeamMessagePage] }
    static func main() async throws {
        let fixture = try JSONDecoder().decode(Fixture.self, from: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1])))
        var state = TeamMessageProjection(version: 1, uid: "owner", teamId: "test-team", generation: 1)
        for page in fixture.pages { try state.apply(page) }
        try state.apply(fixture.pages[0])
        precondition(state.threads["test-chat"]?.cursor == 3)
        precondition(state.threads["test-chat"]?.messages.first?.text == "")
        precondition(state.threads["test-chat"]?.messages.first?.recalled == true)
        precondition(state.conversations.first?.unread == 0)
        precondition(state.conversations.first?.lastChangeSeq == 3)
        var empty = TeamMessageProjection(version: 1, uid: "owner", teamId: "test-team", generation: 1)
        do { try empty.apply(fixture.pages[1]); preconditionFailure("Gap accepted") } catch { }
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let files = TeamMessageFiles(directory: directory)
        let first = fixture.pages[0]
        _ = try await files.update(uid: "owner", teamId: "test-team", generation: 1) { value in
            try value.apply(first)
            value.outbox = [TeamMessageIntent(teamId: "test-team", generation: 1, conversationId: "test-chat", messageId: "message02", changeId: "pending001", operation: "create", text: "Pending", replyTo: nil)]
        }
        let recovered = try await TeamMessageFiles(directory: directory).update(uid: "owner", teamId: "test-team", generation: 1) { _ in }
        precondition(recovered.threads["test-chat"]?.cursor == 1 && recovered.threads["test-chat"]?.messages.first?.text == "Synthetic")
        precondition(recovered.outbox.first?.changeId == "pending001")
        let newGeneration = try await files.update(uid: "owner", teamId: "test-team", generation: 2) { _ in }
        precondition(newGeneration.threads.isEmpty && newGeneration.outbox.isEmpty)
        do { _ = try await files.update(uid: "owner", teamId: "test-team", generation: 1) { _ in throw TeamMessageFailure(code: "INJECTED_CRASH") }; preconditionFailure("Fault accepted") } catch { }
        let afterCrash = try await files.update(uid: "owner", teamId: "test-team", generation: 1) { _ in }
        precondition(afterCrash.threads["test-chat"]?.cursor == 1)
        print("Team messaging: 10 projection/persistence checks passed")
    }
}
