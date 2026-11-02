package com.lingxi.code.voice.audio

/** Tracks exact engine bindings without letting a stale owner fall back to the visible engine. */
internal class AudioOwnerBindingRegistry<T : Any> {
    private var current: T? = null
    private val owners = mutableMapOf<AudioOwnerKey, T?>()

    @Synchronized fun attach(handle: T) { current = handle }
    @Synchronized fun clear() { current = null }
    @Synchronized fun current(): T? = current
    @Synchronized fun reserve(owner: AudioOwnerKey) { if (owner !in owners) owners[owner] = null }
    @Synchronized fun bind(owner: AudioOwnerKey, handle: T) { owners[owner] = handle }
    @Synchronized fun release(owner: AudioOwnerKey) { owners.remove(owner) }
    @Synchronized fun releaseIfBound(owner: AudioOwnerKey, handle: T) {
        if (owners[owner] === handle) owners.remove(owner)
    }
    @Synchronized fun isReserved(owner: AudioOwnerKey): Boolean = owner in owners || owner.requiresEngineBinding()
    @Synchronized fun engineFor(owner: AudioOwnerKey): T? = when {
        owner in owners -> owners[owner]
        owner.requiresEngineBinding() -> null
        else -> current
    }
    @Synchronized fun detach(handle: T) {
        if (current === handle) current = null
        owners.entries.filter { it.value === handle }.forEach { it.setValue(null) }
    }
}
