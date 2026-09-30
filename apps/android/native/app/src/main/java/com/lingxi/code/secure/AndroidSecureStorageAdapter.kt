package com.lingxi.code.secure

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import com.lingxi.code.bindings.android.AndroidSecureStorage
import com.lingxi.code.bindings.android.SecureStorageFfiException
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Android Keystore-backed [AndroidSecureStorage] — the production secure store the
 * app hands to `build_android_engine` (via `VoiceController.buildVoiceEngine`).
 *
 * The Rust seam (`apps/android-aar`) bridges this onto `traits::SecureStorage`
 * (`AndroidSecureStorageBridge`). The engine's serialized `SecureStorageData`
 * arrives as an opaque [blob] keyed by `(service, account)`; we AES-256-GCM-seal
 * it with a NON-EXPORTABLE AndroidKeyStore key (held in the TEE/StrongBox, never
 * leaves hardware) and persist the sealed bytes under [baseDir]. Each entry is
 * `<baseDir>/<b64(service)>/<b64(account)>.bin = iv || AES-256-GCM(blob)`.
 *
 * Injecting this store flips the engine's `oauth_supported` true, so OAuth
 * `/login` persists its tokens instead of failing at the persist step. The
 * seal/unseal path mirrors the device-test `KeystoreGitCredentialProvider`.
 *
 * Mirrors the other `com.lingxi.code.*` adapters: it implements the generated
 * UniFFI callback interface and maps failures onto the flat `SecureStorageFfiException`
 * that Rust fans back out onto `traits::SecureStorageError`.
 */
class AndroidSecureStorageAdapter(private val baseDir: File) : AndroidSecureStorage {

    /** Convenience: root the store at `<filesDir>/secure-store` (the app-private dir). */
    constructor(context: Context) : this(File(context.applicationContext.filesDir, "secure-store"))

    override suspend fun store(service: String, account: String, blob: ByteArray) {
        try {
            val f = fileFor(service, account)
            f.parentFile?.mkdirs()
            f.writeBytes(seal(blob))
        } catch (e: SecureStorageFfiException) {
            throw e
        } catch (t: Throwable) {
            throw ioError("store", t)
        }
    }

    override suspend fun retrieve(service: String, account: String): ByteArray? {
        return try {
            val f = fileFor(service, account)
            if (!f.exists()) null else unseal(f.readBytes())
        } catch (e: SecureStorageFfiException) {
            throw e
        } catch (t: Throwable) {
            throw ioError("retrieve", t)
        }
    }

    override suspend fun delete(service: String, account: String) {
        try {
            // Removing a non-existent entry is not an error (matches the trait).
            fileFor(service, account).delete()
        } catch (t: Throwable) {
            throw ioError("delete", t)
        }
    }

    override suspend fun list(service: String): List<String> {
        return try {
            val dir = File(baseDir, enc(service))
            val files = dir.listFiles() ?: return emptyList()
            files.filter { it.isFile && it.name.endsWith(SUFFIX) }
                .map { dec(it.name.removeSuffix(SUFFIX)) }
        } catch (t: Throwable) {
            throw ioError("list", t)
        }
    }

    private fun fileFor(service: String, account: String): File =
        File(File(baseDir, enc(service)), enc(account) + SUFFIX)

    private fun seal(plain: ByteArray): ByteArray {
        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val iv = cipher.iv
        val ct = cipher.doFinal(plain)
        return iv + ct
    }

    private fun unseal(blob: ByteArray): ByteArray {
        val iv = blob.copyOfRange(0, IV_LEN)
        val ct = blob.copyOfRange(IV_LEN, blob.size)
        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(TAG_BITS, iv))
        return cipher.doFinal(ct)
    }

    private fun ioError(op: String, t: Throwable): SecureStorageFfiException =
        SecureStorageFfiException.Io("secure store $op failed: ${t.message ?: t.javaClass.simpleName}")

    private companion object {
        const val ALIAS = "lingxi_secure_store_key"
        const val TRANSFORM = "AES/GCM/NoPadding"
        const val IV_LEN = 12
        const val TAG_BITS = 128
        const val SUFFIX = ".bin"

        /** URL-safe, padding-free Base64 so `(service, account)` are valid path components. */
        fun enc(s: String): String =
            Base64.encodeToString(
                s.toByteArray(Charsets.UTF_8),
                Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING,
            )

        fun dec(s: String): String =
            String(
                Base64.decode(s, Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING),
                Charsets.UTF_8,
            )

        /** Fetch (or lazily create) the non-exportable AES-256-GCM key in the AndroidKeyStore. */
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
