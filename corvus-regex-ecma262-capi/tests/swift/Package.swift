// swift-tools-version:5.9
// Links the XCFramework (xcframework.ps1, unpacked here by CI as CorvusRegexEcma262.xcframework) the way a Swift
// package does, and calls the C library through its Clang module, CCorvusRegexEcma262.
import PackageDescription

let package = Package(
    name: "CapiSwiftSmoke",
    platforms: [.macOS(.v10_15), .iOS(.v13)],
    products: [.library(name: "Smoke", targets: ["Smoke"])],
    targets: [
        .binaryTarget(name: "CCorvusRegexEcma262", path: "CorvusRegexEcma262.xcframework"),
        .target(name: "Smoke", dependencies: ["CCorvusRegexEcma262"]),
        .testTarget(name: "SmokeTests", dependencies: ["Smoke"]),
    ]
)
