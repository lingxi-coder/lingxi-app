package com.lingxi.code.conversation

/** The tool request is correlated to the local turn token, not the visible screen. */
internal fun computerUseBelongsToTurn(requestKey: String?, turnToken: Long, active: Boolean): Boolean =
    active && requestKey?.substringBefore(':')?.toLongOrNull() == turnToken
