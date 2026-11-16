package com.lingxi.code.teams

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Mic
import androidx.compose.material.icons.outlined.Stop
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.voice.AudioSetupGuidance
import com.lingxi.code.voice.VoiceCapturePhase
import com.lingxi.code.voice.VoiceCaptureStore
import com.lingxi.code.voice.cancelActiveHeldVoiceSession
import com.lingxi.code.voice.rememberVoiceCapture

/** Opens the app's voice settings from a Team conversation. The app shell provides it; without one the setup notice does nothing. */
internal val LocalOpenVoiceSettings = staticCompositionLocalOf<() -> Unit> { {} }

/**
 * Voice in the Team composer: [listening] while the microphone takes words, [guidance] when it could not (the box stays usable for
 * typing), [toggle] to start and to end, [dismiss] to put the guidance away.
 */
internal class TeamVoice(val listening: Boolean, val guidance: AudioSetupGuidance?, val toggle: () -> Unit, val dismiss: () -> Unit)

/** What becomes of words that were being spoken. Plain rules, tested on the JVM. */
internal object TeamVoiceLogic {
    /** Whether words that were spoken for [spokenFor] are put in the box that is for [current] now. Words for anything else are dropped. */
    fun delivers(spokenFor: String?, current: String): Boolean = spokenFor != null && spokenFor == current

    /**
     * What the microphone's own end means while nobody asked it to stop: [VoiceCapturePhase.Completed] has the words,
     * [VoiceCapturePhase.Failed] an explanation; with no permission or nothing heard it only ends. Null: it is still going.
     */
    enum class End { Heard, Failed, Quiet }
    fun end(phase: VoiceCapturePhase): End? = when (phase) {
        VoiceCapturePhase.Completed -> End.Heard
        VoiceCapturePhase.Failed -> End.Failed
        VoiceCapturePhase.PermissionRequired, VoiceCapturePhase.Cancelled -> End.Quiet
        VoiceCapturePhase.Idle, VoiceCapturePhase.Starting, VoiceCapturePhase.Listening, VoiceCapturePhase.Stopping -> null
    }

    /** After the viewer ended it: whether no words will come any more (it was cancelled, as when the app leaves the screen, or it failed). */
    fun abandoned(phase: VoiceCapturePhase): Boolean = phase == VoiceCapturePhase.Cancelled || phase == VoiceCapturePhase.Failed
}

/**
 * The app's own voice capture (`rememberVoiceCapture`: the microphone permission, the audio service, the recognizer) for the box
 * that is for [target] — the account, the team as the viewer is in it, the conversation and the message being changed. What it
 * hears is handed to [onHeard] to be put in the box; nothing is ever sent from here. Words spoken for one target never reach
 * another: when the target changes the capture is cancelled, and a transcript that still arrives is dropped. Leaving the screen
 * disposes the capture, which cancels it.
 */
@Composable
internal fun rememberTeamVoice(target: String, onHeard: (String) -> Unit): TeamVoice {
    var listening by remember { mutableStateOf(false) }
    var guidance by remember { mutableStateOf<AudioSetupGuidance?>(null) }
    /** The target the words being spoken, or being recognized, are for. */
    var spokenFor by remember { mutableStateOf<String?>(null) }
    val current = rememberUpdatedState(target)
    val heard = rememberUpdatedState(onHeard)
    fun deliver(text: String) {
        val owner = spokenFor
        spokenFor = null
        if (TeamVoiceLogic.delivers(owner, current.value)) heard.value(text)
    }
    val (start, stop) = rememberVoiceCapture(
        onTranscript = { deliver(it) },
        onError = { kind, message ->
            guidance = AudioSetupGuidance.fromError(kind, message)
            spokenFor = null
            listening = false
        },
    )
    /** The capture ended by itself (the recognizer decided the speaker was done, it failed, or there is no permission yet). */
    fun ended(end: TeamVoiceLogic.End, state: com.lingxi.code.voice.VoiceCaptureUiState) {
        listening = false
        when (end) {
            TeamVoiceLogic.End.Heard -> deliver(state.finalTranscript)
            TeamVoiceLogic.End.Failed -> { spokenFor = null; guidance = AudioSetupGuidance.fromError(message = state.errorMessage ?: state.statusText) }
            TeamVoiceLogic.End.Quiet -> spokenFor = null
        }
    }
    // After the viewer ends it, the result comes through the callbacks above; before that it is only in the store.
    LaunchedEffect(Unit) {
        VoiceCaptureStore.state.collect { state ->
            if (listening) TeamVoiceLogic.end(state.phase)?.let { ended(it, state) }
            else if (spokenFor != null && TeamVoiceLogic.abandoned(state.phase)) spokenFor = null
        }
    }
    LaunchedEffect(target) {
        if (spokenFor != null && spokenFor != target) {
            cancelActiveHeldVoiceSession()
            spokenFor = null
            listening = false
        }
    }
    return TeamVoice(
        listening = listening, guidance = guidance,
        toggle = {
            if (listening) {
                listening = false
                stop()
            } else if (spokenFor == null) {
                guidance = null
                spokenFor = current.value
                listening = true
                start()
                // A start that could not happen (no permission yet, no recognizer) has already said so.
                val state = VoiceCaptureStore.state.value
                TeamVoiceLogic.end(state.phase)?.takeIf { it != TeamVoiceLogic.End.Heard }?.let { ended(it, state) }
            }
        },
        dismiss = { guidance = null },
    )
}

/** The microphone button of the Team composer. */
@Composable
internal fun TeamVoiceButton(voice: TeamVoice, enabled: Boolean) {
    IconButton(onClick = voice.toggle, enabled = enabled, modifier = Modifier.testTag(TeamsTags.VOICE)) {
        Icon(
            if (voice.listening) Icons.Outlined.Stop else Icons.Outlined.Mic,
            stringResource(if (voice.listening) R.string.composer_stop_recording else R.string.composer_record),
        )
    }
}

/** While the microphone listens: what it has heard so far, above the box. The words go into the box when it ends. */
@Composable
internal fun TeamVoiceListening() {
    val state by VoiceCaptureStore.state.collectAsState()
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp).testTag(TeamsTags.LISTENING).semantics { liveRegion = LiveRegionMode.Polite },
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(Icons.Outlined.Mic, null, Modifier.size(16.dp), tint = MaterialTheme.colorScheme.primary)
        Text(
            state.statusText.ifBlank { stringResource(R.string.voice_listening) },
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 2, overflow = TextOverflow.Ellipsis,
        )
    }
}
