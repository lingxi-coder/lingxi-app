package com.lingxi.code

import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.bindings.androidBundledShellProbe
import org.json.JSONObject
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/**
 * P5a — THE make-or-break gate. P5 ships *executables* (mksh, toybox) inside the
 * APK for the first time. Android 10+ forbids `execve` of app-writable files;
 * the only legal path is a file extracted into `nativeLibraryDir` — i.e. a
 * `lib*.so` packaged with `useLegacyPackaging=true`. This test proves (or
 * disproves) that on a real device/emulator.
 *
 * The Rust side ([android_bundled_shell_probe] in `apps/android-aar`) runs RAW
 * (un-jailed) `std::process::Command` execs — this gate is ONLY about W^X /
 * packaging; the minijail/deny-net jail is unchanged from P2/P3 and proven by
 * [SandboxRunTest]. It returns JSON:
 *
 *   {"mksh_exec_ok":bool,"applet_symlink_ok":bool,"applet_rewrite_ok":bool,"reason":"..."}
 *
 * - mksh_exec_ok      — `libmksh.so -c 'echo hi'` works (W^X execve gate).
 * - applet_symlink_ok — `<filesDir>/applet-bin/grep` symlink → libtoybox.so
 *                       greps via toybox argv[0] multicall dispatch (PREFERRED).
 * - applet_rewrite_ok — `libtoybox.so grep` greps (command-rewrite FALLBACK).
 *
 * Like [SandboxRunTest]/[GitToolTest], this MUST run on a device/emulator: a
 * host build returns `{"error":"host build"}`, so a JVM-host run fails loudly.
 *
 * The gate passes iff `mksh_exec_ok` AND (`applet_symlink_ok` ||
 * `applet_rewrite_ok`). The message records WHICH applet mechanism passed — the
 * decision that drives the P5b T7 symlink-farm vs command-rewrite bootstrap.
 */
@RunWith(AndroidJUnit4::class)
class BundledShellProbeTest {
    @Test
    fun wxExecAndAppletResolutionWork() {
        val ctx = InstrumentationRegistry.getInstrumentation().targetContext
        val nativeLibDir = ctx.applicationInfo.nativeLibraryDir
        val appletDir = File(ctx.filesDir, "applet-bin").absolutePath

        val raw = androidBundledShellProbe(nativeLibDir, appletDir)
        Log.i("P5aGate", "android_bundled_shell_probe => $raw")
        val json = JSONObject(raw)

        val mkshExecOk = json.optBoolean("mksh_exec_ok", false)
        val symlinkOk = json.optBoolean("applet_symlink_ok", false)
        val rewriteOk = json.optBoolean("applet_rewrite_ok", false)

        // The W^X execve of a bundled executable from nativeLibraryDir is the
        // whole point of P5a. If this is false on a real device, W^X blocks the
        // design — P5 cannot ship as-is. Fail LOUDLY with the probe reason.
        assertTrue(
            "P5a GATE FAILED: mksh execve from nativeLibraryDir did not work " +
                "(W^X may block bundled-executable exec) — $json",
            mkshExecOk,
        )

        // At least one applet-resolution mechanism must work.
        assertTrue(
            "P5a GATE FAILED: no toybox applet mechanism worked (neither symlink " +
                "farm nor command-rewrite) — $json",
            symlinkOk || rewriteOk,
        )

        // Record the winning mechanism (drives the P5b bootstrap decision).
        val mechanism = when {
            symlinkOk -> "symlink-farm (PREFERRED)"
            else -> "command-rewrite (fallback)"
        }
        Log.i("P5aGate", "applet mechanism PASSED: $mechanism")
    }
}
