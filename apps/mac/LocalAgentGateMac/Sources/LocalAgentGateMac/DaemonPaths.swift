import Foundation

enum DaemonPaths {
    static var appSupportDir: String {
        let home = ProcessInfo.processInfo.environment["HOME"] ?? NSHomeDirectory()
        return "\(home)/Library/Application Support/local-agent-gate"
    }

    static var socketPath: String {
        "\(appSupportDir)/agent-gate.sock"
    }

    /// Repo root found by walking up from this executable, which in a
    /// development checkout lives under `<repo>/apps/mac/LocalAgentGateMac/.build/...`.
    /// The exact depth varies (SwiftPM nests a target triple), so look for the
    /// workspace manifest rather than counting path components.
    private static var checkoutRoot: String? {
        var url = URL(fileURLWithPath: CommandLine.arguments[0]).resolvingSymlinksInPath()
        while url.path != "/" {
            url.deleteLastPathComponent()
            if FileManager.default.fileExists(atPath: url.appendingPathComponent("Cargo.toml").path) {
                return url.path
            }
        }
        return nil
    }

    /// Locates the `agent-gate` CLI binary, preferring an explicit override,
    /// then a build next to this development checkout, then an installed copy.
    static var cliBinaryPath: String {
        var candidates: [String] = []
        if let override = ProcessInfo.processInfo.environment["AGENT_GATE_CLI"] {
            candidates.append(override)
        }
        if let root = checkoutRoot {
            candidates += [
                "\(root)/target/release/agent-gate",
                "\(root)/target/debug/agent-gate",
            ]
        }
        candidates += [
            "/usr/local/bin/agent-gate",
            "/opt/homebrew/bin/agent-gate",
        ]
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0) }
            ?? "agent-gate"
    }
}
