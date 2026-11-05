package com.lingxi.code.bots

import com.lingxi.code.bindings.android.RemoteBotStatusFfi
import com.lingxi.code.bindings.android.RemoteBotSummaryFfi
import com.lingxi.code.bindings.android.RemoteBotsStateFfi
import com.lingxi.code.bindings.android.RemoteConnectionPhaseFfi
import com.lingxi.code.bindings.android.RemoteConnectionStateFfi
import com.lingxi.code.bindings.android.RemoteHostFfi
import com.lingxi.code.bindings.android.RemotePendingActionFfi
import com.lingxi.code.bindings.android.RemoteSessionStatusFfi
import com.lingxi.code.bindings.android.RemoteSessionSummaryFfi
import com.lingxi.code.remote.RemoteBotChatsState
import com.lingxi.code.remote.RemoteBotPageUi
import com.lingxi.code.remote.RemoteBotsState
import com.lingxi.code.remote.RemoteConnectionUi
import com.lingxi.code.remote.RemoteHostsState
import com.lingxi.code.remote.remoteBotPageUi
import com.lingxi.code.remote.remoteConnectionUi
import com.lingxi.code.tabs.TabLogic

/*
 * The Bots tab's rules (SPEC-mobile-team-bot-ui.md sections 4 and 8.6) as plain functions: no Compose, no Android,
 * no clock, so every state of section 4.5 and every cell of section 4.3 is a JVM test. The shared tables are in
 * `packages/bridge-client/fixtures/mobile-team-bot-vectors.json` (`mobile.chatWindow` ... `mobile.autoConnectTarget`).
 */

// --- what a row shows --------------------------------------------------------------------------------------------

/** One Bot as the tree shows it, whether it came live from the connected computer or from a saved snapshot. */
internal data class BotItem(
    val botId: String,
    val name: String,
    val description: String = "",
    val status: RemoteBotStatusFfi = RemoteBotStatusFfi.IDLE,
    val waitingThreads: Int = 0,
    val lastActivityAtMs: Long? = null,
    /** The chats saved with the Bot (host order, newest first); null = never listed. Live Bots carry none: their chats are the manager's. */
    val chats: List<ChatItem>? = null,
)

/** One chat of a Bot: the host sends no time, so a snapshot keeps the host's order and nothing else. */
internal data class ChatItem(
    val sessionId: String,
    val title: String,
    val status: RemoteSessionStatusFfi = RemoteSessionStatusFfi.IDLE,
    val pendingAction: RemotePendingActionFfi? = null,
)

/** What was last seen of one computer's Bots (section 4.2). Written only for the signed-in [uid] and never read for another. */
internal data class BotsSnapshot(val uid: String, val hostId: String, val savedAtMs: Long, val bots: List<BotItem>)

internal fun RemoteBotSummaryFfi.toItem() = BotItem(
    botId = botId, name = name, description = description, status = status,
    waitingThreads = waitingThreads.toInt().coerceAtLeast(0), lastActivityAtMs = lastActivityAtMs?.toLong(),
)

internal fun RemoteSessionSummaryFfi.toItem() = ChatItem(sessionId, title, status, pendingAction)

/** What the connected computer has said of its Bots: nothing yet, "too old to know Bots", or the list. */
internal sealed interface LiveBots {
    data object Unknown : LiveBots
    data object Unsupported : LiveBots
    data class Count(val n: Int) : LiveBots
}

/** The Bots [hostId] delivered, or [LiveBots.Unknown] while the held state is another computer's. */
internal fun RemoteBotsState.liveFor(hostId: String): LiveBots =
    if (this.hostId != hostId) LiveBots.Unknown
    else when (val held = bots) {
        RemoteBotsStateFfi.Unknown -> LiveBots.Unknown
        RemoteBotsStateFfi.Unsupported -> LiveBots.Unsupported
        is RemoteBotsStateFfi.Bots -> LiveBots.Count(held.bots.size)
    }

/** The live Bots of [hostId] as rows, in the computer's order; empty unless the computer delivered a list. */
internal fun RemoteBotsState.itemsFor(hostId: String): List<BotItem> =
    if (this.hostId != hostId) emptyList() else (bots as? RemoteBotsStateFfi.Bots)?.bots.orEmpty().map { it.toItem() }

// --- the pure functions of section 8.6 ---------------------------------------------------------------------------

internal object BotsLogic {
    /** Chats shown per Bot before "Show N more" (section 11). */
    const val FIRST_CHATS = 5

    /** Caps of a snapshot (section 4.2): computers per user, Bots per computer, chats per Bot. */
    const val MAX_HOSTS = 20
    const val MAX_BOTS = 100
    const val MAX_CHATS = 50

    // chatWindow ----------------------------------------------------------------------------------------------------

    data class ChatWindow(val shown: Int, val hidden: Int, val showMore: Boolean, val showLess: Boolean)

    /** How many of [total] chats a Bot shows: the first [first] until the user asks for all, and the rows that ask. */
    fun chatWindow(total: Int, expanded: Boolean, first: Int = FIRST_CHATS): ChatWindow {
        val count = total.coerceAtLeast(0)
        return if (expanded) ChatWindow(shown = count, hidden = 0, showMore = false, showLess = count > first)
        else {
            val shown = minOf(count, first)
            ChatWindow(shown = shown, hidden = count - shown, showMore = count > shown, showLess = false)
        }
    }

    // sorting -------------------------------------------------------------------------------------------------------

    /** Latest activity first (none counts as 0), then the name by number-aware, case-insensitive order, then the id. */
    fun sortBots(bots: List<BotItem>): List<BotItem> =
        bots.sortedWith(
            compareByDescending<BotItem> { it.lastActivityAtMs ?: 0L }
                .thenComparator { a, b -> compareNames(a.name, b.name) }
                .thenBy { it.botId },
        )

    /** The connected computer first, then the online ones, then the one seen last; ties by name and id. */
    fun sortHosts(hosts: List<RemoteHostFfi>, connectedHostId: String?): List<RemoteHostFfi> =
        hosts.sortedWith(
            compareByDescending<RemoteHostFfi> { it.hostId == connectedHostId }
                .thenByDescending { it.online }
                .thenByDescending { it.lastSeenMs }
                .thenComparator { a, b -> compareNames(a.name, b.name) }
                .thenBy { it.hostId },
        )

    /** "beta 2" before "beta 10", "alpha" equal to "Alpha": runs of digits compare as numbers, the rest ignoring case. */
    internal fun compareNames(left: String, right: String): Int {
        var i = 0
        var j = 0
        while (i < left.length && j < right.length) {
            val a = left[i]
            val b = right[j]
            if (a in '0'..'9' && b in '0'..'9') {
                var iEnd = i
                while (iEnd < left.length && left[iEnd] in '0'..'9') iEnd++
                var jEnd = j
                while (jEnd < right.length && right[jEnd] in '0'..'9') jEnd++
                val x = left.substring(i, iEnd).trimStart('0')
                val y = right.substring(j, jEnd).trimStart('0')
                val byLength = x.length.compareTo(y.length)
                if (byLength != 0) return byLength
                val byValue = x.compareTo(y)
                if (byValue != 0) return byValue
                i = iEnd
                j = jEnd
            } else {
                val byChar = a.lowercaseChar().compareTo(b.lowercaseChar())
                if (byChar != 0) return byChar
                i++
                j++
            }
        }
        return (left.length - i).compareTo(right.length - j)
    }

    // hostBody ------------------------------------------------------------------------------------------------------

    /**
     * What one computer's section shows under its header (section 4.2 and the table of section 8.6). A computer
     * that is connected shows what it delivered, whatever presence says; any other shows its snapshot, dimmed and
     * only as actionable as the computer is online; a computer that is not paired has no usable snapshot.
     */
    fun hostBody(connected: Boolean, online: Boolean, paired: Boolean, live: LiveBots, snapshotBots: Int?): HostBody {
        if (connected) {
            val kind = when (live) {
                LiveBots.Unknown -> BodyKind.Loading
                LiveBots.Unsupported -> BodyKind.Unsupported
                is LiveBots.Count -> if (live.n == 0) BodyKind.NoBots else BodyKind.Bots
            }
            return HostBody(kind, BodySource.Live, dimmed = false, actionable = true, notes = emptyList())
        }
        if (paired && snapshotBots != null) {
            val rows = snapshotBots > 0
            return HostBody(
                kind = if (rows) BodyKind.Bots else BodyKind.NoBots,
                source = BodySource.Cached,
                dimmed = rows,
                actionable = online,
                notes = buildList {
                    add(BodyNote.SavedNote)
                    if (rows && !online) add(BodyNote.OfflineChatsNote)
                },
            )
        }
        return when {
            !online -> HostBody(BodyKind.OfflineNoSnapshot, BodySource.None, dimmed = false, actionable = false, notes = emptyList())
            !paired -> HostBody(BodyKind.NotPaired, BodySource.None, dimmed = false, actionable = true, notes = emptyList())
            else -> HostBody(BodyKind.ConnectToLoad, BodySource.None, dimmed = false, actionable = true, notes = emptyList())
        }
    }

    // the connection ------------------------------------------------------------------------------------------------

    /**
     * The computer the client is connected to: ready, or reconnecting a link that was ready, or dialling again after
     * having been. A first dial, a pairing that waits for the computer, a failure and an ended connection are
     * not "connected": the rows of such a computer are its saved ones.
     */
    fun connectedHostId(connection: RemoteConnectionStateFfi?): String? = connection?.takeIf {
        when (it.phase) {
            RemoteConnectionPhaseFfi.READY, RemoteConnectionPhaseFfi.RECONNECTING -> true
            RemoteConnectionPhaseFfi.CONNECTING -> it.everReady
            RemoteConnectionPhaseFfi.IDLE, RemoteConnectionPhaseFfi.PAIRING, RemoteConnectionPhaseFfi.FAILED -> false
        }
    }?.hostId

    /** Chats waiting for the user on the connected computer: the tab's badge (section 2.5). */
    fun waitingChats(connection: RemoteConnectionStateFfi?, bots: RemoteBotsState): Int {
        val hostId = connectedHostId(connection) ?: return 0
        val live = (bots.takeIf { it.hostId == hostId }?.bots as? RemoteBotsStateFfi.Bots)?.bots ?: return 0
        return TabLogic.botsBadgeCount(live.map { it.waitingThreads.toInt() })
    }

    /**
     * The computer to connect to when the tab is shown, or null to connect nothing: only the last-used one, only when
     * it is paired and online, only when signed in and nothing is connected (section 4.4).
     */
    fun autoConnectTarget(signedIn: Boolean, connectedHostId: String?, lastHostId: String?, hosts: List<RemoteHostFfi>): String? {
        if (!signedIn || connectedHostId != null || lastHostId == null) return null
        return hosts.firstOrNull { it.hostId == lastHostId && it.online && it.paired }?.hostId
    }

    // snapshots -----------------------------------------------------------------------------------------------------

    /**
     * The live Bot list replaces the saved one (first [MAX_BOTS] in the host's order); a Bot that stays keeps its saved
     * chats and a new one has none. The save time is [nowMs].
     */
    fun applyBots(prior: BotsSnapshot?, uid: String, hostId: String, live: List<BotItem>, nowMs: Long): BotsSnapshot {
        val kept = prior?.bots.orEmpty().associateBy { it.botId }
        return BotsSnapshot(
            uid = prior?.uid ?: uid,
            hostId = hostId,
            savedAtMs = nowMs,
            bots = live.take(MAX_BOTS).map { it.copy(chats = kept[it.botId]?.chats) },
        )
    }

    /**
     * One Bot's chats replace the saved ones (first [MAX_CHATS], the host's order) and the save time moves. Chats of a
     * Bot the snapshot does not list change nothing, and chats alone never create a snapshot.
     */
    fun applyChats(prior: BotsSnapshot?, botId: String, chats: List<ChatItem>, nowMs: Long): BotsSnapshot? {
        if (prior == null || prior.bots.none { it.botId == botId }) return prior
        return prior.copy(
            savedAtMs = nowMs,
            bots = prior.bots.map { if (it.botId == botId) it.copy(chats = chats.take(MAX_CHATS)) else it },
        )
    }

    /** The [MAX_HOSTS] computers saved last, newest first. */
    fun evictHosts(snapshots: List<BotsSnapshot>): List<BotsSnapshot> =
        snapshots.sortedByDescending { it.savedAtMs }.take(MAX_HOSTS)

    // the tab's states ----------------------------------------------------------------------------------------------

    /** Which of the tab's whole-page states applies (section 4.5), or the tree. */
    fun rootState(signedIn: Boolean, hosts: RemoteHostsState): RootState = when {
        !signedIn -> RootState.SignedOut
        hosts.error != null && hosts.hosts.isEmpty() -> RootState.Error
        !hosts.loaded -> RootState.Loading
        hosts.hosts.isEmpty() -> RootState.NoComputers
        else -> RootState.Tree
    }

    // gestures ------------------------------------------------------------------------------------------------------

    /** Tapping a Bot row toggles it; expanding a Bot of a computer that is not connected also dials it (section 4.3). */
    fun botTapPlan(facts: HostFacts, wasExpanded: Boolean): BotTapPlan {
        val expand = !wasExpanded
        return BotTapPlan(expanded = expand, connect = expand && !facts.connected && facts.online && facts.paired)
    }

    /** What tapping a chat does. */
    fun openChatPlan(facts: HostFacts): OpenChatPlan = when {
        facts.connected -> OpenChatPlan.OpenNow
        !facts.online -> OpenChatPlan.NoAnswer
        !facts.paired -> OpenChatPlan.Pair
        else -> OpenChatPlan.ConnectThenOpen
    }

    /** What tapping "New chat" does. */
    fun newChatPlan(facts: HostFacts): NewChatPlan = when {
        facts.connected -> NewChatPlan.CreateNow
        facts.online && facts.paired -> NewChatPlan.ConnectThenCreate
        else -> NewChatPlan.Disabled
    }

    /** The Bot the tab expands when the user never chose: the first of the connected computer (section 4.7), as `hostId/botId`. */
    fun defaultExpanded(connectedHostId: String?, bots: RemoteBotsState): Set<String> {
        val hostId = connectedHostId ?: return emptySet()
        val first = sortBots(bots.itemsFor(hostId)).firstOrNull() ?: return emptySet()
        return setOf(botKey(hostId, first.botId))
    }

    fun botKey(hostId: String, botId: String) = "$hostId/$botId"
}

// --- the vocabulary of hostBody and of the gestures ----------------------------------------------------------------

internal enum class BodyKind(val key: String) {
    Loading("loading"),
    Unsupported("unsupported"),
    NoBots("noBots"),
    Bots("bots"),
    ConnectToLoad("connectToLoad"),
    OfflineNoSnapshot("offlineNoSnapshot"),
    NotPaired("notPaired"),
}

internal enum class BodySource(val key: String) { Live("live"), Cached("cached"), None("none") }

internal enum class BodyNote(val key: String) { SavedNote("savedNote"), OfflineChatsNote("offlineChatsNote") }

internal data class HostBody(
    val kind: BodyKind,
    val source: BodySource,
    val dimmed: Boolean,
    val actionable: Boolean,
    val notes: List<BodyNote>,
)

internal enum class RootState { SignedOut, Loading, Error, NoComputers, Tree }

/** What the gestures of section 4.3 need to know of a computer. */
internal data class HostFacts(val hostId: String, val connected: Boolean, val online: Boolean, val paired: Boolean)

internal data class BotTapPlan(val expanded: Boolean, val connect: Boolean)

internal enum class OpenChatPlan { OpenNow, ConnectThenOpen, NoAnswer, Pair }

internal enum class NewChatPlan { CreateNow, ConnectThenCreate, Disabled }

// --- the tree ---------------------------------------------------------------------------------------------------------

/** Everything the tree is made of; [buildTree] turns it into rows and decides nothing else. */
internal data class TreeInput(
    val hosts: List<RemoteHostFfi>,
    val connection: RemoteConnectionStateFfi?,
    val bots: RemoteBotsState,
    val chats: RemoteBotChatsState,
    val snapshots: Map<String, BotsSnapshot>,
    val collapsedHosts: Set<String>,
    /** `hostId/botId` of the expanded Bots; null = the user never chose ([BotsLogic.defaultExpanded]). */
    val expandedBots: Set<String>?,
    /** `hostId/botId` of the Bots that show every chat, not only the first few. */
    val showAll: Set<String>,
)

/** The state of an expanded Bot's body besides its chats. */
internal enum class BotBodyState { ChatsLoading, ChatsFailed, NoChats, Gone }

internal sealed interface TreeRow {
    val key: String

    data class HostHeader(val host: RemoteHostFfi, val connected: Boolean, val collapsed: Boolean) : TreeRow {
        override val key get() = "h/${host.hostId}"
    }

    /** The connection to this computer is not ready: dialling, waiting for the computer's approval, dropped, or failed. */
    data class Connection(val hostId: String, val hostName: String, val ui: RemoteConnectionUi) : TreeRow {
        override val key get() = "hc/$hostId"
    }

    data class Note(val hostId: String, val note: BodyNote, val savedAtMs: Long?) : TreeRow {
        override val key get() = "hn/$hostId/${note.key}"
    }

    /** A section with no Bots to list: loading, unsupported, none, connect to load, offline, not paired. */
    data class State(val hostId: String, val kind: BodyKind, val facts: HostFacts) : TreeRow {
        override val key get() = "hs/$hostId"
    }

    data class Bot(
        val facts: HostFacts,
        val bot: BotItem,
        val expanded: Boolean,
        val dimmed: Boolean,
    ) : TreeRow {
        override val key get() = "b/${facts.hostId}/${bot.botId}"
    }

    data class NewChat(val facts: HostFacts, val botId: String, val enabled: Boolean) : TreeRow {
        override val key get() = "n/${facts.hostId}/$botId"
    }

    data class BotState(val hostId: String, val botId: String, val state: BotBodyState) : TreeRow {
        override val key get() = "bs/$hostId/$botId"
    }

    data class Chat(val facts: HostFacts, val botId: String, val chat: ChatItem, val dimmed: Boolean) : TreeRow {
        override val key get() = "c/${facts.hostId}/$botId/${chat.sessionId}"
    }

    data class ShowMore(val hostId: String, val botId: String, val hidden: Int) : TreeRow {
        override val key get() = "m/$hostId/$botId"
    }

    data class ShowLess(val hostId: String, val botId: String) : TreeRow {
        override val key get() = "l/$hostId/$botId"
    }
}

/** The rows of the tree, top to bottom (section 4.1): every computer, then what is under it unless it is collapsed. */
internal fun buildTree(input: TreeInput): List<TreeRow> {
    val connectedId = BotsLogic.connectedHostId(input.connection)
    val expandedBots = input.expandedBots ?: BotsLogic.defaultExpanded(connectedId, input.bots)
    return buildList {
        // A list the computer or the presence sends twice must not make two rows with one key.
        for (host in BotsLogic.sortHosts(input.hosts.distinctBy { it.hostId }, connectedId)) {
            val connected = host.hostId == connectedId
            val collapsed = host.hostId in input.collapsedHosts
            add(TreeRow.HostHeader(host, connected, collapsed))
            if (collapsed) continue
            val facts = HostFacts(host.hostId, connected, host.online, host.paired)
            val ui = remoteConnectionUi(input.connection?.takeIf { it.hostId == host.hostId })
            if (ui !is RemoteConnectionUi.Idle && ui !is RemoteConnectionUi.Ready) add(TreeRow.Connection(host.hostId, host.name, ui))
            val snapshot = input.snapshots[host.hostId]
            val body = BotsLogic.hostBody(connected, host.online, host.paired, input.bots.liveFor(host.hostId), snapshot?.bots?.size)
            if (body.source == BodySource.Cached) body.notes.forEach { add(TreeRow.Note(host.hostId, it, snapshot?.savedAtMs)) }
            if (body.kind != BodyKind.Bots) {
                add(TreeRow.State(host.hostId, body.kind, facts))
                continue
            }
            val bots = if (body.source == BodySource.Live) input.bots.itemsFor(host.hostId) else snapshot?.bots.orEmpty()
            for (bot in BotsLogic.sortBots(bots.distinctBy { it.botId })) {
                val expanded = BotsLogic.botKey(host.hostId, bot.botId) in expandedBots
                add(TreeRow.Bot(facts, bot, expanded, dimmed = body.dimmed))
                if (expanded) addBotBody(facts, bot, body, input)
            }
        }
    }
}

private fun MutableList<TreeRow>.addBotBody(facts: HostFacts, bot: BotItem, body: HostBody, input: TreeInput) {
    val hostId = facts.hostId
    val showAll = BotsLogic.botKey(hostId, bot.botId) in input.showAll
    val cached = input.snapshots[hostId]?.bots?.firstOrNull { it.botId == bot.botId }?.chats
    val chats: List<ChatItem>
    var dimmed = body.dimmed
    var state: BotBodyState? = null
    var newChat = true
    if (body.source == BodySource.Live) {
        when (val page = remoteBotPageUi(hostId, bot.botId, input.bots, input.chats)) {
            RemoteBotPageUi.Loading -> { chats = emptyList(); state = BotBodyState.ChatsLoading }
            RemoteBotPageUi.Gone -> { chats = emptyList(); state = BotBodyState.Gone; newChat = false }
            is RemoteBotPageUi.Shown -> {
                val live = page.chats
                when {
                    live.threads != null -> {
                        chats = live.threads.map { it.toItem() }
                        if (live.failed) state = BotBodyState.ChatsFailed else if (live.empty) state = BotBodyState.NoChats
                    }
                    live.failed -> { chats = emptyList(); state = BotBodyState.ChatsFailed }
                    // Not listed yet: what was saved stays until the live list replaces it.
                    !cached.isNullOrEmpty() -> { chats = cached; dimmed = true }
                    else -> { chats = emptyList(); state = BotBodyState.ChatsLoading }
                }
            }
        }
    } else {
        chats = cached.orEmpty()
        if (cached == null) { if (facts.online) state = BotBodyState.ChatsLoading } else if (cached.isEmpty()) state = BotBodyState.NoChats
    }
    if (newChat) {
        val plan = BotsLogic.newChatPlan(facts)
        val ready = !facts.connected || input.connection?.phase == RemoteConnectionPhaseFfi.READY
        add(TreeRow.NewChat(facts, bot.botId, enabled = plan != NewChatPlan.Disabled && ready))
    }
    state?.let { add(TreeRow.BotState(hostId, bot.botId, it)) }
    val listed = chats.distinctBy { it.sessionId }
    val window = BotsLogic.chatWindow(listed.size, showAll)
    listed.take(window.shown).forEach { add(TreeRow.Chat(facts, bot.botId, it, dimmed)) }
    if (window.showMore) add(TreeRow.ShowMore(hostId, bot.botId, window.hidden))
    if (window.showLess) add(TreeRow.ShowLess(hostId, bot.botId))
}

// --- relative times ---------------------------------------------------------------------------------------------------

/** How long ago something was, in the steps the session lists use (刚刚 / N 分钟前 / N 小时前 / 昨天 / N 天前 / M月D日). */
internal sealed interface Age {
    data object JustNow : Age
    data class Minutes(val n: Int) : Age
    data class Hours(val n: Int) : Age
    data object Yesterday : Age
    data class Days(val n: Int) : Age

    /** A week or more ago: the caller writes the date. */
    data object Date : Age
}

internal fun ageOf(atMs: Long, nowMs: Long): Age {
    val seconds = (nowMs - atMs) / 1000
    return when {
        seconds < 60 -> Age.JustNow
        seconds < 3_600 -> Age.Minutes((seconds / 60).toInt())
        seconds < 86_400 -> Age.Hours((seconds / 3_600).toInt())
        seconds < 172_800 -> Age.Yesterday
        seconds < 604_800 -> Age.Days((seconds / 86_400).toInt())
        else -> Age.Date
    }
}
