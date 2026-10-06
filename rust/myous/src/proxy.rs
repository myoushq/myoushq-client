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
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            let p = p.clone();
            tokio::spawn(async move {
                let _ = bridge(client, &p).await;
            });
        }
    });
    Ok(Some((addr, Some(task))))
}

/// Serve one SOCKS5 (no auth) CONNECT by tunnelling through the HTTP proxy.
async fn bridge(mut client: TcpStream, proxy: &Url) -> Result<()> {
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
    match connect(proxy, &host, u16::from_be_bytes(port)).await {
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
