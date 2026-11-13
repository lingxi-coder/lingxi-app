package com.lingxi.code.teams

import org.json.JSONArray
import org.json.JSONObject
import java.security.MessageDigest
import java.util.Base64

/**
 * A file as a Team message carries it (Desktop `packages/teams/attachments.ts`). [revoked]: its sender took it back and its bytes
 * are gone. The bytes are read through the service every time, so they are only ever shown to someone who reads that conversation now.
 */
internal data class TeamMessageAttachment(val attachmentId: String, val name: String, val mime: String, val byteLength: Int, val sha256: String, val revoked: Boolean = false) {
    val isImage get() = mime == "image/png" || mime == "image/jpeg"
    companion object {
        /** The files of one stored message, read strictly: a list that breaks the contract fails the message, as any other bad field does. */
        fun list(value: JSONArray?): List<TeamMessageAttachment> {
            if (value == null) return emptyList()
            if (value.length() !in 1..TeamAttachments.MESSAGE_FILES) throw TeamMessageFailure("INVALID_DATA")
            return (0 until value.length()).map { index ->
                val v = value.optJSONObject(index) ?: throw TeamMessageFailure("INVALID_DATA")
                val id = v.opt("attachmentId") as? String; val name = v.opt("name") as? String; val mime = v.opt("mime") as? String
                val length = (v.opt("byteLength") as? Number)?.toLong(); val digest = v.opt("sha256") as? String
                val revoked = if (v.has("revoked")) v.opt("revoked") as? Boolean ?: throw TeamMessageFailure("INVALID_DATA") else false
                if (id == null || !ID.matches(id) || name.isNullOrEmpty() || name.length > 120 || mime !in TeamAttachments.TYPES ||
                    length == null || length !in 1..TeamAttachments.FILE_BYTES || digest == null || !DIGEST.matches(digest)) throw TeamMessageFailure("INVALID_DATA")
                TeamMessageAttachment(id, name, mime!!, length.toInt(), digest, revoked)
            }.also { files -> if (files.map { it.attachmentId }.distinct().size != files.size) throw TeamMessageFailure("INVALID_DATA") }
        }
        private val ID = Regex("^[A-Za-z0-9_-]{8,64}$")
        private val DIGEST = Regex("^[a-f0-9]{64}$")
    }
}

/** On a Bot's reply: a file of the conversation its model was given, as the service checked it. */
internal data class TeamReadFile(val attachmentId: String, val name: String) {
    companion object {
        /** Never throws: what a Bot read is a note under its reply, and a note that cannot be read is left out. */
        fun list(value: JSONArray?): List<TeamReadFile> = (0 until (value?.length() ?: 0)).mapNotNull { index ->
            val v = value!!.optJSONObject(index) ?: return@mapNotNull null
            val id = v.opt("attachmentId") as? String; val name = v.opt("name") as? String
            if (id.isNullOrEmpty() || name.isNullOrEmpty()) null else TeamReadFile(id, name)
        }
    }
}

/** A file in an unsent message. [sent] is how many of its bytes the service has; [error] is the service's code, or a local one, when it failed. */
internal data class TeamDraftFile(val uploadId: String, val name: String, val mime: String, val byteLength: Int, val state: State = State.Uploading, val sent: Int = 0, val error: String? = null) {
    enum class State { Uploading, Ready, Failed }
}

/** A file the viewer chose, read into memory. [error]: it could not be read or made into a kind Teams takes. */
internal class TeamChosenFile(val name: String, val type: String, val bytes: ByteArray, val error: String? = null)

/** A chosen file that did not go into the message, and why. */
internal data class TeamFileRefusal(val name: String, val code: String)

/** The rules for files in a Team message. No Android types, so they are tested on the JVM; iOS has the same in `TeamAttachments.swift`. */
internal object TeamAttachments {
    const val PART_BYTES = 700 * 1024
    const val FILE_BYTES = 10 * 1024 * 1024
    const val MESSAGE_FILES = 5
    const val MESSAGE_BYTES = 20 * 1024 * 1024
    val TYPES = listOf("image/png", "image/jpeg", "application/pdf", "text/plain")
    /** What the system's file chooser is asked for. A picture of another kind is made into a JPEG; anything else that gets through is refused with the reason. */
    val CHOOSER_TYPES = arrayOf("image/*", "application/pdf", "text/plain", "text/markdown", "text/csv", "text/comma-separated-values")
    private val TEXT_ENDINGS = setOf("txt", "md", "markdown", "csv", "log")

    /** The type a file is sent as, from what the system says about it and from its name; null for a kind of file Teams does not take. */
    fun type(name: String, declared: String): String? {
        when (val given = declared.lowercase().substringBefore(';').trim()) {
            "image/png", "image/jpeg", "application/pdf" -> return given
            "image/jpg" -> return "image/jpeg"
            "text/plain", "text/markdown", "text/csv" -> return "text/plain"
            "" -> Unit
            else -> return null
        }
        val ending = if ('.' in name) name.substringAfterLast('.').lowercase() else ""
        return when (ending) {
            "png" -> "image/png"
            "jpg", "jpeg" -> "image/jpeg"
            "pdf" -> "application/pdf"
            else -> if (ending in TEXT_ENDINGS) "text/plain" else null
        }
    }

    /** A file's own name as the service takes it: no path, no control characters, at most 120 UTF-16 units with its ending kept. */
    fun name(name: String): String {
        val base = name.split('/', '\\').last().filterNot { it.code <= 0x1f || it.code == 0x7f }.trim()
        if (base.isEmpty() || base == "." || base == "..") return "file"
        if (base.length <= 120) return base
        val dot = base.lastIndexOf('.')
        val ending = if (dot > 0 && base.length - dot <= 10) base.substring(dot) else ""
        val body = base.substring(0, base.length - ending.length)
        val stem = StringBuilder()
        var index = 0
        // A character is never cut in half.
        while (index < body.length) {
            val units = Character.charCount(body.codePointAt(index))
            if (stem.length + units > 120 - ending.length) break
            stem.append(body, index, index + units); index += units
        }
        return stem.toString().trimEnd() + ending
    }

    /** 1.4 MB, 320 KB, 12 B. */
    fun size(bytes: Int): String = when {
        bytes >= 1024 * 1024 -> { val tenths = Math.round(bytes / (1024.0 * 1024.0) * 10); if (tenths % 10 == 0L) "${tenths / 10} MB" else "${tenths / 10}.${tenths % 10} MB" }
        bytes >= 1024 -> "${Math.round(bytes / 1024.0)} KB"
        else -> "$bytes B"
    }

    class Picked(val name: String, val type: String, val size: Int)
    data class Accepted(val index: Int, val name: String, val mime: String)
    data class Admission(val accepted: List<Accepted>, val refused: List<TeamFileRefusal>)

    /**
     * Which of the chosen files go into a draft that already holds files of [draft] bytes each, in the order they were chosen, and why
     * each of the others does not: not a kind Teams takes, empty, over 10 MiB, a sixth file, or more than 20 MiB together.
     */
    fun admit(draft: List<Int>, files: List<Picked>): Admission {
        val accepted = mutableListOf<Accepted>(); val refused = mutableListOf<TeamFileRefusal>()
        var count = draft.size; var total = draft.sumOf { it.toLong() }
        files.forEachIndexed { index, file ->
            val mime = type(file.name, file.type); val sendName = name(file.name)
            val code = when {
                mime == null -> "ATTACHMENT_TYPE"
                file.size == 0 -> "ATTACHMENT_EMPTY"
                file.size > FILE_BYTES -> "ATTACHMENT_TOO_LARGE"
                count >= MESSAGE_FILES -> "ATTACHMENT_COUNT"
                total + file.size > MESSAGE_BYTES -> "ATTACHMENT_TOTAL"
                else -> null
            }
            if (code != null) refused += TeamFileRefusal(sendName, code)
            else { accepted += Accepted(index, sendName, mime!!); count += 1; total += file.size }
        }
        return Admission(accepted, refused)
    }

    fun sha256Hex(bytes: ByteArray): String = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }

    /** Whether an answer will not change by asking again. Anything else (no connection, a busy service) is tried again. */
    fun isFinal(code: String): Boolean = code.startsWith("UPLOAD_") || code.startsWith("ATTACHMENT_") || code.startsWith("INVALID_") ||
        code in setOf("CONVERSATION_REQUIRED", "MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "SIGN_IN_REQUIRED", "REQUEST_TOO_LARGE", "COMMAND_ID_CONFLICT")

    /** A file of a message put together from its parts and checked against what the message says it is. */
    fun assemble(parts: List<String>, file: TeamMessageAttachment): ByteArray {
        val out = java.io.ByteArrayOutputStream(file.byteLength)
        for (part in parts) out.write(try { Base64.getDecoder().decode(part) } catch (_: IllegalArgumentException) { throw TeamMessageFailure("ATTACHMENT_DAMAGED") })
        val bytes = out.toByteArray()
        if (bytes.size != file.byteLength || sha256Hex(bytes) != file.sha256) throw TeamMessageFailure("ATTACHMENT_DAMAGED")
        return bytes
    }

    /** What a picture's bytes are: `image/png` or `image/jpeg` by their first bytes, null for anything else (which is made into a JPEG before it is sent). */
    fun pictureType(bytes: ByteArray): String? {
        fun at(index: Int) = bytes[index].toInt() and 0xff
        if (bytes.size >= 8 && listOf(0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a).withIndex().all { (index, value) -> at(index) == value }) return "image/png"
        if (bytes.size >= 3 && at(0) == 0xff && at(1) == 0xd8 && at(2) == 0xff) return "image/jpeg"
        return null
    }

    /** Whether the unsent message can be sent: every file is checked, and there is text or a file. */
    fun canSend(text: String, files: List<TeamDraftFile>): Boolean {
        if (files.any { it.state != TeamDraftFile.State.Ready }) return false
        return if (files.isEmpty()) TeamsComposeLogic.canSendText(text) else TeamsComposeLogic.byteLength(text) <= TeamsComposeLogic.MESSAGE_BYTES
    }

    /** What a pending bubble or a snippet says for a message of files alone. */
    fun said(text: String, names: List<String>): String = text.ifEmpty { names.joinToString(", ") }

    /** The names an unsent message's command keeps for its pending bubble; they are not sent. */
    fun names(intent: JSONObject): List<String> = intent.optJSONArray("attachmentNames")?.strings() ?: emptyList()
}
