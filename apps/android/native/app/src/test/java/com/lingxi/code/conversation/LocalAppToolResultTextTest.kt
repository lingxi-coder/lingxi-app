package com.lingxi.code.conversation

import org.junit.Assert.*
import org.junit.Test

class LocalAppToolResultTextTest {
    @Test fun restoredBodyOnlyAndLiveHeadlineBothUseStableReason() {
        val key = "local_app_development_requires_code"
        assertTrue(isLocalAppCodeRequired("LocalApp", AgentToolStatus.Failed, "Failed", "$key: use Code"))
        assertTrue(isLocalAppCodeRequired("LocalApp", AgentToolStatus.Failed, "Error: $key", null))
        assertFalse(isLocalAppCodeRequired("Shell", AgentToolStatus.Failed, key, key))
        assertFalse(isLocalAppCodeRequired("LocalApp", AgentToolStatus.Completed, key, key))
        assertTrue(isLocalAppCodeRequired("LocalApp", AgentToolStatus.Failed, "Failed", "Error: $key: use Code"))
        assertFalse(isLocalAppCodeRequired("LocalApp", AgentToolStatus.Failed, "prefix_$key", null))
        assertFalse(isLocalAppCodeRequired("LocalApp", AgentToolStatus.Failed, "App development requires Code mode", null))
    }

    @Test fun pathsAndOtherErrorDetailsDoNotBecomeCodeRefusals() {
        val key = "local_app_development_requires_code"
        val path = "/tmp/$key/file"
        assertFalse(isLocalAppCodeRequired("LocalApp", AgentToolStatus.Failed, "Path blocked: $path", null))
        assertFalse(isLocalAppCodeRequired("LocalApp", AgentToolStatus.Failed, "Failed", "Error: cannot read $path"))
        assertFalse(isLocalAppCodeRequired("LocalApp", AgentToolStatus.Failed, "Failed", "Other error\n$key: text in details"))
        assertFalse(isLocalAppCodeRequired("LocalApp", AgentToolStatus.Failed, "$key/file", null))
    }
}
