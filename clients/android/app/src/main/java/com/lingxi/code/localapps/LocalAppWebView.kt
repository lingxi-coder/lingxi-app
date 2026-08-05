package com.lingxi.code.localapps

import android.annotation.SuppressLint
import android.graphics.Bitmap
import android.net.Uri
import android.os.Build
import android.webkit.JavascriptInterface
import android.webkit.CookieManager
import android.webkit.SafeBrowsingResponse
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.viewinterop.AndroidView
import com.lingxi.code.BuildConfig
import com.lingxi.code.R
import org.json.JSONArray
import org.json.JSONObject

data class LocalAppBridgeMessage(
    val appId: String,
    val requestId: String,
    val operation: String,
    val payloadJson: String?,
)

data class LocalAppUiTarget(
    val elementId: String? = null,
    val role: String? = null,
    val name: String? = null,
)

sealed interface LocalAppUiAutomationAction {
    data object Inspect : LocalAppUiAutomationAction
    data class Click(val target: LocalAppUiTarget) : LocalAppUiAutomationAction
    data class Fill(val target: LocalAppUiTarget, val value: String) : LocalAppUiAutomationAction
    data class Select(val target: LocalAppUiTarget, val value: String) : LocalAppUiAutomationAction
    data class Toggle(val target: LocalAppUiTarget, val checked: Boolean) : LocalAppUiAutomationAction
    data class Scroll(val x: Int, val y: Int) : LocalAppUiAutomationAction
    data class Navigate(val path: String) : LocalAppUiAutomationAction
    data object Back : LocalAppUiAutomationAction
    data object Reload : LocalAppUiAutomationAction
}

data class LocalAppUiExecutionResult(
    val resultJson: String?,
    val error: String?,
)

private data class RawJson(val json: String)

/**
 * Executes only the versioned, structured UI action vocabulary.
 *
 * Agent input is encoded as JSON string literals and inserted into fixed host
 * scripts; callers cannot supply executable JavaScript. The controller never
 * exposes [WebView.evaluateJavascript] itself.
 */
class LocalAppWebViewController internal constructor(private val webView: WebView) {
    fun execute(
        action: LocalAppUiAutomationAction,
        onResult: (LocalAppUiExecutionResult) -> Unit = {},
    ) {
        when (action) {
            LocalAppUiAutomationAction.Inspect,
            is LocalAppUiAutomationAction.Click,
            is LocalAppUiAutomationAction.Fill,
            is LocalAppUiAutomationAction.Select,
            is LocalAppUiAutomationAction.Toggle,
            is LocalAppUiAutomationAction.Scroll -> executeStructuredAction(action, onResult)
            is LocalAppUiAutomationAction.Navigate -> {
                val current = Uri.parse(webView.url.orEmpty())
                val requested = Uri.parse(action.path)
                val target = if (requested.isAbsolute) {
                    requested
                } else {
                    current.buildUpon().encodedPath(action.path).clearQuery().build()
                }
                if (target.sameTrustedOrigin(current)) {
                    webView.loadUrl(target.toString())
                    onResult(
                        LocalAppUiExecutionResult(
                            resultJson = jsonObjectString(
                                "ok" to true,
                                "action" to "navigate",
                                "url" to target.toString(),
                            ),
                            error = null,
                        ),
                    )
                } else {
                    onResult(LocalAppUiExecutionResult(resultJson = null, error = "Only trusted loopback navigation is allowed"))
                }
            }
            LocalAppUiAutomationAction.Back -> if (webView.canGoBack()) {
                webView.goBack()
                onResult(
                    LocalAppUiExecutionResult(
                        resultJson = jsonObjectString("ok" to true, "action" to "back"),
                        error = null,
                    ),
                )
            } else {
                onResult(LocalAppUiExecutionResult(resultJson = null, error = "WebView cannot navigate back"))
            }
            LocalAppUiAutomationAction.Reload -> {
                webView.reload()
                onResult(
                    LocalAppUiExecutionResult(
                        resultJson = jsonObjectString("ok" to true, "action" to "reload"),
                        error = null,
                    ),
                )
            }
        }
    }

    fun resolveBridgeRequest(requestId: String, ok: Boolean, payloadJson: String?) {
        val encodedPayload = jsonStringLiteral(payloadJson ?: "null")
        val script =
            "window.lingxi?.v1?.__resolve(${jsonStringLiteral(requestId)},${if (ok) "true" else "false"},JSON.parse($encodedPayload))"
        fixedScript(script) {}
    }

    private fun executeStructuredAction(
        action: LocalAppUiAutomationAction,
        onResult: (LocalAppUiExecutionResult) -> Unit,
    ) {
        val requestJson = buildLocalAppUiExecutionRequest(action)
        fixedScript(buildLocalAppUiExecutionScript(requestJson)) { rawResult ->
            onResult(parseLocalAppUiExecutionResult(rawResult))
        }
    }

    private fun fixedScript(script: String, onResult: (String?) -> Unit) {
        webView.evaluateJavascript(script, onResult)
    }
}

internal fun buildLocalAppUiExecutionRequest(action: LocalAppUiAutomationAction): String {
    return when (action) {
        LocalAppUiAutomationAction.Inspect -> jsonObjectString("action" to "inspect")
        is LocalAppUiAutomationAction.Click -> {
            jsonObjectString(
                "action" to "click",
                "target" to RawJson(action.target.toJsonString()),
            )
        }
        is LocalAppUiAutomationAction.Fill -> {
            jsonObjectString(
                "action" to "fill",
                "target" to RawJson(action.target.toJsonString()),
                "value" to action.value,
            )
        }
        is LocalAppUiAutomationAction.Select -> {
            jsonObjectString(
                "action" to "select",
                "target" to RawJson(action.target.toJsonString()),
                "value" to action.value,
            )
        }
        is LocalAppUiAutomationAction.Toggle -> {
            jsonObjectString(
                "action" to "toggle",
                "target" to RawJson(action.target.toJsonString()),
                "checked" to action.checked,
            )
        }
        is LocalAppUiAutomationAction.Scroll -> {
            jsonObjectString(
                "action" to "scroll",
                "x" to action.x,
                "y" to action.y,
            )
        }
        is LocalAppUiAutomationAction.Navigate,
        LocalAppUiAutomationAction.Back,
        LocalAppUiAutomationAction.Reload -> error("Structured script is not used for $action")
    }
}

internal fun buildLocalAppUiExecutionScript(requestJson: String): String =
    """
    (() => {
      const request = $requestJson;
      const clean = value => String(value ?? '').replace(/\s+/g, ' ').trim().slice(0, 500);
      const roleOf = element => clean(element.getAttribute('role') || ({
        BUTTON: 'button',
        A: 'link',
        INPUT: ['button', 'submit', 'reset'].includes((element.type || '').toLowerCase())
          ? 'button'
          : element.type === 'checkbox'
            ? 'checkbox'
            : (element.type === 'radio' ? 'radio' : 'textbox'),
        SELECT: 'combobox',
        TEXTAREA: 'textbox'
      })[element.tagName] || '');
      const nameOf = element => {
        const labelledBy = clean(element.getAttribute('aria-labelledby'));
        const labelled = labelledBy
          ? labelledBy.split(/\s+/).map(id => document.getElementById(id)).find(Boolean)
          : null;
        const explicit = element.id
          ? document.querySelector(`label[for="${'$'}{CSS.escape(element.id)}"]`)
          : null;
        return clean(
          element.getAttribute('aria-label')
            || labelled?.textContent
            || explicit?.textContent
            || element.placeholder
            || element.innerText
            || element.value
        );
      };
      const candidates = () => Array.from(document.querySelectorAll(
        'button,a[href],input,select,textarea,[role],[tabindex],[contenteditable="true"]'
      ));
      const findTarget = target => {
        if (!target) return null;
        if (target.elementId) {
          const byId = document.getElementById(target.elementId);
          if (byId) return byId;
        }
        return candidates().find(element =>
          (!target.role || roleOf(element).toLowerCase() === clean(target.role).toLowerCase()) &&
          (!target.name || nameOf(element).toLowerCase() === clean(target.name).toLowerCase())
        ) || null;
      };
      const dispatchValueChange = element => {
        element.dispatchEvent(new Event('input', { bubbles: true }));
        element.dispatchEvent(new Event('change', { bubbles: true }));
      };
      const setNativeValue = (element, value) => {
        const prototype = element instanceof HTMLTextAreaElement
          ? HTMLTextAreaElement.prototype
          : element instanceof HTMLSelectElement
            ? HTMLSelectElement.prototype
            : HTMLInputElement.prototype;
        const setter = Object.getOwnPropertyDescriptor(prototype, 'value')?.set;
        if (!setter) throw new Error('Target value cannot be changed');
        setter.call(element, value);
      };
      const encode = result => JSON.stringify({ ok: true, result });
      const fail = message => JSON.stringify({
        ok: false,
        error: clean(message) || 'Lingxi UI action failed'
      });
      const snapshot = () => ({
        title: clean(document.title),
        url: location.href,
        elements: candidates().slice(0, 200).map(element => {
          const rect = element.getBoundingClientRect();
          const sensitive = element instanceof HTMLInputElement && ['hidden', 'password'].includes(element.type);
          return {
            elementId: clean(element.id) || null,
            role: roleOf(element) || null,
            name: nameOf(element) || null,
            value: sensitive ? null : clean('value' in element ? element.value : null),
            checked: typeof element.checked === 'boolean' ? element.checked : null,
            disabled: !!element.disabled,
            visible: rect.width > 0 && rect.height > 0
          };
        })
      });
      try {
        if (request.action === 'inspect') return encode(snapshot());
        if (request.action === 'scroll') {
          window.scrollBy({ left: Number(request.x || 0), top: Number(request.y || 0), behavior: 'auto' });
          return encode({ ok: true, action: 'scroll', x: Number(request.x || 0), y: Number(request.y || 0) });
        }
        const element = findTarget(request.target);
        if (!element) throw new Error('UI target was not found');
        if (element.disabled) throw new Error('UI target is disabled');
        if (request.action === 'click') {
          element.click();
        } else if (request.action === 'fill') {
          if (!(element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement || element.isContentEditable)) {
            throw new Error('Target cannot be filled');
          }
          if (element.isContentEditable) {
            element.textContent = request.value || '';
          } else {
            setNativeValue(element, request.value || '');
          }
          dispatchValueChange(element);
        } else if (request.action === 'select') {
          if (!(element instanceof HTMLSelectElement)) throw new Error('Target is not a select element');
          const option = Array.from(element.options).find(item =>
            item.value === request.value || clean(item.textContent) === clean(request.value)
          );
          if (!option) throw new Error('Select option was not found');
          setNativeValue(element, option.value);
          dispatchValueChange(element);
        } else if (request.action === 'toggle') {
          const desired = !!request.checked;
          if (element instanceof HTMLInputElement && (element.type === 'checkbox' || element.type === 'radio')) {
            if (!!element.checked !== desired) element.click();
          } else if (['checkbox', 'switch'].includes(roleOf(element).toLowerCase())) {
            const current = clean(element.getAttribute('aria-checked')).toLowerCase() === 'true';
            if (current !== desired) element.click();
          } else {
            throw new Error('Target is not toggleable');
          }
        } else {
          throw new Error('Unsupported UI action');
        }
        return encode({
          ok: true,
          action: request.action,
          target: {
            elementId: clean(element.id) || null,
            role: roleOf(element) || null,
            name: nameOf(element) || null
          }
        });
      } catch (error) {
        return fail(error && error.message ? error.message : String(error));
      }
    })()
    """.trimIndent()

internal fun parseLocalAppUiExecutionResult(rawResult: String?): LocalAppUiExecutionResult {
    val trimmed = rawResult?.trim().orEmpty()
    if (trimmed.isEmpty() || trimmed == "null" || trimmed == "false" || trimmed == "undefined") {
        return LocalAppUiExecutionResult(resultJson = null, error = "WebView UI automation returned no result")
    }
    val envelopeText = (if (trimmed.startsWith('"')) decodeJsonStringLiteral(trimmed) else trimmed)
        ?: return LocalAppUiExecutionResult(resultJson = null, error = "WebView UI automation returned malformed payload")
    val okField = extractTopLevelJsonField(envelopeText, "ok")
        ?: return LocalAppUiExecutionResult(resultJson = null, error = "WebView UI automation returned malformed payload")
    if (okField == "false") {
        return LocalAppUiExecutionResult(
            resultJson = null,
            error = extractTopLevelJsonField(envelopeText, "error")
                ?.takeIf { it.startsWith('"') }
                ?.let(::decodeJsonStringLiteral)
                ?.takeIf { it.isNotBlank() }
                ?: "WebView UI automation failed",
        )
    }
    if (okField != "true") {
        return LocalAppUiExecutionResult(resultJson = null, error = "WebView UI automation returned malformed payload")
    }
    val resultField = extractTopLevelJsonField(envelopeText, "result")
        ?: return LocalAppUiExecutionResult(resultJson = null, error = "WebView UI automation returned no result")
    if (resultField == "null") {
        return LocalAppUiExecutionResult(resultJson = null, error = "WebView UI automation returned no result")
    }
    return LocalAppUiExecutionResult(resultJson = resultField, error = null)
}

private fun LocalAppUiTarget.toJsonString(): String =
    jsonObjectString(
        "elementId" to elementId,
        "role" to role,
        "name" to name,
    )

private fun jsonObjectString(vararg entries: Pair<String, Any?>): String = buildString {
    append('{')
    var first = true
    for ((key, value) in entries) {
        if (value == null || value == JSONObject.NULL) continue
        if (!first) append(',')
        first = false
        append(jsonStringLiteral(key))
        append(':')
        append(jsonValueToJson(value))
    }
    append('}')
}

private fun jsonValueToJson(value: Any?): String = when (value) {
    null,
    JSONObject.NULL -> "null"
    is RawJson -> value.json
    is JSONObject -> value.toString()
    is JSONArray -> value.toString()
    is Number,
    is Boolean -> value.toString()
    is String -> jsonStringLiteral(value)
    else -> JSONObject.wrap(value)?.toString() ?: jsonStringLiteral(value.toString())
}

private fun jsonStringLiteral(value: String): String = buildString(value.length + 2) {
    append('"')
    value.forEach { ch ->
        when (ch) {
            '\\' -> append("\\\\")
            '"' -> append("\\\"")
            '\b' -> append("\\b")
            '\u000C' -> append("\\f")
            '\n' -> append("\\n")
            '\r' -> append("\\r")
            '\t' -> append("\\t")
            else -> if (ch.code < 0x20) append("\\u%04x".format(ch.code)) else append(ch)
        }
    }
    append('"')
}

private fun decodeJsonStringLiteral(value: String): String? {
    if (value.length < 2 || value.first() != '"' || value.last() != '"') return null
    val out = StringBuilder(value.length - 2)
    var index = 1
    while (index < value.lastIndex) {
        val ch = value[index++]
        if (ch != '\\') {
            out.append(ch)
            continue
        }
        if (index >= value.lastIndex + 1) return null
        when (val escaped = value[index++]) {
            '"', '\\', '/' -> out.append(escaped)
            'b' -> out.append('\b')
            'f' -> out.append('\u000C')
            'n' -> out.append('\n')
            'r' -> out.append('\r')
            't' -> out.append('\t')
            'u' -> {
                if (index + 4 > value.length) return null
                val hex = value.substring(index, index + 4)
                out.append(hex.toIntOrNull(16)?.toChar() ?: return null)
                index += 4
            }
            else -> return null
        }
    }
    return out.toString()
}

private fun extractTopLevelJsonField(json: String, fieldName: String): String? {
    val text = json.trim()
    if (text.length < 2 || text.first() != '{' || text.last() != '}') return null
    var index = 1
    while (index < text.lastIndex) {
        index = skipJsonWhitespace(text, index)
        if (index >= text.lastIndex) break
        if (text[index] == ',') {
            index++
            continue
        }
        if (text[index] != '"') return null
        val keyEnd = findJsonStringEnd(text, index) ?: return null
        val key = decodeJsonStringLiteral(text.substring(index, keyEnd + 1)) ?: return null
        index = skipJsonWhitespace(text, keyEnd + 1)
        if (index >= text.lastIndex || text[index] != ':') return null
        index = skipJsonWhitespace(text, index + 1)
        val valueEnd = findJsonValueEnd(text, index) ?: return null
        if (key == fieldName) return text.substring(index, valueEnd)
        index = valueEnd
    }
    return null
}

private fun skipJsonWhitespace(text: String, start: Int): Int {
    var index = start
    while (index < text.length && text[index].isWhitespace()) index++
    return index
}

private fun findJsonStringEnd(text: String, start: Int): Int? {
    var index = start + 1
    while (index < text.length) {
        when (text[index]) {
            '\\' -> index += 2
            '"' -> return index
            else -> index++
        }
    }
    return null
}

private fun findJsonValueEnd(text: String, start: Int): Int? {
    if (start >= text.length) return null
    return when (text[start]) {
        '"' -> findJsonStringEnd(text, start)?.plus(1)
        '{', '[' -> {
            val open = text[start]
            val close = if (open == '{') '}' else ']'
            var depth = 0
            var index = start
            while (index < text.length) {
                when (text[index]) {
                    '"' -> {
                        index = findJsonStringEnd(text, index) ?: return null
                    }
                    open -> depth++
                    close -> {
                        depth--
                        if (depth == 0) return index + 1
                    }
                }
                index++
            }
            null
        }
        else -> {
            var index = start
            while (index < text.length && text[index] != ',' && text[index] != '}' && text[index] != ']') {
                index++
            }
            index
        }
    }
}

private class BoundLingXiBridge(
    private val appId: String,
    private val onMessage: (LocalAppBridgeMessage) -> Unit,
) {
    @JavascriptInterface
    fun postMessage(message: String) {
        val json = runCatching { JSONObject(message) }.getOrNull() ?: return
        val requestId = json.optString("requestId").takeIf { it.isNotBlank() } ?: return
        val operation = json.optString("operation").takeIf { it.isNotBlank() } ?: return
        onMessage(
            LocalAppBridgeMessage(
                appId = appId,
                requestId = requestId,
                operation = operation,
                payloadJson = json.opt("payload")?.let { payload ->
                    if (payload is String) jsonStringLiteral(payload) else jsonValueToJson(payload)
                },
            ),
        )
    }
}

@SuppressLint("SetJavaScriptEnabled")
@Composable
fun LocalAppWebView(
    appId: String,
    url: String,
    onExternalNavigation: (String) -> Unit,
    modifier: Modifier = Modifier,
    onBridgeRequest: (LocalAppBridgeMessage) -> Unit = {},
    onControllerReady: (LocalAppWebViewController) -> Unit = {},
) {
    var pendingExternalUrl by remember(appId) { mutableStateOf<String?>(null) }
    var webView by remember(appId) { mutableStateOf<WebView?>(null) }
    val trustedOrigin = remember(url) { Uri.parse(url).takeIf { it.isTrustedLoopback() } }

    AndroidView(
        modifier = modifier,
        factory = { context ->
            WebView(context).apply webView@ {
                webView = this
                WebView.setWebContentsDebuggingEnabled(BuildConfig.DEBUG)
                settings.javaScriptEnabled = true
                settings.domStorageEnabled = true
                settings.allowFileAccess = false
                settings.allowContentAccess = false
                @Suppress("DEPRECATION")
                settings.allowFileAccessFromFileURLs = false
                @Suppress("DEPRECATION")
                settings.allowUniversalAccessFromFileURLs = false
                settings.mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
                settings.safeBrowsingEnabled = true
                CookieManager.getInstance().apply {
                    setAcceptCookie(false)
                    setAcceptThirdPartyCookies(this@webView, false)
                }
                addJavascriptInterface(BoundLingXiBridge(appId, onBridgeRequest), "LingXiNativeV1")
                webViewClient = object : WebViewClient() {
                    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                        val target = request.url
                        if (target.sameTrustedOrigin(trustedOrigin)) return false
                        if (request.isForMainFrame) pendingExternalUrl = target.toString()
                        return true
                    }

                    override fun onPageStarted(view: WebView, url: String, favicon: Bitmap?) {
                        if (!Uri.parse(url).sameTrustedOrigin(trustedOrigin)) view.stopLoading()
                    }

                    override fun onPageFinished(view: WebView, url: String) {
                        view.evaluateJavascript(LINGXI_V1_BOOTSTRAP, null)
                    }

                    override fun onSafeBrowsingHit(
                        view: WebView,
                        request: WebResourceRequest,
                        threatType: Int,
                        callback: SafeBrowsingResponse,
                    ) {
                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
                            callback.backToSafety(true)
                        }
                    }

                    override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? {
                        if (!request.url.sameTrustedOrigin(trustedOrigin)) {
                            return WebResourceResponse("text/plain", "utf-8", 403, "Blocked", emptyMap(), null)
                        }
                        return super.shouldInterceptRequest(view, request)
                    }
                }
                onControllerReady(LocalAppWebViewController(this))
                if (trustedOrigin != null) {
                    tag = url
                    loadUrl(url)
                }
            }
        },
        update = { view ->
            if (trustedOrigin != null && view.tag != url) {
                view.tag = url
                view.loadUrl(url)
            }
        },
    )

    DisposableEffect(appId) {
        onDispose {
            webView?.apply {
                stopLoading()
                removeJavascriptInterface("LingXiNativeV1")
                destroy()
            }
            webView = null
        }
    }

    pendingExternalUrl?.let { target ->
        AlertDialog(
            onDismissRequest = { pendingExternalUrl = null },
            title = { Text(stringResource(R.string.local_apps_leave_app_title)) },
            text = { Text(stringResource(R.string.local_apps_leave_app_detail, target)) },
            dismissButton = {
                TextButton(onClick = { pendingExternalUrl = null }) { Text(stringResource(R.string.common_cancel)) }
            },
            confirmButton = {
                TextButton(onClick = {
                    pendingExternalUrl = null
                    onExternalNavigation(target)
                }) { Text(stringResource(R.string.onboarding_cta_continue)) }
            },
        )
    }
}

private fun Uri?.sameTrustedOrigin(other: Uri?): Boolean =
    this != null && other != null &&
        isTrustedLoopback() && other.isTrustedLoopback() &&
        scheme == other.scheme && host == other.host && effectivePort() == other.effectivePort()

private fun Uri.isTrustedLoopback(): Boolean =
    scheme == "http" && host?.lowercase() in setOf("127.0.0.1", "localhost")

private fun Uri.effectivePort(): Int = if (port >= 0) port else if (scheme == "https") 443 else 80

private const val LINGXI_V1_BOOTSTRAP = """
(() => {
  if (window.lingxi?.v1) return;
  const pending = new Map();
  const request = (operation, payload) => new Promise((resolve, reject) => {
    const requestId = (crypto.randomUUID ? crypto.randomUUID() : Date.now().toString(36) + Math.random().toString(36).slice(2));
    pending.set(requestId, {resolve, reject});
    LingXiNativeV1.postMessage(JSON.stringify({requestId, operation, payload}));
  });
  const v1 = Object.freeze({
    data: Object.freeze({
      query: (payload) => request('query_data', payload),
      mutate: (payload) => request('mutate_data', payload)
    }),
    network: Object.freeze({
      fetch: (payload) => request('network_request', payload),
      request: (payload) => request('network_request', payload)
    }),
    runtime: Object.freeze({
      info: () => request('runtime_status', null),
      status: () => request('runtime_status', null)
    }),
    __resolve: (requestId, ok, payload) => {
      const handler = pending.get(requestId);
      if (!handler) return;
      pending.delete(requestId);
      if (ok) handler.resolve(payload); else handler.reject(payload);
    }
  });
  Object.defineProperty(window, 'lingxi', {value: Object.freeze({v1}), configurable: false, writable: false});
})();
"""
