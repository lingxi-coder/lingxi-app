package com.lingxi.code.teams

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.lingxi.code.R
import java.util.UUID

@Composable
fun TeamsScreen(initialInvite: String?, onInviteHandled: () -> Unit, onOpenAccount: () -> Unit, onClose: () -> Unit, model: TeamsViewModel = viewModel()) {
    val state by model.state.collectAsStateWithLifecycle()
    var name by remember { mutableStateOf("") }
    var code by remember { mutableStateOf("") }
    var maxUses by remember { mutableIntStateOf(1) }
    var ttl by remember { mutableIntStateOf(86400) }
    var adminInvite by remember { mutableStateOf(false) }
    var confirmation by remember { mutableStateOf<Pair<Int, Map<String, Any>>?>(null) }
    val clipboard = LocalClipboardManager.current
    DisposableEffect(model) { model.start(); onDispose { model.stop() } }
    LaunchedEffect(initialInvite) { initialInvite?.let { code = it } }
    LaunchedEffect(state.uid) { confirmation = null; name = "" }
    BackHandler(onBack = onClose)
    Surface(modifier = Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding()) {
            Row(Modifier.fillMaxWidth().padding(16.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                TextButton(onClick = onClose) { Text(stringResource(R.string.teams_cancel)) }
                Text(stringResource(R.string.teams_title), style = MaterialTheme.typography.titleLarge)
                TextButton(onClick = model::refresh, enabled = !state.busy) { Text(stringResource(R.string.teams_refresh)) }
            }
            LazyColumn(Modifier.fillMaxSize().padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(14.dp), contentPadding = PaddingValues(bottom = 32.dp)) {
                item { Text(stringResource(R.string.teams_disclosure), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                state.error?.let { error -> item { Text(stringResource(when (error) { "INVITE_UNAVAILABLE" -> R.string.teams_invite_error; "STALE_MEMBERSHIP", "MEMBERSHIP_REQUIRED" -> R.string.teams_stale_error; else -> R.string.teams_error }), color = MaterialTheme.colorScheme.error) } }
                if (state.uid == null) item { Button(onClick = onOpenAccount) { Text(stringResource(R.string.teams_signin)) } }
                else {
                    if (state.loading || state.busy) item { LinearProgressIndicator(Modifier.fillMaxWidth()) }
                    item { Text(stringResource(R.string.teams_my), style = MaterialTheme.typography.titleMedium) }
                    items(state.teams, key = { "team-${it.id}" }) { team ->
                        OutlinedCard(onClick = { adminInvite = false; model.select(team.id) }, enabled = !state.busy, modifier = Modifier.fillMaxWidth()) {
                            Row(Modifier.fillMaxWidth().padding(16.dp), horizontalArrangement = Arrangement.SpaceBetween) { Column { Text(team.name, style = MaterialTheme.typography.titleMedium); Text(teamRole(team.role), style = MaterialTheme.typography.bodySmall) }; Text(team.memberCount.toString()) }
                        }
                    }
                    item {
                        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                            OutlinedTextField(value = name, onValueChange = { if (it.length <= 80) name = it }, label = { Text(stringResource(R.string.teams_name)) }, enabled = !state.busy, modifier = Modifier.fillMaxWidth())
                            Button(onClick = { model.execute(mapOf("action" to "create", "teamId" to UUID.randomUUID().toString(), "name" to name.trim())) { name = "" } }, enabled = !state.busy && name.isNotBlank()) { Text(stringResource(R.string.teams_create)) }
                            OutlinedTextField(value = code, onValueChange = { if (it.length <= 200) code = it }, label = { Text(stringResource(R.string.teams_invite_input)) }, enabled = !state.busy, modifier = Modifier.fillMaxWidth())
                            Text(stringResource(R.string.teams_join_disclosure), style = MaterialTheme.typography.bodySmall)
                            Button(onClick = { TeamInvitation.code(code)?.let { model.execute(mapOf("action" to "join", "code" to it, "requestId" to UUID.randomUUID().toString())) { code = ""; onInviteHandled() } } }, enabled = !state.busy && TeamInvitation.code(code) != null) { Text(stringResource(R.string.teams_join_confirm)) }
                        }
                    }
                    state.detail?.let { detail ->
                        val team = detail.team
                        item { HorizontalDivider(); Text(team.name, style = MaterialTheme.typography.headlineSmall); Text(stringResource(R.string.teams_members), style = MaterialTheme.typography.titleMedium) }
                        items(detail.members, key = { "member-${it.uid}" }) { member ->
                            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                                Text(member.displayName, style = MaterialTheme.typography.titleSmall); Text(teamRole(member.role), style = MaterialTheme.typography.bodySmall)
                                if (member.uid != state.uid && member.role != "owner" && (team.role == "owner" || (team.role == "admin" && member.role == "member"))) {
                                    Row {
                                        if (team.role == "owner") TextButton(enabled = !state.busy, onClick = { confirmation = R.string.teams_confirm_change to mapOf("action" to "role", "teamId" to team.id, "uid" to member.uid, "generation" to member.generation, "role" to if (member.role == "admin") "member" else "admin") }) { Text(stringResource(if (member.role == "admin") R.string.teams_demote else R.string.teams_promote)) }
                                        TextButton(enabled = !state.busy, onClick = { confirmation = R.string.teams_confirm_change to mapOf("action" to "remove", "teamId" to team.id, "uid" to member.uid, "generation" to member.generation) }) { Text(stringResource(R.string.teams_remove), color = MaterialTheme.colorScheme.error) }
                                    }
                                }
                            }
                        }
                        if (team.role != "member") {
                            item {
                                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                                    Text(stringResource(R.string.teams_invites), style = MaterialTheme.typography.titleMedium)
                                    if (team.role == "owner") Row { Checkbox(checked = adminInvite, onCheckedChange = { adminInvite = it }, enabled = !state.busy); Text(stringResource(R.string.teams_admin)) }
                                    Row { Text(stringResource(R.string.teams_uses)); TextButton(enabled = !state.busy && maxUses > 1, onClick = { maxUses-- }) { Text("−") }; Text(maxUses.toString()); TextButton(enabled = !state.busy && maxUses < 50, onClick = { maxUses++ }) { Text("+") } }
                                    Row { listOf(3600 to R.string.teams_hour, 86400 to R.string.teams_day, 604800 to R.string.teams_week).forEach { (value, label) -> FilterChip(selected = ttl == value, onClick = { ttl = value }, label = { Text(stringResource(label)) }, enabled = !state.busy) } }
                                    Button(enabled = !state.busy, onClick = { model.execute(mapOf("action" to "invite", "teamId" to team.id, "requestId" to UUID.randomUUID().toString(), "role" to if (team.role == "owner" && adminInvite) "admin" else "member", "maxUses" to maxUses, "ttlSeconds" to ttl)) }) { Text(stringResource(R.string.teams_invite_create)) }
                                    if (state.invitationLink.isNotEmpty()) {
                                        Text(state.invitationLink, style = MaterialTheme.typography.bodySmall)
                                        OutlinedButton(onClick = { clipboard.setText(AnnotatedString(state.invitationLink)) }) { Text(stringResource(R.string.teams_copy)) }
                                    }
                                }
                            }
                            items(detail.invites, key = { "invite-${it.id}" }) { invite ->
                                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                                    Column(Modifier.weight(1f)) { Text(teamRole(invite.role)); Text("${invite.uses}/${invite.maxUses} · ${invite.expiresAt}", style = MaterialTheme.typography.bodySmall) }
                                    if (invite.revoked) Text(stringResource(R.string.teams_revoked))
                                    else TextButton(enabled = !state.busy && (team.role == "owner" || invite.role == "member"), onClick = { model.execute(mapOf("action" to "revokeInvite", "teamId" to team.id, "inviteId" to invite.id)) }) { Text(stringResource(R.string.teams_revoke)) }
                                }
                            }
                        }
                        item {
                            TextButton(enabled = !state.busy, onClick = { confirmation = if (team.role == "owner") R.string.teams_confirm_delete to mapOf("action" to "delete", "teamId" to team.id) else R.string.teams_confirm_leave to mapOf("action" to "leave", "teamId" to team.id, "generation" to team.generation) }) { Text(stringResource(if (team.role == "owner") R.string.teams_delete else R.string.teams_leave), color = MaterialTheme.colorScheme.error) }
                        }
                    }
                }
            }
        }
        confirmation?.let { (title, command) -> AlertDialog(onDismissRequest = { confirmation = null }, title = { Text(stringResource(title)) }, confirmButton = { TextButton(enabled = !state.busy, onClick = { model.execute(command); confirmation = null }) { Text(stringResource(R.string.teams_confirm)) } }, dismissButton = { TextButton(onClick = { confirmation = null }) { Text(stringResource(R.string.teams_cancel)) } }) }
    }
}

@Composable private fun teamRole(role: String): String = stringResource(when (role) { "owner" -> R.string.teams_owner; "admin" -> R.string.teams_admin; else -> R.string.teams_member })
