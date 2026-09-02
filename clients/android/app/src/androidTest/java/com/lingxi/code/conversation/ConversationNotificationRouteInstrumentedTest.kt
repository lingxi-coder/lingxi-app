package com.lingxi.code.conversation

import android.app.PendingIntent
import android.content.Intent
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.model.SessionMode
import com.lingxi.code.settings.LinuxRuntimeMode
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import org.junit.runner.RunWith

/** Exercises the real Android Intent/PendingIntent contract, not JVM stubs. */
@RunWith(AndroidJUnit4::class)
class ConversationNotificationRouteInstrumentedTest {
    @Test
    fun cancelIntentTargetsPrivateServiceAndUsesOnStartDispatchAction() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val intent = ConversationNotificationRoute.cancelIntent(
            context = context,
            sessionId = "session-a",
            turnId = 42L,
            recoverySpec = ConversationRecoverySpec(
                projectId = null,
                hostPath = null,
                sessionMode = SessionMode.Code,
                linuxRuntimeMode = LinuxRuntimeMode.Legacy,
                workspaceKey = "global",
            ),
        )

        assertEquals(ConversationTurnService.ACTION_CANCEL, intent.action)
        assertEquals(ConversationTurnService::class.java.name, intent.component?.className)
        assertEquals("session-a", intent.getStringExtra(ConversationTurnService.EXTRA_SESSION_ID))
        assertEquals(42L, intent.getLongExtra(ConversationTurnService.EXTRA_TURN_ID, -1L))
        assertEquals("code", intent.getStringExtra(ConversationTurnService.EXTRA_SESSION_MODE))
        assertEquals("global", intent.getStringExtra(ConversationTurnService.EXTRA_WORKSPACE_KEY))
        assertNull(intent.data)
        assertEquals(
            ConversationTurnService.ACTION_CANCEL,
            intent.getStringExtra(ConversationTurnService.EXTRA_ACTION),
        )
        assertEquals(ConversationTurnService.ACTION_CANCEL, conversationServiceAction(intent))

        val pending = PendingIntent.getService(
            context,
            42,
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        assertEquals(context.packageName, pending.creatorPackage)
        pending.cancel()
    }

    @Test
    fun legacyExtraStillDispatchesWhenActionIsAbsent() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val legacy = Intent(context, ConversationTurnService::class.java)
            .putExtra(ConversationTurnService.EXTRA_ACTION, ConversationTurnService.ACTION_CANCEL)

        assertEquals(ConversationTurnService.ACTION_CANCEL, conversationServiceAction(legacy))
    }
}
