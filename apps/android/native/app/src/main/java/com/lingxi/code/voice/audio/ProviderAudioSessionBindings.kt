package com.lingxi.code.voice.audio

import org.json.JSONObject

/** Retains actual resource ownership when a host context still reports an unbound logical profile. */
internal class ProviderAudioSessionBindings<Owner> {
    private data class Source(val sessionId: String, val profileId: String)
    private data class Binding(val source: Source, val profileId: String, val accountScope: String)
    private val sources = mutableMapOf<Owner, Source>()
    private val bindings = mutableMapOf<Owner, Binding>()

    @Synchronized fun clear() { sources.clear(); bindings.clear() }

    @Synchronized fun removeWhere(predicate: (Owner) -> Boolean) {
        sources.keys.removeAll(predicate)
        bindings.keys.removeAll(predicate)
    }

    @Synchronized fun context(owner: Owner, context: JSONObject): JSONObject {
        val output = JSONObject(context.toString())
        val incoming = Source(context.getString("sessionId"), context.getString("profileId"))
        val previous = bindings[owner]
        val scope = context.optString("accountScope")
        if (scope.isNotBlank() && scope != "unbound") {
            val source = sources[owner]?.takeIf { it.sessionId == incoming.sessionId } ?: incoming
            sources[owner] = source
            bindings[owner] = Binding(source, incoming.profileId, scope)
        } else {
            if (previous != null && (previous.source.sessionId != incoming.sessionId ||
                    incoming.profileId !in setOf(previous.source.profileId, previous.profileId))) bindings.remove(owner)
            val retained = bindings[owner]
            sources[owner] = retained?.source ?: incoming
            retained?.let { output.put("profileId", it.profileId).put("accountScope", it.accountScope) }
        }
        return output
    }

    @Synchronized fun record(owner: Owner, context: JSONObject, profileId: String?, accountScope: String?) {
        if (profileId.isNullOrBlank() || accountScope.isNullOrBlank() || accountScope == "unbound") return
        val source = sources[owner] ?: return
        val contextSession = context.getString("sessionId")
        val contextProfile = context.getString("profileId")
        if (source.sessionId != contextSession || contextProfile !in setOf(source.profileId, bindings[owner]?.profileId)) return
        bindings[owner] = Binding(source, profileId, accountScope)
    }
}

/** Keep account routing attached to the precise session owner and engine. */
internal class OwnerScopedProviderAudioBindings<Engine : Any> {
    private class Route<Engine : Any>(val owner: AudioOwnerKey, val engine: Engine) {
        override fun equals(other: Any?): Boolean = other is Route<*> && owner == other.owner && engine === other.engine
        override fun hashCode(): Int = 31 * owner.hashCode() + System.identityHashCode(engine)
    }

    private val engines = AudioOwnerBindingRegistry<Engine>()
    private val accounts = ProviderAudioSessionBindings<Route<Engine>>()

    @Synchronized fun attach(engine: Engine) = engines.attach(engine)
    @Synchronized fun clear() = engines.clear()
    @Synchronized fun currentEngine(): Engine? = engines.current()
    @Synchronized fun reserve(owner: AudioOwnerKey) = engines.reserve(owner)
    @Synchronized fun bind(owner: AudioOwnerKey, engine: Engine) {
        if (engines.engineFor(owner) !== engine) accounts.removeWhere { it.owner == owner }
        engines.bind(owner, engine)
    }
    @Synchronized fun release(owner: AudioOwnerKey) {
        engines.release(owner)
        accounts.removeWhere { it.owner == owner }
    }
    @Synchronized fun releaseIfBound(owner: AudioOwnerKey, engine: Engine) {
        if (engines.isBound(owner, engine)) {
            engines.releaseIfBound(owner, engine)
            accounts.removeWhere { it.owner == owner && it.engine === engine }
        }
    }
    @Synchronized fun detach(engine: Engine) {
        engines.detach(engine)
        accounts.removeWhere { it.engine === engine }
    }
    @Synchronized fun isReserved(owner: AudioOwnerKey): Boolean = engines.isReserved(owner)
    @Synchronized fun engineFor(owner: AudioOwnerKey): Engine? = engines.engineFor(owner)
    @Synchronized fun context(owner: AudioOwnerKey, engine: Engine, value: JSONObject): JSONObject? =
        if (engines.engineFor(owner) !== engine) null else accounts.context(Route(owner, engine), value)
    @Synchronized fun record(owner: AudioOwnerKey, engine: Engine, value: JSONObject, profile: String?, scope: String?) {
        if (engines.engineFor(owner) === engine) accounts.record(Route(owner, engine), value, profile, scope)
    }
}
