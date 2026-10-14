import SwiftUI
import UIKit
import XCTest

@testable import LingxiCode

@MainActor
final class ComposerTextInputTests: XCTestCase {
    private func input(text: String, width: CGFloat = 220, lines: CGFloat = 5) -> ComposerInputView {
        let view = ComposerInputView()
        let font = UIFont.systemFont(ofSize: 15.5)
        view.textView.font = font
        view.hintLabel.font = font
        view.frame = CGRect(x: 0, y: 0, width: width, height: ceil(font.lineHeight * lines))
        view.textView.text = text
        view.setNeedsLayout()
        view.layoutIfNeeded()
        return view
    }

    func testArgumentGhostUsesEndCaretAndTruncatesWithoutChangingDraft() {
        let draft = "/code-review "
        let hint = "[low|medium|high|xhigh|max|ultra] [--fix] [--comment] [<pr#>|<branch>|<path>]"
        let view = input(text: draft, lines: 1)
        view.hintLabel.text = hint
        view.layoutHint()

        let caret = view.convert(view.textView.caretRect(for: view.textView.endOfDocument), from: view.textView)
        XCTAssertEqual(view.textView.text, draft)
        XCTAssertEqual(view.hintLabel.text, hint)
        XCTAssertEqual(view.hintLabel.frame.minX, caret.minX, accuracy: 0.5)
        XCTAssertEqual(view.hintLabel.frame.minY, caret.minY, accuracy: 0.5)
        XCTAssertEqual(view.hintLabel.frame.maxX, view.bounds.maxX, accuracy: 0.5)
        XCTAssertEqual(view.hintLabel.numberOfLines, 1)
        XCTAssertEqual(view.hintLabel.lineBreakMode, .byTruncatingTail)
        XCTAssertFalse(view.hintLabel.isHidden)
        XCTAssertEqual(view.textView.accessibilityIdentifier, "composer.input")
        XCTAssertEqual(view.hintLabel.accessibilityIdentifier, "composer.slash-argument-hint")
    }

    func testArgumentGhostFollowsWrappedAndScrolledEditor() {
        let draft = "/deploy \"" + String(repeating: "quoted argument ", count: 35) + "\" "
        let view = input(text: draft)
        view.hintLabel.text = "[region] [tag]"
        view.layoutHint()
        XCTAssertGreaterThan(view.textView.contentSize.height, view.bounds.height)
        XCTAssertTrue(view.hintLabel.isHidden, "The end-of-draft ghost must be clipped while its line is offscreen")

        let bottomOffset = max(0, view.textView.contentSize.height - view.textView.bounds.height)
        view.textView.setContentOffset(CGPoint(x: 0, y: bottomOffset), animated: false)
        view.layoutHint()
        let caret = view.convert(view.textView.caretRect(for: view.textView.endOfDocument), from: view.textView)
        XCTAssertEqual(view.hintLabel.frame.minY, caret.minY, accuracy: 0.5)
        XCTAssertFalse(view.hintLabel.isHidden)
        XCTAssertLessThan(view.hintLabel.frame.minY, view.bounds.maxY)
        XCTAssertEqual(view.textView.text, draft)
    }

    func testSuggestionHardwareKeysAppearOnlyWhenSuggestionCanBeAccepted() {
        let view = ComposerEditingTextView()
        let before = view.keyCommands?.filter { $0.input == "\t" || $0.input == UIKeyCommand.inputRightArrow }.count ?? 0
        view.acceptSuggestion = {}
        let after = view.keyCommands?.filter { $0.input == "\t" || $0.input == UIKeyCommand.inputRightArrow }.count ?? 0
        XCTAssertEqual(after, before + 2)
    }

    func testReturnSubmitsRawDraftAndPreservesMarkedTextComposition() {
        var draft = "/review "
        var submitted: [String] = []
        let editor = ComposerTextInput(
            text: Binding(get: { draft }, set: { draft = $0 }),
            isFocused: .constant(false), placeholder: "", textColor: .primary, hintColor: .secondary,
            argumentHint: { _, _ in "[instructions]" }, onSubmit: { submitted.append(draft) },
            onAcceptSuggestion: nil
        )
        let coordinator = editor.makeCoordinator()
        let view = input(text: draft)
        coordinator.view = view
        view.textView.delegate = coordinator

        XCTAssertFalse(coordinator.textView(view.textView, shouldChangeTextIn: view.textView.selectedRange, replacementText: "\n"))
        XCTAssertEqual(submitted, ["/review "])
        view.textView.setMarkedText("正在输入", selectedRange: NSRange(location: 4, length: 0))
        XCTAssertNotNil(view.textView.markedTextRange)
        XCTAssertTrue(coordinator.textView(view.textView, shouldChangeTextIn: view.textView.selectedRange, replacementText: "\n"))
        XCTAssertEqual(submitted.count, 1)
    }

    func testShiftReturnInsertsNewlineWithoutSubmitting() throws {
        var draft = "first line"
        var submissions = 0
        let editor = ComposerTextInput(
            text: Binding(get: { draft }, set: { draft = $0 }),
            isFocused: .constant(false), placeholder: "", textColor: .primary, hintColor: .secondary,
            argumentHint: { _, _ in nil }, onSubmit: { submissions += 1 }, onAcceptSuggestion: nil
        )
        let coordinator = editor.makeCoordinator()
        let view = input(text: draft)
        coordinator.view = view
        view.textView.delegate = coordinator
        view.textView.selectedRange = NSRange(location: draft.utf16.count, length: 0)
        let command = try XCTUnwrap(view.textView.keyCommands?.first {
            $0.input == "\r" && $0.modifierFlags == .shift
        })
        _ = view.textView.perform(command.action)
        XCTAssertEqual(view.textView.text, "first line\n")
        XCTAssertEqual(draft, "first line\n")
        XCTAssertEqual(submissions, 0)
    }

    func testAppearanceRefreshPreservesMarkedText() throws {
        let view = input(text: "/review ")
        let font = try XCTUnwrap(view.textView.font)
        view.updateAppearance(font: font, textColor: .label, hintColor: .secondaryLabel)
        view.textView.selectedRange = NSRange(location: view.textView.text.utf16.count, length: 0)
        view.textView.setMarkedText("正在输入", selectedRange: NSRange(location: 4, length: 0))
        let before = try XCTUnwrap(view.textView.markedTextRange)
        let markedText = view.textView.text(in: before)

        view.updateAppearance(font: font, textColor: .label, hintColor: .tertiaryLabel)
        let after = try XCTUnwrap(view.textView.markedTextRange)
        XCTAssertEqual(view.textView.text(in: after), markedText)
        XCTAssertEqual(view.textView.text, "/review 正在输入")
    }
}
