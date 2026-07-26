import Foundation

enum DaemonPaths {
    static var appSupportDir: String {
        let home = ProcessInfo.processInfo.environment["HOME"] ?? NSHomeDirectory()
        return "\(home)/Library/Application Support/local-agent-gate"
    }

    static var socketPath: String {
        "\(appSupportDir)/agent-gate.sock"
    }

    /// Locates the `agent-gate` CLI binary next to this development checkout,
    /// preferring a release build if one has been produced.
    static var cliBinaryPath: String {
        let repoTarget = "/path/to/local-agent-gate/target"
        let release = "\(repoTarget)/release/agent-gate"
        let debug = "\(repoTarget)/debug/agent-gate"
        if FileManager.default.isExecutableFile(atPath: release) {
            return release
        }
        return debug
    }
}
