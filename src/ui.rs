//! Drawing. Only terminal palette colors and modifiers are used, so romi follows your theme.

use crate::app::{App, Level, Pane, PadState, ViewKind};
use crate::state::now;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap};

const DETAILS_MIN_WIDTH: u16 = 110;

struct Theme {
    accent: Color,
    borders: Option<BorderType>,
}

impl Theme {
    fn from(app: &App) -> Theme {
        let t = &app.cfg.theme;
        let borders = match t.borders.as_str() {
            "none" => None,
            "plain" => Some(BorderType::Plain),
            "thick" => Some(BorderType::Thick),
            "double" => Some(BorderType::Double),
            _ => Some(BorderType::Rounded),
        };
        Theme { accent: parse_color(&t.accent).unwrap_or(Color::Blue), borders }
    }

    fn block<'a>(&self, title: impl Into<Line<'a>>, focused: bool) -> Block<'a> {
        let mut b = Block::new().title(title);
        if let Some(bt) = self.borders {
            b = b.borders(Borders::ALL).border_type(bt);
        }
        let border = if focused { Style::new().fg(self.accent) } else { Style::new().add_modifier(Modifier::DIM) };
        b.border_style(border).title_style(Style::new().add_modifier(Modifier::BOLD))
    }

    fn selected(&self, focused: bool) -> Style {
        if focused {
            Style::new().fg(self.accent).add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            Style::new().fg(self.accent).add_modifier(Modifier::BOLD)
        }
    }
}

pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim().to_lowercase().replace(['-', ' '], "_");
    Some(match s.as_str() {
        "default" | "reset" => Color::Reset,
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::Gray,
        "bright_black" | "gray" | "grey" => Color::DarkGray,
        "bright_red" => Color::LightRed,
        "bright_green" => Color::LightGreen,
        "bright_yellow" => Color::LightYellow,
        "bright_blue" => Color::LightBlue,
        "bright_magenta" => Color::LightMagenta,
        "bright_cyan" => Color::LightCyan,
        "bright_white" => Color::White,
        hex if hex.starts_with('#') && hex.len() == 7 => {
            let n = u32::from_str_radix(&hex[1..], 16).ok()?;
            Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8)
        }
        idx => Color::Indexed(idx.parse().ok()?),
    })
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let theme = Theme::from(app);
    let [header, body, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)]).areas(f.area());

    draw_header(f, app, &theme, header);
    if app.running.is_some() {
        draw_playing(f, app, &theme, body);
    } else if app.views.is_empty() {
        draw_empty(f, app, &theme, body);
    } else {
        let details = app.show_details && body.width >= DETAILS_MIN_WIDTH;
        // Sized from real systems only, so it doesn't jump when Favorites/Recent appear.
        let longest = app.lib.systems.iter().map(|s| s.name.chars().count() as u16).max().unwrap_or(0);
        let sys_width = longest.max("Favorites".len() as u16) + 12;
        let mut cols = vec![Constraint::Length(sys_width.min(32).min(body.width / 3)), Constraint::Min(30)];
        if details {
            cols.push(Constraint::Percentage(34));
        }
        let areas = Layout::horizontal(cols).split(body);
        draw_systems(f, app, &theme, areas[0]);
        draw_games(f, app, &theme, areas[1]);
        if details {
            draw_details(f, app, &theme, areas[2]);
        }
    }
    draw_footer(f, app, &theme, footer);
}

fn draw_header(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let left = Line::from(vec![
        Span::styled(" romi", Style::new().fg(theme.accent).bold()),
        Span::styled(format!("  {} games", app.lib.games.len()), Style::new().dim()),
    ]);
    let pad = match &app.pad {
        PadState::Connected(name) => Span::styled(format!("● {name} "), Style::new().fg(theme.accent)),
        PadState::Searching => Span::styled("no controller ", Style::new().dim()),
        PadState::Disabled => Span::raw(""),
        PadState::Unavailable(e) => Span::styled(format!("controller unavailable: {e} "), Style::new().fg(Color::Yellow)),
    };
    f.render_widget(Paragraph::new(left), area);
    f.render_widget(Paragraph::new(Line::from(pad)).alignment(Alignment::Right), area);
}

fn draw_systems(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let focused = app.focus == Pane::Systems && app.search.is_none();
    let block = theme.block(" Systems ", focused);
    let inner_width = block.inner(area).width as usize;
    let items: Vec<ListItem> = app
        .views
        .iter()
        .map(|v| {
            let (icon, problem) = match &v.kind {
                ViewKind::Favorites => ("★ ", false),
                ViewKind::Recent => ("◷ ", false),
                ViewKind::System(i) => ("  ", app.lib.systems[*i].problem.is_some()),
            };
            let count = format!("{:>3} ", v.games.len());
            let mut spans = vec![Span::raw(icon), Span::raw(v.name.clone())];
            let used = Line::from(spans.clone()).width() + count.len() + 2;
            spans.push(Span::raw(" ".repeat(inner_width.saturating_sub(used))));
            spans.push(Span::styled(if problem { "! " } else { "  " }, Style::new().fg(Color::Yellow)));
            spans.push(Span::styled(count, Style::new().dim()));
            ListItem::new(Line::from(spans))
        })
        .collect();
    app.systems_list.select(Some(app.view));
    let list = List::new(items).block(block).highlight_style(theme.selected(focused));
    f.render_stateful_widget(list, area, &mut app.systems_list);
}

fn draw_games(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let focused = app.focus == Pane::Games || app.search.is_some();
    // Pseudo views and search mix systems, so label each row with its system.
    let mixed = app.search.is_some() || !matches!(app.current_view().map(|v| &v.kind), Some(ViewKind::System(_)));
    let ids = app.list().to_vec();
    let title = match (&app.search, app.current_view()) {
        (Some(s), _) => format!(" Search · {} ", s.results.len()),
        (None, Some(v)) => format!(" {} ", v.name),
        (None, None) => String::new(),
    };
    let position = if ids.is_empty() { String::new() } else { format!(" {}/{} ", app.game + 1, ids.len()) };
    let block = theme
        .block(title, focused)
        .title_bottom(Line::from(Span::styled(position, Style::new().dim())).right_aligned());
    let inner = block.inner(area);
    app.page = inner.height.saturating_sub(1) as usize;

    if ids.is_empty() {
        let msg = if app.search.is_some() { "No matches" } else { "Nothing here yet" };
        f.render_widget(Paragraph::new(msg).dim().block(block), area);
        return;
    }

    let width = inner.width as usize;
    let items: Vec<ListItem> = ids
        .iter()
        .map(|&g| {
            let game = &app.lib.games[g];
            let fav = app.state.is_favorite(&game.key);
            let tag = if mixed { format!("{} ", game.system) } else { String::new() };
            // Leave room for the system tag; the name gives way first, then the suffix.
            let room = width.saturating_sub(2 + tag.chars().count() + if mixed { 2 } else { 0 });
            let name = truncate(&game.name, room);
            let mut spans = vec![
                Span::styled(if fav { "★ " } else { "  " }, Style::new().fg(theme.accent)),
                Span::raw(name.clone()),
            ];
            if let Some(sfx) = &game.suffix {
                let left = room.saturating_sub(name.chars().count() + 2);
                if left > 3 {
                    spans.push(Span::styled(format!("  {}", truncate(sfx, left)), Style::new().dim()));
                }
            }
            if mixed {
                let used = Line::from(spans.clone()).width();
                let gap = width.saturating_sub(used + tag.chars().count()).max(2);
                spans.push(Span::raw(" ".repeat(gap)));
                spans.push(Span::styled(tag, Style::new().dim()));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    app.games_list.select(Some(app.game));
    let list = List::new(items).block(block).highlight_style(theme.selected(focused)).scroll_padding(2);
    f.render_stateful_widget(list, area, &mut app.games_list);
}

fn draw_details(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let block = theme.block(" Details ", false).padding(Padding::horizontal(1));
    let Some(g) = app.selected_game() else {
        f.render_widget(block, area);
        return;
    };
    let game = &app.lib.games[g];
    let label = |k: &str| Span::styled(format!("{k:<10}"), Style::new().dim());
    let mut lines = vec![
        Line::from(Span::styled(game.name.clone(), Style::new().fg(theme.accent).bold())),
        Line::from(Span::styled(game.system.clone(), Style::new().dim())),
        Line::raw(""),
    ];
    if let Some(problem) = app.system_problem(g) {
        lines.push(Line::from(Span::styled(format!("! {problem}"), Style::new().fg(Color::Yellow))));
        lines.push(Line::raw(""));
    }
    let m = &game.meta;
    for (k, v) in [("Developer", &m.developer), ("Genre", &m.genre), ("Released", &m.released), ("Players", &m.players)] {
        if let Some(v) = v {
            lines.push(Line::from(vec![label(k), Span::raw(v.clone())]));
        }
    }
    if let Some(&secs) = app.state.playtime.get(&game.key).filter(|&&s| s > 0) {
        lines.push(Line::from(vec![label("Played"), Span::raw(fmt_duration(secs))]));
    }
    if let Some(&at) = app.state.last_played.get(&game.key) {
        lines.push(Line::from(vec![label("Last"), Span::raw(fmt_ago(now().saturating_sub(at)))]));
    }
    if let Some(file) = game.path.as_ref().and_then(|p| p.file_name()) {
        lines.push(Line::from(vec![label("File"), Span::raw(file.to_string_lossy().into_owned())]));
    }
    lines.push(Line::from(vec![label("Runs"), Span::raw(runs_label(&game.launch.command))]));
    if let Some(desc) = &m.description {
        lines.push(Line::raw(""));
        lines.extend(desc.lines().map(|l| Line::raw(l.to_string())));
    }
    f.render_widget(Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }).block(block), area);
}

fn draw_playing(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let Some(run) = &app.running else { return };
    let game = &app.lib.games[run.game];
    let lines = vec![
        Line::from(Span::styled("▶ Now playing", Style::new().fg(theme.accent).bold())),
        Line::raw(""),
        Line::from(Span::styled(game.name.clone(), Style::new().bold())),
        Line::from(Span::styled(game.system.clone(), Style::new().dim())),
        Line::raw(""),
        Line::raw(fmt_clock(run.started.elapsed().as_secs())),
        Line::raw(""),
        Line::from(Span::styled("romi will be back when the game closes", Style::new().dim())),
    ];
    let h = lines.len() as u16 + 2;
    let w = (game.name.chars().count() as u16 + 8).clamp(44, area.width);
    let [_, mid, _] = Layout::vertical([Constraint::Fill(1), Constraint::Length(h), Constraint::Fill(1)]).areas(area);
    let [_, rect, _] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(w), Constraint::Fill(1)]).areas(mid);
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center).block(theme.block("", true)), rect);
}

fn draw_empty(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let lines = vec![
        Line::from(Span::styled("No games found", Style::new().fg(theme.accent).bold())),
        Line::raw(""),
        Line::raw(format!("ROMs are read from {}", app.cfg.roms_dir)),
        Line::raw(format!("Edit {} and press ctrl-r", crate::app::tilde(&app.cfg_path))),
        Line::raw("or run `romi --check` to see what's going on."),
    ];
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center).block(theme.block("", true)), area);
}

fn draw_footer(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    if let Some(s) = &app.search {
        let line = Line::from(vec![
            Span::styled(" / ", Style::new().fg(theme.accent).bold()),
            Span::raw(s.query.clone()),
            Span::styled("█", Style::new().fg(theme.accent)),
            Span::styled("   enter play · esc cancel", Style::new().dim()),
        ]);
        f.render_widget(Paragraph::new(line), area);
        return;
    }
    if let Some(st) = app.visible_status() {
        let style = match st.level {
            Level::Info => Style::new().fg(theme.accent),
            Level::Warn => Style::new().fg(Color::Yellow),
            Level::Error => Style::new().fg(Color::Red),
        };
        f.render_widget(Paragraph::new(Line::from(Span::styled(format!(" {}", st.text), style))), area);
        return;
    }
    if app.running.is_some() {
        f.render_widget(Paragraph::new(" controller and keys paused while playing").dim(), area);
        return;
    }
    let hint = |k: &str, a: &str| vec![Span::styled(format!(" {k} "), Style::new().fg(theme.accent)), Span::styled(a.to_string(), Style::new().dim())];
    let mut spans = Vec::new();
    if matches!(app.pad, PadState::Connected(_)) {
        let (ok, back) = if app.cfg.controller.swap_confirm { ("B", "A") } else { ("A", "B") };
        for (k, a) in [(ok, "play"), (back, "back"), ("Y", "fav"), ("X", "random"), ("LB/RB", "system")] {
            spans.extend(hint(k, a));
        }
    } else {
        for (k, a) in [("enter", "play"), ("tab", "system"), ("/", "search"), ("f", "fav"), ("r", "random"), ("q", "quit")] {
            spans.extend(hint(k, a));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// "retroarch · mgba" for libretro cores, otherwise just the program name.
fn runs_label(command: &str) -> String {
    let mut words = command.split_whitespace();
    let program = words.next().unwrap_or_default();
    let program = program.rsplit('/').next().unwrap_or(program);
    match words.find_map(|w| w.rsplit('/').next()?.strip_suffix("_libretro.so")) {
        Some(core) => format!("{program} · {core}"),
        None => program.to_string(),
    }
}

pub fn fmt_duration(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
    }
}

fn fmt_clock(secs: u64) -> String {
    format!("{:02}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
}

fn fmt_ago(secs: u64) -> String {
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86400 => format!("{}h ago", secs / 3600),
        86400..2_592_000 => format!("{}d ago", secs / 86400),
        _ => format!("{}mo ago", secs / 2_592_000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(parse_color("Bright Magenta"), Some(Color::LightMagenta));
        assert_eq!(parse_color("#ff8000"), Some(Color::Rgb(255, 128, 0)));
        assert_eq!(parse_color("208"), Some(Color::Indexed(208)));
        assert_eq!(parse_color("nope"), None);
    }
}
