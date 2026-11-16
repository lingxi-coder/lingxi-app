import XCTest

@testable import LingxiCode

/// M3-4: the rules for files in a Team message, the same as Desktop's `packages/teams/attachments.ts`
/// (`apps/web/test/teams-attachments.test.ts`), and the commands that carry them.
final class TeamAttachmentsTests: XCTestCase {
    private let mib = 1024 * 1024

    private func command(_ action: String, _ payload: [String: Any]) -> [String: Any] {
        ["schemaVersion": 1, "commandId": "command_0001", "teamId": "team_0001", "generation": 7, "action": action, "payload": payload]
    }
    private func refused(_ action: String, _ payload: [String: Any], file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertThrowsError(try TeamExecutionContract.validate(command(action, payload)), file: file, line: line) {
            XCTAssertEqual(($0 as? TeamMessageFailure)?.code, "INVALID_INPUT", file: file, line: line)
        }
    }

    func testAFileIsSentAsOneOfFourTypesOrNotAtAll() {
        XCTAssertEqual(TeamAttachments.type(name: "a.bin", declared: "image/png"), "image/png")
        XCTAssertEqual(TeamAttachments.type(name: "a", declared: "IMAGE/JPG"), "image/jpeg")
        XCTAssertEqual(TeamAttachments.type(name: "a", declared: "text/plain; charset=utf-8"), "text/plain")
        XCTAssertEqual(TeamAttachments.type(name: "a", declared: "text/markdown"), "text/plain")
        XCTAssertEqual(TeamAttachments.type(name: "a", declared: "text/csv"), "text/plain")
        XCTAssertEqual(TeamAttachments.type(name: "report.PDF", declared: ""), "application/pdf")
        XCTAssertEqual(TeamAttachments.type(name: "photo.JPeG", declared: ""), "image/jpeg")
        XCTAssertEqual(TeamAttachments.type(name: "shot.png", declared: ""), "image/png")
        for name in ["notes.txt", "README.md", "a.markdown", "table.csv", "run.LOG"] { XCTAssertEqual(TeamAttachments.type(name: name, declared: ""), "text/plain", name) }
        // What the system says wins over the name; a kind Teams does not take is refused whatever it is called.
        XCTAssertNil(TeamAttachments.type(name: "a.png", declared: "image/heic"))
        XCTAssertNil(TeamAttachments.type(name: "a.zip", declared: ""))
        XCTAssertNil(TeamAttachments.type(name: "png", declared: ""))
        XCTAssertNil(TeamAttachments.type(name: "noending", declared: ""))
    }

    func testANameIsTheFilesOwnNameWithinTheServicesLimit() {
        XCTAssertEqual(TeamAttachments.name("/Users/me/Documents/plan.pdf"), "plan.pdf")
        XCTAssertEqual(TeamAttachments.name("C:\\files\\plan.pdf"), "plan.pdf")
        XCTAssertEqual(TeamAttachments.name("  a\u{0}b\u{1f}c\u{7f}.txt  "), "abc.txt")
        for empty in ["", "   ", ".", "..", "dir/"] { XCTAssertEqual(TeamAttachments.name(empty), "file", empty) }
        let long = TeamAttachments.name(String(repeating: "x", count: 200) + ".pdf")
        XCTAssertEqual(long.utf16.count, 120)
        XCTAssertTrue(long.hasSuffix(".pdf"))
        // The limit counts UTF-16 units and never cuts a character in half.
        let faces = TeamAttachments.name(String(repeating: "😀", count: 100) + ".png")
        XCTAssertEqual(faces, String(repeating: "😀", count: 58) + ".png")
        XCTAssertLessThanOrEqual(faces.utf16.count, 120)
        // An ending longer than ten units is not an ending.
        XCTAssertEqual(TeamAttachments.name(String(repeating: "y", count: 130) + ".averylongending").utf16.count, 120)
    }

    func testSizesAreSaidShortly() {
        XCTAssertEqual(TeamAttachments.size(12), "12 B")
        XCTAssertEqual(TeamAttachments.size(1024), "1 KB")
        XCTAssertEqual(TeamAttachments.size(320 * 1024), "320 KB")
        XCTAssertEqual(TeamAttachments.size(1_468_006), "1.4 MB")
        XCTAssertEqual(TeamAttachments.size(10 * mib), "10 MB")
    }

    func testChosenFilesAreAdmittedInOrderAndEachRefusalHasItsReason() {
        let chosen: [TeamAttachments.Picked] = [
            .init(name: "a.png", type: "image/png", size: 9 * mib), .init(name: "app.zip", type: "application/zip", size: 10),
            .init(name: "empty.txt", type: "text/plain", size: 0), .init(name: "big.pdf", type: "application/pdf", size: 10 * mib + 1),
            .init(name: "b.pdf", type: "", size: 9 * mib), .init(name: "c.txt", type: "", size: 3 * mib), .init(name: "d.txt", type: "", size: mib),
        ]
        let result = TeamAttachments.admit(draft: [], files: chosen)
        XCTAssertEqual(result.accepted.map(\.index), [0, 4, 6])
        XCTAssertEqual(result.accepted.map(\.mime), ["image/png", "application/pdf", "text/plain"])
        XCTAssertEqual(result.refused.map(\.code), ["ATTACHMENT_TYPE", "ATTACHMENT_EMPTY", "ATTACHMENT_TOO_LARGE", "ATTACHMENT_TOTAL"])
        XCTAssertEqual(result.refused.map(\.name), ["app.zip", "empty.txt", "big.pdf", "c.txt"])
        // A sixth file is refused whatever its size; what the draft already holds counts.
        let full = TeamAttachments.admit(draft: [1, 1, 1, 1], files: [.init(name: "e.txt", type: "", size: 1), .init(name: "f.txt", type: "", size: 1)])
        XCTAssertEqual(full.accepted.map(\.name), ["e.txt"])
        XCTAssertEqual(full.refused.map(\.code), ["ATTACHMENT_COUNT"])
        XCTAssertEqual(TeamAttachments.admit(draft: [15 * mib], files: [.init(name: "g.pdf", type: "", size: 5 * mib)]).accepted.count, 1)
        XCTAssertEqual(TeamAttachments.admit(draft: [15 * mib], files: [.init(name: "g.pdf", type: "", size: 5 * mib + 1)]).refused.map(\.code), ["ATTACHMENT_TOTAL"])
    }

    func testAFileIsCutIntoPartsAndPutTogetherOnlyWhenItIsWhatTheMessageSays() throws {
        let data = Data((0..<2500).map { UInt8($0 % 251) })
        let parts = TeamAttachments.parts(data, partBytes: 1000)
        XCTAssertEqual(parts.map(\.count), [1000, 1000, 500])
        XCTAssertEqual(TeamAttachments.parts(Data(), partBytes: 1000), [])
        XCTAssertEqual(TeamAttachments.sha256Hex(Data("abc".utf8)), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        let file = TeamMessageAttachment(attachmentId: "upload_0001", name: "a.txt", mime: "text/plain", byteLength: data.count, sha256: TeamAttachments.sha256Hex(data))
        let encoded = parts.map { $0.base64EncodedString() }
        XCTAssertEqual(try TeamAttachments.assemble(encoded, file: file), data)
        let damaged: [[String]] = [Array(encoded.dropLast()), [encoded[1], encoded[0], encoded[2]], encoded + ["!"]]
        for wrong in damaged {
            XCTAssertThrowsError(try TeamAttachments.assemble(wrong, file: file)) { XCTAssertEqual(($0 as? TeamMessageFailure)?.code, "ATTACHMENT_DAMAGED") }
        }
        let other = TeamMessageAttachment(attachmentId: "upload_0001", name: "a.txt", mime: "text/plain", byteLength: data.count, sha256: String(repeating: "0", count: 64))
        XCTAssertThrowsError(try TeamAttachments.assemble(encoded, file: other))
    }

    func testOnlyAnswersThatMayChangeAreAskedAgain() {
        for code in ["UPLOAD_QUOTA_EXCEEDED", "UPLOAD_DIGEST_MISMATCH", "ATTACHMENT_REVOKED", "INVALID_INPUT", "INVALID_SESSION", "CONVERSATION_REQUIRED",
                     "MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "SIGN_IN_REQUIRED", "REQUEST_TOO_LARGE", "COMMAND_ID_CONFLICT"] {
            XCTAssertTrue(TeamAttachments.isFinal(code), code)
        }
        for code in ["", "SERVICE_UNAVAILABLE", "INTERNAL", "RATE_LIMITED"] { XCTAssertFalse(TeamAttachments.isFinal(code), code) }
    }

    func testAMessageIsSentWhenEveryFileIsCheckedAndThereIsTextOrAFile() {
        let ready = TeamDraftFile(uploadId: "upload_0001", name: "a.txt", mime: "text/plain", byteLength: 3, state: .ready, sent: 3)
        var uploading = ready; uploading.state = .uploading
        var failed = ready; failed.state = .failed; failed.error = "UPLOAD_QUOTA_EXCEEDED"
        XCTAssertFalse(TeamAttachments.canSend(text: "  ", files: []))
        XCTAssertTrue(TeamAttachments.canSend(text: "hello", files: []))
        XCTAssertTrue(TeamAttachments.canSend(text: "", files: [ready]))
        XCTAssertTrue(TeamAttachments.canSend(text: "see this", files: [ready]))
        XCTAssertFalse(TeamAttachments.canSend(text: "see this", files: [ready, uploading]))
        XCTAssertFalse(TeamAttachments.canSend(text: "see this", files: [failed]))
        XCTAssertFalse(TeamAttachments.canSend(text: String(repeating: "x", count: 8193), files: [ready]))
        XCTAssertEqual(TeamAttachments.said(text: "", names: ["a.txt", "b.pdf"]), "a.txt, b.pdf")
        XCTAssertEqual(TeamAttachments.said(text: "hello", names: ["a.txt"]), "hello")
    }

    func testEveryRefusalHasItsOwnWords() {
        XCTAssertEqual(TeamsErrors.key("ATTACHMENT_TYPE"), "teams_file_error_type")
        XCTAssertEqual(TeamsErrors.key("UPLOAD_TYPE_MISMATCH"), "teams_file_error_type")
        XCTAssertEqual(TeamsErrors.key("ATTACHMENT_TOTAL"), "teams_file_error_total")
        XCTAssertEqual(TeamsErrors.key("UPLOAD_QUOTA_EXCEEDED"), "teams_file_error_quota")
        XCTAssertEqual(TeamsErrors.key("ATTACHMENT_REVOKED"), "teams_file_error_revoked")
        XCTAssertEqual(TeamsErrors.key("ATTACHMENT_DAMAGED"), "teams_file_error_damaged")
        XCTAssertEqual(TeamsErrors.key("ATTACHMENT_REQUIRED"), "teams_file_error_unsendable")
        XCTAssertEqual(TeamsErrors.key("ATTACHMENT_UNREADABLE"), "teams_file_error_unreadable")
    }

    func testAPictureIsKnownByItsFirstBytes() {
        XCTAssertEqual(TeamAttachments.pictureType(Data([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0])), "image/png")
        XCTAssertEqual(TeamAttachments.pictureType(Data([0xff, 0xd8, 0xff, 0xe0, 0])), "image/jpeg")
        XCTAssertNil(TeamAttachments.pictureType(Data([0, 0, 0, 0x18, 0x66, 0x74, 0x79, 0x70, 0x68, 0x65, 0x69, 0x63])))
        XCTAssertNil(TeamAttachments.pictureType(Data()))
    }

    func testAMessageCarriesItsFilesAndABotsReplyNamesWhatItRead() throws {
        let json = """
        {"id":"message_0001","seq":4,"author":{"kind":"user","uid":"user_0001"},"kind":"text","text":"","changeSeq":1,"changeId":"change_0001",
         "attachments":[{"attachmentId":"upload_0001","name":"a.png","mime":"image/png","byteLength":12,"sha256":"\(String(repeating: "a", count: 64))"},
                        {"attachmentId":"upload_0002","name":"b.pdf","mime":"application/pdf","byteLength":9,"sha256":"\(String(repeating: "b", count: 64))","revoked":true}],
         "readFiles":[{"attachmentId":"upload_0001","name":"a.png"}]}
        """
        let message = try JSONDecoder().decode(TeamMessage.self, from: Data(json.utf8))
        XCTAssertEqual(message.files.map(\.name), ["a.png", "b.pdf"])
        XCTAssertTrue(message.files[0].isImage)
        XCTAssertEqual(message.files[1].revoked, true)
        XCTAssertEqual(message.readFiles, [TeamReadFile(attachmentId: "upload_0001", name: "a.png")])
        let recalled = try JSONDecoder().decode(TeamMessage.self, from: Data(json.replacingOccurrences(of: "\"text\":\"\"", with: "\"text\":\"\",\"recalled\":true").utf8))
        XCTAssertEqual(recalled.recalled, true)
        XCTAssertEqual(recalled.files, [])
    }

    func testAMessageWithFilesIsSubmittedAndMayHaveNoTextAndAskNoBot() throws {
        let intent = TeamMessageIntent(teamId: "team_0001", generation: 7, conversationId: "chat_0001", messageId: "message_0001", changeId: "change_0001",
                                       operation: "create", text: "", replyTo: nil, attachments: ["upload_0001", "upload_0002"], attachmentNames: ["a.png", "b.pdf"])
        var body = intent.body
        XCTAssertEqual(body["action"] as? String, "submit")
        var payload = try XCTUnwrap(body["payload"] as? [String: Any])
        XCTAssertEqual(payload["targets"] as? [String], [])
        XCTAssertEqual(payload["attachments"] as? [String], ["upload_0001", "upload_0002"])
        XCTAssertNil(payload["attachmentNames"])
        body["teamId"] = "team_0001"; body["generation"] = 7
        XCTAssertNoThrow(try TeamExecutionContract.validate(body))
        // With a Bot asked as well.
        let asking = TeamMessageIntent(teamId: "team_0001", generation: 7, conversationId: "chat_0001", messageId: "message_0001", changeId: "change_0001",
                                       operation: "create", text: "read this", replyTo: nil, targets: ["shared_bot_1"], attachments: ["upload_0001", "upload_0002"], attachmentNames: ["a.png", "b.pdf"])
        body = asking.body; payload = try XCTUnwrap(body["payload"] as? [String: Any])
        XCTAssertEqual(payload["targets"] as? [String], ["shared_bot_1"])
        XCTAssertEqual(payload["attachments"] as? [String], ["upload_0001", "upload_0002"])
        // The saved intent comes back whole.
        let saved = try JSONDecoder().decode(TeamMessageIntent.self, from: JSONEncoder().encode(asking))
        XCTAssertEqual(saved.attachments, ["upload_0001", "upload_0002"])
        XCTAssertEqual(saved.attachmentNames, ["a.png", "b.pdf"])
        // A message with neither is not a submit, as before.
        let plain = TeamMessageIntent(teamId: "team_0001", generation: 7, conversationId: "chat_0001", messageId: "message_0001", changeId: "change_0001", operation: "create", text: "hi", replyTo: nil)
        XCTAssertNotEqual(plain.body["action"] as? String, "submit")
    }

    func testTheFileCommandsAreCheckedBeforeTheyAreSent() throws {
        let submit: [String: Any] = ["conversationId": "chat_0001", "messageId": "message_0001", "changeId": "change_0001", "text": "", "targets": [String]()]
        refused("submit", submit)
        XCTAssertNoThrow(try TeamExecutionContract.validate(command("submit", submit.merging(["attachments": ["upload_0001"]]) { $1 })))
        refused("submit", submit.merging(["attachments": [String]()]) { $1 })
        refused("submit", submit.merging(["attachments": ["upload_0001", "upload_0001"]]) { $1 })
        refused("submit", submit.merging(["attachments": (1...6).map { "upload_000\($0)" }]) { $1 })
        refused("submit", submit.merging(["text": "hello"]) { $1 })
        XCTAssertNoThrow(try TeamExecutionContract.validate(command("submit", submit.merging(["text": "hello", "targets": ["shared_bot_1"]]) { $1 })))

        let digest = String(repeating: "a", count: 64)
        let create: [String: Any] = ["conversationId": "chat_0001", "uploadId": "upload_0001", "name": "a.png", "mime": "image/png", "byteLength": 12, "sha256": digest]
        XCTAssertNoThrow(try TeamExecutionContract.validate(command("createUpload", create)))
        for wrong in [["name": "dir/a.png"], ["name": " a.png"], ["name": ""], ["mime": "image/gif"], ["byteLength": 0], ["byteLength": 10 * mib + 1], ["sha256": digest.uppercased()], ["uploadId": "short"]] as [[String: Any]] {
            refused("createUpload", create.merging(wrong) { $1 })
        }
        XCTAssertNoThrow(try TeamExecutionContract.validate(command("putUploadPart", ["uploadId": "upload_0001", "index": 14, "data": "QUJD"])))
        refused("putUploadPart", ["uploadId": "upload_0001", "index": 15, "data": "QUJD"])
        refused("putUploadPart", ["uploadId": "upload_0001", "index": 0, "data": "A B="])
        refused("putUploadPart", ["uploadId": "upload_0001", "index": 0, "data": String(repeating: "A", count: 4 * ((700 * 1024 + 2) / 3) + 4)])
        XCTAssertNoThrow(try TeamExecutionContract.validate(command("putUploadPart", ["uploadId": "upload_0001", "index": 0, "data": Data(count: 700 * 1024).base64EncodedString()])))
        for action in ["finalizeUpload", "cancelUpload"] {
            XCTAssertNoThrow(try TeamExecutionContract.validate(command(action, ["uploadId": "upload_0001"])))
            refused(action, ["uploadId": "upload_0001", "index": 0])
        }
        XCTAssertNoThrow(try TeamExecutionContract.validate(command("readAttachmentPart", ["attachmentId": "upload_0001", "index": 0])))
        refused("readAttachmentPart", ["attachmentId": "upload_0001", "index": -1])
        refused("readAttachmentPart", ["attachmentId": "upload_0001"])
        XCTAssertNoThrow(try TeamExecutionContract.validate(command("revokeAttachment", ["attachmentId": "upload_0001", "changeId": "change_0001"])))
        refused("revokeAttachment", ["attachmentId": "upload_0001"])
    }
}
