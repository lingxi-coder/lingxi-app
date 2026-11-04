package com.lingxi.code.teams

import com.google.firebase.firestore.FirebaseFirestore
import com.google.firebase.firestore.ListenerRegistration
import com.google.firebase.firestore.MetadataChanges

/**
 * Directory-only watcher of one team the user belongs to but has not opened (SPEC-mobile-team-bot-ui section 6.6 item 3):
 * `members/{uid}` generation check, the conversations query and one `reads/{uid}` listener per conversation. No messages,
 * no Bots. It reports the team's unread total through [report] once every conversation's read mark has arrived, and 0 when
 * the team stops being readable (membership changed, rules denied, a document broke the stored contract).
 */
internal class TeamsUnreadWatcher(private val db: FirebaseFirestore, private val uid: String, val teamId: String, val generation: Long, private val report: (Int) -> Unit) {
    private var stopped = false
    private var membership: ListenerRegistration? = null
    private var directory: ListenerRegistration? = null
    private val reads = mutableMapOf<String, ListenerRegistration>()
    private val lastSeqs = mutableMapOf<String, Long>()
    private val readSeqs = mutableMapOf<String, Long?>()

    fun start() {
        val root = db.collection("teams").document(teamId)
        membership = root.collection("members").document(uid).addSnapshotListener(MetadataChanges.INCLUDE) { snapshot, error ->
            if (stopped) return@addSnapshotListener
            if (error != null) { stopWith(0); return@addSnapshotListener }
            if (snapshot?.getLong("generation") == generation) { if (directory == null) listenDirectory(root) }
            else if (directory != null || snapshot?.metadata?.isFromCache == false) stopWith(0)
        }
    }

    fun stop() {
        stopped = true
        membership?.remove(); directory?.remove(); reads.values.forEach { it.remove() }
        membership = null; directory = null; reads.clear(); lastSeqs.clear(); readSeqs.clear()
    }

    private fun stopWith(unread: Int) { stop(); report(unread) }

    private fun listenDirectory(root: com.google.firebase.firestore.DocumentReference) {
        directory = root.collection("conversations").whereArrayContains("memberUids", uid).addSnapshotListener(MetadataChanges.INCLUDE) { snapshot, error ->
            if (stopped) return@addSnapshotListener
            if (error != null || snapshot == null) { stopWith(0); return@addSnapshotListener }
            val parsed = try {
                snapshot.documents.associate { doc -> doc.id to TeamMessageConversation.fromDocument(doc.id, doc.data ?: throw TeamMessageFailure("INVALID_DATA"), 0, ::firestoreMillis) }
            } catch (_: TeamMessageFailure) { stopWith(0); return@addSnapshotListener }
            for (id in reads.keys.toList()) if (id !in parsed) { reads.remove(id)?.remove(); readSeqs.remove(id) }
            lastSeqs.keys.retainAll(parsed.keys)
            for ((id, conversation) in parsed) {
                lastSeqs[id] = conversation.lastSeq
                if (id in reads) continue
                readSeqs[id] = null
                reads[id] = root.collection("conversations").document(id).collection("reads").document(uid).addSnapshotListener(MetadataChanges.INCLUDE) { mark, failure ->
                    if (stopped) return@addSnapshotListener
                    if (failure != null) { stopWith(0); return@addSnapshotListener }
                    readSeqs[id] = if (mark?.getLong("generation") == generation) mark.getLong("lastReadSeq") ?: 0 else 0
                    publish()
                }
            }
            publish()
        }
    }

    private fun publish() {
        if (lastSeqs.keys.any { readSeqs[it] == null }) return
        report(lastSeqs.entries.sumOf { (id, lastSeq) -> (lastSeq - (readSeqs[id] ?: 0)).coerceAtLeast(0) }.coerceAtMost(Int.MAX_VALUE.toLong()).toInt())
    }
}
