package com.lingxi.code

import android.util.Log
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
 * libgit2's mbedTLS TLS + the Android system cacerts
 * (`/system/etc/security/cacerts`) verify a real public-repo clone — the one
 * thing the host `file://` tests (P4b/P4c) cannot exercise. The negative
 * counterpart [rejectsUntrustedTlsCertFailClosed] proves mbedTLS *refuses* an
 * untrusted cert (the fail-closed CA-verification half of the mbedTLS swap).
 *
 * The Rust side (`android_git_probe(operationJson, workspace, caCertDir)` in
 * `apps/android-aar`) builds a `BuiltinToolContext` with `android_git` enabled
 * (no token — the clone targets a PUBLIC repo) + `android_git_secret` pointing
 * `ca_dir` at the supplied system cacerts dir, constructs the `GitTool`, and
 * runs `GitTool::call(..)` on a transient runtime — NOT a bypass. It returns
 * the tool's result `data` JSON, or `{"error": "..."}` on failure.
 *
 * This MUST run on a device/emulator WITH NETWORK: a host build of the library
 * returns `{"error":"host build"}`, so a JVM-host run fails loudly rather than
 * silently passing.
 *
 * **Network portability:** the good-clone and bad-cert hosts default to
 * `github.com` / `badssl.com`, but can be overridden for networks where those
 * are unreachable (e.g. a clone host blocked by a national firewall) via
 * instrumentation args:
 * ```
 * adb shell am instrument -w \
 *   -e gitGoodRepoUrl https://gitee.com/oschina/git-osc.git \
 *   -e gitBadCertUrl  https://self-signed.badssl.com/any.git \
 *   com.lingxi.code.debug.test/androidx.test.runner.AndroidJUnitRunner
 * ```
 * Any reachable public repo with a valid publicly-trusted cert proves the same
 * mbedTLS good-cert code path; any reachable host with an untrusted cert proves
 * the fail-closed path.
 */
@RunWith(AndroidJUnit4::class)
class GitToolTest {
    private companion object {
        const val TAG = "GitToolTest"
        const val SYSTEM_CACERTS = "/system/etc/security/cacerts"
        const val CLONE_DIR = "cloned"
        const val BAD_CERT_DIR = "badcert"

        /** Default good-clone host (overridable via `-e gitGoodRepoUrl`). */
        const val DEFAULT_GOOD_REPO_URL = "https://github.com/octocat/Hello-World.git"

        /**
         * Default bad-cert host (overridable via `-e gitBadCertUrl`): serves a
         * deliberately-untrusted (self-signed) TLS cert. badssl.com is a
         * long-standing public TLS test fixture.
         */
        const val DEFAULT_BAD_CERT_URL = "https://self-signed.badssl.com/any.git"
    }

    /** Read an instrumentation arg, falling back to [default]. */
    private fun arg(key: String, default: String): String =
        InstrumentationRegistry.getArguments().getString(key) ?: default

    private fun workspace(): File =
        InstrumentationRegistry.getInstrumentation().targetContext.filesDir

    /** Remove a prior clone of [dir] so the test is repeatable on a warm device. */
    private fun freshClone(dir: String): String {
        val ws = workspace()
        File(ws, dir).deleteRecursively()
        return ws.absolutePath
    }

    /** Entries in [dir] other than `.git` — proves a working tree was checked out. */
    private fun checkedOutFiles(dir: File): List<String> =
        (dir.list()?.toList() ?: emptyList()).filter { it != ".git" }

    @Test
    fun clonesPublicHttpsRepoOverTls() {
        val repoUrl = arg("gitGoodRepoUrl", DEFAULT_GOOD_REPO_URL)
        val ws = freshClone(CLONE_DIR)
        val op = JSONObject()
            .put("operation", "clone")
            .put("repo_url", repoUrl)
            .put("repo", CLONE_DIR)
            .toString()

        val raw = androidGitProbe(op, ws, SYSTEM_CACERTS)
        Log.i(TAG, "clone($repoUrl) -> $raw")
        val json = JSONObject(raw)

        // A real HTTPS clone over mbedTLS + system cacerts must succeed: the
        // result carries the cloned HEAD oid, not an error.
        assertFalse(
            "clone must not error (TLS/cacerts/network): $json",
            json.has("error"),
        )
        assertTrue(
            "clone result must carry a head oid: $json",
            json.has("head") && json.getString("head").isNotEmpty(),
        )

        // The working tree was checked out — proves the clone materialized files,
        // not just negotiated the transport.
        val files = checkedOutFiles(File(ws, CLONE_DIR))
        assertTrue(
            "clone must check out a working tree (files beyond .git): $files",
            files.isNotEmpty(),
        )
    }

    @Test
    fun localLogRoundTripsOnClonedRepo() {
        val repoUrl = arg("gitGoodRepoUrl", DEFAULT_GOOD_REPO_URL)
        val ws = freshClone(CLONE_DIR)
        val cloneOp = JSONObject()
            .put("operation", "clone")
            .put("repo_url", repoUrl)
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

    /**
     * mbedTLS fail-closed: a clone against a host presenting an untrusted
     * (self-signed) certificate MUST be rejected at TLS verification — no head,
     * no checked-out working tree, and the error must name a certificate
     * rejection (NOT a reachability failure like DNS/timeout/EOF). Together with
     * [clonesPublicHttpsRepoOverTls] this proves mbedTLS verifies the system
     * cacerts and refuses unverified peers (HTTPS installs no certificate_check
     * override → hard failure).
     */
    @Test
    fun rejectsUntrustedTlsCertFailClosed() {
        val badUrl = arg("gitBadCertUrl", DEFAULT_BAD_CERT_URL)
        val ws = workspace()
        File(ws, BAD_CERT_DIR).deleteRecursively()
        val op = JSONObject()
            .put("operation", "clone")
            .put("repo_url", badUrl)
            .put("repo", BAD_CERT_DIR)
            .toString()

        val raw = androidGitProbe(op, ws.absolutePath, SYSTEM_CACERTS)
        Log.i(TAG, "badcert clone($badUrl) -> $raw")
        val json = JSONObject(raw)

        assertTrue(
            "untrusted-cert clone must fail closed (error expected): $json",
            json.has("error"),
        )
        assertFalse(
            "untrusted-cert clone must NOT produce a head oid: $json",
            json.has("head"),
        )
        val err = json.getString("error").lowercase()
        // Must fail for the RIGHT reason — a certificate rejection, not because
        // the host was simply unreachable (DNS/timeout/connection reset).
        assertFalse(
            "bad-cert failure must NOT be a mere reachability error: $json",
            err.contains("resolve address") || err.contains("no address") ||
                err.contains("hostname") || err.contains("timed out") ||
                err.contains("timeout") || err.contains("unexpected eof") ||
                err.contains("connection refused"),
        )
        // mbedTLS/libgit2 surfaces an untrusted cert as a certificate/verification
        // error. Match on cert-specific terms only (NOT bare "ssl", which would
        // spuriously match a "badssl.com" hostname inside a network error).
        assertTrue(
            "error must indicate a certificate/verification rejection: $json",
            err.contains("certificate") || err.contains("verif") ||
                err.contains("x509") || err.contains("self-signed") ||
                err.contains("self signed") || err.contains("untrusted"),
        )
        assertFalse(
            "no .git should be created for a rejected-cert clone: $json",
            File(File(ws, BAD_CERT_DIR), ".git").exists(),
        )
    }
}
