//! Writing things out for people: counts, lengths of time, dates, and text cut to fit.

/// `one`, or its plural for any count but 1 ("game", "games").
pub fn plural(n: usize, one: &str) -> String {
    if n == 1 { one.to_string() } else { format!("{one}s") }
}

/// `s` cut to at most `max` characters, ending in "…" when cut.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// `s` broken into lines of at most `width` characters, between words where it can be, and at
/// most `max_lines` of them, the last ending in "…" if there was more.
pub fn wrap(s: &str, width: usize, max_lines: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in s.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        // A word too long for a line of its own is broken where the line ends.
        while !word.is_empty() {
            let used = line.chars().count();
            let gap = usize::from(used > 0);
            if used + gap + word.len() <= width {
                if gap == 1 {
                    line.push(' ');
                }
                line.extend(word.drain(..));
            } else if used > 0 {
                lines.push(std::mem::take(&mut line));
            } else {
                line.extend(word.drain(..width));
                lines.push(std::mem::take(&mut line));
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.len() > max_lines {
        let rest = lines.split_off(max_lines.saturating_sub(1)).join(" ");
        lines.push(truncate(&rest, width));
        lines.truncate(max_lines);
    }
    lines
}

/// A length of time to the nearest unit worth showing: "45s", "12m", "3h 20m".
pub fn duration(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
    }
}

/// A length of time as a clock: "01:02:03".
pub fn clock(secs: u64) -> String {
    format!("{:02}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
}

/// How long ago something was, roughly: "just now", "5m ago", "2d ago".
pub fn ago(secs: u64) -> String {
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86400 => format!("{}h ago", secs / 3600),
        86400..2_592_000 => format!("{}d ago", secs / 86400),
        _ => format!("{}mo ago", secs / 2_592_000),
    }
}

/// A Unix time in the local time zone.
fn local(t: u64) -> Option<libc::tm> {
    let tt = t as libc::time_t;
    // SAFETY: localtime_r only writes the tm it's given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    (!unsafe { libc::localtime_r(&tt, &mut tm) }.is_null()).then_some(tm)
}

/// A Unix time as a local date and time, like "2026-10-24 12:00:00".
pub fn datetime(t: u64) -> String {
    let Some(tm) = local(t) else { return String::new() };
    format!(
        "{}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// A Unix time as a local date, like "Oct 24, 2026".
pub fn date(t: u64) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let Some(tm) = local(t) else { return String::new() };
    format!("{} {}, {}", MONTHS[tm.tm_mon as usize], tm.tm_mday, tm.tm_year + 1900)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_breaks_between_words_and_cuts_what_doesnt_fit() {
        let s = "Help out Professor Birch with the help of your new little friend";
        assert_eq!(wrap(s, 30, 3), ["Help out Professor Birch with", "the help of your new little", "friend"]);
        assert_eq!(wrap(s, 30, 2), ["Help out Professor Birch with", "the help of your new little f…"]);
        assert_eq!(wrap("Supercalifragilistic", 8, 3), ["Supercal", "ifragili", "stic"]);
        assert_eq!(wrap("", 10, 2), Vec::<String>::new());
    }

    #[test]
    fn counts_lengths_and_cuts() {
        assert_eq!(format!("1 {}, 2 {}", plural(1, "game"), plural(2, "game")), "1 game, 2 games");
        assert_eq!(truncate("Chrono Trigger", 6), "Chron…");
        assert_eq!(truncate("Chrono", 6), "Chrono");
        assert_eq!((duration(45), duration(720), duration(12_000)), ("45s".into(), "12m".into(), "3h 20m".into()));
        assert_eq!(clock(3723), "01:02:03");
        assert_eq!(ago(2 * 86400), "2d ago");
    }
}
