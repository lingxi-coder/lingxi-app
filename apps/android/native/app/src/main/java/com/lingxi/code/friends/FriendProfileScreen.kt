package com.lingxi.code.friends

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.teams.TeamsMessageState

/**
 * Profile settings (docs/FRIENDS-CLIENTS.md D11): display name, username and the two discoverability switches, with one Save that sends
 * only what changed. The profile is asked for again on every open (D2); a failure shows inline with Retry.
 */
@Composable
internal fun FriendProfileScreen(
    state: FriendsState, showBack: Boolean, onClose: () -> Unit, onLoad: () -> Unit, onSave: (ProfileForm) -> Unit, modifier: Modifier = Modifier,
) {
    LaunchedEffect(Unit) { onLoad() }
    val profile = state.profile
    val error = state.profileOp.error
    Column(modifier.fillMaxSize().testTag(FriendsTags.PROFILE_SCREEN)) {
        FriendsPageHeader(stringResource(R.string.friends_profile_title), showBack, onClose)
        when {
            profile != null -> ProfileFields(state, profile, onSave)
            error != null -> TeamsMessageState(
                stringResource(friendsErrorRes(error)), Modifier.testTag(FriendsTags.PROFILE_ERROR),
                action = { Button(onClick = onLoad) { Text(stringResource(R.string.common_retry)) } },
            )
            else -> Box(Modifier.fillMaxSize().testTag(FriendsTags.LOADING), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        }
    }
}

@Composable
private fun ProfileFields(state: FriendsState, profile: FriendProfile, onSave: (ProfileForm) -> Unit) {
    var name by rememberSaveable(profile.uid) { mutableStateOf(profile.displayName) }
    var handle by rememberSaveable(profile.uid) { mutableStateOf(profile.handle.orEmpty()) }
    var findByHandle by rememberSaveable(profile.uid) { mutableStateOf(profile.findByHandle) }
    var findByEmail by rememberSaveable(profile.uid) { mutableStateOf(profile.findByEmail) }
    val form = ProfileForm(name, handle, findByHandle, findByEmail)
    val save = state.saveOp
    val handleTaken = save.error == FriendsError.HandleTaken
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).imePadding().padding(16.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
        OutlinedTextField(
            value = name, onValueChange = { name = it }, singleLine = true, enabled = !save.busy,
            label = { Text(stringResource(R.string.friends_profile_name)) }, isError = !FriendsLogic.validDisplayName(name),
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Next),
            modifier = Modifier.fillMaxWidth().testTag(FriendsTags.PROFILE_NAME),
        )
        OutlinedTextField(
            value = handle, onValueChange = { handle = it }, singleLine = true, enabled = !save.busy,
            label = { Text(stringResource(R.string.friends_profile_handle)) }, isError = handleTaken || !form.handleValid,
            supportingText = { Text(stringResource(if (handleTaken) R.string.friends_error_handle_taken else R.string.friends_profile_handle_hint)) },
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false, imeAction = ImeAction.Done),
            modifier = Modifier.fillMaxWidth().testTag(FriendsTags.PROFILE_HANDLE),
        )
        SwitchRow(R.string.friends_profile_find_handle, findByHandle, !save.busy) { findByHandle = it }
        SwitchRow(R.string.friends_profile_find_email, findByEmail, !save.busy) { findByEmail = it }
        Text(stringResource(R.string.friends_profile_find_footer), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        save.error?.takeIf { !handleTaken }?.let { FriendsErrorText(it) }
        Button(
            onClick = { onSave(form) }, enabled = form.valid && form.changes(profile).isNotEmpty() && !save.busy,
            modifier = Modifier.fillMaxWidth().testTag(FriendsTags.PROFILE_SAVE),
        ) {
            if (save.busy) CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp) else Text(stringResource(R.string.common_save))
        }
    }
}

@Composable
private fun SwitchRow(@androidx.annotation.StringRes label: Int, checked: Boolean, enabled: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(stringResource(label), Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
        Switch(checked = checked, onCheckedChange = onChange, enabled = enabled)
    }
}
