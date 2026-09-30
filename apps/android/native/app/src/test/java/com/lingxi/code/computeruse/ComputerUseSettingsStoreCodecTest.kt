package com.lingxi.code.computeruse

import org.junit.Assert.assertEquals
import org.junit.Test

class ComputerUseSettingsStoreCodecTest {
    @Test
    fun appSelectionsRoundTripWithoutBecomingSessionGrants() {
        val selections = linkedMapOf(
            "com.android.chrome" to ComputerUseTier.Full,
            "com.example.reader" to ComputerUseTier.Read,
        )

        assertEquals(
            selections,
            decodeComputerUseAppSelections(encodeComputerUseAppSelections(selections)),
        )
    }

    @Test
    fun malformedOrUnknownSelectionsAreIgnored() {
        assertEquals(
            mapOf("com.android.chrome" to ComputerUseTier.Click),
            decodeComputerUseAppSelections(
                setOf(
                    "com.android.chrome|Click",
                    "missing-tier",
                    "com.example.bad|Admin",
                    "|Read",
                ),
            ),
        )
    }
}
