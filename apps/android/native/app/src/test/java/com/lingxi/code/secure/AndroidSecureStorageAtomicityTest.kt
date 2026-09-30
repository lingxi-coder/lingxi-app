package com.lingxi.code.secure

import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import javax.crypto.spec.SecretKeySpec
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

/** Real production AES-GCM/file operations with a fake key; never opens AndroidKeyStore. */
class AndroidSecureStorageAtomicityTest {
    @get:Rule val temp = TemporaryFolder()
    private val fakeKey = SecretKeySpec(ByteArray(32) { 7 }, "AES")
    private val oldSecret = "old-fake-provider-key".toByteArray()

    private fun store(writer: AtomicCredentialWriter = AtomicCredentialWriter()) =
        AndroidSecureStorageAdapter(temp.root, { fakeKey }, writer)

    @Test fun partialWriteFailurePreservesPriorAuthenticatedCredential() = runBlocking {
        val initial = store()
        initial.store("lingxi", "provider-key-fake", oldSecret)
        val entry = temp.root.walkTopDown().single { it.extension == "bin" }
        val originalCiphertext = entry.readBytes()
        val failing = store(AtomicCredentialWriter(write = { file, bytes ->
            FileOutputStream(file).use { it.write(bytes, 0, 12) }
            throw IOException("injected partial write")
        }))

        val failure = runCatching {
            failing.store("lingxi", "provider-key-fake", ByteArray(2048) { 42 })
        }.exceptionOrNull()

        assertNotNull(failure)
        assertTrue(failure!!.message.orEmpty().contains("partial write"))
        assertArrayEquals(originalCiphertext, entry.readBytes())
        assertArrayEquals(oldSecret, initial.retrieve("lingxi", "provider-key-fake"))
        assertEquals(listOf("provider-key-fake"), initial.list("lingxi"))
        assertFalse(temp.root.walkTopDown().any { it.extension == "tmp" })
    }

    @Test fun atomicReplaceFailurePreservesPriorCredentialAndRemovesTemporaryFile() = runBlocking {
        val initial = store()
        initial.store("lingxi", "provider-key-fake", oldSecret)
        val failing = store(AtomicCredentialWriter(replace = { _, _ ->
            throw IOException("injected rename failure")
        }))

        assertNotNull(runCatching {
            failing.store("lingxi", "provider-key-fake", "new-fake-key".toByteArray())
        }.exceptionOrNull())

        assertArrayEquals(oldSecret, initial.retrieve("lingxi", "provider-key-fake"))
        assertFalse(temp.root.walkTopDown().any { it.extension == "tmp" })
    }

    @Test fun successfulReplacementAndDeleteWorkAcrossAdapterInstances() = runBlocking {
        val first = store()
        val second = store()
        first.store("service-α", "account-β", oldSecret)
        val replacement = "new-fake-key".toByteArray()
        second.store("service-α", "account-β", replacement)
        assertArrayEquals(replacement, first.retrieve("service-α", "account-β"))
        assertEquals(listOf("account-β"), first.list("service-α"))
        second.delete("service-α", "account-β")
        assertNull(first.retrieve("service-α", "account-β"))
        assertEquals(emptyList<String>(), first.list("service-α"))
        second.delete("service-α", "account-β")
    }

    @Test fun deleteWaitsForAnotherAdaptersWriteAndCannotBeResurrected() {
        val writeEntered = CountDownLatch(1)
        val releaseWrite = CountDownLatch(1)
        val deleteEntered = CountDownLatch(1)
        val failure = AtomicReference<Throwable?>()
        val writing = store(AtomicCredentialWriter(write = { file, bytes ->
            writeEntered.countDown()
            check(releaseWrite.await(5, TimeUnit.SECONDS))
            FileOutputStream(file).use { it.write(bytes); it.fd.sync() }
        }))
        val deleting = store()
        val writer = Thread {
            runCatching { runBlocking { writing.store("lingxi", "fake", oldSecret) } }
                .onFailure { failure.compareAndSet(null, it) }
        }.apply { isDaemon = true }
        val deleter = Thread {
            deleteEntered.countDown()
            runCatching { runBlocking { deleting.delete("lingxi", "fake") } }
                .onFailure { failure.compareAndSet(null, it) }
        }.apply { isDaemon = true }
        try {
            writer.start()
            assertTrue(writeEntered.await(5, TimeUnit.SECONDS))
            deleter.start()
            assertTrue(deleteEntered.await(5, TimeUnit.SECONDS))
            val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5)
            while (deleter.state != Thread.State.BLOCKED && deleter.isAlive && System.nanoTime() < deadline) {
                Thread.yield()
            }
            assertEquals("same-entry delete must wait for the complete write", Thread.State.BLOCKED, deleter.state)
        } finally {
            releaseWrite.countDown()
            writer.join(5000)
            if (deleter.state != Thread.State.NEW) deleter.join(5000)
        }
        assertFalse(writer.isAlive)
        assertFalse(deleter.isAlive)
        assertNull(failure.get())
        runBlocking { assertNull(deleting.retrieve("lingxi", "fake")) }
    }
}
