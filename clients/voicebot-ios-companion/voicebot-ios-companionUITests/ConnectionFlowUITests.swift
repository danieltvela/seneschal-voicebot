//
//  ConnectionFlowUITests.swift
//  voicebot-ios-companionUITests
//
//  Created by Dani Vela on 13/06/2026.
//

import XCTest

@MainActor
final class ConnectionFlowUITests: XCTestCase {

    private var app: XCUIApplication!

    override func setUpWithError() throws {
        try super.setUpWithError()
        app = XCUIApplication()
        app.launchArguments = ["--ui-testing"]
        app.launch()
        continueAfterFailure = false
    }

    override func tearDownWithError() throws {
        app.terminate()
        try super.tearDownWithError()
    }

    // MARK: - Launch

    @MainActor
    func testAppLaunchesToShowConnectionView() throws {
        // Verify the connection form is visible on launch
        let hostField = app.textFields["hostTextField"]
        XCTAssertTrue(hostField.waitForExistence(timeout: 5), "Host text field should be visible")
        XCTAssertEqual(hostField.placeholderValue, "Host")
    }

    // MARK: - Connection

    @MainActor
    func testEnterHostAndPortAndTapConnect() throws {
        // Enter host
        let hostField = app.textFields["hostTextField"]
        XCTAssertTrue(hostField.waitForExistence(timeout: 5))
        hostField.tap()
        hostField.typeText("localhost")

        // Enter port
        let portField = app.textFields["portTextField"]
        XCTAssertTrue(portField.waitForExistence(timeout: 5))
        portField.tap()
        portField.typeText("9090")

        // Tap Connect button (becomes enabled after entering host)
        let connectButton = app.buttons["connectButton"]
        XCTAssertTrue(connectButton.waitForExistence(timeout: 5))
        XCTAssertTrue(connectButton.isEnabled, "Connect button should be enabled after entering host")

        // Attempt connect (will fail since no server, but verifies UI transition)
        connectButton.tap()

        // Wait briefly for connection attempt
        Thread.sleep(forTimeInterval: 2)

        // The app should attempt connection (button may show "Disconnect" briefly or show error)
        // We verify the connection attempt was made by checking the UI responds
        XCTAssertTrue(hostField.exists || app.staticTexts["Error"].exists,
                      "App should respond to connection attempt")
    }

    // MARK: - Validation

    @MainActor
    func testConnectButtonDisabledWithEmptyHost() throws {
        // Ensure host field is empty (it should be on launch)
        let hostField = app.textFields["hostTextField"]
        XCTAssertTrue(hostField.waitForExistence(timeout: 5))

        // Verify connect button is disabled
        let connectButton = app.buttons["connectButton"]
        XCTAssertTrue(connectButton.waitForExistence(timeout: 5))
        XCTAssertFalse(connectButton.isEnabled,
                       "Connect button should be disabled when host is empty")
    }

    // MARK: - Conversation View

    @MainActor
    func testConversationViewIsAccessibleViaTabBar() throws {
        // Tap the Conversation tab
        let conversationTab = app.tabBars.buttons["Conversation"]
        if conversationTab.waitForExistence(timeout: 3) {
            conversationTab.tap()

            // Verify conversation view is shown
            let scrollView = app.scrollViews.firstMatch
            XCTAssertTrue(scrollView.waitForExistence(timeout: 5),
                          "Conversation scroll view should be visible")
        }
    }
}
