import Smoke
import XCTest

final class SmokeTests: XCTestCase {
    func testMatches() {
        XCTAssertEqual(firstMatch(pattern: #"\d+"#, flags: "u", in: "ab 123"), 3..<6)
        XCTAssertNil(firstMatch(pattern: #"\d+"#, flags: "u", in: "abc"))
        XCTAssertNil(firstMatch(pattern: "(", flags: "", in: "abc"))
    }

    func testVersion() {
        XCTAssertFalse(libraryVersion.isEmpty)
    }
}
