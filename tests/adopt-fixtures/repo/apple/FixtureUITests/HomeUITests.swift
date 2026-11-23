import XCTest

final class HomeUITests: XCTestCase {
    func testEmptyHome() {
        let app = XCUIApplication()
        app.launch()
        XCTAssertTrue(app.otherElements["home.list"].waitForExistence(timeout: 5))
    }
}
