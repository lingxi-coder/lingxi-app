package com.lingxi.code.remote

import java.io.EOFException
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.channels.FileChannel
import java.nio.channels.WritableByteChannel

private const val DIRECT_COPY_CHUNK = 4L * 1024 * 1024
private const val BUFFERED_COPY_CHUNK = 64 * 1024

/** Counts bytes completed through the direct-copy path and the fallback. */
internal data class RemoteFileCopyResult(val directBytes: Long, val bufferedBytes: Long)

/**
 * Copy an already verified staging file to the selected document. A seekable
 * destination may support a kernel-assisted transfer; providers that return
 * pipes, or a direct transfer that cannot make progress, use bounded buffers.
 * Direct transfer is an OS optimization, not a promise of zero-copy.
 */
internal fun copyVerifiedRemoteFile(
    source: FileChannel,
    destination: WritableByteChannel,
    ensureActive: () -> Unit,
    transfer: (FileChannel, Long, Long, WritableByteChannel) -> Long = { input, at, count, output ->
        input.transferTo(at, count, output)
    },
): RemoteFileCopyResult {
    val size = source.size()
    val output = destination as? FileChannel
    val initialOutputPosition = try { output?.position() } catch (_: IOException) { null }
        catch (_: UnsupportedOperationException) { null }
    var copied = 0L
    if (initialOutputPosition != null) {
        while (copied < size) {
            ensureActive()
            val requested = minOf(DIRECT_COPY_CHUNK, size - copied)
            val count = try {
                transfer(source, copied, requested, destination)
            } catch (error: IOException) {
                // Only retry with buffers if the seekable output proves that
                // this failed call wrote no bytes. Otherwise a retry could
                // duplicate an unknown partial write and silently corrupt it.
                val position = try { output?.position() } catch (_: IOException) { null }
                    catch (_: UnsupportedOperationException) { null }
                if (position != initialOutputPosition + copied) throw error
                break
            } catch (error: UnsupportedOperationException) {
                val position = try { output?.position() } catch (_: IOException) { null }
                    catch (_: UnsupportedOperationException) { null }
                if (position != initialOutputPosition + copied) throw error
                break
            }
            if (count < 0 || count > requested) throw IOException("Invalid direct copy progress")
            if (count == 0L) break
            copied += count
        }
    }
    val direct = copied
    if (copied < size) {
        source.position(copied)
        val buffer = ByteBuffer.allocate(BUFFERED_COPY_CHUNK)
        while (copied < size) {
            ensureActive()
            buffer.clear()
            buffer.limit(minOf(buffer.capacity().toLong(), size - copied).toInt())
            val count = source.read(buffer)
            if (count < 0) throw EOFException("Staging file truncated during export")
            if (count == 0) throw IOException("Staging file read made no progress")
            buffer.flip()
            while (buffer.hasRemaining()) {
                ensureActive()
                if (destination.write(buffer) <= 0) throw IOException("Destination write made no progress")
            }
            copied += count
        }
    }
    return RemoteFileCopyResult(direct, copied - direct)
}
