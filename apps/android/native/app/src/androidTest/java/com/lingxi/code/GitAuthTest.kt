package com.lingxi.code

import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.bindings.androidGitProbeAuthed
import org.json.JSONObject
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/**
 * Authenticated Git device-acceptance (G2 push, G7 SSH, per-op credential
 * provider). Drives the REAL [tool_git_mobile::GitTool] via
 * `androidGitProbeAuthed`, supplying secrets through a Keystore-backed
 * [KeystoreGitCredentialProvider] — so each network op fetches the
 * token/passphrase via a genuine per-op Android Keystore round-trip.
 *
 * **SSH key type:** the engine's libssh2 (mbedTLS backend) now supports
 * **ed25519** (vendored ref10), RSA, and ECDSA — `LIBSSH2_ED25519 1` enables
 * `ssh-ed25519` userauth + host keys + the `curve25519-sha256` KEX. So
 * `-e sshKeyB64` may be any of those key types. (Historically the mbedTLS
 * size-opt swap had dropped ed25519; restored here.)
 *
 * These need live credentials + a writable remote, so each test is **guarded by
 * instrumentation args** and SKIPS (not fails) when they are absent. Run with:
 * ```
 * adb -s <serial> shell am instrument -w \
 *   -e gitToken "$(cat .gittoken)" \
 *   -e gitHttpsRepoUrl https://gitee.com/<you>/<test-repo>.git \
 *   -e gitSshUrl git@gitee.com:<you>/<test-repo>.git \
 *   -e sshKeyPath /data/user/0/com.lingxi.code.debug/files/test_ed25519 \
 *   -e sshPubKeyPath /data/user/0/com.lingxi.code.debug/files/test_ed25519.pub \
 *   -e class com.lingxi.code.GitAuthTest \
 *   com.lingxi.code.debug.test/androidx.test.runner.AndroidJUnitRunner
 * ```
 */
@RunWith(AndroidJUnit4::class)
class GitAuthTest {
    private companion object {
        const val TAG = "GitAuthTest"
        const val SYSTEM_CACERTS = "/system/etc/security/cacerts"
        /** Passphrase of the generated test key (a throwaway test secret, not the user's). */
        const val DEFAULT_SSH_PASSPHRASE = "lingxi-dev-test-passphrase"

        /**
         * gitee.com host-key SHA-256 (raw-blob hex, matching libgit2's
         * `CertHostkey::hash_sha256`). All three offered types are pinned so
         * whichever libssh2 negotiates verifies; cross-checkable against gitee's
         * published openssh `SHA256:` fingerprints.
         */
        val GITEE_HOSTKEYS_SHA256_HEX = listOf(
            "f942f38a3daef7d07d7966054f0d50e04ad81bf69ea472dbbbde8f0140a857cf", // ed25519
            "99e4ec5520a039ab3c7c19c9cb1f043e5509afa890f7aae21660853df5240ed5", // rsa
            "150182f4a9ff7b27b55bc89c74182b429f8a9066281606d5af5edb9a37b2d167", // ecdsa
        )
    }

    private fun arg(key: String): String? =
        InstrumentationRegistry.getArguments().getString(key)

    /**
     * Pinned host-key SHA-256 hex set: `-e sshHostKeysHex "hex1,hex2,…"` overrides
     * the gitee default (e.g. for a local gitea server). Empty entries dropped.
     */
    private fun pinnedHostKeys(): List<String> =
        arg("sshHostKeysHex")
            ?.split(",")
            ?.map { it.trim() }
            ?.filter { it.isNotEmpty() }
            ?: GITEE_HOSTKEYS_SHA256_HEX

    private fun workspace(): File =
        InstrumentationRegistry.getInstrumentation().targetContext.filesDir

    /**
     * Materialize the SSH private (and optional public) key from base64
     * instrumentation args into the app's own filesDir — written by the test
     * process itself (app uid), avoiding run-as/SELinux limits on pushing files
     * into app-private storage. Returns the private-key path, or null if no key
     * arg was supplied (→ the SSH tests skip).
     */
    private fun materializeSshKey(): String? {
        val b64 = arg("sshKeyB64") ?: return null
        val keyFile = File(workspace(), "test_ed25519")
        keyFile.writeBytes(android.util.Base64.decode(b64, android.util.Base64.DEFAULT))
        keyFile.setReadable(false, false); keyFile.setReadable(true, true)
        keyFile.setWritable(false, false); keyFile.setWritable(true, true)
        arg("sshPubKeyB64")?.let {
            File(workspace(), "test_ed25519.pub")
                .writeBytes(android.util.Base64.decode(it, android.util.Base64.DEFAULT))
        }
        return keyFile.absolutePath
    }

    private fun pubKeyPath(): String? =
        File(workspace(), "test_ed25519.pub").takeIf { it.exists() }?.absolutePath

    private fun freshClone(dir: String): String {
        val ws = workspace()
        File(ws, dir).deleteRecursively()
        return ws.absolutePath
    }

    private fun op(operation: String, vararg kv: Pair<String, Any>): String {
        val o = JSONObject().put("operation", operation)
        kv.forEach { (k, v) -> o.put(k, v) }
        return o.toString()
    }

    private fun probe(
        provider: KeystoreGitCredentialProvider?,
        opJson: String,
        ws: String,
        keyPath: String? = null,
        pubPath: String? = null,
        hostKeys: List<String> = emptyList(),
    ): JSONObject {
        val raw = androidGitProbeAuthed(opJson, ws, SYSTEM_CACERTS, provider, keyPath, pubPath, hostKeys)
        Log.i(TAG, "$opJson -> $raw")
        return JSONObject(raw)
    }

    private fun assertNoError(stage: String, json: JSONObject) =
        assertFalse("$stage must not error: $json", json.has("error"))

    /** clone → unique branch → commit → push the new branch; returns the push result. */
    private fun cloneCommitPush(
        provider: KeystoreGitCredentialProvider,
        repoUrl: String,
        dir: String,
        keyPath: String?,
        pubPath: String?,
        hostKeys: List<String>,
    ): JSONObject {
        val ws = freshClone(dir)
        assertNoError("clone", probe(provider, op("clone", "repo_url" to repoUrl, "repo" to dir), ws, keyPath, pubPath, hostKeys))

        val branch = "device-test-${System.currentTimeMillis()}"
        assertNoError("branch_create", probe(provider, op("branch_create", "repo" to dir, "new_branch" to branch), ws, keyPath, pubPath, hostKeys))
        assertNoError("checkout", probe(provider, op("checkout", "repo" to dir, "branch" to branch), ws, keyPath, pubPath, hostKeys))

        File(File(workspace(), dir), "device-$branch.txt")
            .writeText("LingXi device-acceptance push on branch $branch\n")
        assertNoError("add", probe(provider, op("add", "repo" to dir), ws, keyPath, pubPath, hostKeys))
        assertNoError("commit", probe(provider, op("commit", "repo" to dir, "message" to "device push $branch"), ws, keyPath, pubPath, hostKeys))

        val push = probe(provider, op("push", "repo" to dir, "remote" to "origin", "branch" to branch), ws, keyPath, pubPath, hostKeys)
        assertNoError("push", push)
        return push
    }

    /** G2 + per-op credential provider over HTTPS: push authenticates via the Keystore token. */
    @Test
    fun httpsPushAuthenticatesViaKeystoreProvider() {
        val token = arg("gitToken")
        val repoUrl = arg("gitHttpsRepoUrl")
        assumeTrue("requires -e gitToken and -e gitHttpsRepoUrl", token != null && repoUrl != null)

        val provider = KeystoreGitCredentialProvider(token, null)
        val push = cloneCommitPush(provider, repoUrl!!, "authpush", null, null, emptyList())

        assertTrue("push must report the pushed tip oid: $push", push.has("pushed_oid") && push.getString("pushed_oid").isNotEmpty())
        assertTrue("first push of a new branch must set upstream: $push", push.optBoolean("set_upstream", false))
        assertTrue(
            "httpsToken() must be fetched per-op via the Keystore (count=${provider.tokenCalls.get()})",
            provider.tokenCalls.get() >= 1,
        )
    }

    /** G7 + per-op provider over SSH: clone+push with strict host-key pinning + key passphrase. */
    @Test
    fun sshCloneAndPushWithHostKeyPinning() {
        val sshUrl = arg("gitSshUrl")
        val keyPath = materializeSshKey()
        assumeTrue("requires -e gitSshUrl and -e sshKeyB64", sshUrl != null && keyPath != null)
        val pubPath = pubKeyPath()
        val passphrase = arg("sshPassphrase") ?: DEFAULT_SSH_PASSPHRASE

        val provider = KeystoreGitCredentialProvider(null, passphrase)
        val push = cloneCommitPush(provider, sshUrl!!, "authssh", keyPath, pubPath, pinnedHostKeys())

        assertTrue("ssh push must report the pushed tip oid: $push", push.has("pushed_oid") && push.getString("pushed_oid").isNotEmpty())
        assertTrue(
            "sshPassphrase() must be fetched per-op via the Keystore (count=${provider.passphraseCalls.get()})",
            provider.passphraseCalls.get() >= 1,
        )
    }

    /** G7 strict host-key verification: an SSH op against an UNPINNED host key must fail closed. */
    @Test
    fun sshFailsClosedOnUnpinnedHostKey() {
        val sshUrl = arg("gitSshUrl")
        val keyPath = materializeSshKey()
        assumeTrue("requires -e gitSshUrl and -e sshKeyB64", sshUrl != null && keyPath != null)
        val pubPath = pubKeyPath()

        val provider = KeystoreGitCredentialProvider(null, arg("sshPassphrase") ?: DEFAULT_SSH_PASSPHRASE)
        val ws = freshClone("authsshbad")
        // A bogus pinned set (all zeros) — the real host key cannot match, so
        // certificate_check must reject and the clone must fail closed.
        val bogus = listOf("0".repeat(64))
        val json = probe(provider, op("clone", "repo_url" to sshUrl!!, "repo" to "authsshbad"), ws, keyPath, pubPath, bogus)

        assertTrue("unpinned host key must fail closed (error expected): $json", json.has("error"))
        assertFalse("no .git should be created on host-key rejection: $json", File(File(ws, "authsshbad"), ".git").exists())
    }
}
