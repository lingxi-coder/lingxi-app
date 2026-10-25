package com.lingxi.code.account

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.Image
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.lingxi.code.R

@Composable
fun LingxiAccountSection(model: AccountViewModel = viewModel()) {
    val state by model.state.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val activity = context.activity()
    var email by remember { mutableStateOf("") }
    // Passwords intentionally stay out of saved instance state.
    var password by remember { mutableStateOf("") }
    var register by remember { mutableStateOf(false) }
    LaunchedEffect(state.user?.uid) { password = "" }
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) { model.reload() }
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(stringResource(R.string.account_title), style = MaterialTheme.typography.titleLarge)
        val user = state.user
        if (user != null) {
            Text(user.name ?: user.email ?: stringResource(R.string.account_signed_in), modifier = Modifier.testTag("account.profile"))
            user.email?.let { Text(it) }
            Text(stringResource(if (user.verified) R.string.account_email_verified else R.string.account_email_unverified), style = MaterialTheme.typography.bodySmall)
            if (!user.verified && user.email != null) TextButton(onClick = model::verifyEmail, enabled = !state.busy) { Text(stringResource(R.string.account_send_verification)) }
            TextButton(onClick = model::reload, enabled = !state.busy) { Text(stringResource(R.string.account_refresh)) }
            TextButton(onClick = { activity?.let(model::signOut) }, enabled = !state.busy && activity != null, modifier = Modifier.testTag("account.signout")) { Text(stringResource(R.string.account_sign_out)) }
        } else {
            OutlinedTextField(value = email, onValueChange = { email = it }, label = { Text(stringResource(R.string.account_email)) }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Email), singleLine = true, enabled = !state.busy, modifier = Modifier.fillMaxWidth().testTag("account.email"))
            OutlinedTextField(value = password, onValueChange = { password = it }, label = { Text(stringResource(R.string.account_password)) }, visualTransformation = PasswordVisualTransformation(), keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password), singleLine = true, enabled = !state.busy, modifier = Modifier.fillMaxWidth().testTag("account.password"))
            Button(onClick = { model.emailSignIn(email, password, register) }, enabled = !state.busy && email.isNotBlank() && password.isNotEmpty(), modifier = Modifier.testTag("account.submit")) { Text(stringResource(if (register) R.string.account_create else R.string.account_sign_in)) }
            TextButton(onClick = { register = !register; password = "" }, enabled = !state.busy, modifier = Modifier.testTag("account.mode")) { Text(stringResource(if (register) R.string.account_existing else R.string.account_register)) }
            TextButton(onClick = { model.resetPassword(email) }, enabled = !state.busy && email.isNotBlank()) { Text(stringResource(R.string.account_forgot_password)) }
            OutlinedButton(
                onClick = { activity?.let { model.google(it, context.getString(R.string.default_web_client_id)) } },
                enabled = !state.busy && activity != null,
                colors = ButtonDefaults.outlinedButtonColors(containerColor = Color.White, contentColor = Color(0xFF1F1F1F)),
                border = BorderStroke(1.dp, Color(0xFF747775)),
                modifier = Modifier.fillMaxWidth().testTag("account.google"),
            ) {
                Image(painterResource(R.drawable.google_g), contentDescription = null, modifier = Modifier.size(20.dp))
                Spacer(Modifier.width(10.dp))
                Text(stringResource(R.string.account_google))
            }
            OutlinedButton(onClick = { activity?.let(model::apple) }, enabled = !state.busy && activity != null, modifier = Modifier.fillMaxWidth()) { Text(stringResource(R.string.account_apple)) }
        }
        if (state.busy) LinearProgressIndicator(Modifier.fillMaxWidth())
        state.messageRes?.let { Text(stringResource(it), style = MaterialTheme.typography.bodyMedium, modifier = Modifier.testTag("account.message")) }
        val uriHandler = androidx.compose.ui.platform.LocalUriHandler.current
        TextButton(onClick = { uriHandler.openUri("https://lingxios.net/privacy") }) {
            Text(androidx.compose.ui.res.stringResource(R.string.privacy_policy_link))
        }
        HorizontalDivider()
    }
}

private tailrec fun Context.activity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.activity()
    else -> null
}
