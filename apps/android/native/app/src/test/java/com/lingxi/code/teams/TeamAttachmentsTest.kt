package com.lingxi.code.teams

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Base64

/**
 * M3-4: the rules for files in a Team message, the same as Desktop's `packages/teams/attachments.ts`
 * (`apps/web/test/teams-attachments.test.ts`) and iOS `TeamAttachmentsTests`, and the commands that carry them.
 */
class TeamAttachmentsTest {
    private val mib = 1024 * 1024
    private fun command(action: String, payload: JSONObject) =
        JSONObject().put("schemaVersion", 1).put("commandId", "command_0001").put("teamId", "team_0001").put("generation", 7).put("action", action).put("payload", payload)
    private fun refused(action: String, payload: JSONObject) =
        assertEquals("$action $payload", "INVALID_INPUT", assertThrows(TeamMessageFailure::class.java) { TeamExecutionContract.validate(command(action, payload)) }.code)
    private fun accepted(action: String, payload: JSONObject) = TeamExecutionContract.validate(command(action, payload))
    private fun JSONObject.with(key: String, value: Any?) = JSONObject(toString()).put(key, value)

    @Test fun aFileIsSentAsOneOfFourTypesOrNotAtAll() {
        assertEquals("image/png", TeamAttachments.type("a.bin", "image/png"))
        assertEquals("image/jpeg", TeamAttachments.type("a", "IMAGE/JPG"))
        assertEquals("text/plain", TeamAttachments.type("a", "text/plain; charset=utf-8"))
        assertEquals("text/plain", TeamAttachments.type("a", "text/markdown"))
        assertEquals("text/plain", TeamAttachments.type("a", "text/csv"))
        assertEquals("application/pdf", TeamAttachments.type("report.PDF", ""))
        assertEquals("image/jpeg", TeamAttachments.type("photo.JPeG", ""))
        assertEquals("image/png", TeamAttachments.type("shot.png", ""))
        for (name in listOf("notes.txt", "README.md", "a.markdown", "table.csv", "run.LOG")) assertEquals(name, "text/plain", TeamAttachments.type(name, ""))
        // What the system says wins over the name; a kind Teams does not take is refused whatever it is called.
        assertNull(TeamAttachments.type("a.png", "image/heic"))
        assertNull(TeamAttachments.type("a.zip", ""))
        assertNull(TeamAttachments.type("png", ""))
        assertNull(TeamAttachments.type("noending", ""))
    }

    @Test fun aNameIsTheFilesOwnNameWithinTheServicesLimit() {
        assertEquals("plan.pdf", TeamAttachments.name("/Users/me/Documents/plan.pdf"))
        assertEquals("plan.pdf", TeamAttachments.name("C:\\files\\plan.pdf"))
        assertEquals("abc.txt", TeamAttachments.name("  a\u0000b\u001fc\u007f.txt  "))
        for (empty in listOf("", "   ", ".", "..", "dir/")) assertEquals(empty, "file", TeamAttachments.name(empty))
        val long = TeamAttachments.name("x".repeat(200) + ".pdf")
        assertEquals(120, long.length); assertTrue(long.endsWith(".pdf"))
        // The limit counts UTF-16 units and never cuts a character in half.
        assertEquals("😀".repeat(58) + ".png", TeamAttachments.name("😀".repeat(100) + ".png"))
        // An ending longer than ten units is not an ending.
        assertEquals(120, TeamAttachments.name("y".repeat(130) + ".averylongending").length)
    }

    @Test fun sizesAreSaidShortly() {
        assertEquals("12 B", TeamAttachments.size(12))
        assertEquals("1 KB", TeamAttachments.size(1024))
        assertEquals("320 KB", TeamAttachments.size(320 * 1024))
        assertEquals("1.4 MB", TeamAttachments.size(1_468_006))
        assertEquals("10 MB", TeamAttachments.size(10 * mib))
    }

    @Test fun chosenFilesAreAdmittedInOrderAndEachRefusalHasItsReason() {
        val chosen = listOf(
            TeamAttachments.Picked("a.png", "image/png", 9 * mib), TeamAttachments.Picked("app.zip", "application/zip", 10),
            TeamAttachments.Picked("empty.txt", "text/plain", 0), TeamAttachments.Picked("big.pdf", "application/pdf", 10 * mib + 1),
            TeamAttachments.Picked("b.pdf", "", 9 * mib), TeamAttachments.Picked("c.txt", "", 3 * mib), TeamAttachments.Picked("d.txt", "", mib),
        )
        val result = TeamAttachments.admit(emptyList(), chosen)
        assertEquals(listOf(0, 4, 6), result.accepted.map { it.index })
        assertEquals(listOf("image/png", "application/pdf", "text/plain"), result.accepted.map { it.mime })
        assertEquals(listOf(TeamFileRefusal("app.zip", "ATTACHMENT_TYPE"), TeamFileRefusal("empty.txt", "ATTACHMENT_EMPTY"), TeamFileRefusal("big.pdf", "ATTACHMENT_TOO_LARGE"), TeamFileRefusal("c.txt", "ATTACHMENT_TOTAL")), result.refused)
        // A sixth file is refused whatever its size; what the draft already holds counts.
        val full = TeamAttachments.admit(listOf(1, 1, 1, 1), listOf(TeamAttachments.Picked("e.txt", "", 1), TeamAttachments.Picked("f.txt", "", 1)))
        assertEquals(listOf("e.txt"), full.accepted.map { it.name })
        assertEquals(listOf("ATTACHMENT_COUNT"), full.refused.map { it.code })
        assertEquals(1, TeamAttachments.admit(listOf(15 * mib), listOf(TeamAttachments.Picked("g.pdf", "", 5 * mib))).accepted.size)
        assertEquals(listOf("ATTACHMENT_TOTAL"), TeamAttachments.admit(listOf(15 * mib), listOf(TeamAttachments.Picked("g.pdf", "", 5 * mib + 1))).refused.map { it.code })
    }

    @Test fun aFileIsPutTogetherOnlyWhenItIsWhatTheMessageSays() {
        val bytes = ByteArray(2500) { (it % 251).toByte() }
        assertEquals("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", TeamAttachments.sha256Hex("abc".toByteArray()))
        val file = TeamMessageAttachment("upload_0001", "a.txt", "text/plain", bytes.size, TeamAttachments.sha256Hex(bytes))
        val parts = listOf(bytes.copyOfRange(0, 1000), bytes.copyOfRange(1000, 2000), bytes.copyOfRange(2000, 2500)).map { Base64.getEncoder().encodeToString(it) }
        assertArrayEquals(bytes, TeamAttachments.assemble(parts, file))
        for (wrong in listOf(parts.dropLast(1), listOf(parts[1], parts[0], parts[2]), parts + "!")) {
            assertEquals("ATTACHMENT_DAMAGED", assertThrows(TeamMessageFailure::class.java) { TeamAttachments.assemble(wrong, file) }.code)
        }
        assertThrows(TeamMessageFailure::class.java) { TeamAttachments.assemble(parts, file.copy(sha256 = "0".repeat(64))) }
    }

    @Test fun onlyAnswersThatMayChangeAreAskedAgain() {
        for (code in listOf("UPLOAD_QUOTA_EXCEEDED", "UPLOAD_DIGEST_MISMATCH", "ATTACHMENT_REVOKED", "INVALID_INPUT", "INVALID_SESSION", "CONVERSATION_REQUIRED",
            "MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "SIGN_IN_REQUIRED", "REQUEST_TOO_LARGE", "COMMAND_ID_CONFLICT")) assertTrue(code, TeamAttachments.isFinal(code))
        for (code in listOf("", "SERVICE_UNAVAILABLE", "INTERNAL", "RATE_LIMITED")) assertFalse(code, TeamAttachments.isFinal(code))
    }

    @Test fun aMessageIsSentWhenEveryFileIsCheckedAndThereIsTextOrAFile() {
        val ready = TeamDraftFile("upload_0001", "a.txt", "text/plain", 3, TeamDraftFile.State.Ready, 3)
        assertFalse(TeamAttachments.canSend("  ", emptyList()))
        assertTrue(TeamAttachments.canSend("hello", emptyList()))
        assertTrue(TeamAttachments.canSend("", listOf(ready)))
        assertTrue(TeamAttachments.canSend("see this", listOf(ready)))
        assertFalse(TeamAttachments.canSend("see this", listOf(ready, ready.copy(state = TeamDraftFile.State.Uploading))))
        assertFalse(TeamAttachments.canSend("see this", listOf(ready.copy(state = TeamDraftFile.State.Failed, error = "UPLOAD_QUOTA_EXCEEDED"))))
        assertFalse(TeamAttachments.canSend("x".repeat(8193), listOf(ready)))
        assertEquals("a.txt, b.pdf", TeamAttachments.said("", listOf("a.txt", "b.pdf")))
        assertEquals("hello", TeamAttachments.said("hello", listOf("a.txt")))
    }

    @Test fun aPictureIsKnownByItsFirstBytes() {
        assertEquals("image/png", TeamAttachments.pictureType(byteArrayOf(0x89.toByte(), 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0)))
        assertEquals("image/jpeg", TeamAttachments.pictureType(byteArrayOf(0xff.toByte(), 0xd8.toByte(), 0xff.toByte(), 0xe0.toByte(), 0)))
        assertNull(TeamAttachments.pictureType(byteArrayOf(0, 0, 0, 0x18, 0x66, 0x74, 0x79, 0x70, 0x68, 0x65, 0x69, 0x63)))
        assertNull(TeamAttachments.pictureType(ByteArray(0)))
    }

    @Test fun aMessageCarriesItsFilesAndABotsReplyNamesWhatItRead() {
        val base = JSONObject("""{"id":"message_0001","seq":4,"author":{"kind":"user","uid":"user_0001"},"kind":"text","text":"","changeSeq":1,"changeId":"change_0001"}""")
        val files = JSONArray().put(JSONObject().put("attachmentId", "upload_0001").put("name", "a.png").put("mime", "image/png").put("byteLength", 12).put("sha256", "a".repeat(64)))
            .put(JSONObject().put("attachmentId", "upload_0002").put("name", "b.pdf").put("mime", "application/pdf").put("byteLength", 9).put("sha256", "b".repeat(64)).put("revoked", true))
        val message = TeamMessage.parse(base.with("attachments", files).put("readFiles", JSONArray().put(JSONObject().put("attachmentId", "upload_0001").put("name", "a.png"))))
        assertEquals(listOf("a.png", "b.pdf"), message.files.map { it.name })
        assertTrue(message.files[0].isImage); assertTrue(message.files[1].revoked)
        assertEquals(listOf(TeamReadFile("upload_0001", "a.png")), message.readFiles)
        assertTrue(TeamMessage.parse(base.with("attachments", files).put("recalled", true)).files.isEmpty())
        assertTrue(TeamMessage.parse(base).files.isEmpty())
        // A file list that breaks the stored contract fails the message.
        fun broken(change: (JSONObject) -> Unit) = JSONArray().put(JSONObject(files.getJSONObject(0).toString()).also(change))
        for (wrong in listOf(JSONArray(), broken { it.put("mime", "image/gif") }, broken { it.put("byteLength", 0) }, broken { it.put("sha256", "A".repeat(64)) }, broken { it.put("name", "") },
            broken { it.put("revoked", "yes") }, JSONArray().put(files.getJSONObject(0)).put(files.getJSONObject(0)))) {
            assertThrows(wrong.toString(), Exception::class.java) { TeamMessage.parse(base.with("attachments", wrong)) }
        }
        assertThrows(Exception::class.java) { TeamMessage.parse(base.with("attachments", "none")) }
    }

    @Test fun theFileCommandsAreCheckedBeforeTheyAreSent() {
        val submit = JSONObject().put("conversationId", "chat_0001").put("messageId", "message_0001").put("changeId", "change_0001").put("text", "").put("targets", JSONArray())
        refused("submit", submit)
        accepted("submit", submit.with("attachments", JSONArray(listOf("upload_0001"))))
        refused("submit", submit.with("attachments", JSONArray()))
        refused("submit", submit.with("attachments", JSONArray(listOf("upload_0001", "upload_0001"))))
        refused("submit", submit.with("attachments", JSONArray((1..6).map { "upload_000$it" })))
        refused("submit", submit.with("text", "hello"))
        accepted("submit", submit.with("text", "hello").put("targets", JSONArray(listOf("shared_bot_1"))))

        val create = JSONObject().put("conversationId", "chat_0001").put("uploadId", "upload_0001").put("name", "a.png").put("mime", "image/png").put("byteLength", 12).put("sha256", "a".repeat(64))
        accepted("createUpload", create)
        for ((key, value) in listOf("name" to "dir/a.png", "name" to " a.png", "name" to "", "mime" to "image/gif", "byteLength" to 0, "byteLength" to 10 * mib + 1, "sha256" to "A".repeat(64), "uploadId" to "short")) {
            refused("createUpload", create.with(key, value))
        }
        accepted("putUploadPart", JSONObject().put("uploadId", "upload_0001").put("index", 14).put("data", "QUJD"))
        refused("putUploadPart", JSONObject().put("uploadId", "upload_0001").put("index", 15).put("data", "QUJD"))
        refused("putUploadPart", JSONObject().put("uploadId", "upload_0001").put("index", 0).put("data", "A B="))
        refused("putUploadPart", JSONObject().put("uploadId", "upload_0001").put("index", 0).put("data", "A".repeat(4 * ((700 * 1024 + 2) / 3) + 4)))
        accepted("putUploadPart", JSONObject().put("uploadId", "upload_0001").put("index", 0).put("data", Base64.getEncoder().encodeToString(ByteArray(700 * 1024))))
        for (action in listOf("finalizeUpload", "cancelUpload")) {
            accepted(action, JSONObject().put("uploadId", "upload_0001"))
            refused(action, JSONObject().put("uploadId", "upload_0001").put("index", 0))
        }
        accepted("readAttachmentPart", JSONObject().put("attachmentId", "upload_0001").put("index", 0))
        refused("readAttachmentPart", JSONObject().put("attachmentId", "upload_0001").put("index", -1))
        refused("readAttachmentPart", JSONObject().put("attachmentId", "upload_0001"))
        accepted("revokeAttachment", JSONObject().put("attachmentId", "upload_0001").put("changeId", "change_0001"))
        refused("revokeAttachment", JSONObject().put("attachmentId", "upload_0001"))
    }
}
