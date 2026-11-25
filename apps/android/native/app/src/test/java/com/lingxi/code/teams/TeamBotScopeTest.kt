package com.lingxi.code.teams

import com.lingxi.code.R
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.io.File

/**
 * What a shared Bot may use of its maintainer's computer in a Chat (Bot v3 B4c): the effective-scope cases that Web, Android and iOS share
 * (`packages/teams/contracts/bot-scope-vectors.json`), the commands the service takes (`execution-vectors.json`, also run by [TeamBotsTest]), and what is
 * native to this reader: a card and a Chat as the service writes them, what a person's ticks say, who can limit which Bots, and what is sent.
 */
class TeamBotScopeTest {
    private val calendar = """{"id":"calendar","name":"Calendar"}"""
    private val notes = """{"id":"notes","name":"Notes"}"""
    private val orders = """{"source":"orders","dataset":"orders-2026","write":true,"reportReview":false}"""
    private val leads = """{"source":"orders","dataset":"leads","write":false,"reportReview":false}"""
    private val both = """{"localApps":[$calendar,$notes],"datasets":[$orders,$leads]}"""

    private fun published(json: String) = TeamBotScope.parse(JSONObject(json))
    private fun card(id: String = "sharedbot_01", scope: String? = null) = TeamBotCard.parse(JSONObject(
        """{"sharedBotId":"$id","name":"Helper","description":"","maintainer":{"uid":"owner","generation":3},"revision":2,"state":"published","permitted":true,"ready":true,"onlineUntil":0,"usePolicy":{"mode":"all"},"quotas":{"team":1,"member":1,"bot":1}${scope?.let { ""","scope":$it""" } ?: ""}}"""))
    private fun chat(kind: String = "group", participants: List<String> = listOf("user:alice", "user:bob", "shared_bot:sharedbot_01"), botScopes: Any? = null) =
        TeamMessageConversation.fromDocument("chat_0001", buildMap<String, Any?> {
            put("kind", kind); put("title", "Trip"); put("participants", participants); put("lastSeq", 3L); put("lastChangeSeq", 3L); put("lastMessage", null)
            if (botScopes != null) put("botScopes", botScopes)
        }, 0L) { null }
    private fun entry(revision: Long, vararg fields: Pair<String, Any?>): Map<String, Any?> = mapOf("revision" to revision) + fields.toMap()
    private fun root(name: String) = generateSequence(File(System.getProperty("user.dir"))) { it.parentFile }.first { File(it, "packages/teams/contracts/$name").isFile }
    private fun json(name: String) = JSONObject(File(root(name), "packages/teams/contracts/$name").readText())

    /** A parsed JSON value as the plain maps and lists Firestore hands over. */
    private fun plain(value: Any?): Any? = when {
        value === JSONObject.NULL || value == null -> null
        value is JSONObject -> value.keys().asSequence().associateWith { plain(value.get(it)) }
        value is JSONArray -> (0 until value.length()).map { plain(value.get(it)) }
        else -> value
    }

    // The cases the platforms share ---------------------------------------------------------------------------------

    @Test fun theEffectiveScopeVectors() {
        val rows = json("bot-scope-vectors.json").getJSONArray("cases").objects()
        assertTrue(rows.size >= 8)
        rows.forEach { row ->
            val have = row.optJSONObject("published")?.let(TeamBotScope::parse)
            val narrowing = row.optJSONObject("narrowing")?.let { TeamBotScopeRules.decode(mapOf("bot" to plain(it)))?.get("bot") }
            assertEquals(row.getString("name"), TeamBotScope.parse(row.getJSONObject("effective")), TeamBotScopeRules.effective(have, narrowing?.narrowing))
        }
    }

    @Test fun theScopeCommandVectorsAreTheServicesVerdicts() {
        val rows = json("execution-vectors.json").getJSONArray("commands").objects().filter { it.getJSONObject("input").optString("action") == "setConversationBotScope" }
        assertTrue(rows.size >= 8)
        assertTrue(rows.any { it.getBoolean("valid") } && rows.any { !it.getBoolean("valid") })
        rows.forEach { row ->
            assertEquals(row.getString("name"), row.getBoolean("valid"), runCatching { TeamExecutionContract.validate(row.getJSONObject("input")) }.isSuccess)
        }
    }

    // What the service writes -----------------------------------------------------------------------------------------

    @Test fun aCardCarriesWhatItsMaintainerPublished() {
        assertNull("a card has a scope only when it is not empty", card().scope)
        val shared = card(scope = both).scope!!
        assertEquals(listOf("Calendar", "Notes"), shared.localApps.map { it.name })
        assertEquals(listOf(TeamBotScopeDataset("orders", "orders-2026", write = true, reportReview = false), TeamBotScopeDataset("orders", "leads", write = false, reportReview = false)), shared.datasets)
        // A list that is left out is empty; a scope with neither list is the empty scope.
        assertEquals(emptyList<TeamBotScopeDataset>(), card(scope = """{"localApps":[$notes]}""").scope?.datasets)
        assertEquals(emptyList<TeamBotScopeApp>(), card(scope = """{"datasets":[$leads]}""").scope?.localApps)
        assertEquals(TeamBotScope.EMPTY, card(scope = "{}").scope)
        // A list that is there and unreadable breaks the card, as any other field of it does.
        assertTrue(runCatching { card(scope = """{"localApps":"calendar"}""") }.isFailure)
    }

    @Test fun aChatCarriesWhatItNarrowed() {
        assertNull("a Chat that never narrowed has none", chat().botScopes)
        assertNull(chat(botScopes = emptyMap<String, Any?>()).botScopes)
        assertNull("what is not a map is no narrowing, and not a reason to hide the Chat", chat(botScopes = "narrowed").botScopes)
        val held = chat(botScopes = mapOf(
            "sharedbot_01" to entry(2, "localApps" to listOf("notes"), "datasets" to listOf(mapOf("source" to "orders", "dataset" to "leads"))),
            "sharedbot_02" to entry(5),
            "sharedbot_03" to entry(1, "localApps" to emptyList<String>(), "datasets" to emptyList<Map<String, String>>()),
        )).botScopes!!
        assertEquals(TeamBotScopeEntry(2, listOf("notes"), listOf(TeamBotScopeDatasetRef("orders", "leads"))), held["sharedbot_01"])
        assertEquals("a limit that was lifted keeps only its revision", TeamBotScopeEntry(5), held["sharedbot_02"])
        assertEquals("an empty list is not a list that is left out", TeamBotScopeEntry(1, emptyList(), emptyList()), held["sharedbot_03"])
        assertTrue(TeamBotScopeRules.limits(held["sharedbot_01"])); assertTrue(TeamBotScopeRules.limits(held["sharedbot_03"]))
        assertFalse(TeamBotScopeRules.limits(held["sharedbot_02"])); assertFalse(TeamBotScopeRules.limits(null))
    }

    @Test fun anEntryThatIsNotWellFormedIsLeftOut() {
        val read = chat(botScopes = mapOf(
            "revision_zero" to entry(0, "localApps" to listOf("notes")),
            "no_revision" to mapOf("localApps" to listOf("notes")),
            "fraction" to mapOf("revision" to 1.5),
            "flag" to mapOf("revision" to true),
            "text_revision" to mapOf("revision" to "1"),
            "apps_not_a_list" to entry(1, "localApps" to "notes"),
            "apps_null" to entry(1, "localApps" to null),
            "app_not_text" to entry(1, "localApps" to listOf("notes", 4)),
            "dataset_without_id" to entry(1, "datasets" to listOf(mapOf("source" to "orders"))),
            "dataset_not_a_row" to entry(1, "datasets" to listOf("leads")),
            "not_an_entry" to "yes",
            "kept" to entry(1, "stray" to true, "localApps" to listOf("notes")),
            "whole_double" to mapOf("revision" to 3.0),
        )).botScopes!!
        assertEquals(setOf("kept", "whole_double"), read.keys)
        assertEquals("a key the entry has no use for is dropped", TeamBotScopeEntry(1, listOf("notes")), read["kept"])
        assertEquals("a whole number that arrives as a double counts", TeamBotScopeEntry(3), read["whole_double"])
        assertNull("a Chat whose every entry is left out has none", chat(botScopes = mapOf("revision_zero" to entry(0))).botScopes)
    }

    // What the Bot may use here ---------------------------------------------------------------------------------------

    @Test fun whatABotMayUseInAChatIsWhatItPublishedCutDownToWhatTheChatAllows() {
        val sharing = card(scope = both)
        val free = chat()
        assertEquals(published(both), TeamBotScopeRules.effective(sharing, null))
        assertEquals(published(both), TeamBotScopeRules.effective(sharing, free))
        val narrowed = chat(botScopes = mapOf("sharedbot_01" to entry(1, "localApps" to listOf("notes"), "datasets" to listOf(mapOf("source" to "orders", "dataset" to "leads")))))
        val effective = TeamBotScopeRules.effective(sharing, narrowed)
        assertEquals(published("""{"localApps":[$notes],"datasets":[$leads]}"""), effective)
        assertTrue(TeamBotScopeRules.narrowed(sharing.scope, effective))
        assertFalse(TeamBotScopeRules.narrowed(sharing.scope, TeamBotScopeRules.effective(sharing, free)))
        // Another Bot's entry is not this Bot's.
        val other = chat(botScopes = mapOf("sharedbot_02" to entry(1, "localApps" to emptyList<String>())))
        assertEquals(published(both), TeamBotScopeRules.effective(sharing, other))
        // A limit that was lifted limits nothing; one that allows none says that the Bot uses nothing here.
        assertEquals(published(both), TeamBotScopeRules.effective(sharing, chat(botScopes = mapOf("sharedbot_01" to entry(3)))))
        val none = TeamBotScopeRules.effective(sharing, chat(botScopes = mapOf("sharedbot_01" to entry(2, "localApps" to emptyList<String>(), "datasets" to emptyList<Map<String, String>>()))))
        assertTrue(none.isEmpty)
        assertTrue(TeamBotScopeRules.narrowed(sharing.scope, none))
        // A Bot published without a scope has none, whatever the Chat narrowed; and then there is nothing to be limited.
        val plain = card("sharedbot_02")
        val plainEffective = TeamBotScopeRules.effective(plain, other)
        assertTrue(plainEffective.isEmpty)
        assertFalse(TeamBotScopeRules.narrowed(plain.scope, plainEffective))
    }

    // What is ticked --------------------------------------------------------------------------------------------------

    @Test fun whatIsTickedBecomesTheNarrowing() {
        val have = published(both)
        val all = TeamBotScopeRules.choice(have)
        assertEquals(TeamBotScopeChoice(listOf("calendar", "notes"), listOf("orders-2026", "leads")), all)
        assertNull("everything allowed lifts the limit", TeamBotScopeRules.narrowing(all, have))
        assertEquals("a kind with nothing left out is not narrowed, so what is published of it later is allowed too",
            TeamBotScopeNarrowing(listOf("calendar"), null), TeamBotScopeRules.narrowing(TeamBotScopeChoice(listOf("calendar"), all.datasets), have))
        assertEquals(TeamBotScopeNarrowing(null, listOf(TeamBotScopeDatasetRef("orders", "leads"))), TeamBotScopeRules.narrowing(TeamBotScopeChoice(all.localApps, listOf("leads")), have))
        assertEquals("nothing allowed is said as empty lists, not as no narrowing", TeamBotScopeNarrowing(emptyList(), emptyList()), TeamBotScopeRules.narrowing(TeamBotScopeChoice(), have))
        assertEquals("what is not published is never named", TeamBotScopeNarrowing(listOf("notes"), null),
            TeamBotScopeRules.narrowing(TeamBotScopeChoice(listOf("notes", "mail"), listOf("orders-2026", "removed", "leads")), have))
        assertEquals("a kind with nothing published is left out", TeamBotScopeNarrowing(emptyList(), null), TeamBotScopeRules.narrowing(TeamBotScopeChoice(), published("""{"localApps":[$calendar]}""")))
        assertNull(TeamBotScopeRules.narrowing(TeamBotScopeChoice(), TeamBotScope.EMPTY))
        // Whatever is ticked is what the service then lets the Bot use here.
        for (apps in listOf(emptyList(), listOf("calendar"), listOf("notes"), listOf("calendar", "notes"))) for (data in listOf(emptyList(), listOf("orders-2026"), listOf("leads"), listOf("orders-2026", "leads"))) {
            val effective = TeamBotScopeRules.effective(have, TeamBotScopeRules.narrowing(TeamBotScopeChoice(apps, data), have))
            assertEquals("$apps $data", TeamBotScopeChoice(have.localApps.map { it.id }.filter { it in apps }, have.datasets.map { it.dataset }.filter { it in data }), TeamBotScopeRules.choice(effective))
        }
    }

    @Test fun ticksAreToggledWithoutDuplicatesAndComparedWithoutOrder() {
        assertEquals(listOf("a", "b", "c"), TeamBotScopeRules.toggled(listOf("a", "b"), "c", true))
        assertEquals("ticking a ticked box leaves it once", listOf("b", "a"), TeamBotScopeRules.toggled(listOf("a", "b"), "a", true))
        assertEquals(listOf("b"), TeamBotScopeRules.toggled(listOf("a", "b"), "a", false))
        assertEquals(listOf("a"), TeamBotScopeRules.toggled(listOf("a"), "z", false))
        assertTrue(TeamBotScopeRules.same(TeamBotScopeChoice(listOf("a", "b"), listOf("x")), TeamBotScopeChoice(listOf("b", "a"), listOf("x"))))
        assertFalse(TeamBotScopeRules.same(TeamBotScopeChoice(listOf("a"), emptyList()), TeamBotScopeChoice(listOf("a"), listOf("x"))))
        assertFalse(TeamBotScopeRules.same(TeamBotScopeChoice(listOf("a", "a"), emptyList()), TeamBotScopeChoice(listOf("a"), emptyList())))
    }

    @Test fun theTicksStartAgainWhenWhatTheyWereTickedAgainstChanges() {
        val have = published(both)
        val first = TeamBotScopeBasis.of(have, null)
        assertEquals(0L, first.revision)
        assertEquals(TeamBotScopeRules.choice(have), first.allowed)
        assertEquals("nothing moved", first, TeamBotScopeBasis.of(have, null))
        val saved = TeamBotScopeBasis.of(have, TeamBotScopeEntry(1, listOf("notes")))
        assertNotEquals("a save, or someone else's", first, saved)
        assertEquals(TeamBotScopeChoice(listOf("notes"), listOf("orders-2026", "leads")), saved.allowed)
        assertNotEquals("the revision alone moves it", saved, TeamBotScopeBasis.of(have, TeamBotScopeEntry(2, listOf("notes"))))
        assertNotEquals("the maintainer published something else", saved, TeamBotScopeBasis.of(published("""{"localApps":[$calendar,$notes],"datasets":[$leads]}"""), TeamBotScopeEntry(1, listOf("notes"))))
        assertEquals(TeamBotScopeChoice(), TeamBotScopeBasis.of(null, null).allowed)
    }

    // Who can limit what ----------------------------------------------------------------------------------------------

    @Test fun onlyAPersonInABotChatCanLimitTheBotsThatShareSomething() {
        val cards = listOf(card(scope = both), card("sharedbot_02"), card("sharedbot_03", scope = "{}"))
        val all = listOf("user:alice", "user:bob", "shared_bot:sharedbot_01", "shared_bot:sharedbot_02", "shared_bot:sharedbot_03")
        fun limitable(conversation: TeamMessageConversation, uid: String = "alice") = TeamBotScopeRules.limitable(conversation, cards, uid).map { it.sharedBotId }
        assertEquals("a Bot that shares nothing has nothing to limit", listOf("sharedbot_01"), limitable(chat(participants = all)))
        assertEquals(listOf("sharedbot_01"), limitable(chat(kind = "channel", participants = all)))
        assertEquals(listOf("sharedbot_01"), limitable(chat(kind = "bot_dm", participants = listOf("user:alice", "shared_bot:sharedbot_01"))))
        assertEquals("only a person who takes part in the Chat limits it", emptyList<String>(), limitable(chat(participants = all), "carol"))
        assertEquals("a Bot that is not in the Chat is not limited by it", emptyList<String>(), limitable(chat(participants = listOf("user:alice", "user:bob", "shared_bot:sharedbot_02"))))
        assertEquals("a chat with no Bots", emptyList<String>(), limitable(chat(kind = "dm", participants = listOf("user:alice", "user:bob"))))
        assertTrue(TeamBotScopeRules.inBotChat(chat(), "bob"))
        assertFalse(TeamBotScopeRules.inBotChat(chat(kind = "dm", participants = listOf("user:alice", "shared_bot:sharedbot_01")), "alice"))
    }

    // What is sent ----------------------------------------------------------------------------------------------------

    private fun command(payload: JSONObject) = JSONObject().put("schemaVersion", 1).put("commandId", "command_0001").put("teamId", "team_0001").put("generation", 7)
        .put("action", "setConversationBotScope").put("payload", payload)
    private fun send(narrowing: TeamBotScopeNarrowing?, expectedRevision: Long = 0) = command(JSONObject().put("conversationId", "chat_0001").put("sharedBotId", "sharedbot_01")
        .put("expectedRevision", expectedRevision).put("scope", narrowing?.json() ?: JSONObject.NULL))

    @Test fun whatTheEditorSendsIsWhatTheServiceAccepts() {
        TeamExecutionContract.validate(send(null))
        TeamExecutionContract.validate(send(TeamBotScopeNarrowing(emptyList(), emptyList()), 3))
        TeamExecutionContract.validate(send(TeamBotScopeNarrowing(listOf("calendar", "mail.app:1"), null), 1))
        TeamExecutionContract.validate(send(TeamBotScopeNarrowing(null, listOf(TeamBotScopeDatasetRef("orders", "leads")))))
        assertEquals(0, TeamBotScopeNarrowing(null, null).json().length())
        val scope = TeamBotScopeNarrowing(listOf("calendar"), listOf(TeamBotScopeDatasetRef("orders", "leads"))).json()
        assertEquals(setOf("localApps", "datasets"), scope.keys().asSequence().toSet())
        assertEquals("calendar", scope.getJSONArray("localApps").getString(0))
        assertEquals(setOf("source", "dataset"), scope.getJSONArray("datasets").getJSONObject(0).keys().asSequence().toSet())
        assertEquals("leads", scope.getJSONArray("datasets").getJSONObject(0).getString("dataset"))
        // The lifted narrowing is an explicit null, never a key that is left out.
        assertTrue(send(null).getJSONObject("payload").has("scope"))
        assertSame(JSONObject.NULL, send(null).getJSONObject("payload").get("scope"))
    }

    @Test fun theCommandIsRefusedLikeTheServiceRefusesIt() {
        fun refused(why: String, scope: String?, expectedRevision: String = "0", sharedBotId: String = "sharedbot_01", extra: String = "") {
            val payload = """{"conversationId":"chat_0001","sharedBotId":"$sharedBotId","expectedRevision":$expectedRevision${scope?.let { ""","scope":$it""" } ?: ""}$extra}"""
            assertTrue(why, runCatching { TeamExecutionContract.validate(command(JSONObject(payload))) }.isFailure)
        }
        refused("a scope that is left out is not a lifted narrowing", null)
        refused("a key it does not know", "null", extra = ""","extra":1""")
        refused("a revision below zero", "null", expectedRevision = "-1")
        refused("a revision that is not whole", "null", expectedRevision = "1.5")
        refused("no Bot", "null", sharedBotId = "")
        refused("a Local App twice", """{"localApps":["a","a"]}""")
        refused("a Local App id with a character the service does not take", """{"localApps":["has space"]}""")
        refused("more than 64 Local Apps", """{"localApps":[${(0..64).joinToString(",") { "\"app$it\"" }}]}""")
        refused("a dataset twice, whatever its source", """{"datasets":[{"source":"orders","dataset":"a"},{"source":"other","dataset":"a"}]}""")
        refused("flags the Chat cannot set", """{"datasets":[{"source":"orders","dataset":"a","write":true}]}""")
        refused("a dataset without its id", """{"datasets":[{"source":"orders"}]}""")
        refused("a dataset that is not a row", """{"datasets":["a"]}""")
        refused("a kind it does not know", """{"secrets":["a"]}""")
        refused("a scope that is not a narrowing", "\"all\"")
        refused("a list that is not a list", """{"localApps":"calendar"}""")
        // And what it does accept at its limits: 64 Local Apps, 64 datasets.
        TeamExecutionContract.validate(command(JSONObject("""{"conversationId":"chat_0001","sharedBotId":"sharedbot_01","expectedRevision":0,"scope":{"localApps":[${(0 until 64).joinToString(",") { "\"app$it\"" }}],"datasets":[${(0 until 64).joinToString(",") { """{"source":"orders","dataset":"data$it"}""" }}]}}""")))
    }

    @Test fun theFailuresOfAScopeChangeAreToldAsTheyAre() {
        assertEquals(R.string.teams_error_scope_conflict, teamsErrorRes("BOT_SCOPE_REVISION_CONFLICT"))
        assertEquals(R.string.teams_error_scope_not_published, teamsErrorRes("SCOPE_NOT_PUBLISHED"))
        assertEquals(R.string.teams_error, teamsErrorRes("SOMETHING_NEW"))
    }
}
