package com.lingxi.code.remote

import android.os.Bundle
import android.os.SystemClock
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.security.MessageDigest
import java.util.UUID
import org.junit.Assert.assertEquals
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith

/** Opt-in device benchmark; compares only the final local export, not WebRTC. */
@RunWith(AndroidJUnit4::class)
class RemoteFileCopyBenchmark {
    @Test fun compareVerifiedFileExports() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        assumeTrue(InstrumentationRegistry.getArguments().getString("remoteFileBench") == "true")
        val directory = File(instrumentation.targetContext.cacheDir, "rc-copy-bench-${UUID.randomUUID()}")
        check(directory.mkdir())
        val size = 64 * 1024 * 1024
        val source = File(directory, "source")
        val pattern = ByteArray(64 * 1024) { (it % 251).toByte() }
        try {
            source.outputStream().use { output -> repeat(size / pattern.size) { output.write(pattern) } }
            val expected = digest(source)
            for (round in 0..2) {
                // Alternate order to reduce, but not eliminate, cache/order bias.
                val modes = if (round % 2 == 0) listOf("buffered", "direct") else listOf("direct", "buffered")
                for (mode in modes) {
                    val target = File(directory, "$round-$mode")
                    var directBytes = 0L
                    var bufferedBytes = 0L
                    val start = SystemClock.elapsedRealtimeNanos()
                    source.inputStream().use { input ->
                        target.outputStream().use { output ->
                            if (mode == "buffered") {
                                val buffer = ByteArray(64 * 1024)
                                while (true) {
                                    val count = input.read(buffer)
                                    if (count < 0) break
                                    output.write(buffer, 0, count)
                                    bufferedBytes += count
                                }
                            } else {
                                val result = copyVerifiedRemoteFile(input.channel, output.channel, {})
                                directBytes = result.directBytes
                                bufferedBytes = result.bufferedBytes
                            }
                        }
                    }
                    val elapsed = SystemClock.elapsedRealtimeNanos() - start
                    assertEquals(size.toLong(), target.length())
                    assertEquals(expected, digest(target))
                    instrumentation.sendStatus(1, Bundle().apply {
                        putString("remote_file_copy_benchmark", "round=$round mode=$mode bytes=$size elapsed_ns=$elapsed direct_bytes=$directBytes buffered_bytes=$bufferedBytes hash_ok=true")
                    })
                    check(target.delete())
                }
            }
        } finally {
            source.delete()
            directory.delete()
        }
    }

    private fun digest(file: File): String {
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val buffer = ByteArray(64 * 1024)
            while (true) {
                val n = input.read(buffer)
                if (n < 0) break
                digest.update(buffer, 0, n)
            }
        }
        return digest.digest().joinToString("") { "%02x".format(it) }
    }
}
