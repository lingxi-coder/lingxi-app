package com.lingxi.code.conversation
import org.junit.Assert.*
import org.junit.Test
class ToolDenialPresentationTest {
    @Test fun preservesReasonAndOnlyLabelsAutoReviewWhenIdentified() {
        val reason = "Automatic approval review failed: Bad Request"
        assertEquals(ToolDenialPresentation(reason, true), toolDenialPresentation(true, "Failed", reason))
        assertEquals(false, toolDenialPresentation(true, "Permission to use Bash has been denied.", null)?.autoReview)
        assertEquals("Permission denied", toolDenialPresentation(true, "Permission denied", " ")?.reason)
    }
    @Test fun commandOutputAndSuccessfulQuotesAreNotApprovalRefusals() {
        assertNull(toolDenialPresentation(false, "Permission denied", null))
        assertNull(toolDenialPresentation(true, "Failed", "bash: ./private: Permission denied"))
        assertNull(toolDenialPresentation(true, "Process exited with code 1", null))
    }
}
