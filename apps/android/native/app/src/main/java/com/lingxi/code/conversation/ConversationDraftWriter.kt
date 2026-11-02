package com.lingxi.code.conversation

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch

/** Keeps the latest failed draft for each owner until storage becomes writable. */
internal class ConversationDraftWriter(private val scope: CoroutineScope) {
    data class Key(val scopeKey: String, val sessionId: String)
    private class Pending(val key: Key, val text: String)

    private val lock = Any()
    private val pending = linkedMapOf<Key, Pending>()
    private var tail: Job? = null

    fun write(
        key: Key,
        text: String,
        persist: suspend (Key, String) -> Unit,
        onFailure: (Exception) -> Unit,
    ): Job = synchronized(lock) {
        val next = Pending(key, text)
        pending[key] = next
        schedule(next, persist, onFailure)
    }

    fun retry(
        persist: suspend (Key, String) -> Unit,
        onFailure: (Exception) -> Unit,
    ): List<Job> = synchronized(lock) {
        pending.values.toList().map { schedule(it, persist, onFailure) }
    }

    private fun schedule(
        item: Pending,
        persist: suspend (Key, String) -> Unit,
        onFailure: (Exception) -> Unit,
    ): Job {
        val previous = tail
        val job = scope.launch(start = CoroutineStart.LAZY) {
            previous?.join()
            if (!synchronized(lock) { pending[item.key] === item }) return@launch
            try {
                persist(item.key, item.text)
                synchronized(lock) {
                    if (pending[item.key] === item) pending.remove(item.key)
                }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Exception) {
                onFailure(failure)
            }
        }
        tail = job
        job.start()
        return job
    }

    suspend fun await() {
        while (true) {
            val current = synchronized(lock) { tail }
            current?.join()
            if (synchronized(lock) { tail === current }) return
        }
    }
}
