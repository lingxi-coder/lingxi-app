package com.lingxi.code.voice.offline

import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.SocketTimeoutException
import java.net.URL
import java.nio.file.Files
import java.util.ArrayDeque

class VoiceModelDownloaderTest {
    @Test
    fun cancellingDuringExtractionDoesNotPublishOrLeaveStaging() {
        val root = Files.createTempDirectory("voice-extract-cancel").toFile()
        val model = OfflineModelCatalog.all.first()
        val archive = root.resolve("model.tar.bz2")
        val destination = root.resolve(model.id)
        val staging = root.resolve(".${model.id}.installing")
        val job = kotlinx.coroutines.Job()
        try {
            val payload = ByteArray(192 * 1024) { (it % 127).toByte() }
            org.apache.commons.compress.compressors.bzip2.BZip2CompressorOutputStream(archive.outputStream()).use { bz ->
                org.apache.commons.compress.archivers.tar.TarArchiveOutputStream(bz).use { tar ->
                    val entry = org.apache.commons.compress.archivers.tar.TarArchiveEntry("wrapper/${model.files.first()}")
                    entry.size = payload.size.toLong()
                    tar.putArchiveEntry(entry)
                    tar.write(payload)
                    tar.closeArchiveEntry()
                }
            }
            destination.mkdirs()
            destination.resolve("old-model").writeText("preserve")
            var interrupted = false
            val failure = runCatching {
                VoiceModelDownloader.install(archive, model, destination) {
                    if (staging.resolve(model.files.first()).length() > 0) {
                        interrupted = true
                        job.cancel()
                    }
                    if (!job.isActive) throw kotlinx.coroutines.CancellationException("cancelled during copy")
                }
            }.exceptionOrNull()
            assertTrue(interrupted)
            assertTrue(failure is kotlinx.coroutines.CancellationException)
            assertEquals("preserve", destination.resolve("old-model").readText())
            assertTrue(!staging.exists())
            assertTrue(archive.exists())
        } finally {
            job.cancel()
            root.deleteRecursively()
        }
    }

    @Test
    fun activityReattachPreservesActiveInstallationStaging() {
        val root = Files.createTempDirectory("voice-reattach").toFile()
        val context = object : android.content.ContextWrapper(null) {
            override fun getApplicationContext(): android.content.Context = this
            override fun getFilesDir(): java.io.File = root.resolve("files")
            override fun getCacheDir(): java.io.File = root.resolve("cache")
        }
        val contextField = VoiceModelDownloader::class.java.getDeclaredField("appContext").apply { isAccessible = true }
        val previous = contextField.get(VoiceModelDownloader)
        contextField.set(VoiceModelDownloader, null)
        try {
            val model = OfflineModelCatalog.all.first()
            val staging = root.resolve("files/voice_models/.${model.id}.installing")
            staging.mkdirs()
            staging.resolve("stale").writeText("interrupted")
            VoiceModelDownloader.attach(context)
            assertTrue(!staging.exists())
            staging.mkdirs()
            val activeFile = staging.resolve("active-model")
            activeFile.writeText("extracted data")
            VoiceModelDownloader.attach(context)
            assertEquals("extracted data", activeFile.readText())
        } finally {
            contextField.set(VoiceModelDownloader, previous)
            root.deleteRecursively()
        }
    }

    @Test
    fun interrupted_transfer_retries_from_preserved_byte_offset() = runTest {
        val payload = "abcdefghij".toByteArray()
        val first = FakeConnection(
            status = 200,
            contentLength = payload.size.toLong(),
            input = { TimeoutAfterChunk(payload.copyOfRange(0, 4)) },
        )
        val second = FakeConnection(
            status = 206,
            contentLength = 6,
            headers = mapOf("Content-Range" to "bytes 4-9/10"),
            input = { ByteArrayInputStream(payload.copyOfRange(4, payload.size)) },
        )
        val connections = ArrayDeque(listOf(first, second))
        val destination = Files.createTempFile("voice-model", ".part").toFile().apply { delete() }
        val progress = mutableListOf<Pair<Long, Long>>()

        try {
            downloadArchiveWithResume(
                sourceUrl = "https://example.test/model.tar.bz2",
                destination = destination,
                fallbackTotal = payload.size.toLong(),
                connectionFactory = { connections.removeFirst() },
                retryDelay = {},
                onProgress = { bytes, total -> progress += bytes to total },
            )

            assertArrayEquals(payload, destination.readBytes())
            assertEquals("bytes=4-", second.requestHeaders["Range"])
            assertEquals(10L to 10L, progress.last())
        } finally {
            destination.delete()
        }
    }

    @Test
    fun server_ignoring_range_restarts_instead_of_appending_duplicate_bytes() = runTest {
        val payload = "replacement".toByteArray()
        val connection = FakeConnection(
            status = 200,
            contentLength = payload.size.toLong(),
            input = { ByteArrayInputStream(payload) },
        )
        val destination = Files.createTempFile("voice-model", ".part").toFile().apply {
            writeText("stale partial")
        }

        try {
            downloadArchiveWithResume(
                sourceUrl = "https://example.test/model.tar.bz2",
                destination = destination,
                fallbackTotal = payload.size.toLong(),
                connectionFactory = { connection },
                retryDelay = {},
                onProgress = { _, _ -> },
            )

            assertArrayEquals(payload, destination.readBytes())
            assertEquals("bytes=13-", connection.requestHeaders["Range"])
        } finally {
            destination.delete()
        }
    }

    @Test
    fun permanent_http_error_does_not_retry_or_delete_partial_file() = runTest {
        var attempts = 0
        val destination = Files.createTempFile("voice-model", ".part").toFile().apply {
            writeText("partial")
        }

        try {
            val failure = runCatching {
                downloadArchiveWithResume(
                    sourceUrl = "https://example.test/model.tar.bz2",
                    destination = destination,
                    fallbackTotal = 100,
                    maxAttempts = 4,
                    connectionFactory = {
                        attempts += 1
                        FakeConnection(status = 404, contentLength = 0, input = { ByteArrayInputStream(ByteArray(0)) })
                    },
                    retryDelay = {},
                    onProgress = { _, _ -> },
                )
            }.exceptionOrNull()

            assertTrue(failure?.message?.contains("HTTP 404") == true)
            assertEquals(1, attempts)
            assertEquals("partial", destination.readText())
        } finally {
            destination.delete()
        }
    }

    @Test
    fun aggregate_progress_uses_real_per_model_totals() {
        val pack = VOICE_PACKS.first { it.language == "zh" }
        val first = pack.models[0]
        val second = pack.models[1]
        val state = aggregatePackState(
            states = mapOf(
                first.id to ModelState.Downloading(bytes = 50, total = first.approxSizeBytes + 100),
                second.id to ModelState.Downloading(bytes = 25, total = second.approxSizeBytes + 200),
            ),
            pack = pack,
        ) as ModelState.Downloading

        assertEquals(75, state.bytes)
        assertEquals(first.approxSizeBytes + second.approxSizeBytes + 300, state.total)
    }

    @Test
    fun pack_progress_identifies_each_model_without_looking_like_a_restart() {
        val pack = VOICE_PACKS.first { it.language == "zh" }
        val first = pack.models[0]
        val second = pack.models[1]
        val installingFirst = voicePackProgress(
            states = mapOf(
                first.id to ModelState.Extracting,
                second.id to ModelState.Queued(bytes = 0, total = second.approxSizeBytes),
            ),
            pack = pack,
        )

        assertEquals(first, installingFirst.activeModel)
        assertEquals(0, installingFirst.activeModelIndex)
        assertEquals(second, installingFirst.nextModel)
        assertEquals(first.approxSizeBytes, installingFirst.downloadedBytes)

        val downloadingSecond = voicePackProgress(
            states = mapOf(
                first.id to ModelState.Ready,
                second.id to ModelState.Downloading(bytes = 1024, total = second.approxSizeBytes),
            ),
            pack = pack,
        )

        assertEquals(second, downloadingSecond.activeModel)
        assertEquals(1, downloadingSecond.activeModelIndex)
        assertEquals(null, downloadingSecond.nextModel)
        assertTrue(downloadingSecond.downloadedBytes >= installingFirst.downloadedBytes)
        assertEquals(first.approxSizeBytes + 1024, downloadingSecond.downloadedBytes)
    }

    @Test
    fun onboarding_packs_use_complete_mobile_sized_models() {
        val chinese = VOICE_PACKS.first { it.language == "zh" }
        val english = VOICE_PACKS.first { it.language == "en" }

        assertEquals(221_351_135L, chinese.totalBytes)
        assertEquals(134_187_246L, english.totalBytes)
        assertTrue(chinese.totalBytes < 250L * 1024 * 1024)
        assertTrue(english.totalBytes < 150L * 1024 * 1024)
        assertTrue(chinese.models.all { it.files.isNotEmpty() && it.sha256.length == 64 })
        assertTrue(english.models.all { it.files.isNotEmpty() && it.sha256.length == 64 })
        assertEquals(
            listOf("dict"),
            chinese.models.first { it.kind == ModelKind.Tts }.requiredDirectories,
        )
        assertEquals(
            listOf("espeak-ng-data"),
            english.models.first { it.kind == ModelKind.Tts }.requiredDirectories,
        )
    }

    private class FakeConnection(
        private val status: Int,
        private val contentLength: Long,
        private val headers: Map<String, String> = emptyMap(),
        private val input: () -> InputStream,
    ) : HttpURLConnection(URL("https://example.test")) {
        val requestHeaders = mutableMapOf<String, String>()

        override fun connect() {
            connected = true
        }

        override fun disconnect() {
            connected = false
        }

        override fun usingProxy(): Boolean = false

        override fun getResponseCode(): Int = status

        override fun getContentLengthLong(): Long = contentLength

        override fun getHeaderField(name: String?): String? =
            headers.entries.firstOrNull { it.key.equals(name, ignoreCase = true) }?.value

        override fun getInputStream(): InputStream = input()

        override fun setRequestProperty(key: String, value: String) {
            requestHeaders[key] = value
        }
    }

    private class TimeoutAfterChunk(
        private val chunk: ByteArray,
    ) : InputStream() {
        private var emitted = false

        override fun read(): Int {
            if (emitted) throw SocketTimeoutException("simulated timeout")
            emitted = true
            return chunk.firstOrNull()?.toInt()?.and(0xff) ?: -1
        }

        override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
            if (emitted) throw SocketTimeoutException("simulated timeout")
            emitted = true
            val count = minOf(length, chunk.size)
            chunk.copyInto(buffer, destinationOffset = offset, endIndex = count)
            return count
        }
    }
}
