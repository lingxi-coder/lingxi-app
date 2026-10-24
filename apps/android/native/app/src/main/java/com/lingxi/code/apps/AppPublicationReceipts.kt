package com.lingxi.code.apps

import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.util.UUID

/** Reads only committed disk state; a failed write never becomes a reusable identity. */
internal class AppPublicationReceipts(private val directory: File) {
    private val file = File(directory, "receipts.json")
    private val lock get() = locks.computeIfAbsent(file.canonicalPath) { Any() }
    private fun load() = if (file.exists()) JSONObject(file.readText()) else JSONObject()
    fun getString(name: String, fallback: String?): String? = synchronized(lock) { load().let { if (it.has(name)) it.getString(name) else fallback } }
    fun contains(name: String): Boolean = synchronized(lock) { load().has(name) }
    fun save(name: String, value: String) = synchronized(lock) {
        val values = load().put(name, value)
        check(directory.isDirectory || directory.mkdirs()) { "Unable to save publication receipt" }
        val temporary = File(directory, "receipts-${UUID.randomUUID()}.tmp")
        try {
            FileOutputStream(temporary).use { output -> output.write(values.toString().toByteArray(Charsets.UTF_8)); output.fd.sync() }
            Files.move(temporary.toPath(), file.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        } finally { temporary.delete() }
    }
    companion object { private val locks = java.util.concurrent.ConcurrentHashMap<String, Any>() }
}
