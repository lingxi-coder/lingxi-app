/*
 * Copyright (C) 2025 OpenMinis contributors
 * Copyright (C) 2026 LingXi contributors
 * SPDX-License-Identifier: GPL-3.0-only
 */
package com.lingxi.code.terminal.view

import android.content.Context
import android.text.InputType
import android.view.KeyEvent
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import android.widget.EditText
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.viewinterop.AndroidView

class TerminalInputController {
    internal var view: TerminalInputEditText? = null
    val isFocused get() = view?.isFocused == true

    fun showKeyboard() = view?.let {
        it.requestFocus()
        (it.context.getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager)
            ?.showSoftInput(it, InputMethodManager.SHOW_IMPLICIT)
    }

    fun hideKeyboard() = view?.let {
        (it.context.getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager)
            ?.hideSoftInputFromWindow(it.windowToken, 0)
        it.clearFocus()
    }
}

@Composable
fun rememberTerminalInputController() = remember { TerminalInputController() }

@Composable
fun TerminalInputView(
    onInput: (ByteArray) -> Unit,
    applicationCursorKeys: Boolean,
    controller: TerminalInputController,
    modifier: Modifier = Modifier,
) {
    AndroidView(
        modifier = modifier,
        factory = { context -> TerminalInputEditText(context) },
        update = { view ->
            view.bind(onInput) { applicationCursorKeys }
            controller.view = view
        },
    )
    DisposableEffect(controller) {
        onDispose { controller.view = null }
    }
}

class TerminalInputEditText(context: Context) : EditText(context) {
    private var onInput: (ByteArray) -> Unit = {}
    private var applicationCursorKeys: () -> Boolean = { false }

    init {
        alpha = 0f
        isCursorVisible = false
        setSingleLine()
        isFocusableInTouchMode = true
        inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS or
            InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD
        imeOptions = EditorInfo.IME_FLAG_NO_FULLSCREEN or EditorInfo.IME_FLAG_NO_EXTRACT_UI
        setBackgroundColor(android.graphics.Color.TRANSPARENT)
    }

    fun bind(onInput: (ByteArray) -> Unit, applicationCursorKeys: () -> Boolean) {
        this.onInput = onInput
        this.applicationCursorKeys = applicationCursorKeys
    }

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
        outAttrs.inputType = inputType
        outAttrs.imeOptions = imeOptions
        return object : BaseInputConnection(this, false) {
            override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
                text?.takeIf { it.isNotEmpty() }?.let { send(it.toString().toByteArray()) }
                return true
            }
            override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean =
                commitText(text, newCursorPosition)
            override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
                repeat(beforeLength) { send(byteArrayOf(0x7f)) }
                return true
            }
            override fun sendKeyEvent(event: KeyEvent?): Boolean =
                event?.let(::handleKey) ?: false
            override fun finishComposingText() = true
        }
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean =
        handleKey(event) || super.onKeyDown(keyCode, event)

    private fun handleKey(event: KeyEvent): Boolean {
        if (event.action != KeyEvent.ACTION_DOWN) return true
        val bytes = when (event.keyCode) {
            KeyEvent.KEYCODE_ENTER -> byteArrayOf(0x0d)
            KeyEvent.KEYCODE_DEL -> byteArrayOf(0x7f)
            KeyEvent.KEYCODE_FORWARD_DEL -> "\u001b[3~".toByteArray()
            KeyEvent.KEYCODE_TAB -> byteArrayOf(9)
            KeyEvent.KEYCODE_ESCAPE -> byteArrayOf(0x1b)
            KeyEvent.KEYCODE_DPAD_UP -> arrow('A')
            KeyEvent.KEYCODE_DPAD_DOWN -> arrow('B')
            KeyEvent.KEYCODE_DPAD_RIGHT -> arrow('C')
            KeyEvent.KEYCODE_DPAD_LEFT -> arrow('D')
            KeyEvent.KEYCODE_MOVE_HOME -> "\u001b[H".toByteArray()
            KeyEvent.KEYCODE_MOVE_END -> "\u001b[F".toByteArray()
            KeyEvent.KEYCODE_PAGE_UP -> "\u001b[5~".toByteArray()
            KeyEvent.KEYCODE_PAGE_DOWN -> "\u001b[6~".toByteArray()
            else -> {
                val unicode = event.unicodeChar
                if (unicode == 0) return false
                val character = unicode.toChar()
                when {
                    event.isCtrlPressed && character.uppercaseChar() in 'A'..'Z' ->
                        byteArrayOf((character.uppercaseChar() - 'A' + 1).toByte())
                    event.isAltPressed -> byteArrayOf(0x1b) + character.toString().toByteArray()
                    else -> character.toString().toByteArray()
                }
            }
        }
        send(bytes)
        return true
    }

    private fun arrow(direction: Char): ByteArray =
        if (applicationCursorKeys()) "\u001bO$direction".toByteArray()
        else "\u001b[$direction".toByteArray()

    private fun send(bytes: ByteArray) = onInput(bytes)
}
