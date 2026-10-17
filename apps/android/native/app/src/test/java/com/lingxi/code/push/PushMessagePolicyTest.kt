package com.lingxi.code.push

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PushMessagePolicyTest {
    private val message = mapOf("recipientUid" to "user-a", "expiresAt" to "2000", "title" to "Title", "body" to "Body")
    @Test fun acceptsCurrentAccountBeforeExpiry() {
        assertTrue(PushMessagePolicy.accepts(message, "user-a", true, 1000))
    }
    @Test fun rejectsLoggedOutAndSwitchedAccounts() {
        assertFalse(PushMessagePolicy.accepts(message, null, true, 1000))
        assertFalse(PushMessagePolicy.accepts(message, "user-b", true, 1000))
    }
    @Test fun rejectsExpiredMalformedAndDisabledMessages() {
        assertFalse(PushMessagePolicy.accepts(message, "user-a", true, 2000))
        assertFalse(PushMessagePolicy.accepts(message + ("expiresAt" to "bad"), "user-a", true, 1000))
        assertFalse(PushMessagePolicy.accepts(message - "recipientUid", "user-a", true, 1000))
        assertFalse(PushMessagePolicy.accepts(message, "user-a", false, 1000))
    }
}
