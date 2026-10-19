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
    val sessionMode: SessionMode = SessionMode.Code,
)

internal data class ConversationCancelRouteSpec(
    val sessionId: String,
    val turnId: Long?,
    val action: String,
    val targetClassName: String,
)

object ConversationNotificationRoute {
    fun parse(intent: Intent?): ConversationLaunchRequest? {
        return parse(intent?.dataString)
    }

    internal fun parse(route: String?): ConversationLaunchRequest? {
        val link = com.lingxi.code.navigation.DeepLink.parse(route) ?: return null
        if (!link.route.startsWith("session/")) return null
        return ConversationLaunchRequest(
            sessionId = link.route.substringAfter('/'),
            turnId = null,
            workspaceKey = link.params["workspace"],
            sessionMode = sessionModeFromWireValue(link.params["mode"]),
        )
    }

    fun uri(
        sessionId: String,
        turnId: Long? = null,
        recoverySpec: ConversationRecoverySpec? = null,
    ): Uri = Uri.parse(canonicalUrl(sessionId, recoverySpec))

    internal fun canonicalUrl(sessionId: String, recoverySpec: ConversationRecoverySpec? = null): String =
        com.lingxi.code.navigation.DeepLink.build("session/${sessionId.removePrefix("sess:")}", buildMap {
            recoverySpec?.workspaceKey?.let { put("workspace", it) }
            recoverySpec?.sessionMode?.let { put("mode", it.wireValue) }
        })

    fun openIntent(
        context: Context,
        sessionId: String,
        turnId: Long? = null,
        recoverySpec: ConversationRecoverySpec? = null,
    ): Intent =
        Intent(
            Intent.ACTION_VIEW,
            uri(sessionId = sessionId, turnId = turnId, recoverySpec = recoverySpec),
            context,
            MainActivity::class.java,
        )
            .addCategory(Intent.CATEGORY_BROWSABLE)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)

    fun cancelIntent(
        context: Context,
        sessionId: String,
        turnId: Long? = null,
        recoverySpec: ConversationRecoverySpec? = null,
    ): Intent {
        val route = cancelRouteSpec(sessionId, turnId)
        return Intent(context, ConversationTurnService::class.java)
            .setAction(route.action)
            .putExtra(ConversationTurnService.EXTRA_SESSION_ID, route.sessionId)
            .apply {
                if (route.turnId != null) {
                    putExtra(ConversationTurnService.EXTRA_TURN_ID, route.turnId)
                }
                recoverySpec?.let { spec ->
                    putExtra(ConversationTurnService.EXTRA_PROJECT_ID, spec.projectId)
                    putExtra(ConversationTurnService.EXTRA_HOST_PATH, spec.hostPath)
                    putExtra(ConversationTurnService.EXTRA_SESSION_MODE, spec.sessionMode.wireValue)
                    putExtra(ConversationTurnService.EXTRA_LINUX_RUNTIME_MODE, spec.linuxRuntimeMode.name)
                    putExtra(ConversationTurnService.EXTRA_WORKSPACE_KEY, spec.workspaceKey)
                }
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
