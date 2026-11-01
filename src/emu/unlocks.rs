//! Unlocks the server hasn't taken yet, kept in a file so they survive the game stopping.
//!
//! rcheevos sends each unlock and tries again while the client lives, but a game that stops with
//! the server out of reach takes the queue with it (as would a crash). So each unlock request is
//! written down as it's sent and crossed out once the server answers, and what's left is sent
//! again the next time romi starts a game or opens. The server takes an unlock it already has as
//! done, so asking twice is harmless. The file holds login tokens, so it's readable only by you.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

/// Unlocks older than this are let go: the account has most likely moved on.
const KEEP: u64 = 30 * 24 * 3600;

/// One unlock waiting for the server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pending {
    pub url: String,
    pub user: String,
    pub token: String,
    pub achievement: u64,
    pub hardcore: bool,
    pub game_hash: String,
    /// When it was earned, in seconds since 1970.
    pub earned: u64,
}

fn path() -> PathBuf {
    crate::config::state_dir().join("retroachievements-pending.json")
}

/// Only one thread changes the file at a time. (Two romi processes could still race; the worst
/// is an unlock sent twice, or one that waits for the next start.)
static LOCK: Mutex<()> = Mutex::new(());

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn read() -> Vec<Pending> {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write(all: &[Pending]) -> Result<()> {
    if all.is_empty() {
        return match std::fs::remove_file(path()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        };
    }
    super::cheevos::write_private(&path(), &serde_json::to_vec_pretty(all)?)
}

/// The fields of a form-encoded body, decoded.
fn fields(body: &str) -> Vec<(String, String)> {
    body.split('&').filter_map(|pair| pair.split_once('=')).map(|(k, v)| (k.to_string(), decode(v))).collect()
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() && bytes[i + 1].is_ascii_hexdigit() && bytes[i + 2].is_ascii_hexdigit() => {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(b'%'));
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The unlock a request to the server is sending, if it is one.
pub fn parse(url: &str, body: &str) -> Option<Pending> {
    let f = fields(body);
    let get = |k: &str| f.iter().find(|(name, _)| name == k).map(|(_, v)| v.as_str());
    if get("r")? != "awardachievement" {
        return None;
    }
    let since: u64 = get("o").and_then(|o| o.parse().ok()).unwrap_or(0);
    Some(Pending {
        url: url.to_string(),
        user: get("u")?.to_string(),
        token: get("t")?.to_string(),
        achievement: get("a")?.parse().ok()?,
        hardcore: get("h") == Some("1"),
        game_hash: get("m").unwrap_or_default().to_string(),
        earned: now().saturating_sub(since),
    })
}

/// The same request as it would be sent now, signed as the server expects.
fn request(p: &Pending) -> String {
    use crate::art::encode;
    let since = now().saturating_sub(p.earned);
    let h = u8::from(p.hardcore);
    let mut sign = format!("{}{}{h}", p.achievement, p.user);
    let mut body = format!(
        "r=awardachievement&u={}&t={}&a={}&h={h}&m={}",
        encode(&p.user),
        encode(&p.token),
        p.achievement,
        encode(&p.game_hash)
    );
    if since > 0 {
        body.push_str(&format!("&o={since}"));
        sign.push_str(&format!("{}{since}", p.achievement));
    }
    format!("{body}&v={:x}", md5::compute(sign))
}

fn same(a: &Pending, b: &Pending) -> bool {
    (&a.user, a.achievement, a.hardcore, &a.game_hash) == (&b.user, b.achievement, b.hardcore, &b.game_hash)
}

/// Writes an unlock down, when it's about to be sent. Resending one already there keeps when it
/// was first earned.
pub fn add(p: Pending) {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = read();
    match all.iter_mut().find(|q| same(q, &p)) {
        Some(q) => {
            q.token = p.token;
            q.url = p.url;
        }
        None => all.push(p),
    }
    let _ = write(&all);
}

/// Crosses an unlock out, when the server has answered it.
pub fn remove(p: &Pending) {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = read();
    let before = all.len();
    all.retain(|q| !same(q, p));
    if all.len() != before {
        let _ = write(&all);
    }
}

/// Whether the server took a request, by rcheevos' own rule for trying again: a missing answer,
/// or one that says it's busy, hasn't.
pub fn answered(status: i32, body: &[u8]) -> bool {
    let busy = matches!(status, -2 | 0 | 429 | 502..=504 | 521..=525);
    !busy && !body.is_empty()
}

/// Sends again what an earlier game left, in the background. Unlocks the server still can't be
/// reached for stay for the next time.
pub fn resend(agent: ureq::Agent, user_agent: String) {
    let waiting = {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = read();
        let before = all.len();
        all.retain(|p| now().saturating_sub(p.earned) < KEEP);
        if all.len() != before {
            let _ = write(&all);
        }
        all
    };
    if waiting.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        for p in waiting {
            let sent = super::cheevos::send(&agent, &user_agent, &p.url, Some(&request(&p)), None);
            match sent {
                Ok((status, body)) if answered(status, &body) => {
                    super::cheevos::note(&format!("sent the unlock of {} that was waiting", p.achievement));
                    remove(&p);
                }
                // Unreachable: the rest wouldn't get through either.
                _ => break,
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_read_and_signed_again() {
        let sig = |s: &str| format!("{:x}", md5::compute(s));
        let url = "https://retroachievements.org/dorequest.php";
        let body = format!("r=awardachievement&u=Lan%20tern&t=T0K&a=77&h=0&m=abc&v={}", sig("77Lan tern0"));
        let p = parse(url, &body).unwrap();
        assert_eq!((p.user.as_str(), p.achievement, p.hardcore), ("Lan tern", 77, false));
        // Sent at once, it carries no offset, and the signature is the plain one.
        assert_eq!(
            request(&p),
            format!("r=awardachievement&u=Lan%20tern&t=T0K&a=77&h=0&m=abc&v={}", sig("77Lan tern0"))
        );
        // Sent later, it says how long ago it was earned, and the signature covers that.
        let old = Pending { earned: now() - 90, ..p };
        let o = old.earned;
        assert_eq!(
            request(&old),
            format!("r=awardachievement&u=Lan%20tern&t=T0K&a=77&h=0&m=abc&o=90&v={}", sig("77Lan tern07790")),
            "earned at {o}"
        );
    }

    #[test]
    fn other_requests_are_left_alone() {
        assert!(parse("u", "r=ping&u=x&t=y&g=1").is_none());
    }

    #[test]
    fn a_busy_server_hasnt_answered() {
        assert!(!answered(503, b"x") && !answered(0, b"x") && !answered(200, b""));
        assert!(answered(200, b"{}") && answered(403, b"{}"));
    }
}
