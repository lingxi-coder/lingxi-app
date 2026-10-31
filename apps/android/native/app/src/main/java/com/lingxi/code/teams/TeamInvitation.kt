package com.lingxi.code.teams

import java.net.URI
import java.net.URLDecoder

object TeamInvitation {
    private val pattern = Regex("^[A-Za-z0-9_-]{32}$")
    fun code(input: String): String? {
        val value = input.trim()
        if (pattern.matches(value)) return value
        return runCatching {
            val uri = URI(value)
            require(uri.userInfo == null && uri.port == -1 && uri.rawFragment == null)
            require((uri.scheme == "lingxi" && uri.host == "team" && uri.path == "/join") ||
                (uri.scheme == "https" && uri.host == "lingxios.net" && uri.path in setOf("/open/team/join", "/teams/join")))
            val pairs = requireNotNull(uri.rawQuery).split('&')
            require(pairs.size == 1)
            val parts = pairs.single().split('=', limit = 2)
            require(parts.size == 2 && URLDecoder.decode(parts[0], "UTF-8") == "code")
            URLDecoder.decode(parts[1], "UTF-8").also { require(pattern.matches(it)) }
        }.getOrNull()
    }
    fun link(code: String): String { require(pattern.matches(code)); return "https://lingxios.net/open/team/join?code=$code" }
}
