import SwiftUI
import UIKit

/// Keeps argument hints outside the editable text while using the actual
/// editor's caret geometry, including wrapping and vertical scrolling.
struct ComposerTextInput: UIViewRepresentable {
    @Binding var text: String
    @Binding var isFocused: Bool
    let placeholder: String
    let textColor: Color
    let hintColor: Color
    let argumentHint: (String, Int) -> String?
    let onSubmit: () -> Void
    let onAcceptSuggestion: (() -> Void)?
    var onDismissCommandSurface: (() -> Void)? = nil
    var historySessionKey: String = ""
    var historyMessages: [String] = []

    func makeUIView(context: Context) -> ComposerInputView {
        let view = ComposerInputView()
        view.textView.delegate = context.coordinator
        context.coordinator.view = view
        view.onMoveToWindow = { [weak coordinator = context.coordinator] in
            coordinator?.synchronizeFocus()
        }
        return view
    }

    func updateUIView(_ view: ComposerInputView, context: Context) {
        let coordinator = context.coordinator
        coordinator.parent = self
        coordinator.updating = true
        let font = UIFont.preferredFont(forTextStyle: .body)
        view.updateAppearance(font: font, textColor: UIColor(textColor), hintColor: UIColor(hintColor))
        view.placeholderLabel.text = placeholder
        view.textView.acceptSuggestion = onAcceptSuggestion
        view.textView.dismissCommandSurface = onDismissCommandSurface
        view.textView.configureHistory(sessionKey: historySessionKey, messages: historyMessages)
        view.textView.accessibilityLabel = String(localized: "composer_placeholder")

        if view.textView.text != text {
            view.textView.text = text
            view.textView.selectedRange = NSRange(location: text.utf16.count, length: 0)
            view.scrollSelectionAfterLayout = true
        }
        coordinator.updating = false
        coordinator.refreshHint()
        coordinator.synchronizeFocus()
        view.invalidateIntrinsicContentSize()
        view.setNeedsLayout()
    }

    func sizeThatFits(_ proposal: ProposedViewSize, uiView: ComposerInputView, context: Context) -> CGSize? {
        guard let width = proposal.width, width > 0 else { return nil }
        let lineHeight = uiView.textView.font?.lineHeight ?? 19
        let height = uiView.textView.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude)).height
        return CGSize(width: width, height: ceil(min(max(height, lineHeight), lineHeight * 5)))
    }

    func makeCoordinator() -> Coordinator { Coordinator(parent: self) }

    final class Coordinator: NSObject, UITextViewDelegate {
        var parent: ComposerTextInput
        weak var view: ComposerInputView?
        var updating = false
        private var focusSynchronizationScheduled = false

        init(parent: ComposerTextInput) {
            self.parent = parent
            super.init()
            NotificationCenter.default.addObserver(self, selector: #selector(didBecomeActive),
                                                   name: UIApplication.didBecomeActiveNotification, object: nil)
        }

        deinit { NotificationCenter.default.removeObserver(self) }

        @objc private func didBecomeActive() { synchronizeFocus() }

        func synchronizeFocus() {
            guard !focusSynchronizationScheduled else { return }
            focusSynchronizationScheduled = true
            // UIKit focus changes can reenter SwiftUI's focus graph. Wait until
            // the representable's current layout transaction has completed.
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.focusSynchronizationScheduled = false
                guard let textView = self.view?.textView, textView.window != nil,
                      UIApplication.shared.applicationState == .active else { return }
                if self.parent.isFocused && !textView.isFirstResponder {
                    textView.becomeFirstResponder()
                } else if !self.parent.isFocused && textView.isFirstResponder {
                    textView.resignFirstResponder()
                }
            }
        }

        func refreshHint() {
            guard let view else { return }
            let text = view.textView.text ?? ""
            view.placeholderLabel.isHidden = !text.isEmpty || parent.placeholder.isEmpty
            view.hintLabel.text = parent.argumentHint(text, view.textView.selectedRange.location)
            view.hintLabel.accessibilityValue = view.hintLabel.text
            view.layoutHint()
        }

        func textViewDidBeginEditing(_ textView: UITextView) {
            if !updating { parent.isFocused = true }
        }

        func textViewDidEndEditing(_ textView: UITextView) {
            if !updating { parent.isFocused = false }
        }

        func textViewDidChange(_ textView: UITextView) {
            guard !updating else { return }
            if let editor = textView as? ComposerEditingTextView, !editor.recallingHistory {
                editor.promptHistory.reset()
            }
            parent.text = textView.text
            refreshHint()
            view?.invalidateIntrinsicContentSize()
        }

        func textViewDidChangeSelection(_ textView: UITextView) {
            if !updating { refreshHint() }
        }

        func scrollViewDidScroll(_ scrollView: UIScrollView) {
            view?.layoutHint()
        }

        func textView(_ textView: UITextView, shouldChangeTextIn range: NSRange, replacementText text: String) -> Bool {
            if text == "\n", textView.markedTextRange == nil {
                parent.onSubmit()
                return false
            }
            return true
        }
    }
}

final class ComposerInputView: UIView {
    let textView = ComposerEditingTextView()
    let placeholderLabel = UILabel()
    let hintLabel = UILabel()
    var onMoveToWindow: (() -> Void)?
    var scrollSelectionAfterLayout = false

    override init(frame: CGRect) {
        super.init(frame: frame)
        clipsToBounds = true
        textView.backgroundColor = .clear
        textView.textContainerInset = .zero
        textView.textContainer.lineFragmentPadding = 0
        textView.contentInsetAdjustmentBehavior = .never
        textView.returnKeyType = .send
        textView.adjustsFontForContentSizeCategory = true
        textView.accessibilityIdentifier = "composer.input"
        textView.onLayout = { [weak self] in self?.layoutHint() }
        // This editor is UIKit-backed, so attach keyboard controls directly to
        // its responder rather than relying on SwiftUI's keyboard toolbar.
        let keyboardToolbar = UIToolbar(frame: CGRect(x: 0, y: 0, width: 0, height: 44))
        let dismissKeyboard = UIBarButtonItem(
            image: UIImage(systemName: "keyboard.chevron.compact.down"),
            style: .plain, target: textView, action: #selector(UIResponder.resignFirstResponder))
        dismissKeyboard.accessibilityLabel = String(localized: "composer_done")
        dismissKeyboard.accessibilityIdentifier = "composer.keyboard.dismiss"
        keyboardToolbar.items = [.flexibleSpace(), dismissKeyboard]
        textView.inputAccessoryView = keyboardToolbar
        placeholderLabel.isUserInteractionEnabled = false
        placeholderLabel.adjustsFontForContentSizeCategory = true
        placeholderLabel.isAccessibilityElement = false
        hintLabel.numberOfLines = 1
        hintLabel.adjustsFontForContentSizeCategory = true
        hintLabel.lineBreakMode = .byTruncatingTail
        hintLabel.isUserInteractionEnabled = false
        hintLabel.accessibilityIdentifier = "composer.slash-argument-hint"
        hintLabel.accessibilityLabel = String(localized: "slash_command_argument_hint")
        addSubview(textView)
        addSubview(placeholderLabel)
        addSubview(hintLabel)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        onMoveToWindow?()
    }

    func updateAppearance(font: UIFont, textColor: UIColor, hintColor: UIColor) {
        // Reapplying the font on every draft update needlessly rewrites the
        // editor's attributed text, including an active IME composition.
        if textView.font != font { textView.font = font }
        if textView.textColor != textColor { textView.textColor = textColor }
        placeholderLabel.font = font
        placeholderLabel.textColor = hintColor
        hintLabel.font = font
        hintLabel.textColor = hintColor
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        textView.frame = bounds
        placeholderLabel.frame = CGRect(origin: .zero, size: CGSize(width: bounds.width, height: textView.font?.lineHeight ?? 19))
        if scrollSelectionAfterLayout {
            scrollSelectionAfterLayout = false
            textView.scrollRangeToVisible(textView.selectedRange)
        }
        layoutHint()
    }

    func layoutHint() {
        guard let hint = hintLabel.text, !hint.isEmpty else {
            hintLabel.isHidden = true
            return
        }
        let caret = textView.caretRect(for: textView.endOfDocument)
        let position = convert(caret, from: textView)
        let width = max(0, bounds.maxX - position.minX)
        hintLabel.frame = CGRect(x: position.minX, y: position.minY, width: width, height: position.height)
        hintLabel.isHidden = width <= 0 || !bounds.intersects(hintLabel.frame)
    }
}

final class ComposerEditingTextView: UITextView {
    var acceptSuggestion: (() -> Void)?
    var promptHistory = ComposerPromptHistory()
    private var historySessionKey = ""
    private var historyMessages: [String] = []
    private(set) var recallingHistory = false

    func configureHistory(sessionKey: String, messages: [String]) {
        if historySessionKey != sessionKey {
            promptHistory.reset()
            historySessionKey = sessionKey
        }
        historyMessages = messages
    }

    private func canRecallHistory(older: Bool) -> Bool {
        guard markedTextRange == nil, selectedRange.length == 0 else { return false }
        return promptHistory.canNavigate(text: text ?? "", cursor: selectedRange.location,
                                         older: older, hasMessages: historyMessages.contains { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty })
    }

    func recallHistory(older: Bool) {
        guard canRecallHistory(older: older),
              let recalled = promptHistory.navigate(messages: historyMessages, text: text ?? "",
                                                    cursor: selectedRange.location, older: older) else { return }
        recallingHistory = true
        text = recalled
        selectedRange = NSRange(location: recalled.utf16.count, length: 0)
        delegate?.textViewDidChange?(self)
        recallingHistory = false
        scrollRangeToVisible(selectedRange)
    }

    @objc private func recallOlderPrompt() { recallHistory(older: true) }
    @objc private func recallNewerPrompt() { recallHistory(older: false) }

    var dismissCommandSurface: (() -> Void)?
    var onLayout: (() -> Void)?

    override var keyCommands: [UIKeyCommand]? {
        var commands = super.keyCommands ?? []
        if dismissCommandSurface != nil, markedTextRange == nil {
            let command = UIKeyCommand(input: UIKeyCommand.inputEscape, modifierFlags: [], action: #selector(closeCommandSurface))
            command.wantsPriorityOverSystemBehavior = true
            commands.append(command)
        }
        commands.append(UIKeyCommand(input: "\r", modifierFlags: .shift, action: #selector(insertLineBreak)))
        if acceptSuggestion != nil {
            for input in ["\t", UIKeyCommand.inputRightArrow] {
                let command = UIKeyCommand(input: input, modifierFlags: [], action: #selector(useSuggestion))
                command.wantsPriorityOverSystemBehavior = true
                commands.append(command)
            }
        }
        for (input, older) in [(UIKeyCommand.inputUpArrow, true), (UIKeyCommand.inputDownArrow, false)] {
            if canRecallHistory(older: older) {
                let command = UIKeyCommand(input: input, modifierFlags: [],
                    action: older ? #selector(recallOlderPrompt) : #selector(recallNewerPrompt))
                command.wantsPriorityOverSystemBehavior = true
                commands.append(command)
            }
        }
        return commands
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        onLayout?()
    }

    @objc private func closeCommandSurface() { dismissCommandSurface?() }
    @objc private func useSuggestion() { acceptSuggestion?() }

    @objc private func insertLineBreak() {
        guard let selectedTextRange else { return }
        replace(selectedTextRange, withText: "\n")
        delegate?.textViewDidChange?(self)
    }
}

/// Matches the composer recall contract without depending on UIKit cursor geometry.
struct ComposerPromptHistory {
    private var entries: [String] = []
    private var index: Int?
    private var recalledText: String?

    mutating func reset() {
        entries = []
        index = nil
        recalledText = nil
    }

    func canNavigate(text: String, cursor: Int, older: Bool, hasMessages: Bool) -> Bool {
        if text.isEmpty { return older && hasMessages }
        return index != nil && text == recalledText && (cursor == 0 || cursor == text.utf16.count)
    }

    mutating func navigate(messages: [String], text: String, cursor: Int, older: Bool) -> String? {
        guard canNavigate(text: text, cursor: cursor, older: older, hasMessages: messages.contains {
            !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }) else { return nil }
        if index == nil || text != recalledText {
            entries = messages.filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
            index = entries.count
        }
        guard let index else { return nil }
        let next = min(entries.count, max(0, index + (older ? -1 : 1)))
        if next == entries.count {
            reset()
            return ""
        }
        self.index = next
        recalledText = entries[next]
        return entries[next]
    }
}
