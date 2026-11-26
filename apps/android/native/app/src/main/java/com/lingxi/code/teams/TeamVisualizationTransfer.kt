package com.lingxi.code.teams

import android.util.Base64
import java.security.MessageDigest
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

internal object TeamVisualizationTransfer {
    const val maximumBytes = 50 * 1024 * 1024
    const val partBytes = 2 * 1024 * 1024
    const val editableChars = 128 * 1024

    data class Prepared(val manifest: JSONObject, val payload: ByteArray)

    private fun digest(bytes: ByteArray, start: Int = 0, size: Int = bytes.size): String =
        MessageDigest.getInstance("SHA-256").apply { update(bytes, start, size) }
            .digest().joinToString("") { "%02x".format(it.toInt() and 255) }

    suspend fun prepare(bundle: JSONObject): Prepared = withContext(Dispatchers.Default) {
        val html = bundle.getString("html")
        if (html.isBlank()) throw TeamMessageFailure("INVALID_DATA")
        val markup = html.toByteArray()
        val assets = bundle.getJSONObject("assets")
        val names = assets.keys().asSequence().sorted().toList()
        val size = names.fold(markup.size.toLong()) { total, name ->
            total + assets.getJSONObject(name).getString("data").length
        }
        if (size > maximumBytes) throw TeamMessageFailure("VISUALIZATION_TOO_LARGE")
        val payload = ByteArray(size.toInt())
        markup.copyInto(payload)
        var start = markup.size
        val manifestAssets = JSONArray()
        for (name in names) {
            val asset = assets.getJSONObject(name)
            val content = asset.getString("data")
            val bytes = content.toByteArray(Charsets.US_ASCII)
            if (bytes.size != content.length) throw TeamMessageFailure("INVALID_DATA")
            bytes.copyInto(payload, start)
            manifestAssets.put(objectOf("name" to name, "mime" to asset.getString("mime"), "bytes" to bytes.size))
            start += bytes.size
        }
        val parts = JSONArray()
        for (offset in payload.indices step partBytes) {
            parts.put(digest(payload, offset, minOf(partBytes, payload.size - offset)))
        }
        Prepared(objectOf("title" to bundle.getString("title"), "htmlBytes" to markup.size, "assets" to manifestAssets, "parts" to parts), payload)
    }

    suspend fun upload(transfer: Prepared, plan: JSONObject, request: suspend (JSONObject) -> JSONObject) {
        val message = plan.getString("message")
        val version = plan.getString("version")
        val missing = plan.getJSONArray("missing")
        val parts = transfer.manifest.getJSONArray("parts").length()
        if (missing.length() > parts) throw TeamMessageFailure("INVALID_DATA")
        for (position in 0 until missing.length()) {
            currentCoroutineContext().ensureActive()
            val index = missing.getInt(position)
            if (index !in 0 until parts) throw TeamMessageFailure("INVALID_DATA")
            val start = index * partBytes
            val data = withContext(Dispatchers.Default) {
                Base64.encodeToString(transfer.payload, start, minOf(partBytes, transfer.payload.size - start), Base64.NO_WRAP)
            }
            request(objectOf("action" to "putPart", "message" to message, "version" to version, "index" to index, "data" to data))
        }
    }

    suspend fun open(request: suspend (JSONObject) -> JSONObject): JSONObject {
        val opened = request(objectOf("action" to "open"))
        val manifest = opened.getJSONObject("package")
        val htmlBytes = manifest.getInt("htmlBytes")
        val assets = manifest.getJSONArray("assets")
        val parts = manifest.getJSONArray("parts")
        if (htmlBytes !in 1..maximumBytes || assets.length() > 32 || parts.length() > maximumBytes / partBytes) throw TeamMessageFailure("INVALID_DATA")
        var total = htmlBytes
        for (index in 0 until assets.length()) {
            val bytes = assets.getJSONObject(index).getInt("bytes")
            if (bytes !in 0..maximumBytes - total) throw TeamMessageFailure("INVALID_DATA")
            total += bytes
        }
        if (parts.length() != (total + partBytes - 1) / partBytes) throw TeamMessageFailure("INVALID_DATA")
        val payload = ByteArray(total)
        for (index in 0 until parts.length()) {
            currentCoroutineContext().ensureActive()
            val value = request(objectOf("action" to "readPart", "index" to index))
            withContext(Dispatchers.Default) {
                val start = index * partBytes
                val expectedSize = minOf(partBytes, payload.size - start)
                val data = value.getString("data")
                if (data.length != (expectedSize + 2) / 3 * 4) throw TeamMessageFailure("INVALID_DATA")
                val bytes = Base64.decode(data, Base64.NO_WRAP)
                if (bytes.size != expectedSize || digest(bytes) != parts.getString(index)) throw TeamMessageFailure("INVALID_DATA")
                bytes.copyInto(payload, start)
            }
        }
        return withContext(Dispatchers.Default) {
            val html = payload.decodeToString(0, htmlBytes, throwOnInvalidSequence = true)
            if (html.isBlank()) throw TeamMessageFailure("INVALID_DATA")
            val bundleAssets = JSONObject()
            var start = htmlBytes
            for (index in 0 until assets.length()) {
                val asset = assets.getJSONObject(index)
                val name = asset.getString("name")
                val count = asset.getInt("bytes")
                if (bundleAssets.has(name)) throw TeamMessageFailure("INVALID_DATA")
                val data = payload.decodeToString(start, start + count, throwOnInvalidSequence = true)
                bundleAssets.put(name, objectOf("mime" to asset.getString("mime"), "data" to data))
                start += count
            }
            objectOf("bundle" to objectOf("title" to manifest.getString("title"), "html" to html, "assets" to bundleAssets),
                "state" to opened.get("state"))
        }
    }
}
