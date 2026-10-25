package com.lingxi.code.remote

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.PreferenceDataStoreFactory
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStoreFile
import com.lingxi.code.bindings.android.AndroidRemoteKeyStore
import com.lingxi.code.bindings.android.RemoteFfiException
import com.lingxi.code.bindings.android.RemotePinnedHostFfi
import java.security.GeneralSecurityException
import java.security.KeyStore
import java.util.Base64
import javax.crypto.AEADBadTagException
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.first
import org.json.JSONObject

/** Seals small secrets with a key the app cannot read back; [aad] ties a ciphertext to its slot. */
interface RemoteCipher {
    fun seal(plain: ByteArray, aad: ByteArray): ByteArray

    /** @throws GeneralSecurityException when the ciphertext does not authenticate or the key is gone. */
    fun open(sealed: ByteArray, aad: ByteArray): ByteArray
}

/**
 * [RemoteCipher] over a non-exportable AES-256-GCM key in the AndroidKeyStore, the
 * way [com.lingxi.code.secure.AndroidSecureStorageAdapter] seals the engine's
 * secrets, under its own alias so forgetting the remote identity never touches those.
 * A sealed value is `iv || ciphertext`.
 */
class KeystoreRemoteCipher(private val keyProvider: () -> SecretKey = { key() }) : RemoteCipher {
    override fun seal(plain: ByteArray, aad: ByteArray): ByteArray {
        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(Cipher.ENCRYPT_MODE, keyProvider())
        cipher.updateAAD(aad)
        return cipher.iv + cipher.doFinal(plain)
    }

    override fun open(sealed: ByteArray, aad: ByteArray): ByteArray {
        if (sealed.size <= IV_LEN) throw GeneralSecurityException("sealed value too short")
        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(
            Cipher.DECRYPT_MODE,
            keyProvider(),
            GCMParameterSpec(TAG_BITS, sealed, 0, IV_LEN),
        )
        cipher.updateAAD(aad)
        return cipher.doFinal(sealed, IV_LEN, sealed.size - IV_LEN)
    }

    private companion object {
        const val ALIAS = "lingxi_remote_control_key"
        const val TRANSFORM = "AES/GCM/NoPadding"
        const val IV_LEN = 12
        const val TAG_BITS = 128

        /** Fetch (or lazily create) the non-exportable key. */
        fun key(): SecretKey {
            val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            (keyStore.getEntry(ALIAS, null) as? KeyStore.SecretKeyEntry)?.let { return it.secretKey }
            val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
            generator.init(
                KeyGenParameterSpec.Builder(
                    ALIAS,
                    KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
                )
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .setKeySize(256)
                    .build(),
            )
            return generator.generateKey()
        }
    }
}

/**
 * The device key (PKCS#8) and the pinned host keys of Remote Control. Each value
 * is sealed by [cipher] and kept in a Preferences DataStore of its own
 * (`remote_control.preferences_pb`); Rust writes no files and the app keeps no
 * plaintext copy.
 *
 * A value that fails authentication or whose Keystore key was permanently
 * invalidated is gone for good, so it reads as absent and is dropped: the client
 * then makes a new identity and pairs again. Any other failure is reported.
 */
class DataStoreRemoteKeyStore(
    private val store: DataStore<Preferences>,
    private val cipher: RemoteCipher,
) : AndroidRemoteKeyStore {

    override suspend fun loadIdentity(): ByteArray? = read(IDENTITY, IDENTITY_AAD)

    override suspend fun storeIdentity(pkcs8: ByteArray) = write(IDENTITY, IDENTITY_AAD, pkcs8)

    override suspend fun deleteIdentity() = remove(IDENTITY)

    override suspend fun loadPinnedHost(hostId: String): RemotePinnedHostFfi? {
        val bytes = read(pinKey(hostId), pinAad(hostId)) ?: return null
        return try {
            val json = JSONObject(String(bytes, Charsets.UTF_8))
            RemotePinnedHostFfi(
                hostId = hostId,
                hostKey = json.getString("host_key"),
                name = json.getString("name"),
                pairedAtMs = json.getLong("paired_at_ms").toULong(),
            )
        } catch (_: Exception) {
            remove(pinKey(hostId))
            null
        }
    }

    override suspend fun storePinnedHost(host: RemotePinnedHostFfi) {
        val json = JSONObject()
            .put("host_key", host.hostKey)
            .put("name", host.name)
            .put("paired_at_ms", host.pairedAtMs.toLong())
        write(pinKey(host.hostId), pinAad(host.hostId), json.toString().toByteArray(Charsets.UTF_8))
    }

    override suspend fun deletePinnedHost(hostId: String) = remove(pinKey(hostId))

    private suspend fun read(key: String, aad: ByteArray): ByteArray? {
        val encoded = try {
            store.data.first()[stringPreferencesKey(key)]
        } catch (error: Exception) {
            throw keyStoreError("read", error)
        } ?: return null
        return try {
            cipher.open(Base64.getDecoder().decode(encoded), aad)
        } catch (_: AEADBadTagException) {
            remove(key)
            null
        } catch (_: KeyPermanentlyInvalidatedException) {
            remove(key)
            null
        } catch (_: IllegalArgumentException) {
            remove(key)
            null
        } catch (error: GeneralSecurityException) {
            // A Keystore that is briefly unavailable must not cost the identity.
            throw keyStoreError("open", error)
        }
    }

    private suspend fun write(key: String, aad: ByteArray, plain: ByteArray) {
        try {
            val sealed = Base64.getEncoder().encodeToString(cipher.seal(plain, aad))
            store.edit { it[stringPreferencesKey(key)] = sealed }
        } catch (error: Exception) {
            throw keyStoreError("write", error)
        }
    }

    private suspend fun remove(key: String) {
        try {
            store.edit { it.remove(stringPreferencesKey(key)) }
        } catch (error: Exception) {
            throw keyStoreError("delete", error)
        }
    }

    private fun keyStoreError(operation: String, error: Exception) =
        RemoteFfiException.KeyStore("remote key store $operation failed: ${error.message ?: error.javaClass.simpleName}")

    companion object {
        private const val IDENTITY = "identity"
        private const val PIN_PREFIX = "pin."
        private val IDENTITY_AAD = "lingxi-rc/identity".toByteArray()

        private fun pinKey(hostId: String) = PIN_PREFIX + hostId
        private fun pinAad(hostId: String) = "lingxi-rc/pin/$hostId".toByteArray()

        /** The production store: the Keystore cipher over `remote_control.preferences_pb`. */
        fun create(context: Context, scope: CoroutineScope): DataStoreRemoteKeyStore =
            DataStoreRemoteKeyStore(
                PreferenceDataStoreFactory.create(scope = scope) {
                    context.applicationContext.preferencesDataStoreFile("remote_control")
                },
                KeystoreRemoteCipher(),
            )
    }
}
