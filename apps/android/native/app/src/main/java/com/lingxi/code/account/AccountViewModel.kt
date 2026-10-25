package com.lingxi.code.account

import android.app.Activity
import com.lingxi.code.R
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
data class AccountUiState(val user: AccountProfile? = null, val busy: Boolean = false, val messageRes: Int? = null)

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
        mutableState.update { it.copy(busy = true, messageRes = null) }
        viewModelScope.launch {
            try { action() }
            catch (error: CancellationException) { throw error }
            catch (error: Exception) { mutableState.update { it.copy(messageRes = errorMessage(error)) } }
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
        mutableState.update { it.copy(messageRes = R.string.account_reset_sent) }
    }
    fun verifyEmail() = perform {
        checkNotNull(auth.currentUser).sendEmailVerification().await()
        mutableState.update { it.copy(messageRes = R.string.account_verification_sent) }
    }
    fun reload() {
        if (auth.currentUser != null) perform { auth.currentUser?.reload()?.await() }
    }
    fun signOut(activity: Activity) = perform {
        auth.signOut()
        CredentialManager.create(activity).clearCredentialState(ClearCredentialStateRequest())
    }
    override fun onCleared() { auth.removeAuthStateListener(listener) }

    private fun errorMessage(error: Exception): Int {
        if (error is GetCredentialCancellationException) return R.string.account_error_cancelled
        if (error is FirebaseAuthWebException && error.errorCode == "ERROR_WEB_CONTEXT_CANCELED") return R.string.account_error_cancelled
        return when ((error as? FirebaseAuthException)?.errorCode) {
            "ERROR_INVALID_CREDENTIAL", "ERROR_WRONG_PASSWORD", "ERROR_USER_NOT_FOUND" -> R.string.account_error_credentials
            "ERROR_EMAIL_ALREADY_IN_USE" -> R.string.account_error_existing
            "ERROR_WEAK_PASSWORD" -> R.string.account_error_weak_password
            "ERROR_OPERATION_NOT_ALLOWED" -> R.string.account_error_provider_disabled
            "ERROR_ACCOUNT_EXISTS_WITH_DIFFERENT_CREDENTIAL" -> R.string.account_error_other_provider
            "ERROR_USER_DISABLED" -> R.string.account_error_disabled
            else -> R.string.account_error_configuration
        }
    }
}
