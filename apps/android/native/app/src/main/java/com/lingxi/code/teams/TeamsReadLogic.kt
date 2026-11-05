package com.lingxi.code.teams

/** When the app marks a conversation read by itself (D12, SPEC-mobile-team-bot-ui section 6.3 and 8.6). */
internal object TeamsReadLogic {
    /** The newest message must stay in the viewport this long. `null` disables automatic mark-read. */
    val AUTO_MARK_READ_MS: Long? = 1000L

    /**
     * The `seq` to send as `lastReadSeq`, or null for "do nothing". Never offline, never from the cache, never in the
     * background, and only when the newest *rendered* message is ahead of the watermark and has been at the bottom
     * of the viewport for at least [thresholdMs] ([atBottomMs] = how long it has been there, null = not at the bottom).
     */
    fun autoMarkReadSeq(online: Boolean, fromCache: Boolean, foreground: Boolean, atBottomMs: Long?, newestRenderedSeq: Long, lastReadSeq: Long, thresholdMs: Long? = AUTO_MARK_READ_MS): Long? {
        if (thresholdMs == null || !online || fromCache || !foreground) return null
        if (atBottomMs == null || atBottomMs < thresholdMs) return null
        return newestRenderedSeq.takeIf { it > lastReadSeq }
    }
}
