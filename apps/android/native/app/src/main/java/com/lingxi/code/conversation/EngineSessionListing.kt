package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.ClientCommand

/** Project persistence replaces its index, so every listing must be complete. */
internal fun completeSessionListCommand(): ClientCommand.ListSessions =
    ClientCommand.ListSessions(limit = UInt.MAX_VALUE)
