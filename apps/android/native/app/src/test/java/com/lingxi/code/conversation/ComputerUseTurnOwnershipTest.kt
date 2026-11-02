package com.lingxi.code.conversation

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ComputerUseTurnOwnershipTest {
    @Test fun stopTargetsTheRunningToolOwnerAndIgnoresTheVisibleUnrelatedTurn() {
        assertTrue(computerUseBelongsToTurn("7:tool-code", 7, true))
        assertFalse(computerUseBelongsToTurn(null, 2, true))
        assertFalse(computerUseBelongsToTurn("1:old-tool", 2, true))
        assertFalse(computerUseBelongsToTurn("7:tool-code", 7, false))
    }
}
