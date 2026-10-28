//! `romi status`, `romi stats` and `romi achievements`.

use super::*;

// ---- achievements ----

pub(super) fn achievements(cfg: &Config, json: bool) -> Result<()> {
    let lib = Library::load(cfg);
    // Without RetroAchievements, Steam's own cache may still have something.
    let ach = if crate::ra::credentials(cfg).is_some() {
        crate::ra::Achievements::refresh(cfg, &lib).or_else(|e| fail(exit::UNAVAILABLE, format!("{e:#}")))?
    } else {
        crate::ra::Achievements::cached(cfg, &lib)
    };
    if ach.games.is_empty() && crate::ra::credentials(cfg).is_none() {
        let msg = format!(
            "RetroAchievements isn't set up: add \"retroachievements\": {{ \"username\": \"...\" }} to the config, \
             and put your web API key (retroachievements.org → Settings) in {} or RA_API_KEY",
            cfg.retroachievements.key_file
        );
        return fail(exit::CONFIG, msg);
    }
    // Games you've started first, furthest along first; then the rest by system and name.
    let mut games: Vec<(usize, &crate::ra::Info)> = lib
        .systems
        .iter()
        .flat_map(|s| &s.games)
        .filter_map(|&g| Some((g, ach.games.get(&lib.games[g].key)?)))
        .collect();
    games.sort_by_key(|&(_, a)| std::cmp::Reverse((a.earned * 1000).checked_div(a.total).unwrap_or(0)));
    if json {
        let rows: Vec<_> = games
            .iter()
            .map(|&(g, a)| serde_json::json!({ "key": lib.games[g].key, "name": lib.games[g].name, "achievements": a }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    let (ok, other): (Vec<_>, Vec<_>) = games.iter().partition(|(_, a)| a.supported);
    let points: u32 = ok.iter().map(|(_, a)| a.points).sum();
    let started = ok.iter().filter(|(_, a)| a.earned > 0).count();
    let mut out = stdout().lock();
    let points = if points > 0 { format!(", {points} RetroAchievements points") } else { String::new() };
    writeln!(out, "{} of your games have achievements{points}; you've earned some in {started}.", ok.len())?;
    let row = |out: &mut std::io::StdoutLock, g: usize, text: String| {
        let game = &lib.games[g];
        writeln!(out, "  {:<20} {:<40} {text}", text::truncate(&game.system, 20), text::truncate(&game.name, 40))
    };
    for (g, a) in &ok {
        row(&mut out, *g, a.summary())?;
    }
    if !other.is_empty() {
        writeln!(out, "\n{} more have achievements for another version of the ROM than yours:", other.len())?;
        for (g, a) in &other {
            row(&mut out, *g, format!("{} achievements · {}", a.total, a.hashes_url()))?;
        }
    }
    Ok(())
}

// ---- stats ----

pub(super) fn stats(cfg: &Config, json: bool) -> Result<()> {
    let lib = Library::load(cfg);
    let s = crate::stats::Stats::compute(&lib, &State::load(), &crate::stats::history(), now());
    if json {
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(());
    }
    let dur = crate::text::duration;
    if s.games == 0 {
        println!("Nothing played from romi yet.");
        return Ok(());
    }
    println!("Played {} across {} {}", dur(s.total_secs), s.games, text::plural(s.games, "game"));
    let table = |title: &str, rows: &[(String, u64)]| {
        let most = rows.first().map_or(1, |r| r.1.max(1));
        println!("\n{title}");
        for (name, secs) in rows {
            let bar = "█".repeat(((secs * 20).div_ceil(most)) as usize);
            println!("  {:<32} {:>8}  {bar}", text::truncate(name, 32), dur(*secs));
        }
    };
    table("By system", &s.systems);
    table("Most played", &s.top);
    let y = &s.year;
    println!("\n{}", y.year);
    match y.since {
        None => println!("  Sessions are recorded from now on; the months fill in as you play."),
        Some(_) if y.sessions == 0 => println!("  Nothing played yet this year."),
        Some(since) => {
            let year_total: u64 = y.months.iter().sum();
            let n = y.sessions;
            println!("  {} in {n} {}", dur(year_total), text::plural(n, "session"));
            let most = y.months.iter().copied().max().unwrap_or(1).max(1);
            for (m, secs) in MONTHS.iter().zip(y.months).filter(|(_, s)| *s > 0) {
                println!("  {m}  {:>8}  {}", dur(secs), "█".repeat(((secs * 20).div_ceil(most)) as usize));
            }
            if crate::stats::local_year_month(since).0 == y.year {
                println!("  (recorded since {})", crate::text::date(since));
            }
        }
    }
    if !y.top.is_empty() {
        table(&format!("Most played in {}", y.year), &y.top);
    }
    Ok(())
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

pub(super) fn status(json: bool) -> Result<()> {
    let running = session::running();
    if json {
        let games: Vec<_> = running
            .iter()
            .map(|r| {
                serde_json::json!({
                    "key": r.key,
                    "name": r.name,
                    "system": r.system,
                    "system_name": r.system_name,
                    "pid": r.pid,
                    "started": r.started,
                    "elapsed_secs": now().saturating_sub(r.started),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&games)?);
    } else if running.is_empty() {
        println!("nothing running");
    } else {
        for r in &running {
            let elapsed = crate::text::duration(now().saturating_sub(r.started));
            println!("{}  ({}, {elapsed})", r.name, r.system_name);
        }
    }
    Ok(())
}
