package com.lingxi.code.remote

import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.PreferenceDataStoreFactory
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import com.lingxi.code.bindings.android.RemotePinnedHostFfi
import java.io.File
import com.lingxi.code.bindings.android.RemoteFfiException
import java.security.KeyStoreException
import javax.crypto.AEADBadTagException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class RemoteKeyStoreTest {
    /** XOR with a key byte plus a tag over the AAD: not secure, but fails like GCM when the AAD or key changes. */
    private class FakeCipher(private val keyByte: Byte = 0x5a) : RemoteCipher {
        override fun seal(plain: ByteArray, aad: ByteArray): ByteArray =
            byteArrayOf(tag(aad)) + plain.map { (it.toInt() xor keyByte.toInt()).toByte() }.toByteArray()

        override fun open(sealed: ByteArray, aad: ByteArray): ByteArray {
            if (sealed.isEmpty() || sealed[0] != tag(aad)) throw AEADBadTagException("tag mismatch")
            return sealed.drop(1).map { (it.toInt() xor keyByte.toInt()).toByte() }.toByteArray()
        }

        private fun tag(aad: ByteArray): Byte = (aad.sumOf { it.toInt() } xor keyByte.toInt()).toByte()
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val dir = File.createTempFile("remote-keys", "").let { it.delete(); it.mkdirs(); it }
    private val file = File(dir, "remote_control.preferences_pb")
    private val dataStore: DataStore<Preferences> = PreferenceDataStoreFactory.create(scope = scope) { file }

    @After fun cleanUp() {
        scope.cancel()
        dir.deleteRecursively()
    }

    private fun store(cipher: RemoteCipher = FakeCipher()) = DataStoreRemoteKeyStore(dataStore, cipher)

    private val pkcs8 = ByteArray(48) { it.toByte() }

    @Test fun identityRoundTripsAndNeverSitsInPlaintext() = runBlocking {
        val keys = store()
        assertNull(keys.loadIdentity())
        keys.storeIdentity(pkcs8)
        assertArrayEquals(pkcs8, keys.loadIdentity())
        val raw = dataStore.data.first().asMap().values.single() as String
        assertFalse(raw.contains(java.util.Base64.getEncoder().encodeToString(pkcs8)))
        keys.deleteIdentity()
        assertNull(keys.loadIdentity())
    }

    @Test fun pinnedHostsRoundTripPerHost() = runBlocking {
        val keys = store()
        val a = RemotePinnedHostFfi("host-a", "key-a", "Mac A", 1000uL)
        val b = RemotePinnedHostFfi("host-b", "key-b", "Mac “B”", 2000uL)
        keys.storePinnedHost(a)
        keys.storePinnedHost(b)
        assertEquals(a, keys.loadPinnedHost("host-a"))
        assertEquals(b, keys.loadPinnedHost("host-b"))
        assertNull(keys.loadPinnedHost("host-c"))
        keys.deletePinnedHost("host-a")
        assertNull(keys.loadPinnedHost("host-a"))
        assertNotNull(keys.loadPinnedHost("host-b"))
    }

    @Test fun aSurvivingStoreReadsWhatAnEarlierOneWrote() = runBlocking {
        store().storeIdentity(pkcs8)
        assertArrayEquals(pkcs8, store().loadIdentity())
    }

    @Test fun aValueSealedByAnotherKeyReadsAsAbsentAndIsDropped() = runBlocking {
        store(FakeCipher(0x11)).storeIdentity(pkcs8)
        val keys = store(FakeCipher(0x22))
        assertNull(keys.loadIdentity())
        assertTrue(dataStore.data.first().asMap().isEmpty())
    }

    @Test fun aCiphertextMovedToAnotherSlotDoesNotOpen() = runBlocking {
        val keys = store()
        keys.storePinnedHost(RemotePinnedHostFfi("host-a", "key-a", "Mac A", 1uL))
        val sealed = dataStore.data.first()[stringPreferencesKey("pin.host-a")]!!
        dataStore.edit { it[stringPreferencesKey("pin.host-b")] = sealed }
        assertNull(keys.loadPinnedHost("host-b"))
        assertNotNull(keys.loadPinnedHost("host-a"))
    }

    @Test fun garbageInTheFileReadsAsAbsent() = runBlocking {
        dataStore.edit { it[stringPreferencesKey("identity")] = "not base64 !!" }
        assertNull(store().loadIdentity())
        assertTrue(dataStore.data.first().asMap().isEmpty())
    }

    @Test fun anUnavailableKeystoreIsReportedAndKeepsTheIdentity() = runBlocking {
        store().storeIdentity(pkcs8)
        val unavailable = object : RemoteCipher {
            override fun seal(plain: ByteArray, aad: ByteArray): ByteArray = throw KeyStoreException("busy")
            override fun open(sealed: ByteArray, aad: ByteArray): ByteArray = throw KeyStoreException("busy")
        }
        try {
            store(unavailable).loadIdentity()
            fail("expected a key store error")
        } catch (_: RemoteFfiException.KeyStore) {
        }
        assertArrayEquals(pkcs8, store().loadIdentity())
    }

    @Test fun theDataStoreFileIsTheDedicatedOne() = runBlocking {
        store().storeIdentity(pkcs8)
        assertTrue(file.isFile)
        assertEquals("remote_control.preferences_pb", file.name)
    }
}
