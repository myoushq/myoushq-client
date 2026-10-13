//! The app's use of the network: whether this computer reaches the hub
//! (the Set up card's check), and the newest release (`latest_release` in
//! /config.json).

use std::time::Duration;

/// The hub: $MYOUS_HUB, else myoushq.com.
pub fn url() -> String {
    match std::env::var("MYOUS_HUB") {
        Ok(h) if !h.is_empty() => h.trim_end_matches('/').to_string(),
        _ => "https://myoushq.com".into(),
    }
}

/// The tag the hub announces ("v0.6.1").
pub fn latest_release() -> Result<String, String> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let text = agent
        .get(format!("{}/config.json", url()))
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(v.get("latest_release")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string())
}

/// Whether this computer reaches the hub: "ok", or "down" with why.
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct HubCheck {
    pub state: String,
    pub detail: String,
}

/// One GET of the hub's `/config.json` (what every client fetches first;
/// the hub has no separate health address), 5 s at most.
pub fn check() -> HubCheck {
    check_url(&url(), Duration::from_secs(5))
}

pub fn check_url(base: &str, timeout: Duration) -> HubCheck {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let verdict = match agent.get(format!("{base}/config.json")).call() {
        Err(e) => Err(e.to_string()),
        Ok(mut r) => {
            let status = r.status().as_u16();
            let body = r
                .body_mut()
                .with_config()
                .limit(256 * 1024)
                .read_to_string()
                .unwrap_or_default();
            reply_ok(status, &body)
        }
    };
    match verdict {
        Ok(()) => HubCheck {
            state: "ok".into(),
            detail: String::new(),
        },
        Err(detail) => HubCheck {
            state: "down".into(),
            detail,
        },
    }
}

/// The hub's reply to `/config.json`: 200 and its JSON with the relays.
/// Anything else (a captive portal's page, a proxy's error) is not the hub.
pub fn reply_ok(status: u16, body: &str) -> Result<(), String> {
    if status != 200 {
        return Err(format!("it answered with HTTP {status}"));
    }
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|_| "the answer wasn't the hub's".to_string())?;
    match v.get("relays").and_then(|r| r.as_array()) {
        Some(relays) if !relays.is_empty() => Ok(()),
        _ => Err("the answer wasn't the hub's".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    const CONFIG: &str = r#"{"version":1,"relays":["wss://relay.myoushq.com"],"pair_api":"https://myoushq.com/api/pair","latest_release":"v0.6.1"}"#;

    #[test]
    fn judges_the_reply() {
        assert_eq!(reply_ok(200, CONFIG), Ok(()));
        assert!(reply_ok(502, "Bad Gateway").unwrap_err().contains("502"));
        // A captive portal or a proxy's page: answered, but not the hub.
        assert!(reply_ok(200, "<html>Sign in to the Wi-Fi</html>").is_err());
        assert!(reply_ok(200, r#"{"relays":[]}"#).is_err());
        assert!(reply_ok(200, "{}").is_err());
    }

    /// A one-shot server on this machine answering one request with `reply`
    /// (None: accept, then say nothing).
    fn serve(reply: Option<String>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", l.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                match reply {
                    Some(r) => {
                        let _ = s.write_all(r.as_bytes());
                    }
                    None => std::thread::sleep(Duration::from_secs(3)),
                }
            }
        });
        addr
    }

    fn http(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn checks_a_local_hub() {
        let t = Duration::from_secs(5);
        assert_eq!(
            check_url(&serve(Some(http("200 OK", CONFIG))), t).state,
            "ok"
        );
        let c = check_url(&serve(Some(http("503 Service Unavailable", "down"))), t);
        assert_eq!(
            (c.state.as_str(), c.detail.as_str()),
            ("down", "it answered with HTTP 503")
        );
        // Nothing listening.
        let closed = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        assert_eq!(check_url(&closed, t).state, "down");
        // Listening but silent: the timeout ends it.
        let start = std::time::Instant::now();
        let c = check_url(&serve(None), Duration::from_secs(1));
        assert_eq!(c.state, "down");
        assert!(start.elapsed() < Duration::from_secs(3), "{c:?}");
    }
}
