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

    @Test fun resolvesOriginInTheSelectedLanguage() {
        val strings = ConversationStrings { _, _, args -> if (args.isEmpty()) "From another session" else "From another session · ${args[0]}" }
        assertEquals("From another session", crossSessionMessage("<cross-session-message>hello</cross-session-message>", strings)?.label)
        assertEquals("From another session · Alice", crossSessionMessage("<cross-session-message from-name=\"Alice\">hello</cross-session-message>", strings)?.label)
    }
}
