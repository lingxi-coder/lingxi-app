package com.lingxi.code.conversation

import android.content.Context
import android.content.Intent
import android.net.Uri
import com.lingxi.code.MainActivity
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.sessionModeFromWireValue

data class ConversationLaunchRequest(
    val sessionId: String,
    val turnId: Long?,
    val workspaceKey: String? = null,
    val sessionMode: SessionMode,
    val runtimeGeneration: String? = null,
    val transcriptPath: String? = null,
)

internal data class ConversationCancelRouteSpec(
    val sessionId: String,
    val turnId: Long?,
    val action: String,
    val targetClassName: String,
)

object ConversationNotificationRoute {
    const val EXTRA_NOTIFICATION_OWNER = "conversation_notification_owner"
    private const val EXTRA_NOTIFICATION_TRANSCRIPT_PATH = "conversation_notification_transcript_path"
    fun parse(intent: Intent?): ConversationLaunchRequest? {
        if (intent?.getBooleanExtra(EXTRA_NOTIFICATION_OWNER, false) != true) return null
        return parseNotification(intent.dataString, intent.getStringExtra(EXTRA_NOTIFICATION_TRANSCRIPT_PATH))
    }

    internal fun parseNotification(route: String?, transcriptPath: String?): ConversationLaunchRequest? {
        if (transcriptPath?.startsWith('/') != true) return null
        return parse(route)?.copy(transcriptPath = transcriptPath)
    }

    internal fun parse(route: String?): ConversationLaunchRequest? {
        val link = com.lingxi.code.navigation.DeepLink.parse(route) ?: return null
        if (!link.route.startsWith("session/")) return null
        val mode = link.params["mode"]?.let { runCatching { sessionModeFromWireValue(it) }.getOrNull() } ?: return null
        val workspace = link.params["workspace"] ?: return null
        val generation = link.params["generation"] ?: return null
        return ConversationLaunchRequest(
            sessionId = link.route.substringAfter('/'),
            turnId = link.params["turn"]?.toLongOrNull(),
            workspaceKey = workspace,
            sessionMode = mode,
            runtimeGeneration = generation,
        )
    }

    fun uri(
        sessionId: String,
        turnId: Long? = null,
        recoverySpec: ConversationRecoverySpec,
    ): Uri = Uri.parse(canonicalUrl(sessionId, recoverySpec, turnId))

    internal fun canonicalUrl(sessionId: String, recoverySpec: ConversationRecoverySpec, turnId: Long? = null): String =
        com.lingxi.code.navigation.DeepLink.build("session/${sessionId.removePrefix("sess:")}", buildMap {
            put("workspace", requireNotNull(recoverySpec.workspaceKey) { "Notification owner has no workspace" })
            put("mode", recoverySpec.sessionMode.wireValue)
            put("generation", recoverySpec.runtimeGeneration)
            turnId?.let { put("turn", it.toString()) }
        })

    fun openIntent(
        context: Context,
        sessionId: String,
        turnId: Long? = null,
        recoverySpec: ConversationRecoverySpec,
    ): Intent =
        Intent(
            Intent.ACTION_VIEW,
            uri(sessionId = sessionId, turnId = turnId, recoverySpec = recoverySpec),
            context,
            MainActivity::class.java,
        )
            .addCategory(Intent.CATEGORY_BROWSABLE)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
            .putExtra(EXTRA_NOTIFICATION_OWNER, true)
            .putExtra(EXTRA_NOTIFICATION_TRANSCRIPT_PATH, recoverySpec.transcriptPath)

    fun cancelIntent(
        context: Context,
        sessionId: String,
        turnId: Long? = null,
        recoverySpec: ConversationRecoverySpec? = null,
        activeTaskIds: Set<String> = emptySet(),
    ): Intent {
        val route = cancelRouteSpec(sessionId, turnId)
        return Intent(context, ConversationTurnService::class.java)
            .setAction(route.action)
            .setData(Uri.Builder().scheme("lingxi-internal").authority("conversation-stop")
                .appendPath(sessionId).appendPath(turnId?.toString().orEmpty())
                .appendPath(recoverySpec?.runtimeGeneration.orEmpty()).build())
            .putExtra(ConversationTurnService.EXTRA_SESSION_ID, route.sessionId)
            .apply {
                if (route.turnId != null) {
                    putExtra(ConversationTurnService.EXTRA_TURN_ID, route.turnId)
                }
                recoverySpec?.let { spec ->
                    putExtra(ConversationTurnService.EXTRA_PROJECT_ID, spec.projectId)
                    putExtra(ConversationTurnService.EXTRA_HOST_PATH, spec.hostPath)
                    putExtra(ConversationTurnService.EXTRA_GUEST_PATH, spec.guestPath)
                    putExtra(ConversationTurnService.EXTRA_ADDITIONAL_DIRECTORIES, spec.additionalDirectories.toTypedArray())
                    putExtra(ConversationTurnService.EXTRA_SESSION_MODE, spec.sessionMode.wireValue)
                    putExtra(ConversationTurnService.EXTRA_RUNTIME_GENERATION, spec.runtimeGeneration)
                    putExtra(ConversationTurnService.EXTRA_LINUX_RUNTIME_MODE, spec.linuxRuntimeMode.name)
                    putExtra(ConversationTurnService.EXTRA_WORKSPACE_KEY, spec.workspaceKey)
                    putExtra(ConversationTurnService.EXTRA_TRANSCRIPT_PATH, spec.transcriptPath)
                }
                putExtra(ConversationTurnService.EXTRA_ACTIVE_TASK_IDS, activeTaskIds.toTypedArray())
            }
    }

    internal fun cancelRouteSpec(
        sessionId: String,
        turnId: Long?,
    ): ConversationCancelRouteSpec = ConversationCancelRouteSpec(
        sessionId = sessionId,
        turnId = turnId,
        action = ConversationTurnService.ACTION_CANCEL,
        targetClassName = ConversationTurnService::class.java.name,
    )

}
