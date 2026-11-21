import XCTest

final class LingxiDataStagingUITests: XCTestCase {
    private var usb: DataStagingUSB?

    override func setUpWithError() throws {
        try super.setUpWithError()
        let env = ProcessInfo.processInfo.environment
        if let value = env["LINGXI_REPORT_USB_SECRET"], let secret = Data(base64Encoded: value) {
            let ports = ["LINGXI_DATA_STAGING_AUTH", "LINGXI_DATA_STAGING_FIRESTORE", "LINGXI_DATA_STAGING_API",
                         "LINGXI_REPORT_LAUNCH_PORT", "LINGXI_REPORT_GATEWAY_PORT"].compactMap { env[$0].flatMap(UInt16.init) }
            usb = try DataStagingUSB(ports: ports, secret: secret)
        }
    }

    override func tearDown() { usb?.close(); usb = nil; super.tearDown() }
    @MainActor
    func testReaderOpensPublishedReport() throws {
        continueAfterFailure = false
        let app = try openRecord(account: "reader", report: true)
        XCTAssertTrue(app.staticTexts["Source report"].waitForExistence(timeout: 15), app.debugDescription)
        let visible = app.descendants(matching: .any).matching(identifier: "teams.report.point.a").firstMatch
        XCTAssertTrue(visible.exists)
        XCTAssertTrue((visible.label + String(describing: visible.value)).contains("1"))
        XCTAssertFalse(app.descendants(matching: .any).matching(identifier: "teams.report.point.b").firstMatch.exists)
        XCTAssertFalse(app.staticTexts["Review a partition"].exists)
    }

    @MainActor
    func testDeviceLocalAppOpensReportAndReviews() throws {
        let env = ProcessInfo.processInfo.environment
        guard usb != nil, let launchPort = env["LINGXI_REPORT_LAUNCH_PORT"] else {
            throw XCTSkip("Requires a paired USB device and an isolated Local App")
        }
        continueAfterFailure = false
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_ISOLATED_SIM_TEST"] = "1"
        app.launchEnvironment["LINGXI_DATA_STAGING_ACCOUNT"] = "owner"
        for key in ["LINGXI_DATA_STAGING_AUTH", "LINGXI_DATA_STAGING_FIRESTORE", "LINGXI_DATA_STAGING_API"] {
            app.launchEnvironment[key] = env[key]
        }
        app.launchEnvironment["LINGXI_REPORT_LAUNCH_PORT"] = launchPort
        app.launch()
        let link = app.descendants(matching: .any).matching(identifier: "teams.staging.browser").firstMatch
        XCTAssertTrue(link.waitForExistence(timeout: 30), app.debugDescription)
        link.tap()
        let browser = XCUIApplication(bundleIdentifier: "com.apple.mobilesafari")
        XCTAssertTrue(browser.wait(for: .runningForeground, timeout: 15))
        let button = browser.buttons["Open review case"]
        XCTAssertTrue(button.waitForExistence(timeout: 25), browser.debugDescription)
        XCTAssertTrue(browser.staticTexts["Source report"].exists, browser.debugDescription)
        button.tap()
        XCTAssertTrue(browser.staticTexts["Review case open"].waitForExistence(timeout: 15), browser.debugDescription)
    }
    @MainActor
    func testReaderOpensTeamRecordAndTemplate() throws {
        continueAfterFailure = false
        let app = try openRecord(account: "reader")
        selectTemplate(app)
        XCTAssertTrue(app.staticTexts["Prepare shipment"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.buttons["teams.data.save"].exists)
    }

    @MainActor
    func testLocalAppWritesAndRendersInSimulatorBrowser() throws {
        #if targetEnvironment(simulator)
        guard let marker = ProcessInfo.processInfo.environment["LINGXI_DATA_STAGING_BROWSER_MARKER"],
            marker.range(of: "^[0-9a-f]{8}$", options: .regularExpression) != nil else {
            throw XCTSkip("Requires a Local App browser staging session")
        }
        continueAfterFailure = false
        // The staging driver opens the URL through the simulator's system resolver.
        let browser = XCUIApplication(bundleIdentifier: "com.apple.mobilesafari")
        let save = browser.buttons["Save staging record"]
        XCTAssertTrue(save.waitForExistence(timeout: 35), browser.debugDescription)
        save.tap()
        XCTAssertTrue(browser.staticTexts["Saved revision 1"].waitForExistence(timeout: 10), browser.debugDescription)
        XCTAssertTrue(browser.staticTexts["Packed on mobile " + marker].waitForExistence(timeout: 10), browser.debugDescription)
        #else
        throw XCTSkip("Simulator loopback staging is required")
        #endif
    }

    @MainActor
    private func openRecord(account: String, report: Bool = false) throws -> XCUIApplication {
        let env = ProcessInfo.processInfo.environment
        let names = ["LINGXI_DATA_STAGING_AUTH", "LINGXI_DATA_STAGING_FIRESTORE", "LINGXI_DATA_STAGING_API"]
        guard names.allSatisfy({ env[$0].flatMap(Int.init).map { (1024...65535).contains($0) } == true }) else {
            throw XCTSkip("Requires a running loopback Data staging instance")
        }
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_ISOLATED_SIM_TEST"] = "1"
        app.launchEnvironment["LINGXI_DATA_STAGING_ACCOUNT"] = account
        for name in names { app.launchEnvironment[name] = env[name] }
        app.launchArguments += ["-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()

        let record = app.buttons["teams.message.data.staging-message"]
        let entry = app.buttons.matching(NSPredicate(format: "identifier IN %@",
            ["teams.row.orders-chat", "teams.message.data.staging-message"])).firstMatch
        XCTAssertTrue(entry.waitForExistence(timeout: 30), app.debugDescription)
        let room = app.buttons["teams.row.orders-chat"]
        if !record.exists, room.exists {
            let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
            let alert = springboard.alerts.firstMatch
            if alert.exists {
                let labels = alert.staticTexts.allElementsBoundByIndex.map(\.label)
                let message = labels.joined(separator: " ").lowercased()
                if message.contains("lingxi") || message.contains("灵犀") || message.contains("靈犀") {
                    if message.contains("wireless data") || message.contains("无线数据") {
                        let wifiOnly = alert.buttons.allElementsBoundByIndex.first { button in
                            let label = button.label.lowercased()
                            let wifi = label.contains("wlan") || label.contains("wi-fi") || label.contains("无线局域网")
                            return wifi && !label.contains("cellular") && !label.contains("mobile") && !label.contains("蜂窝")
                        }
                        if let wifiOnly { wifiOnly.tap() }
                    } else if message.contains("notification") || message.contains("通知") {
                        let deny = alert.buttons.matching(NSPredicate(format: "label IN %@", ["Don’t Allow", "Don't Allow", "不允许", "不允許"])).firstMatch
                        if deny.exists { deny.tap() }
                    } else if message.contains("local network") || message.contains("本地网络") {
                        let allow = alert.buttons.matching(NSPredicate(format: "label IN %@", ["Allow", "允许", "允許"])).firstMatch
                        if allow.exists { allow.tap() }
                    }
                }
            }
            room.tap()
        }
        if report {
            let button = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH %@", "teams.report.open.")).firstMatch
            if !button.waitForExistence(timeout: 3), room.exists, room.isHittable { room.tap() }
            XCTAssertTrue(button.waitForExistence(timeout: 20), app.debugDescription)
            button.tap()
            return app
        }
        XCTAssertTrue(record.waitForExistence(timeout: 20), app.debugDescription)
        record.tap()
        XCTAssertTrue(app.buttons["teams.data.close"].waitForExistence(timeout: 10))
        return app
    }

    @MainActor
    private func selectTemplate(_ app: XCUIApplication) {
        let picker = app.descendants(matching: .any).matching(identifier: "teams.data.view").firstMatch
        XCTAssertTrue(picker.waitForExistence(timeout: 20), app.debugDescription)
        picker.tap()
        let template = app.descendants(matching: .any).matching(NSPredicate(format: "label == %@", "order-summary")).firstMatch
        XCTAssertTrue(template.waitForExistence(timeout: 10), app.debugDescription)
        template.tap()
    }
}
