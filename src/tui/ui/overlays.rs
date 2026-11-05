//! What opens over the panels: menus, prompts, the on-screen keyboard and the full screens.

use super::{Theme, centered};
use crate::filter::ROWS as FILTER_ROWS;
use crate::text;
use crate::tui::app::{App, Level, PadState};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, Padding, Paragraph, Wrap};

pub(super) fn draw_filter(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let Some(sel) = app.filter_panel else { return };
    let lines: Vec<Line> = FILTER_ROWS
        .iter()
        .enumerate()
        .map(|(i, &row)| {
            let style = if i == sel { theme.selected(true) } else { Style::new() };
            if row == "Clear all" {
                let dim = if app.filter.is_active() { Style::new() } else { Style::new().dim() };
                return Line::from(Span::styled(format!("  {row:<34}"), style.patch(dim)));
            }
            let value = app.filter.value(i, &app.filter_options);
            Line::from(vec![
                Span::styled(format!("  {row:<9}"), style.patch(Style::new().dim())),
                Span::styled(format!("‹ {value:<20} ›   "), style),
            ])
        })
        .collect();
    let rect = centered(area, 42, lines.len() as u16 + 4);
    f.render_widget(Clear, rect);
    let count = Line::from(Span::styled(format!(" {} games ", app.filtered_count()), Style::new().dim()));
    let block = theme.block(" Filter ", true).title_bottom(count.right_aligned()).padding(Padding::vertical(1));
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

/// The options: each setting and what it's set to, changed in place with left and right.
pub(super) fn draw_options(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    use crate::tui::options::{ACCOUNT_ROW, SETTINGS};
    let Some(sel) = app.options else { return };
    let mut lines: Vec<Line> = SETTINGS
        .iter()
        .enumerate()
        .map(|(i, &setting)| {
            let style = if i == sel { theme.selected(true) } else { Style::new() };
            Line::from(vec![
                Span::styled(format!("  {:<18}", setting.label()), style.patch(Style::new().dim())),
                Span::styled(format!("‹ {:<17} ›   ", setting.shown(&app.cfg)), style),
            ])
        })
        .collect();
    let style = if sel == ACCOUNT_ROW { theme.selected(true) } else { Style::new() };
    let account = text::truncate(app.account.as_deref().unwrap_or("log in…"), 19);
    lines.push(Line::from(vec![
        Span::styled(format!("  {:<18}", "RetroAchievements"), style.patch(Style::new().dim())),
        Span::styled(format!("  {account:<22}"), style),
    ]));
    let rect = centered(area, 46, lines.len() as u16 + 4);
    f.render_widget(Clear, rect);
    // What saving says, since the status line is hidden under the panel's hints.
    let saved = match &app.status {
        Some(s) if matches!(s.level, Level::Error) => {
            let text = text::truncate(&s.text, 40);
            Line::from(Span::styled(format!(" {text} "), Style::new().fg(Color::Red)))
        }
        _ => {
            let file = app.cfg_path.file_name().map_or("the config".into(), |n| n.to_string_lossy().into_owned());
            Line::from(Span::styled(format!(" saved in {file} "), Style::new().dim()))
        }
    };
    let block = theme.block(" Options ", true).title_bottom(saved.right_aligned()).padding(Padding::vertical(1));
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

/// The RetroAchievements login, and the on-screen keyboard while something's being typed.
pub(super) fn draw_login(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    use crate::tui::login::Step;
    let Some(screen) = &app.login else { return };
    let note = "Unlocks in romi are softcore, and romi keeps a login token, not your password.";
    let option = |picked: bool, name: String| {
        let style = if picked { theme.selected(true) } else { Style::new() };
        Line::from(Span::styled(format!(" {} {name} ", if picked { "›" } else { " " }), style))
    };
    let two = |first: String, second: &str| {
        vec![option(screen.choice == 0, first), option(screen.choice == 1, second.to_string())]
    };
    let mut lines = match screen.step {
        Step::Found => {
            let name = screen.retroarch.as_ref().map_or("", |l| l.username.as_str());
            let mut lines = vec![Line::raw(format!("RetroArch is logged in as {name}.")), Line::raw("")];
            lines.extend(two(format!("Use {name} here too"), "Log in with another account"));
            lines.extend([Line::raw(""), Line::from(Span::styled(note, Style::new().dim()))]);
            lines
        }
        Step::Username | Step::Password => vec![Line::from(Span::styled(note, Style::new().dim()))],
        Step::Waiting => vec![Line::raw(format!("Logging in as {}…", screen.username))],
        Step::LogOut => {
            let mut lines = vec![Line::raw(format!("Logged in as {}.", screen.username)), Line::raw("")];
            lines.extend(two("Stay logged in".to_string(), "Log out"));
            lines
        }
    };
    if let Some(problem) = &screen.problem {
        lines.extend([Line::raw(""), Line::from(Span::styled(problem.clone(), Style::new().fg(Color::Red)))]);
    }
    const WIDTH: u16 = 60;
    let height = lines.iter().map(|l| (l.width() as u16).div_ceil(WIDTH - 4).max(1)).sum::<u16>() + 4;
    let mut rect = centered(area, WIDTH, height);
    if screen.typing() {
        // Above the keyboard.
        rect.y = area.y + 1;
    }
    f.render_widget(Clear, rect);
    let block = theme.block(" RetroAchievements ", true).padding(Padding::new(1, 1, 1, 0));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), rect);
    if screen.typing() {
        draw_osk(f, app, theme, area);
    }
}

/// The controls screen: each of the game's buttons, and the one on the pad that sends it.
pub(super) fn draw_controls(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    use crate::tui::controls::{Row, pressed_label};
    let Some(screen) = &app.controls else { return };
    let pad = matches!(app.pad, PadState::Connected(_));
    let (below, now) = screen.remaps(&app.cfg, &app.lib);
    let game = &app.lib.games[screen.game];
    let system = &app.lib.system_of(screen.game).name;
    const WIDTH: usize = 44;
    let lines: Vec<Line> = screen
        .rows()
        .into_iter()
        .enumerate()
        .map(|(i, row)| {
            let style = if i == screen.selected { theme.selected(true) } else { Style::new() };
            let dim = style.patch(Style::new().dim());
            match row {
                Row::For => {
                    let shown = if screen.for_system { format!("All {system} games") } else { "This game".into() };
                    let shown = text::truncate(&shown, WIDTH - 17);
                    Line::from(vec![
                        Span::styled(format!("  {:<11}", "For"), dim),
                        Span::styled(format!("‹ {shown:<width$} ›  ", width = WIDTH - 17), style),
                    ])
                }
                Row::Button(place) => {
                    let pressed = match screen.capturing && i == screen.selected {
                        true => "press a button…".to_string(),
                        false => {
                            let by: Vec<_> = now.sent_by(place).map(|b| pressed_label(b, pad)).collect();
                            if by.is_empty() { "nothing".into() } else { by.join(" / ") }
                        }
                    };
                    let changed = now.sent_by(place).ne(below.sent_by(place));
                    let mark = if changed { "•" } else { " " };
                    let name = text::truncate(screen.name(place), 18);
                    Line::from(vec![
                        Span::styled(format!("  {name:<18} "), dim),
                        Span::styled(format!("{mark} {pressed:<width$}", width = WIDTH - 21), style),
                    ])
                }
                Row::Reset => {
                    let changed = now.controls_over(&below);
                    let label = format!("  {:<width$}", "Reset", width = WIDTH);
                    Line::from(Span::styled(label, if changed.is_empty() { dim } else { style }))
                }
            }
        })
        .collect();
    let rect = centered(area, WIDTH as u16 + 4, lines.len() as u16 + 4);
    // Scrolled to keep the selected row in sight, when they don't all fit.
    let shown = rect.height.saturating_sub(4) as usize;
    let scroll = (screen.selected + 1).saturating_sub(shown) as u16;
    f.render_widget(Clear, rect);
    let saved = match &app.status {
        Some(s) if matches!(s.level, Level::Error) => {
            let text = text::truncate(&s.text, WIDTH - 2);
            Line::from(Span::styled(format!(" {text} "), Style::new().fg(Color::Red)))
        }
        _ => {
            let file = match screen.for_system {
                true => app.cfg_path.file_name().map_or("the config".into(), |n| n.to_string_lossy().into_owned()),
                false => crate::overrides::OVERRIDES_FILE.to_string(),
            };
            Line::from(Span::styled(format!(" saved in {file} "), Style::new().dim()))
        }
    };
    let title = format!(" Controls: {} ", text::truncate(&game.name, WIDTH - 12));
    let block = theme.block(title, true).title_bottom(saved.right_aligned()).padding(Padding::vertical(1));
    f.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), rect);
}

/// The players screen: each controller and the player it is, the ones not pressed yet dimmed.
pub(super) fn draw_players(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let Some(screen) = &app.players else { return };
    const WIDTH: usize = 44;
    let next = screen.pressed.len() + 1;
    let mut lines = vec![
        Line::from(Span::styled(format!("  Press a button on player {next}'s controller"), Style::new().dim())),
        Line::default(),
    ];
    for (i, pad) in screen.order(&app.pads, &app.state.players).iter().enumerate() {
        let pressed = screen.pressed.contains(pad);
        let style = if pressed { Style::new().fg(theme.accent) } else { Style::new().dim() };
        let pad = text::truncate(pad, WIDTH - 13);
        lines.push(Line::from(vec![
            Span::styled(format!("  Player {}  ", i + 1), style.bold()),
            Span::styled(pad, style),
        ]));
    }
    let rect = centered(area, WIDTH as u16 + 4, lines.len() as u16 + 4);
    f.render_widget(Clear, rect);
    let block = theme.block(" Players ", true).padding(Padding::vertical(1));
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

pub(super) fn draw_menu(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let Some(menu) = &app.menu else { return };
    const WIDTH: usize = 26;
    let items: Vec<Line> = app
        .menu_items()
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            let label = text::truncate(&app.menu_label(item), WIDTH);
            let style = if i == menu.selected { theme.selected(true) } else { Style::new() };
            Line::from(Span::styled(format!("  {label:<WIDTH$}"), style))
        })
        .collect();
    let rect = centered(area, WIDTH as u16 + 6, items.len() as u16 + 4);
    f.render_widget(Clear, rect);
    let title = match menu.game {
        Some(g) => format!(" {} ", text::truncate(&app.lib.games[g].name, WIDTH)),
        None => " romi ".into(),
    };
    let block = theme.block(title, true).padding(Padding::vertical(1));
    f.render_widget(Paragraph::new(items).block(block), rect);
}

/// The prompt for a game's new cover.
pub(super) fn draw_art_prompt(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let Some(prompt) = &app.art_prompt else { return };
    let width = area.width.min(66);
    // Inside the border, a space each side, the prompt and the cursor.
    let room = width.saturating_sub(8) as usize;
    // The end of a long path is what tells it apart.
    let extra = prompt.text.chars().count().saturating_sub(room);
    let shown = match extra {
        0 => prompt.text.clone(),
        _ => format!("…{}", prompt.text.chars().skip(extra + 1).collect::<String>()),
    };
    let mut lines = vec![
        Line::raw("Drag an image here, or paste its path. Esc to cancel."),
        Line::raw(""),
        Line::from(vec![
            Span::styled("> ", Style::new().fg(theme.accent).bold()),
            Span::raw(shown),
            Span::styled("█", Style::new().fg(theme.accent)),
        ]),
    ];
    let mut height = lines.len() as u16 + 4;
    if let Some(error) = &prompt.error {
        lines.push(Line::raw(""));
        lines.push(Line::styled(error.clone(), Style::new().fg(Color::Red)));
        // Words wrap, which can take a line more than the length says.
        let wrapped = error.chars().count().div_ceil(room.max(1) + 2) + 1;
        height += 1 + wrapped as u16;
    }
    let rect = centered(area, width, height);
    f.render_widget(Clear, rect);
    let title = format!(" Add art: {} ", text::truncate(&prompt.name, 40));
    let block = theme.block(title, true).padding(Padding::uniform(1));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), rect);
}

/// The on-screen keyboard, docked at the bottom of the games pane so results stay visible above.
pub(super) fn draw_osk(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let Some((row, col)) = app.osk else { return };
    let (title, prompt, query) = match (&app.login, &app.tag_name, &app.cheevos) {
        (Some(l), ..) if l.step == crate::tui::login::Step::Password => {
            (" Password ", " > ", "•".repeat(l.password.chars().count()))
        }
        (Some(l), ..) => (" Username ", " > ", l.username.clone()),
        (_, Some(name), _) => (" New tag ", " # ", name.clone()),
        (_, None, Some(screen)) if screen.typing => (" Search ", " / ", screen.query.clone()),
        _ => (" Search ", " / ", app.search.as_ref().map(|s| s.query.clone()).unwrap_or_default()),
    };
    let capitals = app.login.as_ref().is_some_and(|l| l.shift);
    let keys = app.osk_keys();
    let mut lines = vec![
        Line::from(vec![
            Span::styled(prompt, Style::new().fg(theme.accent).bold()),
            Span::raw(query),
            Span::styled("█", Style::new().fg(theme.accent)),
        ]),
        Line::raw(""),
    ];
    for (r, row_keys) in keys.iter().enumerate() {
        let spans = row_keys.iter().enumerate().map(|(c, k)| {
            let style = if (r, c) == (row, col) { theme.selected(true) } else { Style::new() };
            let k = if capitals { k.to_ascii_uppercase() } else { k.to_string() };
            let k = if k.is_empty() { " ".to_string() } else { k };
            Span::styled(format!(" {k} "), style)
        });
        lines.push(Line::from(spans.collect::<Vec<_>>()).centered());
    }
    let h = lines.len() as u16 + 2;
    let w = (keys[0].len() as u16 * 3 + 4).min(area.width);
    let rect = Rect::new(area.x + (area.width - w) / 2, (area.y + area.height).saturating_sub(h + 1).max(area.y), w, h);
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(theme.block(title, true)), rect);
}

/// The stats screen: totals, play time by system and by game, and this year month by month.
pub(super) fn draw_stats(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let Some(s) = &app.stats else { return };
    f.render_widget(Clear, area);
    let block = theme.block(" Stats ", true).padding(Padding::new(2, 2, 1, 0));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let accent = Style::new().fg(theme.accent);
    let heading = |t: String| Line::from(Span::styled(t, accent.bold()));
    if s.games == 0 {
        f.render_widget(Paragraph::new("Nothing played from romi yet.").dim(), inner);
        return;
    }
    let summary = Line::from(vec![
        Span::styled(text::duration(s.total_secs), accent.bold()),
        Span::raw(format!(" played across {} {}", s.games, text::plural(s.games, "game"))),
    ]);
    let [top, columns, year] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(4), Constraint::Length(6)]).areas(inner);
    f.render_widget(Paragraph::new(summary), top);

    // Name, time and a bar scaled to the first (biggest) row.
    let table = |title: &str, rows: &[(String, u64)], width: u16| {
        let most = rows.first().map_or(1, |r| r.1.max(1));
        let bar_room = u64::from(width.saturating_sub(4) / 4);
        let name_w = (width as usize).saturating_sub(bar_room as usize + 11).max(8);
        let mut lines = vec![heading(title.to_string())];
        for (name, secs) in rows {
            let bar = "█".repeat((secs * bar_room).div_ceil(most) as usize);
            lines.push(Line::from(vec![
                Span::raw(format!("{:<name_w$} ", text::truncate(name, name_w))),
                Span::styled(format!("{:>8}  ", text::duration(*secs)), Style::new().dim()),
                Span::styled(bar, accent),
            ]));
        }
        lines
    };
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).spacing(4).areas(columns);
    f.render_widget(Paragraph::new(table("By system", &s.systems, left.width)), left);
    f.render_widget(Paragraph::new(table("Most played", &s.top, right.width)), right);

    // The year: a bar per month, or a note while the history is new.
    let y = &s.year;
    let mut lines = vec![heading(y.year.to_string())];
    if y.sessions == 0 {
        let note = if y.since.is_none() {
            "Sessions are recorded from now on; the months fill in as you play."
        } else {
            "Nothing played yet this year."
        };
        lines.push(Line::from(Span::raw(note).dim()));
    } else {
        const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
        const LETTERS: [&str; 12] = ["J", "F", "M", "A", "M", "J", "J", "A", "S", "O", "N", "D"];
        let most = y.months.iter().copied().max().unwrap_or(1).max(1);
        let bars: String = y
            .months
            .iter()
            .map(|&m| if m == 0 { "  ".to_string() } else { format!("{} ", BARS[((m * 7).div_ceil(most)) as usize]) })
            .collect();
        let total: u64 = y.months.iter().sum();
        let best = y.top.first().map(|(n, t)| format!(" · most: {n} ({})", text::duration(*t))).unwrap_or_default();
        lines.push(Line::from(Span::styled(bars, accent)));
        lines.push(Line::from(Span::raw(LETTERS.join(" ")).dim()));
        let sessions = text::plural(y.sessions, "session");
        lines.push(Line::raw(format!("{} in {} {sessions}{best}", text::duration(total), y.sessions)));
    }
    f.render_widget(Paragraph::new(lines), year);
}

/// The achievements screen: the games list with its filters, or one game's achievements.
pub(super) fn draw_cheevos(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let Some(screen) = &app.cheevos else { return };
    f.render_widget(Clear, area);
    let accent = Style::new().fg(theme.accent);
    if let Some(open) = &screen.open {
        let game = &app.lib.games[open.game];
        let block = theme.block(format!(" {} ", game.name), true).padding(Padding::horizontal(1));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let mut lines = Vec::new();
        if let Some(a) = app.achievements.games.get(&game.key) {
            let refreshing = if screen.loading() && open.cheevos.is_some() { "  refreshing…" } else { "" };
            lines.push(Line::from(vec![Span::styled(a.summary(), accent.bold()), Span::raw(refreshing).dim()]));
            if !a.supported {
                lines.push(Line::from(
                    Span::raw("Your ROM can't earn these: RetroAchievements supports other dumps.").dim(),
                ));
            }
        }
        if let Some(e) = &open.error {
            lines.push(Line::from(Span::styled(format!("Couldn't refresh: {e}"), Style::new().fg(Color::Yellow))));
        }
        lines.push(Line::raw(""));
        let Some(cheevos) = &open.cheevos else {
            lines.push(Line::from(Span::raw("Loading…").dim()));
            f.render_widget(Paragraph::new(lines), inner);
            return;
        };
        let width = inner.width as usize;
        for c in cheevos.iter().skip(open.top) {
            let earned = c.earned_hardcore.as_ref().or(c.earned.as_ref());
            let mark = if earned.is_some() { Span::styled("✓ ", accent.bold()) } else { Span::raw("· ").dim() };
            let mut notes = Vec::new();
            if c.points > 0 {
                notes.push(format!("{} points", c.points));
            }
            if c.rarity >= 0.0 {
                notes.push(format!("{:.0}% of players", c.rarity * 100.0));
            }
            notes.extend(c.kind.as_ref().map(|k| k.replace('_', " ")));
            if let Some(when) = earned {
                // A login doesn't get the date.
                let date = when.split(' ').next().unwrap_or(when);
                notes.push(match (c.earned_hardcore.is_some(), date.is_empty()) {
                    (false, true) => "earned".to_string(),
                    (false, false) => format!("earned {date}"),
                    (true, true) => "earned hardcore".to_string(),
                    (true, false) => format!("earned hardcore, {date}"),
                });
            }
            let title = if earned.is_some() { accent.bold() } else { Style::new().bold() };
            lines.push(Line::from(vec![
                mark,
                Span::styled(c.title.clone(), title),
                Span::raw(format!("  {}", notes.join(" · "))).dim(),
            ]));
            lines.push(Line::from(
                Span::raw(format!("  {}", text::truncate(&c.description, width.saturating_sub(2)))).dim(),
            ));
        }
        app.page = inner.height as usize;
        f.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let rows = screen.rows(&app.lib, &app.achievements, app.show_hidden);
    let block = theme.block(" Achievements ", true).padding(Padding::horizontal(1));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let system = screen.system.as_deref().unwrap_or("all systems");
    let mut filters = vec![
        Span::raw("‹ ").dim(),
        Span::styled(system.to_string(), accent),
        Span::raw(" ›   ").dim(),
        Span::raw("showing ").dim(),
        Span::styled(screen.status.label(), accent),
    ];
    if screen.typing || !screen.query.is_empty() {
        filters.push(Span::raw("   / ").dim());
        filters.push(Span::raw(screen.query.clone()));
        if screen.typing {
            filters.push(Span::styled("█", accent));
        }
    }
    let count = format!("{} {}", rows.len(), text::plural(rows.len(), "game"));
    let [head, list] = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(inner);
    f.render_widget(Paragraph::new(vec![Line::from(filters), Line::from(Span::raw(count).dim())]), head);

    app.page = list.height as usize;
    // Every column but the name has a fixed width, so they line up down the list.
    const BAR: usize = 10;
    const AWARD: usize = 16; // "beaten hardcore", "another dump"
    let system_w = if screen.system.is_none() {
        rows.iter().map(|&g| app.lib.games[g].system.chars().count()).max().unwrap_or(0) + 2
    } else {
        0
    };
    let name_w = (list.width as usize).saturating_sub(system_w + BAR + 2 + 19 + AWARD);
    let items: Vec<ListItem> = rows
        .iter()
        .map(|&g| {
            let game = &app.lib.games[g];
            let a = &app.achievements.games[&game.key];
            let counts =
                if a.supported { format!("{:>4}/{:<4}", a.earned, a.total) } else { format!("{:>9}", a.total) };
            let award = match (&a.award, a.supported) {
                (_, false) => "another dump".to_string(),
                (Some(award), true) => award.replace('-', " "),
                (None, true) => String::new(),
            };
            let points =
                if a.source == crate::ra::Source::Steam { " ".repeat(9) } else { format!("{:>5} pts", a.points) };
            let status = format!("{counts} {points}  {award:<AWARD$}");
            let filled = (a.earned as usize * BAR).checked_div(a.total as usize).unwrap_or(0).min(BAR);
            let bar = format!("{}{}", "█".repeat(filled), "░".repeat(BAR - filled));
            let system = if system_w > 0 { format!("{:<w$}", game.system, w = system_w) } else { String::new() };
            let style = if a.supported { Style::new() } else { Style::new().dim() };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<name_w$}  ", text::truncate(&game.name, name_w)), style),
                Span::raw(system).dim(),
                Span::styled(bar, accent),
                Span::styled(format!("  {status}"), style),
            ]))
        })
        .collect();
    let mut state = ratatui::widgets::ListState::default().with_selected((!rows.is_empty()).then_some(screen.selected));
    f.render_stateful_widget(List::new(items).highlight_style(theme.selected(true)), list, &mut state);
}
