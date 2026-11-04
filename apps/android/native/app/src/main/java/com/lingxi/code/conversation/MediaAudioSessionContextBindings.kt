package com.lingxi.code.conversation

import com.lingxi.code.bindings.android.MediaAudioSessionBindingFfi

/** Retains the complete Host-issued token, including its engine attachment identity. */
internal class MediaAudioSessionContextBindings<Engine : Any>(
    private val removeBinding: (Engine, MediaAudioSessionBindingFfi) -> Unit,
) {
    private data class Registration<Engine>(val engine: Engine, val binding: MediaAudioSessionBindingFfi)
    private val lock = Any()
    private val bindings = mutableListOf<Registration<Engine>>()
    private var closed = false

    fun register(engine: Engine, binding: MediaAudioSessionBindingFfi) {
        val removeImmediately = synchronized(lock) {
            if (closed) true else {
                val index = bindings.indexOfFirst { it.engine === engine && it.binding.sessionId == binding.sessionId }
                val registration = Registration(engine, binding)
                if (index < 0) bindings.add(registration) else bindings[index] = registration
                false
            }
        }
        if (removeImmediately) runCatching { removeBinding(engine, binding) }
    }

    fun remove(engine: Engine, sessionId: String) {
        val registration = synchronized(lock) {
            val index = bindings.indexOfFirst { it.engine === engine && it.binding.sessionId == sessionId }
            if (index < 0) null else bindings.removeAt(index)
        } ?: return
        runCatching { removeBinding(registration.engine, registration.binding) }
    }

    fun removeEngine(engine: Engine) {
        val registrations = synchronized(lock) {
            bindings.filter { it.engine === engine }.also { bindings.removeAll { registration -> registration.engine === engine } }
        }
        registrations.forEach { runCatching { removeBinding(it.engine, it.binding) } }
    }

    fun close() {
        val registrations = synchronized(lock) {
            closed = true
            bindings.toList().also { bindings.clear() }
        }
        registrations.forEach { registration -> runCatching { removeBinding(registration.engine, registration.binding) } }
    }
}
