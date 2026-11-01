package com.lingxi.code.voice.audio

import org.json.JSONObject

/** Retains actual resource ownership when a host context still reports an unbound logical profile. */
internal class ProviderAudioSessionBindings<Owner> {
    private data class Source(val sessionId: String, val profileId: String)
    private data class Binding(val source: Source, val profileId: String, val accountScope: String)
    private val sources = mutableMapOf<Owner, Source>()
    private val bindings = mutableMapOf<Owner, Binding>()

    @Synchronized fun clear() { sources.clear(); bindings.clear() }

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
