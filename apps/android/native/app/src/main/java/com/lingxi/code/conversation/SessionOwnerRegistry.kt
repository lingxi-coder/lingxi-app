package com.lingxi.code.conversation

import com.lingxi.code.model.SessionMode

/** Serializes a storage-fault publication with admission of a new owner. */
internal object SessionAdmissionGate { val lock = Any() }

/** Keep the current owner mounted while fencing a damaged index from new owners. */
internal fun shouldBlockForUnavailableStorage(storageUnavailable: Boolean, hasRegisteredOwner: Boolean): Boolean =
    storageUnavailable && !hasRegisteredOwner

internal fun shouldBlockForOwnerStateValidation(valid: Boolean?, hasRegisteredOwner: Boolean): Boolean =
    shouldBlockForUnavailableStorage(valid != true, hasRegisteredOwner)

/** Associates an immutable mode and exact workspace/session identity with one owner. */
internal class SessionOwnerRegistry<T : Any> {
    private data class Owner<T>(val workspace: String, val mode: SessionMode, val value: T)
    private val owners = mutableMapOf<String, Owner<T>>()

    @Synchronized fun register(workspace: String, session: String, mode: SessionMode, owner: T, allowNew: Boolean = true): Boolean {
        val old = owners[session]
        if (old != null) return old.workspace == workspace && old.mode == mode && old.value === owner
        if (!allowNew) return false
        owners[session] = Owner(workspace, mode, owner)
        return true
    }

    @Synchronized fun find(workspace: String, session: String, mode: SessionMode? = null): T? {
        val saved = owners[session] ?: return null
        return saved.value.takeIf { saved.workspace == workspace && (mode == null || mode == saved.mode) }
    }

    @Synchronized fun contains(session: String): Boolean = session in owners

    @Synchronized fun removeOwner(owner: T) {
        owners.entries.removeAll { it.value.value === owner }
    }
}
