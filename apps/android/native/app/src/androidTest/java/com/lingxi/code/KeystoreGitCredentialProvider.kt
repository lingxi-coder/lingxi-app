package com.lingxi.code

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import com.lingxi.code.bindings.android.AndroidGitCredentialProvider
import java.security.KeyStore
import java.util.concurrent.atomic.AtomicInteger
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * A host [AndroidGitCredentialProvider] whose secrets are sealed in the Android
 * Keystore (AES-256-GCM, non-exportable key in the TEE) and **decrypted per
 * call**. Each `httpsToken()` / `sshPassphrase()` the engine makes inside
 * libgit2's credentials callback is therefore a genuine Keystore round-trip —
 * no plaintext secret is held resident between ops, which is exactly the
 * property the per-op credential-provider FFI exists to deliver.
 *
 * The call counters let the device test assert the per-op fetch actually
 * happened during a real network op (clone / push).
 *
 * This is a TEST provider (androidTest source set): the token/passphrase are
 * seeded once from instrumentation args; a production provider would seed from
 * a secure enrollment flow, but the seal/unseal path is identical.
 */
class KeystoreGitCredentialProvider(
    token: String?,
    passphrase: String?,
) : AndroidGitCredentialProvider {
    val tokenCalls = AtomicInteger(0)
    val passphraseCalls = AtomicInteger(0)

    private val tokenBlob: ByteArray? = token?.let { seal(it) }
    private val passBlob: ByteArray? = passphrase?.let { seal(it) }

    override fun httpsToken(): String? {
        tokenCalls.incrementAndGet()
        return tokenBlob?.let { unseal(it) }
    }

    override fun sshPassphrase(): String? {
        passphraseCalls.incrementAndGet()
        return passBlob?.let { unseal(it) }
    }

    private fun seal(plain: String): ByteArray {
        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val iv = cipher.iv
        val ct = cipher.doFinal(plain.toByteArray(Charsets.UTF_8))
        return iv + ct
    }

    private fun unseal(blob: ByteArray): String {
        val iv = blob.copyOfRange(0, IV_LEN)
        val ct = blob.copyOfRange(IV_LEN, blob.size)
        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(TAG_BITS, iv))
        return String(cipher.doFinal(ct), Charsets.UTF_8)
    }

    private companion object {
        const val ALIAS = "lingxi_git_device_test_key"
        const val TRANSFORM = "AES/GCM/NoPadding"
        const val IV_LEN = 12
        const val TAG_BITS = 128

        /** Fetch (or lazily create) the non-exportable AES key in the AndroidKeyStore. */
        fun key(): SecretKey {
            val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            (ks.getEntry(ALIAS, null) as? KeyStore.SecretKeyEntry)?.let { return it.secretKey }
            val kg = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
            kg.init(
                KeyGenParameterSpec.Builder(
                    ALIAS,
                    KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
                )
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .setKeySize(256)
                    .build(),
            )
            return kg.generateKey()
        }
    }
}
