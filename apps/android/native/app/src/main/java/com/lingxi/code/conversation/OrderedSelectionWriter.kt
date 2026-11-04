package com.lingxi.code.conversation

import com.lingxi.code.model.SessionMode
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch

/** One queue preserves explicit selection order across conversation owners. */
internal class OrderedSelectionWriter(private val scope: CoroutineScope) {
    private var last: Job? = null

    fun write(mode: SessionMode, persist: suspend (SessionMode) -> Unit, onFailure: (Exception) -> Unit) {
        val previous = last
        last = scope.launch {
            previous?.join()
            try { persist(mode) }
            catch (cancelled: CancellationException) { throw cancelled }
            catch (failure: Exception) { onFailure(failure) }
        }
    }

    suspend fun <T> read(load: suspend () -> T): T {
        while (true) {
            val current = last
            current?.join()
            if (current === last) return load()
        }
    }
}
