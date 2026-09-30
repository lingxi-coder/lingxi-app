package com.lingxi.code.conversation

import com.lingxi.code.bindings.TaskStatusDto

/** Rebuilding the source invalidates live work and connection-scoped user requests. */
internal fun ChatState.blocksEngineReconnect(hasPendingPermission: Boolean): Boolean =
    requiresBackgroundExecution || sessionTransitioning || cancellationInFlight ||
        hasPendingPermission || pendingQuestions.isNotEmpty() ||
        backgroundTasks.values.any { it.status == TaskStatusDto.PENDING || it.status == TaskStatusDto.RUNNING } ||
        workflowRuns.values.any { it.status == TaskStatusDto.PENDING || it.status == TaskStatusDto.RUNNING } ||
        sessionAgents.any { it.status.lowercase() in setOf("running", "working", "pending", "queued", "initializing") }
