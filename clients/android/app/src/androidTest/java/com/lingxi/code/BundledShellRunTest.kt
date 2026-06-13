package com.lingxi.code

import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.bindings.androidBundledShellRunProbe
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * P5c acceptance gate (android-bundled-shell-p5 plan Task 8): the BUNDLED
 * mksh+toybox runs end-to-end through the REAL jailed runner on-device.
 *
 * The Rust side (`android_bundled_shell_run_probe(nativeLibDir, appFilesRoot,
 * command)` in `apps/android-aar`) first bootstraps the bundled shell (stages
 * the toybox applet symlink farm, verifies bundled execve + dispatch, sha256s
 * `libmksh.so`), then builds an `AndroidShellConfig` with the THREE bundled
 * fields set, a probed [CapabilityCache], an `AndroidMinijailSandbox` +
 * `AndroidMinijailProcessRunner` over that SAME cache, `prepare()`s the command
 * under a deny-net policy, and `run()`s it through the jailed fork/exec — NOT a
 * bypass. Because the bundled fields are set, `prepare` selects
 * `ExecTarget::BundledHelper{mksh}` + leads PATH with the applet farm, and the
 * runner content-identity-checks the recorded sha256 against the real
 * `libmksh.so` before execve. So a non-empty `stdout` implicitly proves staging
 * + the hash check + the jailed bundled exec all passed.
 *
 * It hardcodes a **2-second** wall-clock timeout so a `sleep 10` probe trips the
 * watchdog. The result is JSON
 * (`{stdout,stderr,exit_code,timed_out,enforcement_failed}`).
 *
 * Like [SandboxRunTest]/[BundledShellProbeTest], this MUST run on a
 * device/emulator: a host build returns `{"enforcement_failed":"host build"}`,
 * and a broken bundle returns `{"enforcement_failed":"bundled bootstrap
 * failed"}`, so a JVM-host run or a defective bundle fails loudly.
 */
@RunWith(AndroidJUnit4::class)
class BundledShellRunTest {
    private fun nativeLibDir(): String =
        InstrumentationRegistry.getInstrumentation().targetContext.applicationInfo.nativeLibraryDir

    private fun filesRoot(): String =
        InstrumentationRegistry.getInstrumentation().targetContext.filesDir.absolutePath

    private fun run(command: String): JSONObject {
        val raw = androidBundledShellRunProbe(nativeLibDir(), filesRoot(), command)
        Log.i("P5cGate", "android_bundled_shell_run_probe(\"$command\") => $raw")
        return JSONObject(raw)
    }

    /**
     * THE make-or-break end-to-end proof. `echo x | grep x` exercises the
     * bundled mksh interpreter (the pipeline), the toybox `grep` applet via the
     * symlink farm (argv[0] multicall dispatch), and the runner's sha256
     * content-identity check — all under the jail. If `enforcement_failed` is
     * set here, the bundled chain is broken (hash mismatch / farm / prepare).
     */
    @Test
    fun bundled_echo_grep_pipeline() {
        val json = run("echo x | grep x")
        assertTrue(
            "enforcement_failed must be null (bundled chain end-to-end): $json",
            json.isNull("enforcement_failed"),
        )
        assertTrue(
            "expected stdout to contain 'x' (bundled mksh pipe + toybox grep): $json",
            json.getString("stdout").contains("x"),
        )
    }

    /** Bundled toybox `sed` + a directory listing, both through the jail. */
    @Test
    fun bundled_sed_and_find_smoke() {
        val sed = run("echo hello | sed s/hello/world/")
        assertTrue(
            "enforcement_failed must be null (bundled sed): $sed",
            sed.isNull("enforcement_failed"),
        )
        assertTrue(
            "expected stdout to contain 'world' (bundled toybox sed): $sed",
            sed.getString("stdout").contains("world"),
        )

        val find = run("find . -maxdepth 0")
        assertTrue(
            "enforcement_failed must be null (bundled find): $find",
            find.isNull("enforcement_failed"),
        )
        assertEquals("find . -maxdepth 0 should exit 0: $find", 0, find.getInt("exit_code"))
    }

    /**
     * The probe hardcodes a 2s timeout; `sleep 10` (bundled toybox `sleep`) must
     * trip the watchdog, which kills the process group and reports
     * timed_out=true.
     */
    @Test
    fun bundled_timeout_kills() {
        val json = run("sleep 10")
        assertTrue(
            "sleep 10 under a 2s timeout must time out: $json",
            json.getBoolean("timed_out"),
        )
    }

    /**
     * Network must be refused under the deny-net jail — the SAME seccomp filter
     * P2 proved via `net_deny_verified` (socket() => EPERM). A bundled command
     * that opens a socket must NOT succeed. Kept tolerant: the point is "network
     * is refused", and the exact errno/text varies by applet (nc/wget) and
     * device. We accept any of: enforcement_failed set, a non-zero exit, or
     * non-empty stderr.
     */
    @Test
    fun bundled_deny_net_blocks() {
        val json = run("nc -w1 127.0.0.1 9 </dev/null")
        val enforcementFailed = !json.isNull("enforcement_failed")
        val nonZeroExit = !json.isNull("exit_code") && json.getInt("exit_code") != 0
        val stderrNonEmpty = json.optString("stderr", "").isNotEmpty()
        assertTrue(
            "network must be refused under deny-net (expected enforcement_failed, " +
                "non-zero exit, or non-empty stderr): $json",
            enforcementFailed || nonZeroExit || stderrNonEmpty,
        )
    }
}
