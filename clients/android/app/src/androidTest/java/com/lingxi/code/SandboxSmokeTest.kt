package com.lingxi.code

import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.bindings.androidSandboxSmoke
import org.json.JSONObject
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * P0a global gate (android-sandbox plan Task 17): libminijail statically links
 * into `libandroid_aar.so`, `no_new_privs` applies, and a jailed
 * `/system/bin/sh -c true` forks + exits 0 on this device.
 *
 * The Rust side (`android_sandbox_smoke()` in `apps/android-aar`) runs
 * `platform_android_minijail::minijail_smoke()` and returns the `SmokeResult`
 * as JSON. Like [EngineRoundtripTest], this MUST run on a device/emulator —
 * a host build of the library reports `{"ok":false,"reason":"host build"}`,
 * so a JVM-host run fails loudly rather than silently passing.
 *
 * FAIL ⇒ STOP: per the plan, P2+ sandbox plans are blocked until this passes;
 * debug via `adb logcat` + the JSON `reason` carried in the assert message.
 */
@RunWith(AndroidJUnit4::class)
class SandboxSmokeTest {
    @Test
    fun minijailSmokePasses() {
        val json = JSONObject(androidSandboxSmoke())
        // Keys are serde's default snake_case (SmokeResult has no rename_all).
        assertTrue("smoke failed: $json", json.getBoolean("ok"))
        assertTrue("no_new_privs not applied: $json", json.getBoolean("no_new_privs"))
        assertTrue("jailed child did not exit 0: $json", json.getBoolean("child_exit_zero"))
    }
}
