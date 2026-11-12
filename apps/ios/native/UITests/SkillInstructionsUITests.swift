import XCTest

final class SkillInstructionsUITests: XCTestCase {
    func testToolsAndInstructionsStartCollapsedAndOpenOnTapInBothThemes() {
        continueAfterFailure = false
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
        app.launchEnvironment["LINGXI_ISOLATED_SIM_TEST"] = "1"
        app.launchEnvironment["LINGXI_UI_TEST_SKILL_INSTRUCTIONS"] = "1"
        for theme in ["light", "dark"] {
            app.launchArguments = ["-AppleLanguages", "(en)", "-theme", theme]
            app.launch()
            let toggle = app.buttons["conversation.skill-instructions.toggle"]
            XCTAssertTrue(toggle.waitForExistence(timeout: 15), app.debugDescription)
            app.buttons["conversation.summary-menu"].tap()
            app.buttons["conversation.summary.category.tools"].tap()
            let tools = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH %@", "conversation.timeline.tool-batch.")).firstMatch
            let child = app.descendants(matching: .any)["conversation.tool-call.ui-skill-read-second"]
            XCTAssertTrue(tools.exists, app.debugDescription)
            XCTAssertFalse(child.exists)
            tools.tap()
            XCTAssertTrue(child.waitForExistence(timeout: 5), app.debugDescription)
            capture(app, "tools-expanded-\(theme)")
            tools.tap()
            XCTAssertFalse(child.exists)
            app.buttons["Close"].tap()
            XCTAssertEqual(toggle.frame.height, 44, accuracy: 1)
            XCTAssertFalse(app.staticTexts["LingXi API guide"].exists)
            capture(app, "skill-collapsed-\(theme)")
            toggle.tap()
            XCTAssertTrue(app.staticTexts["LingXi API guide"].waitForExistence(timeout: 5), app.debugDescription)
            XCTAssertFalse(app.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", "Base directory for this skill")).firstMatch.exists)
            capture(app, "skill-expanded-\(theme)")
            toggle.tap()
            XCTAssertFalse(app.staticTexts["LingXi API guide"].exists)
            app.terminate()
        }
    }

    private func capture(_ app: XCUIApplication, _ name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}
