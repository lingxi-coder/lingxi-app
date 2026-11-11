package com.lingxi.code.friends

import com.lingxi.code.teams.TeamLastMessage
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.Locale

/**
 * The shared Friends test vectors (`packages/teams/fixtures/friends-vectors.json`, docs/FRIENDS-CLIENTS.md section 5) and the adapters from
 * their row shapes to the app types. A row that fails here and not on another client is a parity bug in this platform, never a reason to
 * edit the file. The file is found by walking up from the working directory, as `TeamBotsTest` and `FriendLinkTest` do.
 */
internal object FriendsVectors {
    val root: JSONObject by lazy {
        val file = generateSequence(File(checkNotNull(System.getProperty("user.dir")))) { it.parentFile }
            .map { File(it, "packages/teams/fixtures/friends-vectors.json") }.first { it.isFile }
        JSONObject(file.readText())
    }

    fun section(vararg path: String): Any = path.fold(root as Any) { current, key ->
        checkNotNull((current as? JSONObject)?.opt(key)) { "friends vectors: ${path.joinToString(".")} has no '$key'" }
    }

    fun obj(vararg path: String): JSONObject = section(*path) as JSONObject

    fun rows(vararg path: String): List<JSONObject> = (section(*path) as JSONArray).let { array -> (0 until array.length()).map { array.getJSONObject(it) } }

    fun strings(array: JSONArray?): List<String> = array?.let { (0 until it.length()).map(it::getString) }.orEmpty()

    /** The five UI languages of the app, the locales the sort rows were checked with. */
    val locales: List<Locale> = listOf("zh-Hans", "zh-Hant", "en", "ja", "ko").map(Locale::forLanguageTag)

    /** A card from a sort, filter or badge row. `since` and `lastAtMs` are epoch milliseconds; `lastAtMs` makes a pointer. */
    fun card(row: JSONObject, state: String? = null): FriendCard {
        val uid = row.optString("uid", "u")
        val kind = checkNotNull(FriendState.of(state ?: row.getString("state"))) { "state of $row" }
        val pointer = if (row.has("lastAtMs")) TeamLastMessage("m", 1, row.getLong("lastAtMs"), "user", "x", null, "hi") else null
        return FriendCard(
            uid, kind, row.optString("displayName", ""), row.optString("handle", "").ifEmpty { null }, null, row.optLong("since", 0),
            if (kind == FriendState.Friends) PairRef("f_$uid", 1) else null, row.optLong("lastSeq", 0), row.optLong("lastReadSeq", 0), pointer,
        )
    }
}
