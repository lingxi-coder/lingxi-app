package com.lingxi.code.conversation

import android.content.Context
import android.content.Intent
import android.net.Uri
import com.lingxi.code.MainActivity
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.sessionModeFromWireValue
import java.net.URI
import java.net.URLDecoder
import java.nio.charset.StandardCharsets

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
        val raw = route?.trim().takeUnless { it.isNullOrEmpty() } ?: return null
        val uri = runCatching { URI(raw) }.getOrNull() ?: return null
        val authority = uri.authority ?: uri.host
        if (!uri.scheme.equals("lingxi", ignoreCase = true) ||
            !authority.equals("open_conversation", ignoreCase = true)
        ) {
            return null
        }
        val params = uri.rawQuery
            ?.split('&')
            ?.mapNotNull { pair ->
                if (pair.isEmpty()) return@mapNotNull null
                val parts = pair.split('=', limit = 2)
                val key = decode(parts[0]).trim()
                val value = decode(parts.getOrElse(1) { "" }).trim()
                key.takeIf(String::isNotEmpty)?.let { it to value }
            }
            ?.toMap()
            .orEmpty()
        val sessionId = params["sessionId"]?.takeUnless(String::isEmpty) ?: return null
        val turnId = params["turnId"]?.takeUnless(String::isEmpty)?.toLongOrNull()
        return ConversationLaunchRequest(
            sessionId = sessionId,
            turnId = turnId,
            workspaceKey = params["workspaceKey"]?.takeUnless(String::isEmpty),
            sessionMode = sessionModeFromWireValue(params["sessionMode"]),
        )
    }

    fun uri(
        sessionId: String,
        turnId: Long? = null,
        recoverySpec: ConversationRecoverySpec? = null,
    ): Uri =
        Uri.Builder()
            .scheme("lingxi")
            .authority("open_conversation")
            .appendQueryParameter("sessionId", sessionId)
            .apply {
                if (turnId != null) {
                    appendQueryParameter("turnId", turnId.toString())
                }
                recoverySpec?.workspaceKey?.let { appendQueryParameter("workspaceKey", it) }
                recoverySpec?.sessionMode?.let { appendQueryParameter("sessionMode", it.wireValue) }
            }
            .build()

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
            // Keep the legacy extra during rollout; the service prefers
            // Intent.action and only falls back to this value on redelivery.
            .putExtra(ConversationTurnService.EXTRA_ACTION, route.action)
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

    // `URLDecoder.decode(String, Charset)` is API 33+; this module's minSdk is
    // 26 and core-library desugaring is not enabled, so the Charset overload
    // throws NoSuchMethodError on API 26-32 the moment a conversation
    // notification is tapped. The charset-NAME overload has existed since API 1
    // and decodes identically.
    private fun decode(value: String): String =
        URLDecoder.decode(value, StandardCharsets.UTF_8.name())
}
