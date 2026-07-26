// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "LocalAgentGateMac",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "LocalAgentGateMac",
            path: "Sources/LocalAgentGateMac"
        )
    ]
)
