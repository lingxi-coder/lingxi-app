package com.lingxi.code.cron

/**
 * Source-compatibility marker for the removed foreground-service cron path.
 *
 * Production wiring moved to [CronExecutionWorker]. Keeping this type for one
 * migration release avoids breaking downstream debug references while ensuring
 * no Service, WakeLock, or foreground-service behavior remains reachable.
 */
@Deprecated("Cron execution is owned by WorkManager")
internal object CronRunService
