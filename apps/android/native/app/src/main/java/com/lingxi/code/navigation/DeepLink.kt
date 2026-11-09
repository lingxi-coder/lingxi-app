package com.lingxi.code.navigation

import java.net.URI
import java.net.URLDecoder
import java.nio.charset.StandardCharsets
import org.json.JSONObject
import org.json.JSONArray
import java.net.URLEncoder

/** Parsing never runs commands, fetches an import or changes application state. */
data class DeepLink(val route: String, val params: Map<String, String>, val payload: Map<String, Any?>) {
    companion object {
        fun parse(raw: String?): DeepLink? = runCatching {
            require(raw != null && raw.length <= 32768 && raw.none { it <= ' ' || it == '\u007f' })
            val uri = URI(raw)
            require(uri.userInfo == null && uri.port == -1)
            val route = when {
                uri.scheme == "lingxi" -> (uri.host ?: error("Missing host")) + (uri.rawPath ?: "")
                uri.scheme == "https" && uri.host == "lingxios.net" -> (uri.rawPath ?: "").removePrefix("/open/").also { require(uri.rawPath.startsWith("/open/")) }
                else -> error("Unsupported origin")
            }.split('/').joinToString("/") { segment ->
                val decoded = decode(segment.replace("+", "%2B"))
                require(decoded.isNotEmpty() && decoded != "." && decoded != ".." && decoded.none { it == '/' || it == '\\' || it < ' ' || it == '\u007f' })
                decoded
            }
            require(Regex("^(session/[^/]+|project/[^/]+|new|settings/[^/]+|provider/[^/]+|extensions/(mcp|skill|plugin)/[^/]+|cron/(job|run)/[^/]+|terminal/[^/]+|apps/[^/]+|market/(bot/)?[^/]+|runtime/(agent|task|plan|review|activity)/[^/]+|import/(mcp|plugin|skill|provider)|remote/[^/]+/[^/]+|team/(join|inbox)|friend/(add|requests|chat))$").matches(route))
            val params = pairs(uri.rawQuery).mapValues { (key, value) ->
                if (key == "message") value.lowercase(java.util.Locale.ROOT) else value
            }
            require(params.all { (k, v) -> k in setOf("workspace", "message", "mode", "scope", "review", "runId", "revision", "code", "uid", "turn", "generation", "versionId", "intentId") && v.isNotEmpty() && v.length <= 512 && v.none { it < ' ' || it == '\u007f' } })
            if (route == "team/inbox" || route == "friend/requests") require(params.isEmpty())
            // A team invitation and a friend link each carry exactly one 32-character code, which no other route accepts.
            if (route == "team/join" || route == "friend/add") require(params.keys == setOf("code") && Regex("^[A-Za-z0-9_-]{32}$").matches(params.getValue("code")))
            else require("code" !in params)
            if (route == "friend/chat") require(params.keys == setOf("uid") && Regex("^[A-Za-z0-9_-]{1,128}$").matches(params.getValue("uid")))
            else require("uid" !in params)
            require(params["review"] == null || (route.startsWith("apps/") && params["review"] == "agent"))
            require(params["runId"] == null || (route.startsWith("apps/") && params["review"] == "agent" && params.getValue("runId").length <= 128))
            require(params["revision"] == null || (route.startsWith("apps/") && params["review"] == "agent" && params["runId"] != null &&
                Regex("^[1-9][0-9]*$").matches(params.getValue("revision")) && params.getValue("revision").toLongOrNull() != null))
            require(params["mode"] == null || params["mode"] in setOf("chat", "code"))
            if (route.startsWith("session/")) require(params["mode"] != null)
            require(params["turn"] == null || (route.startsWith("session/") && params.getValue("turn").toLongOrNull()?.let { it > 0L } == true))
            require(params["generation"] == null || (route.startsWith("session/") &&
                runCatching { java.util.UUID.fromString(params.getValue("generation")).toString() == params.getValue("generation") }.getOrDefault(false)))
            require(params["message"] == null || (route.startsWith("session/") && Regex("^[0-9a-fA-F]{8}(-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}$").matches(params.getValue("message"))))
            require(params["intentId"] == null || (route.startsWith("market/bot/") && Regex("^[A-Za-z0-9_-]{1,128}$").matches(params.getValue("intentId"))))
            require(params["versionId"] == null || (route.startsWith("market/bot/") && Regex("^[A-Za-z0-9_-]{1,128}$").matches(params.getValue("versionId"))))
            if (route.startsWith("market/bot/")) require(Regex("^[A-Za-z0-9_-]{1,128}$").matches(route.substringAfter("market/bot/")) && params.keys.all { it == "versionId" || it == "intentId" })
            val payload = if (uri.rawFragment != null) {
                require(route == "new" || route.startsWith("terminal/") || route.startsWith("import/"))
                val fragment = pairs(uri.rawFragment)
                require(fragment.keys == setOf("payload"))
                val json = JSONObject(fragment.getValue("payload"))
                val expected = if (route == "new") "prompt" else "command"
                if (route.startsWith("import/")) validateImport(json)
                json.keys().asSequence().associateWith { key ->
                    if (!route.startsWith("import/")) { require(key == expected); require(json.get(key) is String) }
                    json.get(key)
                }
            } else emptyMap()
            DeepLink(route, params, payload)
        }.getOrNull()
        fun build(route: String, params: Map<String, String> = emptyMap(), payload: Map<String, Any?> = emptyMap()): String {
            fun encode(value: String) = URLEncoder.encode(value, StandardCharsets.UTF_8.name()).replace("+", "%20")
            val query = params.mapValues { (key, value) -> if (key == "message") value.lowercase(java.util.Locale.ROOT) else value }.toSortedMap().entries.joinToString("&") { encode(it.key) + "=" + encode(it.value) }
            val fragment = if (payload.isEmpty()) "" else "#payload=" + encode(JSONObject(payload).toString())
            val raw = "https://lingxios.net/open/" + route.split('/').joinToString("/") { encode(it) } + (if (query.isEmpty()) "" else "?$query") + fragment
            require(parse(raw) != null)
            return raw
        }
        private fun validateImport(value: Any?, depth: Int = 0) {
            require(depth <= 16)
            when (value) {
                is JSONObject -> value.keys().forEach { key ->
                    require(!Regex("^(api[-_]?key|token|access[-_]?token|refresh[-_]?token|authorization|password|secret|headers|env)$", RegexOption.IGNORE_CASE).matches(key))
                    validateImport(value.get(key), depth + 1)
                }
                is JSONArray -> (0 until value.length()).forEach { validateImport(value.get(it), depth + 1) }
                is String -> if (value.startsWith("http://", true) || value.startsWith("https://", true)) {
                    val url = URI(value)
                    require(url.userInfo == null && url.rawQuery == null)
                }
            }
        }
        private fun decode(s: String) = URLDecoder.decode(s, StandardCharsets.UTF_8.name())
        private fun pairs(raw: String?): Map<String, String> {
            val result = linkedMapOf<String, String>()
            raw?.split('&')?.forEach { pair ->
                val parts = pair.split('=', limit = 2)
                val key = decode(parts[0])
                require(!result.containsKey(key))
                result[key] = decode(parts.getOrElse(1) { "" })
            }
            return result
        }
    }
}
