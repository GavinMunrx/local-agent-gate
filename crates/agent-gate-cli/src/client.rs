use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// Sends a JSON POST over a Unix domain socket using a hand-rolled HTTP/1.1
/// request. Kept dependency-free (no UDS-aware HTTP client crate) while
/// staying wire-compatible with `curl --unix-socket` for manual testing.
pub async fn post_json(
    socket_path: &std::path::Path,
    path: &str,
    body: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let mut stream = UnixStream::connect(socket_path).await.map_err(|e| {
        anyhow::anyhow!(
            "failed to connect to daemon at {}: {e}. Is `agent-gate daemon` running?",
            socket_path.display()
        )
    })?;

    let body_bytes = serde_json::to_vec(body)?;
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body_bytes.len()
    );

    stream.write_all(request.as_bytes()).await?;
    stream.write_all(&body_bytes).await?;
    stream.flush().await?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;

    let header_end = find_header_end(&response)
        .ok_or_else(|| anyhow::anyhow!("malformed HTTP response from daemon"))?;
    let status_line = String::from_utf8_lossy(&response[..header_end]);
    let status_line = status_line.lines().next().unwrap_or("");
    if !status_line.contains("200") {
        anyhow::bail!("daemon returned non-200 response: {status_line}");
    }

    let value: serde_json::Value = serde_json::from_slice(&response[header_end + 4..])?;
    Ok(value)
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|w| w == b"\r\n\r\n")
}
