import XCTest

/// Cold launch and terminal through the real product UI, without fixture flags.
final class RealProductRuntimeUITests: XCTestCase {
    func testColdLaunchAndRealDefaultUserTerminal() throws {
        guard ProcessInfo.processInfo.environment["SIMULATOR_DEVICE_NAME"] == nil else {
            throw XCTSkip("Requires a physical iPhone with the bundled Linux runtime")
        }
        continueAfterFailure = false
        let app = XCUIApplication()
        app.launchArguments += ["-AppleLanguages", "(zh-Hans)"]
        app.terminate()
        app.launch()
        XCTAssertTrue(app.scrollViews["conversation.message-list"].waitForExistence(timeout: 30), app.debugDescription)
        let trigger = app.navigationBars.firstMatch.buttons.element(boundBy: 0)
        XCTAssertTrue(trigger.waitForExistence(timeout: 10), app.debugDescription)
        trigger.tap()
        let terminal = app.buttons["drawer.terminal"]
        XCTAssertTrue(terminal.waitForExistence(timeout: 10), app.debugDescription)
        terminal.tap()
        let input = app.textFields["terminal.input"]
        XCTAssertTrue(input.waitForExistence(timeout: 30), app.debugDescription)
        let enabled = NSPredicate(format: "enabled == true")
        expectation(for: enabled, evaluatedWith: input)
        waitForExpectations(timeout: 60)
        input.tap()
        input.typeText("printf 'PRODUCT_UI:%s:%s:%s\\n' \"$(id -u)\" \"$HOME\" \"$PWD\"\n")
        let result = app.staticTexts.containing(NSPredicate(format: "label CONTAINS %@", "PRODUCT_UI:1000:/home/lingxi:/home/lingxi/lingxi")).firstMatch
        XCTAssertTrue(result.waitForExistence(timeout: 20), app.debugDescription)
        input.typeText("set -e; found=0; for file in \"$HOME\"/.lingxi-product-acceptance-cold-*; do [ -f \"$file\" ] || continue; value=$(cat \"$file\"); [ \"$file\" = \"$HOME/.lingxi-product-acceptance-cold-$value\" ]; rm \"$file\"; found=1; done; [ \"$found\" = 1 ]; printf 'PRODUCT_COLD_%s\\n' OK\n")
        let coldProof = app.staticTexts.containing(NSPredicate(format: "label CONTAINS %@", "PRODUCT_COLD_OK")).firstMatch
        XCTAssertTrue(coldProof.waitForExistence(timeout: 20), app.debugDescription)
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "real-product-default-user-terminal"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        XCTAssertEqual(app.state, .runningForeground)
    }
}
