package com.lingxi.code.model

import org.junit.Assert.assertEquals
import org.junit.Test

class NewChatTargetTest {
    @Test fun newChatUsesCurrentModeAfterCrossModeNavigationAndAlwaysStartsGlobal() {
        var currentMode = SessionMode.Code
        assertEquals(ConversationScope.Global to SessionMode.Code, defaultNewChatTarget(currentMode))
        currentMode = SessionMode.Chat // Handoff or opening a Chat session changes the visible owner.
        assertEquals(ConversationScope.Global to SessionMode.Chat, defaultNewChatTarget(currentMode))
        currentMode = SessionMode.Code
        assertEquals(ConversationScope.Global to SessionMode.Code, defaultNewChatTarget(currentMode))
    }
}
