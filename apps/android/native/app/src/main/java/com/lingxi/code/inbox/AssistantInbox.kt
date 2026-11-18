package com.lingxi.code.inbox

import com.lingxi.code.conversation.ConversationLaunchRequest
import com.lingxi.code.remote.KeystoreRemoteCipher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.security.SecureRandom
import java.util.Base64
import java.util.concurrent.ConcurrentHashMap

internal data class InboxAssistantRoute(val local: ConversationLaunchRequest? = null, val hostId: String? = null, val sessionId: String)

internal data class AssistantCatalogEntry(
    val item: InboxItem, val route: InboxAssistantRoute, val publishable: Boolean, val previewKind: String? = null,
)

/** Typed catalog descriptors are optional on legacy native SDKs. The JSON payload is the stable wire boundary. */
internal object AssistantActivityBinding {
    private val getters = ConcurrentHashMap<Class<*>, Result<java.lang.reflect.Method>>()

    fun activity(dto: Any): String? = runCatching {
        getters.computeIfAbsent(dto.javaClass) { type -> runCatching { type.getMethod("getActivityJson") } }
            .getOrNull()?.invoke(dto) as? String
    }.getOrNull()
}

internal fun assistantEntry(
    route: InboxAssistantRoute,
    title: String,
    label: String,
    catalogSource: String,
    updatedAt: Long,
    activityJson: String?,
    offline: Boolean,
): AssistantCatalogEntry? {
    val activity = AssistantActivity.parse(activityJson)
    val ref = InboxRef("assistant", activity?.sourceId ?: catalogSource, route.sessionId.removePrefix("sess:"), 0)
    if (!ref.isValid()) return null
    return AssistantCatalogEntry(
        item = InboxItem(
            ref = ref, category = "assistant", title = title, context = label,
            lastActivityAt = activity?.at ?: updatedAt, sourceCursor = activity?.cursor ?: 0,
            sourceRevision = activity?.revision ?: 0, readThrough = activity?.baseline ?: 0, baselineCursor = activity?.baseline ?: 0,
            unreadCount = activity?.let { (it.cursor - it.baseline).coerceIn(0, 100).toInt() } ?: 0,
            pendingActionCount = activity?.pendingActions ?: 0,
            pendingActionAt = activity?.pendingActionAt,
            pendingActionRevision = activity?.pendingActionRevision,
            failureAt = activity?.failureAt,
            failureRevision = activity?.failureRevision,
            attentionKind = if (activity?.failed == true) "failure" else null,
            state = when {
                (activity?.pendingActions ?: 0) > 0 -> if (activity?.pendingActionKind == "approval") "waiting_for_approval" else "waiting_for_input"
                activity?.failed == true -> "failed"
                activity?.previewKind != null -> "completed"
                else -> "idle"
            },
            localOnly = true, offline = offline,
        ),
        route = route, publishable = activity != null, previewKind = activity?.previewKind,
    )
}

/** Only publishes host-owned descriptors after consent. The source registration secret is bound to the account and host. */
internal class InboxAssistantPublisher(
    private val storage: InboxStorage,
    private val send: suspend (String, JSONObject) -> JSONObject,
) {
    private val cipher = KeystoreRemoteCipher()
    private data class Registration(val consentRevision: Long, val generation: Long)
    private val generations = mutableMapOf<Pair<String, String>, Registration>()

    suspend fun sync(owner: String, consent: AssistantConsent, entries: List<AssistantCatalogEntry>): Boolean {
        var changed = false
        val active = entries.groupBy { it.item.ref.sourceId }
        val (tracked, choices) = withContext(Dispatchers.IO) { trackedSources(owner) to storage.conversationConsents(owner) }
        val removals = choices.values.filterNot { it.consent.enabled }.groupBy { it.ref.sourceId }
        val sourceIds = if (consent.enabled) active.keys + removals.keys else tracked
        for (source in sourceIds) {
            val secret = sourceSecret(owner, source)
            val sourceKey = owner to source
            val generation = generations[sourceKey]?.takeIf { consent.enabled && it.consentRevision == consent.revision }?.generation ?: run {
                // A request may reach Firebase after cancellation. Track it before sending.
                withContext(Dispatchers.IO) {
                    storage.setValue(owner, "assistantRegistered", (trackedSources(owner) + source).joinToString(","))
                }
                val accepted = send(owner, JSONObject().put("action", "assistantSource").put("sourceId", source)
                    .put("sourceSecret", secret).put("enabled", consent.enabled).put("consentRevision", consent.revision))
                val acceptedRevision = accepted.optLong("consentRevision", -1)
                if (acceptedRevision != consent.revision || consent.enabled != accepted.optBoolean("enabled")) return changed
                val acceptedGeneration = accepted.getLong("generation")
                if (consent.enabled) generations[sourceKey] = Registration(consent.revision, acceptedGeneration)
                else {
                    withContext(Dispatchers.IO) {
                        storage.setValue(owner, "assistantRegistered", (trackedSources(owner) - source).joinToString(","))
                    }
                    generations.remove(sourceKey)
                }
                changed = true
                acceptedGeneration
            }
            if (!consent.enabled) continue
            for (choice in removals[source].orEmpty()) {
                val removedKey = "removed:$generation:${choice.consent.revision}:${choice.ref.key}"
                if (withContext(Dispatchers.IO) { storage.value(owner, removedKey) } != null) continue
                val command = JSONObject().put("action", "assistantRemove").put("sourceId", source)
                    .put("sourceSecret", secret).put("conversationId", choice.ref.conversationId)
                    .put("generation", generation).put("sourceRevision", choice.sourceRevision)
                    .put("conversationConsentRevision", choice.consent.revision)
                sendScoped(owner, sourceKey, command)
                withContext(Dispatchers.IO) { storage.setValue(owner, removedKey, "true") }
                changed = true
            }
            for (entry in active[source].orEmpty()) {
                val item = entry.item
                val choice = choices[item.key]?.consent ?: AssistantConsent(enabled = true)
                if (!choice.enabled) continue
                val checkpoint = "published:$generation:${choice.revision}:${item.key}"
                val saved = withContext(Dispatchers.IO) { storage.value(owner, checkpoint)?.toLongOrNull() }
                if (saved != null && saved >= item.sourceRevision) continue
                val command = JSONObject().put("action", "assistantPublish").put("sourceId", source)
                    .put("sourceSecret", secret).put("conversationId", item.ref.conversationId)
                    .put("generation", generation).put("sourceRevision", item.sourceRevision).put("sourceCursor", item.sourceCursor)
                    .put("baseline", item.baselineCursor).put("lastActivityAt", item.lastActivityAt)
                    .put("state", item.state).put("pendingActionCount", item.pendingActionCount)
                    .put("conversationConsentRevision", choice.revision)
                    .put("previewKind", entry.previewKind ?: JSONObject.NULL).put("sourceLabel", "Android")
                item.pendingActionRevision?.let { command.put("pendingActionRevision", it) }
                item.pendingActionAt?.let { command.put("pendingActionAt", it) }
                item.failureAt?.let { command.put("failureAt", it) }
                item.failureRevision?.let { command.put("failureRevision", it) }
                sendScoped(owner, sourceKey, command)
                if (saved == null) migrateLocalRead(owner, item, generation)
                withContext(Dispatchers.IO) { storage.setValue(owner, checkpoint, item.sourceRevision.toString()) }
                changed = true
            }
        }
        return changed
    }

    private suspend fun sendScoped(owner: String, sourceKey: Pair<String, String>, command: JSONObject) {
        try {
            send(owner, command)
        } catch (failure: com.lingxi.code.teams.TeamMessageFailure) {
            if (failure.code in setOf("STALE_MEMBERSHIP", "SOURCE_ACCESS_REQUIRED")) generations.remove(sourceKey)
            throw failure
        }
    }

    private fun trackedSources(owner: String): Set<String> =
        storage.value(owner, "assistantRegistered").orEmpty().split(',').filter { it.isNotBlank() }.toSet()

    private suspend fun migrateLocalRead(owner: String, item: InboxItem, generation: Long) {
        val local = withContext(Dispatchers.IO) {
            storage.value(owner, "local:${item.key}")?.let { InboxItem.parse(JSONObject(it)) }
        } ?: return
        val through = minOf(local.readThrough, item.sourceCursor)
        if (through <= item.baselineCursor) return
        val operation = InboxOperation.read(item.ref.copy(generation = generation), through, 0L)
        operation.body.put("operationId", java.util.UUID.nameUUIDFromBytes("${item.key}:$generation:$through".toByteArray()).toString())
        send(owner, operation.body)
    }

    private suspend fun sourceSecret(owner: String, source: String): String = withContext(Dispatchers.IO) {
        val key = "assistantSecret:$source"
        val aad = "$owner:$key".toByteArray()
        val existing = storage.value(owner, key)
        if (existing != null) return@withContext String(cipher.open(Base64.getUrlDecoder().decode(existing), aad))
        val secret = Base64.getUrlEncoder().withoutPadding().encodeToString(ByteArray(32).also(SecureRandom()::nextBytes))
        storage.setValue(owner, key, Base64.getUrlEncoder().withoutPadding().encodeToString(cipher.seal(secret.toByteArray(), aad)))
        secret
    }
}
