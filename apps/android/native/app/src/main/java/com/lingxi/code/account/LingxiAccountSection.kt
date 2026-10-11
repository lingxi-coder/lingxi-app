package com.lingxi.code.account

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
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
        Text("LingXi 账户", style = MaterialTheme.typography.titleLarge)
        val user = state.user
        if (user != null) {
            Text(user.name ?: user.email ?: "已登录", modifier = Modifier.testTag("account.profile"))
            user.email?.let { Text(it) }
            Text(if (user.verified) "邮箱已验证" else "邮箱尚未验证", style = MaterialTheme.typography.bodySmall)
            if (!user.verified && user.email != null) TextButton(onClick = model::verifyEmail, enabled = !state.busy) { Text("发送验证邮件") }
            TextButton(onClick = model::reload, enabled = !state.busy) { Text("刷新账户") }
            TextButton(onClick = { activity?.let(model::signOut) }, enabled = !state.busy && activity != null, modifier = Modifier.testTag("account.signout")) { Text("退出登录") }
        } else {
            OutlinedTextField(value = email, onValueChange = { email = it }, label = { Text("邮箱") }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Email), singleLine = true, enabled = !state.busy, modifier = Modifier.fillMaxWidth().testTag("account.email"))
            OutlinedTextField(value = password, onValueChange = { password = it }, label = { Text("密码") }, visualTransformation = PasswordVisualTransformation(), keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password), singleLine = true, enabled = !state.busy, modifier = Modifier.fillMaxWidth().testTag("account.password"))
            Button(onClick = { model.emailSignIn(email, password, register) }, enabled = !state.busy && email.isNotBlank() && password.isNotEmpty(), modifier = Modifier.testTag("account.submit")) { Text(if (register) "创建账户" else "登录") }
            TextButton(onClick = { register = !register; password = "" }, enabled = !state.busy, modifier = Modifier.testTag("account.mode")) { Text(if (register) "已有账户，登录" else "注册新账户") }
            TextButton(onClick = { model.resetPassword(email) }, enabled = !state.busy && email.isNotBlank()) { Text("忘记密码") }
            OutlinedButton(onClick = { activity?.let { model.google(it, context.getString(R.string.default_web_client_id)) } }, enabled = !state.busy && activity != null, modifier = Modifier.fillMaxWidth().testTag("account.google")) { Text("通过 Google 登录") }
            OutlinedButton(onClick = { activity?.let(model::apple) }, enabled = !state.busy && activity != null, modifier = Modifier.fillMaxWidth()) { Text("通过 Apple 登录") }
        }
        if (state.busy) LinearProgressIndicator(Modifier.fillMaxWidth())
        state.message?.let { Text(it, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.testTag("account.message")) }
        HorizontalDivider()
    }
}

private tailrec fun Context.activity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.activity()
    else -> null
}
