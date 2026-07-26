import Foundation

enum DaemonError: Error {
    case pathTooLong
    case connectFailed
    case malformedResponse
    case nonSuccessStatus(String)
}

/// Speaks plain HTTP/1.1 over a raw POSIX Unix-domain socket. Mirrors the
/// hand-rolled client in the Rust CLI (`agent-gate-cli/src/client.rs`) so
/// both talk to the same daemon the same way, without needing a UDS-aware
/// URLSession/Network.framework dependency.
final class DaemonClient {
    let socketPath: String

    init(socketPath: String) {
        self.socketPath = socketPath
    }

    func get(_ path: String) throws -> Data {
        try request(method: "GET", path: path, body: nil)
    }

    func post(_ path: String, body: Data) throws -> Data {
        try request(method: "POST", path: path, body: body)
    }

    private func request(method: String, path: String, body: Data?) throws -> Data {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw DaemonError.connectFailed }
        defer { close(fd) }

        let pathBytes = Array(socketPath.utf8)
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let maxLen = MemoryLayout.size(ofValue: addr.sun_path)
        guard pathBytes.count < maxLen else { throw DaemonError.pathTooLong }

        withUnsafeMutableBytes(of: &addr.sun_path) { rawPtr in
            let buffer = rawPtr.bindMemory(to: Int8.self)
            for (i, byte) in pathBytes.enumerated() {
                buffer[i] = Int8(bitPattern: byte)
            }
            buffer[pathBytes.count] = 0
        }

        let addrLen = socklen_t(MemoryLayout<sockaddr_un>.size)
        let connectResult = withUnsafePointer(to: &addr) { ptr -> Int32 in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockaddrPtr in
                connect(fd, sockaddrPtr, addrLen)
            }
        }
        guard connectResult == 0 else { throw DaemonError.connectFailed }

        let header =
            "\(method) \(path) HTTP/1.1\r\n"
            + "Host: localhost\r\n"
            + "Content-Type: application/json\r\n"
            + "Content-Length: \(body?.count ?? 0)\r\n"
            + "Connection: close\r\n\r\n"

        var outgoing = Data(header.utf8)
        if let body {
            outgoing.append(body)
        }

        try outgoing.withUnsafeBytes { rawBuf -> Void in
            var written = 0
            let total = rawBuf.count
            guard let base = rawBuf.baseAddress else { return }
            while written < total {
                let n = write(fd, base.advanced(by: written), total - written)
                if n <= 0 { throw DaemonError.connectFailed }
                written += n
            }
        }

        var response = Data()
        var buffer = [UInt8](repeating: 0, count: 4096)
        while true {
            let n = read(fd, &buffer, buffer.count)
            if n <= 0 { break }
            response.append(buffer, count: n)
        }

        guard let headerEnd = findHeaderEnd(response) else {
            throw DaemonError.malformedResponse
        }
        let headerData = response[response.startIndex..<headerEnd]
        let headerString = String(data: headerData, encoding: .utf8) ?? ""
        let statusLine = headerString.components(separatedBy: "\r\n").first ?? ""
        guard statusLine.contains("200") else {
            throw DaemonError.nonSuccessStatus(statusLine)
        }

        let bodyStart = response.index(headerEnd, offsetBy: 4)
        return response[bodyStart...]
    }

    private func findHeaderEnd(_ data: Data) -> Data.Index? {
        let pattern: [UInt8] = [13, 10, 13, 10]
        let bytes = [UInt8](data)
        guard bytes.count >= pattern.count else { return nil }
        for i in 0...(bytes.count - pattern.count) {
            if Array(bytes[i..<(i + pattern.count)]) == pattern {
                return data.index(data.startIndex, offsetBy: i)
            }
        }
        return nil
    }
}
