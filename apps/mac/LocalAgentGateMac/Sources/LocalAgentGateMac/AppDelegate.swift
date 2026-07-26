import AppKit
import Foundation

final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusItem: NSStatusItem!
    private var timer: Timer?
    private let client = DaemonClient(socketPath: DaemonPaths.socketPath)
    private var pendingRequests: [PendingRequest] = []
    private var daemonReachable = false
    private var auditWindow: NSWindow?

    func applicationDidFinishLaunching(_ notification: Notification) {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        statusItem.button?.title = "\u{1F6E1}"
        rebuildMenu()
        poll()
        timer = Timer.scheduledTimer(withTimeInterval: 2.0, repeats: true) { [weak self] _ in
            self?.poll()
        }
    }

    private func poll() {
        DispatchQueue.global(qos: .utility).async { [weak self] in
            guard let self else { return }
            var reachable = false
            var requests: [PendingRequest] = []
            do {
                _ = try self.client.get("/health")
                reachable = true
                let data = try self.client.get("/pending")
                requests = try JSONDecoder().decode([PendingRequest].self, from: data)
            } catch {
                reachable = false
                requests = []
            }
            DispatchQueue.main.async {
                self.daemonReachable = reachable
                self.pendingRequests = requests
                self.rebuildMenu()
            }
        }
    }

    private func rebuildMenu() {
        let menu = NSMenu()

        let statusTitle = daemonReachable ? "Daemon: Running" : "Daemon: Not Running"
        menu.addItem(NSMenuItem(title: statusTitle, action: nil, keyEquivalent: ""))
        menu.addItem(NSMenuItem.separator())

        if pendingRequests.isEmpty {
            menu.addItem(NSMenuItem(title: "No pending approvals", action: nil, keyEquivalent: ""))
        } else {
            for request in pendingRequests {
                let item = NSMenuItem(
                    title: "\(request.action.command)  [\(request.risk.level)]",
                    action: nil,
                    keyEquivalent: ""
                )
                let submenu = NSMenu()

                let projectItem = NSMenuItem(title: "Project: \(request.project.name)", action: nil, keyEquivalent: "")
                submenu.addItem(projectItem)
                if let reason = request.risk.reasons.first {
                    submenu.addItem(NSMenuItem(title: "Reason: \(reason)", action: nil, keyEquivalent: ""))
                }
                submenu.addItem(NSMenuItem.separator())

                let allowItem = NSMenuItem(title: "Allow", action: #selector(allow(_:)), keyEquivalent: "")
                allowItem.target = self
                allowItem.representedObject = request.id
                submenu.addItem(allowItem)

                let denyItem = NSMenuItem(title: "Deny", action: #selector(deny(_:)), keyEquivalent: "")
                denyItem.target = self
                denyItem.representedObject = request.id
                submenu.addItem(denyItem)

                item.submenu = submenu
                menu.addItem(item)
            }
        }

        menu.addItem(NSMenuItem.separator())
        let auditItem = NSMenuItem(title: "View Audit Log", action: #selector(showAuditLog), keyEquivalent: "")
        auditItem.target = self
        menu.addItem(auditItem)

        menu.addItem(NSMenuItem.separator())
        let quitItem = NSMenuItem(title: "Quit Local Agent Gate", action: #selector(quit), keyEquivalent: "q")
        quitItem.target = self
        menu.addItem(quitItem)

        statusItem.menu = menu
        statusItem.button?.title = pendingRequests.isEmpty ? "\u{1F6E1}" : "\u{1F6E1} \(pendingRequests.count)"
    }

    @objc private func allow(_ sender: NSMenuItem) {
        decide(id: sender.representedObject as? String, decision: "allow_once")
    }

    @objc private func deny(_ sender: NSMenuItem) {
        decide(id: sender.representedObject as? String, decision: "deny_once")
    }

    private func decide(id: String?, decision: String) {
        guard let id else { return }
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self else { return }
            do {
                let body = try JSONSerialization.data(withJSONObject: ["decision": decision])
                _ = try self.client.post("/pending/\(id)/decide", body: body)
            } catch {
                NSLog("[LocalAgentGateMac] failed to submit decision: \(error)")
            }
            self.poll()
        }
    }

    @objc private func showAuditLog() {
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self else { return }
            let process = Process()
            process.executableURL = URL(fileURLWithPath: DaemonPaths.cliBinaryPath)
            process.arguments = ["audit", "--limit", "50"]
            let pipe = Pipe()
            process.standardOutput = pipe

            let text: String
            do {
                try process.run()
                process.waitUntilExit()
                let data = pipe.fileHandleForReading.readDataToEndOfFile()
                text = String(data: data, encoding: .utf8) ?? "(no output)"
            } catch {
                text = "Failed to run agent-gate audit: \(error)"
            }

            DispatchQueue.main.async {
                self.presentAuditWindow(text: text)
            }
        }
    }

    private func presentAuditWindow(text: String) {
        let frame = NSRect(x: 0, y: 0, width: 760, height: 420)

        let textView = NSTextView(frame: frame)
        textView.string = text
        textView.isEditable = false
        textView.font = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
        textView.autoresizingMask = [.width, .height]

        let scrollView = NSScrollView(frame: frame)
        scrollView.documentView = textView
        scrollView.hasVerticalScroller = true

        let window = NSWindow(
            contentRect: frame,
            styleMask: [.titled, .closable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Local Agent Gate — Audit Log"
        window.contentView = scrollView
        window.center()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        auditWindow = window
    }

    @objc private func quit() {
        NSApp.terminate(nil)
    }
}
