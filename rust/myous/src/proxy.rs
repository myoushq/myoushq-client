//! Outbound proxy support, for agents whose network only lets them out
//! through an HTTP proxy (common in sandboxes and offices).
//!
//! The standard variables choose the proxy: `HTTPS_PROXY` for wss:// and
//! https://, `HTTP_PROXY` for ws:// and http://, `ALL_PROXY` as a fallback,
//! and `NO_PROXY` for exceptions (lowercase names work too). Hub requests
//! use them through reqwest. The relay connection goes through nostr-sdk,
//! which only speaks SOCKS5, so for an http:// proxy we run a small SOCKS5
//! server on 127.0.0.1 that turns each connection into an HTTP CONNECT
//! through the proxy. TLS to the relay stays end to end: the proxy sees the
//! relay's host name, not the traffic.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use anyhow::{anyhow, bail, Result};
use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use url::Url;

/// The proxy URL to use for `target`, from the environment, if any.
pub fn proxy_for(target: &str) -> Option<String> {
    let u = Url::parse(target).ok()?;
    if bypassed(u.host_str()?) {
        return None;
    }
    let names: &[&str] = if matches!(u.scheme(), "https" | "wss") {
        &["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
    } else {
        &["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"]
    };
    names.iter().find_map(|n| std::env::var(n).ok().filter(|v| !v.is_empty()))
}

fn bypassed(host: &str) -> bool {
    let list = std::env::var("NO_PROXY").or_else(|_| std::env::var("no_proxy")).unwrap_or_default();
    list.split(',').map(|e| e.trim().trim_start_matches('.')).filter(|e| !e.is_empty()).any(|e| {
        e == "*" || host.eq_ignore_ascii_case(e) || host.to_ascii_lowercase().ends_with(&format!(".{}", e.to_ascii_lowercase()))
    })
}

/// A SOCKS5 address for nostr-sdk to reach the relays through, or None to
/// connect directly. A bridge task, if one was started, comes with it; it
/// stops when the handle is aborted.
pub async fn relay_proxy(relays: &[String]) -> Result<Option<(SocketAddr, Option<JoinHandle<()>>)>> {
    let Some(proxy) = relays.iter().find_map(|r| proxy_for(r)) else { return Ok(None) };
    let proxy = if proxy.contains("://") { proxy } else { format!("http://{proxy}") };
    let p = Url::parse(&proxy)?;
    match p.scheme() {
        "socks5" | "socks5h" => {
            if !p.username().is_empty() {
                bail!("SOCKS5 proxies with a password aren't supported; use an http:// proxy");
            }
            let host = p.host_str().ok_or_else(|| anyhow!("proxy URL has no host"))?;
            let addr = tokio::net::lookup_host((host, p.port().unwrap_or(1080)))
                .await?
                .next()
                .ok_or_else(|| anyhow!("can't resolve proxy {host}"))?;
            return Ok(Some((addr, None)));
        }
        "http" => {}
        other => bail!("unsupported proxy {other}://; use http:// or socks5://"),
    }
    // Only the relays: any local process can connect to the bridge, and
    // each CONNECT carries the user's proxy credentials.
    let allowed = relays.iter().filter_map(|r| {
        let u = Url::parse(r).ok()?;
        Some((u.host_str()?.to_ascii_lowercase(), u.port_or_known_default()?))
    }).collect();
    let (addr, task) = serve_bridge(p, allowed).await?;
    Ok(Some((addr, Some(task))))
}

/// A SOCKS5 server (no auth) on 127.0.0.1 that tunnels through the HTTP
/// proxy `p`, to the (host, port)s in `allowed` only.
async fn serve_bridge(p: Url, allowed: Vec<(String, u16)>) -> Result<(SocketAddr, JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let allowed = std::sync::Arc::new(allowed);
    let task = tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            let (p, allowed) = (p.clone(), allowed.clone());
            tokio::spawn(async move {
                let _ = bridge(client, &p, &allowed).await;
            });
        }
    });
    Ok((addr, task))
}

/// Serve one SOCKS5 (no auth) CONNECT by tunnelling through the HTTP proxy,
/// if it is for one of the `allowed` (host, port)s.
async fn bridge(mut client: TcpStream, proxy: &Url, allowed: &[(String, u16)]) -> Result<()> {
    let mut hello = [0u8; 2];
    client.read_exact(&mut hello).await?;
    let mut methods = vec![0u8; hello[1] as usize];
    client.read_exact(&mut methods).await?;
    if hello[0] != 5 || !methods.contains(&0) {
        bail!("not SOCKS5 without auth");
    }
    client.write_all(&[5, 0]).await?;
    let mut req = [0u8; 4];
    client.read_exact(&mut req).await?;
    if req[0] != 5 || req[1] != 1 {
        client.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await?; // only CONNECT
        bail!("unsupported SOCKS command");
    }
    let host = match req[3] {
        1 => {
            let mut a = [0u8; 4];
            client.read_exact(&mut a).await?;
            Ipv4Addr::from(a).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            client.read_exact(&mut name).await?;
            String::from_utf8(name)?
        }
        4 => {
            let mut a = [0u8; 16];
            client.read_exact(&mut a).await?;
            format!("[{}]", Ipv6Addr::from(a))
        }
        _ => bail!("bad address type"),
    };
    let mut port = [0u8; 2];
    client.read_exact(&mut port).await?;
    let port = u16::from_be_bytes(port);
    if !allowed.iter().any(|(h, p)| h.eq_ignore_ascii_case(&host) && *p == port) {
        client.write_all(&[5, 2, 0, 1, 0, 0, 0, 0, 0, 0]).await?; // not allowed by ruleset
        bail!("{host}:{port} is not a relay");
    }
    match connect(proxy, &host, port).await {
        Ok(mut upstream) => {
            client.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
            Ok(())
        }
        Err(e) => {
            client.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await?; // connection refused
            Err(e)
        }
    }
}

/// Open a tunnel to host:port through the proxy with HTTP CONNECT.
async fn connect(proxy: &Url, host: &str, port: u16) -> Result<TcpStream> {
    let proxy_host = proxy.host_str().ok_or_else(|| anyhow!("proxy URL has no host"))?;
    let mut s = TcpStream::connect((proxy_host, proxy.port().unwrap_or(80))).await?;
    let target = format!("{host}:{port}");
    let mut req = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
    if !proxy.username().is_empty() {
        let decode = |v: &str| percent_encoding::percent_decode_str(v).decode_utf8_lossy().into_owned();
        let creds = format!("{}:{}", decode(proxy.username()), decode(proxy.password().unwrap_or("")));
        let b64 = base64::engine::general_purpose::STANDARD.encode(creds);
        req.push_str(&format!("Proxy-Authorization: Basic {b64}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).await?;
    // Read the response head a byte at a time so nothing past it is consumed.
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut b = [0u8; 1];
        if s.read(&mut b).await? == 0 || head.len() > 65536 {
            bail!("proxy closed the connection");
        }
        head.push(b[0]);
    }
    let status_line = String::from_utf8_lossy(&head).lines().next().unwrap_or_default().to_string();
    if status_line.split_whitespace().nth(1) != Some("200") {
        bail!("proxy refused CONNECT: {status_line}");
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reply code of a SOCKS5 CONNECT to host:port through the bridge.
    async fn socks_connect(bridge: SocketAddr, host: &str, port: u16) -> u8 {
        let mut s = TcpStream::connect(bridge).await.unwrap();
        s.write_all(&[5, 1, 0]).await.unwrap();
        let mut hello = [0u8; 2];
        s.read_exact(&mut hello).await.unwrap();
        let mut req = vec![5, 1, 0, 3, host.len() as u8];
        req.extend_from_slice(host.as_bytes());
        req.extend_from_slice(&port.to_be_bytes());
        s.write_all(&req).await.unwrap();
        let mut reply = [0u8; 10];
        s.read_exact(&mut reply).await.unwrap();
        reply[1]
    }

    #[tokio::test]
    async fn the_bridge_tunnels_only_to_the_relays() {
        // An HTTP proxy that records each CONNECT line and says yes.
        let fake = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fake_addr = fake.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut c, _)) = fake.accept().await {
                let mut buf = vec![0u8; 4096];
                let n = c.read(&mut buf).await.unwrap_or(0);
                let text = String::from_utf8_lossy(&buf[..n]).to_string();
                log.lock().unwrap().push(text.lines().next().unwrap_or_default().to_string());
                let _ = c.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await;
            }
        });
        let proxy = Url::parse(&format!("http://alice:s3cret@{fake_addr}")).unwrap();
        let (bridge, task) = serve_bridge(proxy, vec![("relay.example.com".into(), 443)]).await.unwrap();
        assert_eq!(socks_connect(bridge, "Relay.Example.com", 443).await, 0);
        assert_eq!(socks_connect(bridge, "internal.example.net", 22).await, 2);
        assert_eq!(socks_connect(bridge, "relay.example.com", 22).await, 2);
        task.abort();
        assert_eq!(*seen.lock().unwrap(), vec!["CONNECT Relay.Example.com:443 HTTP/1.1".to_string()]);
    }
}
