package com.lingxi.code.apps

import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.canonicalSessionId
import com.lingxi.code.project.SessionWorkspaceState

internal fun isRuntimeAppRequest(op: String): Boolean = op in setOf(
    "list", "get", "releases", "getRelease", "grant", "stop", "revoke", "checkTarget",
    "start", "restart", "repair", "suspend", "runs", "logs", "remove", "prepareAuthoring", "export", "layout", "status",
)

internal fun isRuntimeAgentRequest(op: String): Boolean = op in setOf(
    "catalog", "settings", "configure", "connectMcp", "interactions", "cancel", "answer", "tasks.list", "tasks.cancel",
)

/** The native request completes for its original owner; a newer navigation only parks its result. */
internal suspend fun <T : Any> awaitAdmittedAuthoringSource(
    submit: suspend () -> T,
    navigationIsCurrent: () -> Boolean,
): T? = submit().takeIf { navigationIsCurrent() }

internal fun coldAuthoringResumeMatches(intent: AuthoringIntent, sessionId: String?, mode: SessionMode?,
    resumed: Boolean, workspace: SessionWorkspaceState?): Boolean = resumed && mode == SessionMode.Code &&
    sessionId != null && canonicalSessionId(sessionId) == intent.sessionId && workspace != null &&
    canonicalSessionId(workspace.sessionId) == intent.sessionId && workspace.cwd == intent.sourceDirectory &&
    workspace.transcriptPath == intent.transcriptPath && !workspace.transcriptPath.isBlank()

/** The callbacks may suspend, so verify the native owner again before opening its gate. */
internal suspend fun completeColdAuthoringRecovery(matches: () -> Boolean, register: suspend () -> Unit,
    persist: suspend () -> Unit, admit: () -> Boolean) {
    check(matches()) { "The resumed authoring conversation does not match its source" }
    register()
    check(matches()) { "The resumed authoring conversation changed during registration" }
    persist()
    check(matches() && admit()) { "The resumed authoring conversation changed before admission" }
}

/** A lost acknowledgement is resolved through the exact durable receipt before retrying. */
internal suspend fun <T> registerAuthoringWithReceipt(
    submit: suspend () -> Unit,
    lookup: suspend () -> T,
    isNotFound: (Throwable) -> Boolean,
    matches: (T) -> Boolean,
    lookupBeforeSubmit: Boolean = false,
    receiptProvesSubmission: Boolean = true,
): T {
    if (lookupBeforeSubmit && receiptProvesSubmission) {
        val existing = try { lookup() } catch (error: Exception) {
            if (!isNotFound(error)) throw error
            null
        }
        if (existing != null) {
            check(matches(existing)) { "Authoring owner receipt does not match the selected source" }
            return existing
        }
    }
    repeat(2) { attempt ->
        val failure = try { submit(); null } catch (error: Exception) { error }
        // A pre-existing owner receipt cannot prove a different source-to-app request committed.
        if (failure != null && !receiptProvesSubmission) throw failure
        val receipt = try { lookup() } catch (error: Exception) {
            if (failure != null && attempt == 0 && isNotFound(error)) return@repeat
            throw failure ?: error
        }
        check(matches(receipt)) { "Authoring owner receipt does not match the selected source" }
        return receipt
    }
    error("Authoring registration did not complete")
}

/** Missing receipts may be repaired only after the saved source ID is revalidated by native. */
internal suspend fun <T> lookupOrRecoverAuthoringReceipt(
    source: String,
    projectId: String,
    sessionId: String,
    lookup: suspend () -> T,
    isNotFound: (Throwable) -> Boolean,
    readIntent: suspend () -> AuthoringIntent?,
    prepareSource: suspend (AuthoringIntent) -> Unit,
    submitRecovery: suspend (AuthoringIntent) -> Unit,
    matches: (T, AuthoringIntent) -> Boolean,
): T {
    val existing = try { lookup() } catch (error: Exception) {
        if (!isNotFound(error)) throw error
        null
    }
    val intent = checkNotNull(readIntent()) { "Authoring source intent is unavailable" }
    require(intent.sourceDirectory == source && intent.projectId == projectId && intent.sessionId == sessionId)
    if (existing != null) {
        require(matches(existing, intent)) { "Authoring receipt conflicts with the saved owner intent" }
        return existing
    }
    prepareSource(intent)
    return registerAuthoringWithReceipt(
        submit = { submitRecovery(intent) }, lookup = lookup, isNotFound = isNotFound,
        matches = { matches(it, intent) }, lookupBeforeSubmit = true,
    )
}
