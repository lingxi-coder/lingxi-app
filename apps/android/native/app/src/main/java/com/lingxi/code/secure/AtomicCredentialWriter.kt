package com.lingxi.code.secure

import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.nio.file.Files
import java.nio.file.StandardCopyOption

/** Publish only a complete, synced ciphertext; failed writes leave the old entry intact. */
internal class AtomicCredentialWriter(
    private val write: (File, ByteArray) -> Unit = { file, bytes ->
        FileOutputStream(file).use { output ->
            output.write(bytes)
            output.fd.sync()
        }
    },
    private val replace: (File, File) -> Unit = { temporary, destination ->
        Files.move(
            temporary.toPath(),
            destination.toPath(),
            StandardCopyOption.ATOMIC_MOVE,
            StandardCopyOption.REPLACE_EXISTING,
        )
    },
) {
    fun store(destination: File, ciphertext: ByteArray) {
        val directory = destination.parentFile
            ?: throw IOException("credential entry has no parent directory")
        if (!directory.mkdirs() && !directory.isDirectory) {
            throw IOException("credential directory is unavailable")
        }
        val temporary = File.createTempFile(".credential-", ".tmp", directory)
        try {
            write(temporary, ciphertext)
            // Do not fall back to an in-place copy on unsupported filesystems.
            // Reporting failure must preserve the previously stored credential.
            replace(temporary, destination)
        } finally {
            temporary.delete()
        }
    }
}
