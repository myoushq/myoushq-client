//! What differs per desktop below the window: the names the screens use
//! for this system, the owner's timezone and language for the container.

pub fn os() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// "this PC" on Windows, "this computer" on Linux, "this Mac" here for tests.
pub fn system_name() -> &'static str {
    match os() {
        "windows" => "this PC",
        "macos" => "this Mac",
        _ => "this computer",
    }
}

pub fn product_name() -> &'static str {
    match os() {
        "windows" => "myous for Windows",
        "macos" => "myous for Mac",
        _ => "myous for Linux",
    }
}

pub fn download_path() -> &'static str {
    match os() {
        "windows" => "/download/windows",
        "macos" => "/download/mac",
        _ => "/download/linux",
    }
}

/// This computer's short host name, the default worker name.
pub fn hostname() -> String {
    let h = hostname::get()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let h = h.split('.').next().unwrap_or("").to_string();
    if h.is_empty() {
        "My computer".into()
    } else {
        h
    }
}

/// The owner's timezone name for the container (MYOUS_TZ).
pub fn local_zone() -> String {
    if let Ok(tz) = std::env::var("TZ") {
        if !tz.is_empty() {
            return tz;
        }
    }
    if let Ok(link) = std::fs::read_link("/etc/localtime") {
        let s = link.to_string_lossy().to_string();
        if let Some(i) = s.find("zoneinfo/") {
            return s[i + "zoneinfo/".len()..].to_string();
        }
    }
    if let Ok(text) = std::fs::read_to_string("/etc/timezone") {
        let t = text.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    #[cfg(windows)]
    {
        if let Some(tz) = windows_zone() {
            return tz;
        }
    }
    "UTC".into()
}

/// Windows keeps its own zone names; `tzutil /g` prints one ("Pacific
/// Standard Time"). The container maps it (it accepts both forms).
#[cfg(windows)]
fn windows_zone() -> Option<String> {
    let out = crate::engine::output("tzutil", &["/g"], std::time::Duration::from_secs(5));
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// The owner's language as a BCP 47 tag (MYOUS_LANG), from the locale
/// variables; empty when they say nothing. A Mac app opened from the
/// Finder or at login has none of them, so there it is the first of the
/// owner's preferred languages, as myous for Mac sends.
pub fn language() -> String {
    for k in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(k) {
            if !v.is_empty() {
                let v = v.split('.').next().unwrap_or("").to_string();
                if v != "C" && v != "POSIX" {
                    return v.replace('_', "-");
                }
            }
        }
    }
    if cfg!(target_os = "macos") {
        return first_apple_language(&crate::engine::output(
            "/usr/bin/defaults",
            &["read", "-g", "AppleLanguages"],
            std::time::Duration::from_secs(5),
        ));
    }
    String::new()
}

/// The first entry of `defaults read -g AppleLanguages`, a property list
/// in the old text form: `(\n    "en-US",\n    de\n)`.
fn first_apple_language(out: &str) -> String {
    out.lines()
        .map(|l| l.trim().trim_end_matches(',').trim_matches('"'))
        .find(|l| !l.is_empty() && *l != "(" && *l != ")")
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_macs_preferred_language() {
        assert_eq!(
            first_apple_language("(\n    \"en-US\",\n    \"de-DE\"\n)\n"),
            "en-US"
        );
        assert_eq!(first_apple_language("(\n    fr\n)"), "fr");
        assert_eq!(first_apple_language("(\n)"), "");
        assert_eq!(first_apple_language(""), "");
    }
}
