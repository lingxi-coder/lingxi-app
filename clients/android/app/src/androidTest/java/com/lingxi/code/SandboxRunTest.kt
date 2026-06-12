package com.lingxi.code

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.bindings.androidSandboxCapabilities
import com.lingxi.code.bindings.androidSandboxRunProbe
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * P2 acceptance gate (android-sandbox-p2 plan Task 8): the REAL
 * prepare→runner→`run_jailed` path executes on-device.
 *
 * The Rust side (`android_sandbox_run_probe(command, workspace)` in
 * `apps/android-aar`) builds a probed [CapabilityCache], an
 * `AndroidMinijailSandbox` + `AndroidMinijailProcessRunner` over that SAME
 * cache, `prepare()`s the command under a deny-net policy, and `run()`s it —
 * NOT a bypass. It hardcodes a **2-second** wall-clock timeout so a `sleep 10`
 * probe reliably trips the watchdog. The result is returned as JSON
 * (`{stdout,stderr,exit_code,timed_out,enforcement_failed}`).
 *
 * Like [SandboxSmokeTest], this MUST run on a device/emulator: a host build of
 * the library returns `{"enforcement_failed":"host build"}`, so a JVM-host run
 * fails loudly rather than silently passing.
 */
@RunWith(AndroidJUnit4::class)
class SandboxRunTest {
    private fun workspace(): String =
        InstrumentationRegistry.getInstrumentation().targetContext.filesDir.absolutePath

    @Test
    fun echoRunsAndSucceeds() {
        val json = JSONObject(androidSandboxRunProbe("echo hello", workspace()))
        assertTrue(
            "expected stdout to contain 'hello': $json",
            json.getString("stdout").contains("hello"),
        )
        assertEquals("exit_code should be 0: $json", 0, json.getInt("exit_code"))
        assertFalse("should not time out: $json", json.getBoolean("timed_out"))
        // enforcement_failed must be JSON null on success.
        assertTrue("enforcement_failed should be null: $json", json.isNull("enforcement_failed"))
    }

    @Test
    fun falseCommandExitsNonZero() {
        val json = JSONObject(androidSandboxRunProbe("false", workspace()))
        assertTrue("enforcement_failed should be null: $json", json.isNull("enforcement_failed"))
        assertEquals("`false` should exit 1: $json", 1, json.getInt("exit_code"))
    }

    @Test
    fun longSleepTimesOut() {
        // The probe hardcodes a 2s timeout; `sleep 10` must trip the watchdog,
        // which kills the process group and reports timed_out=true.
        val json = JSONObject(androidSandboxRunProbe("sleep 10", workspace()))
        assertTrue("enforcement_failed should be null: $json", json.isNull("enforcement_failed"))
        assertTrue("sleep 10 under a 2s timeout must time out: $json", json.getBoolean("timed_out"))
    }

    @Test
    fun netDenyVerifiedByProbe() {
        // The strong proof of net-deny enforcement: the capability probe forked a
        // child under the net-deny seccomp filter and observed socket() => EPERM.
        val json = JSONObject(androidSandboxCapabilities())
        assertTrue("probe must have run on-device: $json", json.getBoolean("probed"))
        assertTrue(
            "net_deny_verified must be true (socket() => EPERM under the filter): $json",
            json.getBoolean("net_deny_verified"),
        )
    }
}
