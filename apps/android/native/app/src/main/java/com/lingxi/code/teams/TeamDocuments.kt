package com.lingxi.code.teams

import org.json.JSONObject

/** Who wrote a revision of a shared document: a member, or the Bot whose task produced it. */
internal data class TeamRevisionAuthor(val kind: String, val uid: String? = null, val sharedBotId: String? = null)
internal data class TeamArtifactRevision(val revision: Long, val parentRevision: Long?, val digest: String, val title: String, val author: TeamRevisionAuthor, val createdAt: Long, val sourceTaskId: String? = null)
/** The history of a document as `listArtifactRevisions` returns it: newest first, without bodies. */
internal data class TeamArtifactRevisions(val artifactId: String, val currentRevision: Long, val revisions: List<TeamArtifactRevision>) {
    companion object {
        fun parse(v: JSONObject) = TeamArtifactRevisions(v.getString("artifactId"), v.getLong("currentRevision"), v.getJSONArray("revisions").objects().map { row ->
            val author = row.getJSONObject("author")
            TeamArtifactRevision(row.getLong("revision"), if (row.isNull("parentRevision")) null else row.getLong("parentRevision"), row.getString("digest"), row.getString("title"),
                TeamRevisionAuthor(author.getString("kind"), author.stringOrNull("uid"), author.stringOrNull("sharedBotId")), row.getLong("createdAt"), row.stringOrNull("sourceTaskId"))
        })
    }
}

/** The text a member is editing. It is saved only on top of the revision it was made from. */
internal data class TeamDocumentDraft(val title: String, val body: String) {
    constructor(artifact: TeamArtifact) : this(artifact.title, artifact.body)

    val trimmedTitle get() = title.trim()
    val tooLong get() = body.toByteArray(Charsets.UTF_8).size > TeamExecutionContract.DOCUMENT_BYTES
    /** What the service stores: a title of 1 to 120 UTF-16 units and a body that is not empty and within the size limit. */
    val valid get() = trimmedTitle.isNotEmpty() && trimmedTitle.length <= 120 && body.isNotEmpty() && !tooLong
    fun changed(base: TeamArtifact) = trimmedTitle != base.title || body != base.body
    fun json(): JSONObject = JSONObject().put("title", trimmedTitle).put("body", body)
}
/**
 * The identity of one exact edit of one revision. Saving the same text again after a lost answer sends the same id and creates no
 * second revision; other text, or the same text on another revision, gets a new id.
 */
internal data class TeamDocumentOperation(val revision: Long, val title: String, val body: String, val id: String) {
    companion object {
        fun of(current: TeamDocumentOperation?, revision: Long, draft: TeamDocumentDraft, newId: () -> String = { java.util.UUID.randomUUID().toString() }) =
            if (current != null && current.revision == revision && current.title == draft.trimmedTitle && current.body == draft.body) current
            else TeamDocumentOperation(revision, draft.trimmedTitle, draft.body, newId())
    }
}
internal object TeamDocumentRules {
    /** The name the history shows for a revision's author; a member who left or an unpublished Bot keeps a generic name. */
    fun authorName(author: TeamRevisionAuthor, members: List<TeamMember>, bots: List<TeamBotCard>, strings: TeamsListStrings): String =
        if (author.kind == "user") members.firstOrNull { it.uid == author.uid }?.displayName.orEmpty().ifEmpty { strings.member }
        else bots.firstOrNull { it.sharedBotId == author.sharedBotId }?.name.orEmpty().ifEmpty { strings.bot }
}
