package com.lingxi.code.voice

import com.lingxi.code.bindings.AndroidDeviceClassFfi
import com.lingxi.code.bindings.AndroidExecutionTargetFfi
import org.junit.Assert.assertEquals
import org.junit.Test

class AndroidHostEnvironmentTest {

    @Test
    fun `device class uses the stable smallest width breakpoint`() {
        assertEquals(AndroidDeviceClassFfi.UNKNOWN, androidDeviceClass(0))
        assertEquals(AndroidDeviceClassFfi.PHONE, androidDeviceClass(599))
        assertEquals(AndroidDeviceClassFfi.TABLET, androidDeviceClass(600))
    }

    @Test
    fun `execution target detects common emulator fingerprints`() {
        assertEquals(
            AndroidExecutionTargetFfi.EMULATOR,
            androidExecutionTarget(
                fingerprint = "google/sdk_gphone64_arm64/emu64a:16/test-keys",
                model = "sdk_gphone64_arm64",
                manufacturer = "Google",
                brand = "google",
                device = "emu64a",
                product = "sdk_gphone64_arm64",
                hardware = "ranchu",
            ),
        )
    }

    @Test
    fun `execution target treats populated non emulator facts as physical`() {
        assertEquals(
            AndroidExecutionTargetFfi.PHYSICAL_DEVICE,
            androidExecutionTarget(
                fingerprint = "google/husky/husky:16/release-keys",
                model = "Pixel 8 Pro",
                manufacturer = "Google",
                brand = "google",
                device = "husky",
                product = "husky",
                hardware = "tensor",
            ),
        )
    }

    @Test
    fun `execution target falls back to unknown without usable build facts`() {
        assertEquals(
            AndroidExecutionTargetFfi.UNKNOWN,
            androidExecutionTarget("", "", "", "", "", "", ""),
        )
        assertEquals(
            AndroidExecutionTargetFfi.UNKNOWN,
            androidExecutionTarget(
                "unknown", "unknown", "unknown", "unknown", "unknown", "unknown", "unknown",
            ),
        )
    }

    @Test
    fun `host version combines release and API without provider state`() {
        assertEquals("16 (API 36)", androidHostOsVersion("16", 36))
        assertEquals("API 36", androidHostOsVersion("", 36))
        assertEquals("16", androidHostOsVersion("16", 0))
        assertEquals(null, androidHostOsVersion("", 0))
    }
}
