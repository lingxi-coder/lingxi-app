package com.lingxi.code.cron

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.listSaver
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.components.PageTopBar
import com.lingxi.code.components.blockTouchesBehind
import com.lingxi.code.conversation.ConversationSource
import com.lingxi.code.project.ProjectStore
import com.lingxi.code.theme.LingXiTheme

/** Where the scheduled-task page is opened (decision D14: a pushed page of the Chats tab, not a Settings page). */
sealed interface CronPageRoute {
    /** The task list. */
    data object Tasks : CronPageRoute

    /** One task's detail. */
    data class Task(val taskKey: String) : CronPageRoute

    /** One run's result. */
    data class Run(val runId: String) : CronPageRoute

    /** The new-task form with [prompt] already filled in. */
    data class New(val prompt: String) : CronPageRoute

    companion object {
        /** A blank task key or prompt has nothing to open, so it falls back to the task list. */
        fun task(taskKey: String?): CronPageRoute = taskKey?.takeIf(String::isNotBlank)?.let(::Task) ?: Tasks
        fun new(prompt: String): CronPageRoute = prompt.takeIf(String::isNotBlank)?.let(::New) ?: Tasks

        /** Saves "which cron page is open (or none)" across rotation and process death. */
        val Saver: Saver<CronPageRoute?, Any> = listSaver(
            save = { route ->
                when (route) {
                    null -> listOf("none")
                    Tasks -> listOf("tasks")
                    is Task -> listOf("task", route.taskKey)
                    is Run -> listOf("run", route.runId)
                    is New -> listOf("new", route.prompt)
                }
            },
            restore = { saved ->
                val argument = saved.getOrNull(1)
                when (saved.firstOrNull()) {
                    "tasks" -> Tasks
                    "task" -> argument?.let(::Task)
                    "run" -> argument?.let(::Run)
                    "new" -> argument?.let(::New)
                    else -> null
                }
            },
        )
    }
}

/**
 * The scheduled-task page, mounted by `RootScreen` above the chat. Shell stub (SPEC-mobile-team-bot-ui
 * section 9.2): the page the Settings graph used to carry, re-hosted unchanged under a back-arrow
 * top bar; the A-SETTINGS slice owns it from here and keeps this signature.
 */
@Composable
fun CronPage(
    route: CronPageRoute,
    engineSource: ConversationSource?,
    projectStore: ProjectStore?,
    onOpenSession: (CronRunRecord) -> Unit,
    onClose: () -> Unit,
) {
    // Registered before the screen's own handler (which closes its detail view first), so this one
    // only runs when the screen has nothing left to close.
    BackHandler(onBack = onClose)
    Column(
        Modifier
            .fillMaxSize()
            .background(LingXiTheme.palette.windowBg)
            .blockTouchesBehind()
            .windowInsetsPadding(WindowInsets.systemBars)
            .imePadding(),
    ) {
        PageTopBar(title = stringResource(R.string.settings_title_cron), onBack = onClose)
        Box(
            Modifier
                .weight(1f)
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp)
                .padding(top = 16.dp, bottom = 28.dp),
        ) {
            CronScreen(
                initialTaskKey = (route as? CronPageRoute.Task)?.taskKey,
                initialRunId = (route as? CronPageRoute.Run)?.runId,
                initialPrompt = (route as? CronPageRoute.New)?.prompt,
                engineSource = engineSource,
                projectStore = projectStore,
                onOpenSession = onOpenSession,
            )
        }
    }
}
