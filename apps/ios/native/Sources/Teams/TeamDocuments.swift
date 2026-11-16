import Foundation

/// Who wrote a revision of a shared document: a member, or the Bot whose task produced it.
struct TeamRevisionAuthor: Decodable, Equatable, Sendable {
    let kind: String
    var uid: String? = nil
    var sharedBotId: String? = nil
}
struct TeamArtifactRevision: Decodable, Equatable, Identifiable, Sendable {
    let revision: Int
    let parentRevision: Int?
    let digest: String
    let title: String
    let author: TeamRevisionAuthor
    let createdAt: Double
    var sourceTaskId: String? = nil
    var id: Int { revision }
}
/// The history of a document as `listArtifactRevisions` returns it: newest first, without bodies.
struct TeamArtifactRevisions: Decodable, Equatable, Sendable {
    let artifactId: String
    let currentRevision: Int
    let revisions: [TeamArtifactRevision]
}
struct TeamArtifactRevised: Decodable, Sendable { let artifactId: String; let revision: Int; let digest: String }

/// The text a member is editing. It is saved only on top of the revision it was made from.
struct TeamDocumentDraft: Equatable, Sendable {
    var title: String
    var body: String

    init(_ document: TeamArtifact.Document) { title = document.title; body = document.body }
    init(title: String, body: String) { self.title = title; self.body = body }

    var trimmedTitle: String { title.trimmingCharacters(in: .whitespacesAndNewlines) }
    var tooLong: Bool { body.utf8.count > TeamExecutionContract.documentBytes }
    /// What the service stores: a title of 1 to 120 UTF-16 units and a body that is not empty and within the size limit.
    var valid: Bool { !trimmedTitle.isEmpty && trimmedTitle.utf16.count <= 120 && !body.isEmpty && !tooLong }
    func changed(from base: TeamArtifact.Document) -> Bool { trimmedTitle != base.title || body != base.body }
    var payload: [String: Any] { ["title": trimmedTitle, "body": body] }
}
/// The identity of one exact edit of one revision. Saving the same text again after a lost answer sends the same id and
/// creates no second revision; other text, or the same text on another revision, gets a new id.
struct TeamDocumentOperation: Equatable, Sendable {
    let revision: Int
    let title: String
    let body: String
    let id: String

    static func of(_ current: TeamDocumentOperation?, revision: Int, draft: TeamDocumentDraft, newId: () -> String = { UUID().uuidString }) -> TeamDocumentOperation {
        if let current, current.revision == revision, current.title == draft.trimmedTitle, current.body == draft.body { return current }
        return TeamDocumentOperation(revision: revision, title: draft.trimmedTitle, body: draft.body, id: newId())
    }
}
enum TeamDocumentRules {
    /// The name the history shows for a revision's author; a member who left or an unpublished Bot keeps a generic name.
    static func authorName(_ author: TeamRevisionAuthor, members: [TeamMember], bots: [TeamBotCard], strings: TeamsListStrings) -> String {
        if author.kind == "user" {
            let name = members.first { $0.uid == author.uid }?.displayName ?? ""
            return name.isEmpty ? strings.member : name
        }
        let name = bots.first { $0.sharedBotId == author.sharedBotId }?.name ?? ""
        return name.isEmpty ? strings.bot : name
    }
}
