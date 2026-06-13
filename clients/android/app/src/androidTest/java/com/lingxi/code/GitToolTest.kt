package com.lingxi.code

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.bindings.androidGitProbe
import org.json.JSONObject
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/**
 * P4d acceptance gate (android-git-p4 plan Task 11): the REAL
 * [tool_git_mobile::GitTool] path runs a HTTPS `clone` on-device, proving
 * libgit2's OpenSSL TLS + the Android system cacerts
 * (`/system/etc/security/cacerts`) verify a real public-repo clone — the one
 * thing the host `file://` tests (P4b/P4c) cannot exercise.
 *
 * The Rust side (`android_git_probe(operationJson, workspace, caCertDir)` in
 * `apps/android-aar`) builds a `BuiltinToolContext` with `android_git` enabled
 * (no token — the clone targets a PUBLIC repo) + `android_git_secret` pointing
 * `ca_dir` at the supplied system cacerts dir, constructs the `GitTool`, and
 * runs `GitTool::call(..)` on a transient runtime — NOT a bypass. It returns
 * the tool's result `data` JSON, or `{"error": "..."}` on failure.
 *
 * Like [SandboxRunTest], this MUST run on a device/emulator WITH NETWORK: a
 * host build of the library returns `{"error":"host build"}`, so a JVM-host
 * run fails loudly rather than silently passing. The emulator must be able to
 * reach `github.com` over HTTPS.
 */
@RunWith(AndroidJUnit4::class)
class GitToolTest {
    /** Tiny, canonical, reliably-public GitHub repo with a known `README` file. */
    private companion object {
        const val REPO_URL = "https://github.com/octocat/Hello-World.git"
        const val SYSTEM_CACERTS = "/system/etc/security/cacerts"
        const val CLONE_DIR = "cloned"
        const val KNOWN_FILE = "README"
    }

    private fun workspace(): File =
        InstrumentationRegistry.getInstrumentation().targetContext.filesDir

    /** Remove a prior clone so the test is repeatable on a warm device. */
    private fun freshWorkspaceDir(): String {
        val ws = workspace()
        File(ws, CLONE_DIR).deleteRecursively()
        return ws.absolutePath
    }

    @Test
    fun clonesPublicHttpsRepoOverTls() {
        val ws = freshWorkspaceDir()
        val op = JSONObject()
            .put("operation", "clone")
            .put("repo_url", REPO_URL)
            .put("repo", CLONE_DIR)
            .toString()

        val json = JSONObject(androidGitProbe(op, ws, SYSTEM_CACERTS))

        // A real HTTPS clone over OpenSSL + system cacerts must succeed: the
        // result carries the cloned HEAD oid, not an error.
        assertFalse(
            "clone must not error (TLS/cacerts/network): $json",
            json.has("error"),
        )
        assertTrue(
            "clone result must carry a head oid: $json",
            json.has("head") && json.getString("head").isNotEmpty(),
        )

        // The known file landed in the working tree — proves the clone checked
        // out, not just negotiated the transport.
        val known = File(File(ws, CLONE_DIR), KNOWN_FILE)
        assertTrue(
            "cloned working file '$KNOWN_FILE' must exist at ${known.absolutePath}",
            known.exists(),
        )
    }

    @Test
    fun localLogRoundTripsOnClonedRepo() {
        val ws = freshWorkspaceDir()
        val cloneOp = JSONObject()
            .put("operation", "clone")
            .put("repo_url", REPO_URL)
            .put("repo", CLONE_DIR)
            .toString()
        val cloneResult = JSONObject(androidGitProbe(cloneOp, ws, SYSTEM_CACERTS))
        assertFalse("clone must succeed before log round-trip: $cloneResult", cloneResult.has("error"))

        // A local `log` on the freshly-cloned repo proves the tool works
        // post-clone (the in-process libgit2 repo is valid + readable).
        val logOp = JSONObject()
            .put("operation", "log")
            .put("repo", CLONE_DIR)
            .put("max", 5)
            .toString()
        val logResult = JSONObject(androidGitProbe(logOp, ws, SYSTEM_CACERTS))
        assertFalse("log must not error: $logResult", logResult.has("error"))
        assertTrue(
            "log must return at least one commit: $logResult",
            logResult.getJSONArray("commits").length() > 0,
        )
    }
}
