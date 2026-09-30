package com.lingxi.code.project

import android.net.Uri
import androidx.test.ext.junit.runners.AndroidJUnit4
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.security.MessageDigest
import java.util.UUID

@RunWith(AndroidJUnit4::class)
class SafProjectSynchronizerInstrumentedTest {
    @Test
    fun conflictResolutionOnlyForcesActualConflictPaths() {
        assertEquals(
            ProjectSyncAction.CopyExternal,
            decideProjectSyncAction(
                direction = ProjectSyncDirection.ExternalToInternal,
                resolution = ConflictResolution.KeepExternal,
                baselineSha256 = "base",
                externalSha256 = "external",
                internalSha256 = "internal",
            ),
        )
        assertEquals(
            ProjectSyncAction.Skip,
            decideProjectSyncAction(
                direction = ProjectSyncDirection.ExternalToInternal,
                resolution = ConflictResolution.KeepExternal,
                baselineSha256 = "base",
                externalSha256 = "base",
                internalSha256 = "local-change",
            ),
        )
        assertEquals(
            ProjectSyncAction.Skip,
            decideProjectSyncAction(
                direction = ProjectSyncDirection.InternalToExternal,
                resolution = ConflictResolution.KeepInternal,
                baselineSha256 = "base",
                externalSha256 = "external-change",
                internalSha256 = "base",
            ),
        )
    }

    @Test
    fun interruptedExternalWriteLeavesOriginalDocumentUntouched() = runBlocking {
        val backend = FakeSafDocumentBackend(failWritesAfterBytes = 3)
        val parent = Uri.parse("content://fake/root")
        val target = backend.seed(parent, "notes.txt", "original".encodeToByteArray())

        val failed = runCatching {
            commitExternalDocument(
                backend = backend,
                parentUri = parent,
                targetUri = target,
                displayName = "notes.txt",
                mimeType = "application/octet-stream",
                expectedSha256 = sha256("replacement".encodeToByteArray()),
                source = { ByteArrayInputStream("replacement".encodeToByteArray()) },
            )
        }

        assertTrue(failed.isFailure)
        assertEquals("original", backend.bytesNamed("notes.txt")?.decodeToString())
        assertFalse(backend.names().any(::isLingxiSyncArtifact))
    }

    @Test
    fun successfulExternalWriteSwapsVerifiedTempDocument() = runBlocking {
        val backend = FakeSafDocumentBackend()
        val parent = Uri.parse("content://fake/root")
        val target = backend.seed(parent, "notes.txt", "original".encodeToByteArray())
        val replacement = "replacement".encodeToByteArray()

        commitExternalDocument(
            backend = backend,
            parentUri = parent,
            targetUri = target,
            displayName = "notes.txt",
            mimeType = "application/octet-stream",
            expectedSha256 = sha256(replacement),
            source = { ByteArrayInputStream(replacement) },
        )

        assertEquals("replacement", backend.bytesNamed("notes.txt")?.decodeToString())
        assertFalse(backend.names().any(::isLingxiSyncArtifact))
    }

    @Test
    fun failedExternalRenameRestoresOriginalDocument() = runBlocking {
        val backend = FakeSafDocumentBackend(failRenameToOnce = "notes.txt")
        val parent = Uri.parse("content://fake/root")
        val target = backend.seed(parent, "notes.txt", "original".encodeToByteArray())
        val replacement = "replacement".encodeToByteArray()

        val failed = runCatching {
            commitExternalDocument(
                backend = backend,
                parentUri = parent,
                targetUri = target,
                displayName = "notes.txt",
                mimeType = "application/octet-stream",
                expectedSha256 = sha256(replacement),
                source = { ByteArrayInputStream(replacement) },
            )
        }

        assertTrue(failed.isFailure)
        assertEquals("original", backend.bytesNamed("notes.txt")?.decodeToString())
        assertFalse(backend.names().any(::isLingxiSyncArtifact))
    }

    @Test
    fun userFileMerelyContainingMarkerTextIsNotAnArtifact() {
        assertFalse(isLingxiSyncArtifact("notes.txt.__lingxi_temp__draft"))
        assertFalse(isLingxiSyncArtifact("notes.txt.__lingxi_backup__not-a-uuid"))
    }

    @Test
    fun exportRequiresPersistedWritePermission() {
        assertTrue(
            persistedPermissionAllows(
                ProjectSyncDirection.ExternalToInternal,
                read = true,
                write = false,
            ),
        )
        assertFalse(
            persistedPermissionAllows(
                ProjectSyncDirection.InternalToExternal,
                read = true,
                write = false,
            ),
        )
        assertTrue(
            persistedPermissionAllows(
                ProjectSyncDirection.InternalToExternal,
                read = true,
                write = true,
            ),
        )
    }

    private class FakeSafDocumentBackend(
        private val failWritesAfterBytes: Int? = null,
        private var failRenameToOnce: String? = null,
    ) : SafDocumentBackend {
        private data class Document(
            val parent: Uri,
            var name: String,
            var bytes: ByteArray,
        )

        private val documents = linkedMapOf<Uri, Document>()

        fun seed(parent: Uri, name: String, bytes: ByteArray): Uri =
            Uri.parse("content://fake/${UUID.randomUUID()}").also { uri ->
                documents[uri] = Document(parent, name, bytes)
            }

        fun bytesNamed(name: String): ByteArray? =
            documents.values.firstOrNull { it.name == name }?.bytes

        fun names(): List<String> = documents.values.map { it.name }

        override fun createDocument(parentUri: Uri, mimeType: String, displayName: String): Uri =
            seed(parentUri, displayName, ByteArray(0))

        override fun openInput(uri: Uri): InputStream =
            ByteArrayInputStream(documents.getValue(uri).bytes)

        override fun openOutput(uri: Uri): OutputStream {
            val document = documents.getValue(uri)
            val sink = ByteArrayOutputStream()
            return object : OutputStream() {
                private var written = 0

                override fun write(value: Int) {
                    failIfNeeded(1)
                    sink.write(value)
                    written++
                }

                override fun write(buffer: ByteArray, offset: Int, length: Int) {
                    failIfNeeded(length)
                    sink.write(buffer, offset, length)
                    written += length
                }

                private fun failIfNeeded(nextBytes: Int) {
                    if (failWritesAfterBytes != null && written + nextBytes > failWritesAfterBytes) {
                        throw IOException("injected provider write failure")
                    }
                }

                override fun close() {
                    document.bytes = sink.toByteArray()
                }
            }
        }

        override fun renameDocument(uri: Uri, displayName: String): Uri {
            if (failRenameToOnce == displayName) {
                failRenameToOnce = null
                throw IOException("injected provider rename failure")
            }
            documents.getValue(uri).name = displayName
            return uri
        }

        override fun deleteDocument(uri: Uri) {
            documents.remove(uri)
        }
    }
}

private fun sha256(bytes: ByteArray): String =
    MessageDigest.getInstance("SHA-256")
        .digest(bytes)
        .joinToString("") { "%02x".format(it) }
