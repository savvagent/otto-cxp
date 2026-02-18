// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "CxpSdk",
    platforms: [.iOS(.v16), .macOS(.v13)],
    products: [
        .library(name: "CxpSdk", targets: ["CxpSdk"]),
    ],
    targets: [
        .target(name: "CxpSdk", path: "Sources/CxpSdk"),
    ]
)
