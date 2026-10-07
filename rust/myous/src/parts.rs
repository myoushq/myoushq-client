//! Long messages: splitting text into parts and putting it back together.
//! See PROTOCOL.md, "Long messages".
//!
//! One message holds about 28 KB of text. Longer text goes out as up to 16
//! parts, each a normal message tagged ["part", id, index, total]; the
//! receiver buffers them and delivers one message when all have arrived.

use std::collections::BTreeMap;

use anyhow::{bail, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};

pub const PART_BYTES: usize = 24_000; // JSON-escaped size of one part's text
pub const MAX_PARTS: usize = 16;
pub const MAX_BYTES: usize = 262_144; // UTF-8 size of a whole message
pub const MAX_UNFINISHED: usize = 4; // per sender
pub const UNFINISHED_TTL: u64 = 3600; // seconds after the first part arrived

fn escaped_size(c: char) -> usize {
    match c {
        '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
        '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => 6,
        c if (c as u32) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// The parts to send `text` in (one, if it fits), or an error if it's too
/// long to send at all.
pub fn split(text: &str) -> Result<Vec<String>> {
    if text.len() > MAX_BYTES {
        bail!("message is {} bytes; the limit is {MAX_BYTES}. Shorten it or send it in several messages", text.len());
    }
    let mut parts = vec![];
    let (mut cur, mut size) = (String::new(), 0);
    for c in text.chars() {
        let n = escaped_size(c);
        if !cur.is_empty() && size + n > PART_BYTES {
            parts.push(std::mem::take(&mut cur));
            size = 0;
        }
        cur.push(c);
        size += n;
    }
    parts.push(cur);
    if parts.len() > MAX_PARTS {
        bail!("message needs {} parts; the limit is {MAX_PARTS}. Shorten it", parts.len());
    }
    Ok(parts)
}

pub fn new_id() -> String {
    let mut b = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub id: String,
    pub index: usize,
    pub total: usize,
}

/// Reads a ["part", id, index, total] tag.
pub fn parse_tag(t: &[String]) -> Option<Part> {
    let [_, id, index, total, ..] = t else { return None };
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return None;
    }
    let (index, total): (usize, usize) = (index.parse().ok()?, total.parse().ok()?);
    if !(2..=MAX_PARTS).contains(&total) || !(1..=total).contains(&index) {
        return None;
    }
    Some(Part { id: id.clone(), index, total })
}

/// A long message waiting for parts, in the "partials" document.
#[derive(Serialize, Deserialize, Clone)]
pub struct Unfinished {
    pub sender: String,
    pub total: usize,
    pub parts: BTreeMap<String, String>,
    pub first: u64,
    #[serde(default)]
    pub sent_at: Option<u64>,
    #[serde(default)]
    pub ms: Option<u64>,
}

pub type Buffer = BTreeMap<String, Unfinished>;

/// Buffers one part. Returns (text, sent_at, ms) once the message is complete.
pub fn add(buf: &mut Buffer, sender: &str, part: &Part, text: &str, sent_at: u64, ms: u64, now: u64) -> Option<(String, u64, u64)> {
    let key = format!("{sender}:{}", part.id);
    if !buf.contains_key(&key) {
        let mut mine: Vec<(u64, String)> =
            buf.iter().filter(|(_, u)| u.sender == sender).map(|(k, u)| (u.first, k.clone())).collect();
        mine.sort();
        for (_, old) in mine.iter().take((mine.len() + 1).saturating_sub(MAX_UNFINISHED)) {
            buf.remove(old);
        }
        buf.insert(key.clone(), Unfinished {
            sender: sender.to_string(), total: part.total, parts: BTreeMap::new(), first: now, sent_at: None, ms: None,
        });
    }
    let u = buf.get_mut(&key)?;
    let idx = part.index.to_string();
    if u.total != part.total || u.parts.contains_key(&idx) {
        return None;
    }
    if u.parts.values().map(String::len).sum::<usize>() + text.len() > MAX_BYTES {
        buf.remove(&key);
        return None;
    }
    u.parts.insert(idx, text.to_string());
    if part.index == 1 {
        u.sent_at = Some(sent_at);
        u.ms = Some(ms);
    }
    if u.parts.len() < u.total {
        return None;
    }
    let u = buf.remove(&key)?;
    let joined: String = (1..=u.total).map(|i| u.parts[&i.to_string()].as_str()).collect();
    Some((joined, u.sent_at.unwrap_or(sent_at), u.ms.unwrap_or(ms)))
}

/// Removes messages unfinished for too long. Returns (sender, text, sent_at)
/// for each, with markers where parts are missing.
pub fn expire(buf: &mut Buffer, now: u64) -> Vec<(String, String, u64)> {
    let old: Vec<String> = buf.iter().filter(|(_, u)| now.saturating_sub(u.first) > UNFINISHED_TTL).map(|(k, _)| k.clone()).collect();
    old.into_iter()
        .filter_map(|k| buf.remove(&k))
        .map(|u| {
            let text: String = (1..=u.total)
                .map(|i| u.parts.get(&i.to_string()).cloned().unwrap_or_else(|| format!("[part {i} of {} missing]", u.total)))
                .collect();
            (u.sender, text, u.sent_at.unwrap_or(u.first))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_and_join() {
        let text = "a".repeat(30_000) + &"😀".repeat(2000) + &"\"<".repeat(5000);
        let parts = split(&text).unwrap();
        assert_eq!(parts.concat(), text);
        assert!(parts.iter().all(|p| p.chars().map(escaped_size).sum::<usize>() <= PART_BYTES));
        assert_eq!(split("hello").unwrap().len(), 1);
        assert!(split(&"a".repeat(MAX_BYTES + 1)).is_err());
    }

    #[test]
    fn reassemble_and_expire() {
        let mut buf = Buffer::new();
        let p = |i| Part { id: "ab".into(), index: i, total: 3 };
        assert!(add(&mut buf, "s", &p(3), "c", 103, 0, 1000).is_none());
        assert!(add(&mut buf, "s", &p(1), "a", 101, 0, 1000).is_none());
        assert!(add(&mut buf, "s", &p(1), "dup", 101, 0, 1000).is_none());
        assert_eq!(add(&mut buf, "s", &p(2), "b", 102, 0, 1000).map(|r| r.0), Some("abc".to_string()));
        assert!(buf.is_empty());
        add(&mut buf, "s", &p(1), "x", 101, 0, 1000);
        let gone = expire(&mut buf, 1000 + UNFINISHED_TTL + 1);
        assert_eq!(gone[0].1, "x[part 2 of 3 missing][part 3 of 3 missing]");
        for i in 0..MAX_UNFINISHED + 2 {
            add(&mut buf, "s", &Part { id: format!("{i}"), index: 1, total: 2 }, "t", 0, 0, i as u64);
        }
        assert_eq!(buf.len(), MAX_UNFINISHED);
    }
}
