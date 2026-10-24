package com.lingxi.code.conversation

import org.junit.Assert.*
import org.junit.Test

class MessageOriginTest {
    @Test fun projectsCompleteEnvelope() {
        val result = crossSessionMessage("""<cross-session-message from="session-a" from-name="设计 &amp; 开发">
第一行🙂
**正文**
</cross-session-message>""")!!
        assertEquals("来自其他会话 · 设计 & 开发", result.label)
        assertEquals("第一行🙂\n**正文**", result.text)
    }
    @Test fun leavesOrdinaryAndIncompleteTextAlone() {
        assertNull(crossSessionMessage("Example: <cross-session-message>hi</cross-session-message>"))
        assertNull(crossSessionMessage("<cross-session-message>hi"))
        assertNull(crossSessionMessage("<cross-session-messages>hi</cross-session-message>"))
    }
}
