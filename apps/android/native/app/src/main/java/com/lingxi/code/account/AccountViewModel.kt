package com.lingxi.code.account

import android.app.Activity
import androidx.credentials.ClearCredentialStateRequest
import androidx.credentials.CredentialManager
import androidx.credentials.CustomCredential
import androidx.credentials.GetCredentialRequest
import androidx.credentials.exceptions.GetCredentialCancellationException
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.google.android.libraries.identity.googleid.GetSignInWithGoogleOption
import com.google.android.libraries.identity.googleid.GoogleIdTokenCredential
import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.auth.FirebaseAuthException
import com.google.firebase.auth.FirebaseAuthWebException
import com.google.firebase.auth.GoogleAuthProvider
import com.google.firebase.auth.OAuthProvider
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.tasks.await

data class AccountProfile(val uid: String, val email: String?, val name: String?, val verified: Boolean)
data class AccountUiState(val user: AccountProfile? = null, val busy: Boolean = false, val message: String? = null)

class AccountViewModel : ViewModel() {
    private val auth = FirebaseAuth.getInstance()
    private val mutableState = MutableStateFlow(AccountUiState())
    val state = mutableState.asStateFlow()
    private val listener = FirebaseAuth.AuthStateListener { updateUser() }

    init {
        auth.addAuthStateListener(listener)
        updateUser()
        // Reattach an in-progress browser flow after process / Activity recreation.
        auth.pendingAuthResult?.let { pending -> perform { pending.await(); updateUser() } }
    }
    private fun updateUser() {
        val user = auth.currentUser
        mutableState.update { it.copy(user = user?.let { AccountProfile(it.uid, it.email, it.displayName?.takeIf(String::isNotBlank), it.isEmailVerified) }) }
    }
    private fun perform(action: suspend () -> Unit) {
        if (mutableState.value.busy) return
        mutableState.update { it.copy(busy = true, message = null) }
        viewModelScope.launch {
            try { action() }
            catch (error: CancellationException) { throw error }
            catch (error: Exception) { mutableState.update { it.copy(message = errorMessage(error)) } }
            finally { updateUser(); mutableState.update { it.copy(busy = false) } }
        }
    }
    fun emailSignIn(email: String, password: String, register: Boolean) = perform {
        require(email.trim().isNotEmpty() && password.isNotEmpty())
        if (register) auth.createUserWithEmailAndPassword(email.trim(), password).await()
        else auth.signInWithEmailAndPassword(email.trim(), password).await()
    }
    fun google(activity: Activity, clientId: String) = perform {
        val option = GetSignInWithGoogleOption.Builder(clientId).build()
        val result = CredentialManager.create(activity).getCredential(activity, GetCredentialRequest.Builder().addCredentialOption(option).build())
        val credential = result.credential
        check(credential is CustomCredential && credential.type == GoogleIdTokenCredential.TYPE_GOOGLE_ID_TOKEN_CREDENTIAL)
        val token = GoogleIdTokenCredential.createFrom(credential.data).idToken
        auth.signInWithCredential(GoogleAuthProvider.getCredential(token, null)).await()
    }
    fun apple(activity: Activity) = perform {
        val provider = OAuthProvider.newBuilder("apple.com").setScopes(listOf("email", "name")).build()
        (auth.pendingAuthResult ?: auth.startActivityForSignInWithProvider(activity, provider)).await()
    }
    fun resetPassword(email: String) = perform {
        auth.sendPasswordResetEmail(email.trim()).await()
        mutableState.update { it.copy(message = "如果此邮箱已注册，将收到密码重置邮件。") }
    }
    fun verifyEmail() = perform {
        checkNotNull(auth.currentUser).sendEmailVerification().await()
        mutableState.update { it.copy(message = "验证邮件已发送，请检查收件箱。") }
    }
    fun reload() {
        if (auth.currentUser != null) perform { auth.currentUser?.reload()?.await() }
    }
    fun signOut(activity: Activity) = perform {
        auth.signOut()
        CredentialManager.create(activity).clearCredentialState(ClearCredentialStateRequest())
    }
    override fun onCleared() { auth.removeAuthStateListener(listener) }

    private fun errorMessage(error: Exception): String {
        if (error is GetCredentialCancellationException) return "登录已取消。"
        if (error is FirebaseAuthWebException && error.errorCode == "ERROR_WEB_CONTEXT_CANCELED") return "登录已取消。"
        return when ((error as? FirebaseAuthException)?.errorCode) {
            "ERROR_INVALID_CREDENTIAL", "ERROR_WRONG_PASSWORD", "ERROR_USER_NOT_FOUND" -> "邮箱或密码不正确。"
            "ERROR_EMAIL_ALREADY_IN_USE" -> "此邮箱已注册，请登录或重置密码。"
            "ERROR_WEAK_PASSWORD" -> "密码强度不足，请使用更长的密码。"
            "ERROR_OPERATION_NOT_ALLOWED" -> "此登录方式尚未启用。"
            "ERROR_ACCOUNT_EXISTS_WITH_DIFFERENT_CREDENTIAL" -> "请先使用原有登录方式登录此邮箱。"
            "ERROR_USER_DISABLED" -> "此账号已停用。"
            else -> "操作未完成，请检查网络或登录服务配置后重试。"
        }
    }
}
