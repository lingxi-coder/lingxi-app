package com.lingxi.code.remote

import java.io.ByteArrayOutputStream
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.channels.Channels
import java.nio.file.Files
import kotlinx.coroutines.CancellationException
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class RemoteFileCopyTest {
    @get:Rule val temporary = TemporaryFolder()
    private val bytes = ByteArray(256 * 1024 + 13) { (it % 251).toByte() }

    @Test fun shortDirectTransfersCompleteWithoutLosingBytes() {
        val source = temporary.newFile("source").apply { writeBytes(bytes) }
        val target = temporary.newFile("target")
        val result = source.inputStream().channel.use { input ->
            target.outputStream().channel.use { output ->
                copyVerifiedRemoteFile(input, output, {}, transfer = { src, at, count, dst ->
                    src.transferTo(at, minOf(count, 8191), dst)
                })
            }
        }
        assertEquals(bytes.size.toLong(), result.directBytes)
        assertEquals(0L, result.bufferedBytes)
        assertArrayEquals(bytes, target.readBytes())
    }

    @Test fun zeroProgressAndUnsupportedTransferFallBackFromTheExactOffset() {
        for (unsupported in listOf(false, true)) {
            val source = temporary.newFile("source-$unsupported").apply { writeBytes(bytes) }
            val target = temporary.newFile("target-$unsupported")
            var calls = 0
            val result = source.inputStream().channel.use { input ->
                target.outputStream().channel.use { output ->
                    copyVerifiedRemoteFile(input, output, {}, transfer = { src, at, count, dst ->
                        calls++
                        if (calls == 1) src.transferTo(at, minOf(count, 8191), dst)
                        else if (unsupported) throw IOException("unsupported") else 0L
                    })
                }
            }
            assertEquals(8191L, result.directBytes)
            assertEquals(bytes.size.toLong() - 8191L, result.bufferedBytes)
            assertArrayEquals(bytes, target.readBytes())
        }
    }

    @Test fun aThrowAfterAnUnknownPartialWriteDoesNotRetryOrDuplicateData() {
        val source = temporary.newFile("source").apply { writeBytes(bytes) }
        val target = temporary.newFile("target")
        source.inputStream().channel.use { input ->
            target.outputStream().channel.use { output ->
                assertThrows(IOException::class.java) {
                    copyVerifiedRemoteFile(input, output, {}, transfer = { _, _, _, dst ->
                        dst.write(ByteBuffer.wrap(bytes, 0, 5))
                        throw IOException("failed after partial write")
                    })
                }
            }
        }
        assertArrayEquals(bytes.copyOfRange(0, 5), target.readBytes())
    }

    @Test fun nonSeekableDestinationUsesBoundedFallback() {
        val source = temporary.newFile("source").apply { writeBytes(bytes) }
        val resultBytes = ByteArrayOutputStream()
        val result = source.inputStream().channel.use { input ->
            Channels.newChannel(resultBytes).use { output ->
                copyVerifiedRemoteFile(input, output, {}, transfer = { _, _, _, _ -> error("must use fallback") })
            }
        }
        assertEquals(0L, result.directBytes)
        assertEquals(bytes.size.toLong(), result.bufferedBytes)
        assertArrayEquals(bytes, resultBytes.toByteArray())
    }

    @Test fun cancellationStopsBeforeTheNextChunk() {
        val source = temporary.newFile("source").apply { writeBytes(bytes) }
        val target = temporary.newFile("target")
        var checks = 0
        source.inputStream().channel.use { input ->
            target.outputStream().channel.use { output ->
                assertThrows(CancellationException::class.java) {
                    copyVerifiedRemoteFile(input, output, { if (++checks == 2) throw CancellationException() },
                        transfer = { src, at, count, dst -> src.transferTo(at, minOf(count, 8191), dst) })
                }
            }
        }
        assertEquals(8191L, Files.size(target.toPath()))
    }
}
