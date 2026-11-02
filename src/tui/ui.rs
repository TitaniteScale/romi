//! Drawing. Only terminal palette colors and modifiers are used, so romi follows your theme.

mod overlays;
mod panels;

use crate::tui::app::{App, Level, PadState};
use overlays::{
    draw_art_prompt, draw_cheevos, draw_controls, draw_filter, draw_menu, draw_options, draw_osk, draw_players,
    draw_stats,
};
use panels::{draw_details, draw_embedded, draw_empty, draw_games, draw_grid, draw_playing, draw_systems};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};

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
    if let Some(c) = &mut app.covers {
        c.begin_frame();
    }
    let theme = Theme::from(app);
    let [header, body, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)]).areas(f.area());

    draw_header(f, app, &theme, header);
    let mut osk_area = None;
    if app.running.is_some() {
        draw_playing(f, app, &theme, body);
    } else if app.views.is_empty() {
        draw_empty(f, app, &theme, body);
    } else {
        // The cover grid takes the place of the list and the details.
        let grid = app.state.grid;
        let details = !grid && app.show_details && body.width >= DETAILS_MIN_WIDTH;
        // Sized from real systems only, so it doesn't jump when Favorites/Recent appear.
        // Systems and your tags' collections.
        let longest = app.views.iter().map(|v| v.name.chars().count() as u16).max().unwrap_or(0);
        let sys_width = longest.max("Favorites".len() as u16) + 12;
        let mut cols = vec![Constraint::Length(sys_width.min(32).min(body.width / 3)), Constraint::Min(30)];
        if details {
            cols.push(Constraint::Percentage(34));
        }
        let areas = Layout::horizontal(cols).split(body);
        app.middle = areas[1];
        draw_systems(f, app, &theme, areas[0]);
        if app.embedded.is_some() {
            draw_embedded(f, app, &theme, areas[1]);
        } else if grid {
            draw_grid(f, app, &theme, areas[1]);
        } else {
            draw_games(f, app, &theme, areas[1]);
        }
        if details {
            draw_details(f, app, &theme, areas[2]);
        }
        if let Some(letter) = app.flash() {
            draw_flash(f, &theme, areas[1], letter);
        }
        osk_area = Some(areas[1]);
    }
    if app.menu.is_some() {
        draw_menu(f, app, &theme, body);
    }
    // After the menu, since naming a tag brings it up over the menu.
    if let (Some(area), Some(_)) = (osk_area, app.osk) {
        draw_osk(f, app, &theme, area);
    }
    if app.art_prompt.is_some() {
        draw_art_prompt(f, app, &theme, body);
    }
    if app.filter_panel.is_some() {
        draw_filter(f, app, &theme, body);
    }
    if app.options.is_some() {
        draw_options(f, app, &theme, body);
    }
    if app.controls.is_some() {
        draw_controls(f, app, &theme, body);
    }
    if app.players.is_some() {
        draw_players(f, app, &theme, body);
    }
    if app.stats.is_some() {
        draw_stats(f, app, &theme, body);
    }
    if app.cheevos.is_some() {
        draw_cheevos(f, app, &theme, body);
        if let (Some(_), Some(screen)) = (app.osk, &app.cheevos)
            && screen.typing
        {
            draw_osk(f, app, &theme, body);
        }
    }
    draw_footer(f, app, &theme, footer);
}

/// A centered rect of the given size, clipped to `area`.
fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let (w, h) = (w.min(area.width), h.min(area.height));
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

fn draw_flash(f: &mut Frame, theme: &Theme, area: Rect, letter: &str) {
    let rect = centered(area, 9, 5);
    f.render_widget(Clear, rect);
    let text = Line::from(Span::styled(letter.to_string(), Style::new().fg(theme.accent).bold())).centered();
    f.render_widget(Paragraph::new(text).block(theme.block("", true).padding(Padding::vertical(1))), rect);
}

fn draw_header(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let left = Line::from(vec![
        Span::styled(" romi", Style::new().fg(theme.accent).bold()),
        Span::styled(
            if app.filter.is_active() {
                format!("  {} of {} games", app.filtered_count(), app.visible_count())
            } else {
                format!("  {} games", app.visible_count())
            },
            Style::new().dim(),
        ),
    ]);
    let pad = match &app.pad {
        PadState::Connected(name) => Span::styled(format!("● {name} "), Style::new().fg(theme.accent)),
        PadState::Searching => Span::styled("no controller ", Style::new().dim()),
        PadState::Disabled => Span::raw(""),
        PadState::Unavailable(e) => {
            Span::styled(format!("controller unavailable: {e} "), Style::new().fg(Color::Yellow))
        }
    };
    f.render_widget(Paragraph::new(left), area);
    f.render_widget(Paragraph::new(Line::from(pad)).alignment(Alignment::Right), area);
}

/// An achievement notice across the bottom of the game's picture, laid out by `notice`: its
/// badge, if it has one, in placeholder cells for the badge's image, and its lines beside it.
fn draw_toast(f: &mut Frame, theme: &Theme, layout: &crate::emu::notice::Layout) {
    use crate::emu::notice::Weight;
    f.render_widget(Clear, layout.area);
    f.render_widget(theme.block("", true), layout.area);
    for &(cells, slot) in &layout.badges {
        crate::emu::kitty::placeholders(f.buffer_mut(), cells, crate::emu::embedded::badge_image(slot));
    }
    let lines: Vec<Line> = layout
        .lines
        .iter()
        .map(|(text, weight)| {
            let style = match weight {
                Weight::Heading => Style::new().fg(theme.accent).bold(),
                Weight::Bold => Style::new().bold(),
                Weight::Plain => Style::new(),
            };
            Line::from(Span::styled(text.clone(), style))
        })
        .collect();
    let alignment = if layout.centered { Alignment::Center } else { Alignment::Left };
    f.render_widget(Paragraph::new(lines).alignment(alignment), layout.text);
}

fn draw_footer(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let hint = |k: &str, a: &str| {
        vec![
            Span::styled(format!(" {k} "), Style::new().fg(theme.accent)),
            Span::styled(a.to_string(), Style::new().dim()),
        ]
    };
    let hints = |pairs: &[(&str, &str)]| Line::from(pairs.iter().flat_map(|(k, a)| hint(k, a)).collect::<Vec<_>>());
    let pad = matches!(app.pad, PadState::Connected(_));
    let (ok, back) = if app.cfg.controller.swap_confirm { ("B", "A") } else { ("A", "B") };

    if let Some(screen) = &app.players {
        let line = match screen.pressed.is_empty() {
            true => hints(&[("any button", "next player"), ("esc", "cancel")]),
            false => hints(&[("any button", "next player"), (back, "done"), ("esc", "done")]),
        };
        f.render_widget(Paragraph::new(line), area);
        return;
    }
    if let Some(screen) = &app.controls {
        let line = match (screen.capturing, pad) {
            (true, true) => hints(&[("any button", "for it")]),
            (true, false) => hints(&[("a key", "for it"), ("esc", "cancel")]),
            (false, true) => hints(&[(ok, "change"), ("↑/↓", "button"), ("←/→", "game or system"), (back, "done")]),
            (false, false) => {
                hints(&[("enter", "change"), ("↑/↓", "button"), ("←/→", "game or system"), ("esc", "done")])
            }
        };
        f.render_widget(Paragraph::new(line), area);
        return;
    }
    if app.filter_panel.is_some() || app.options.is_some() {
        let line = if pad {
            hints(&[("←/→", "change"), ("↑/↓", "row"), (back, "done")])
        } else {
            hints(&[("←/→", "change"), ("↑/↓", "row"), ("esc", "done")])
        };
        f.render_widget(Paragraph::new(line), area);
        return;
    }
    if let Some(screen) = &app.cheevos {
        let line = match (&screen.open, pad) {
            (Some(_), true) => hints(&[("↑/↓", "scroll"), (back, "back")]),
            (Some(_), false) => hints(&[("↑/↓", "scroll"), ("esc", "back")]),
            (None, true) => {
                hints(&[(ok, "open"), ("←/→", "system"), ("X", "status"), ("Select", "search"), (back, "close")])
            }
            (None, false) => {
                hints(&[("enter", "open"), ("←/→", "system"), ("tab", "status"), ("/", "search"), ("esc", "close")])
            }
        };
        f.render_widget(Paragraph::new(line), area);
        return;
    }
    if app.stats.is_some() {
        let line = if pad { hints(&[(back, "close")]) } else { hints(&[("esc", "close")]) };
        f.render_widget(Paragraph::new(line), area);
        return;
    }
    if app.art_prompt.is_some() {
        let line = if pad {
            hints(&[("enter", "save"), (back, "cancel")])
        } else {
            hints(&[("enter", "save"), ("esc", "cancel")])
        };
        f.render_widget(Paragraph::new(line), area);
        return;
    }
    if app.tag_name.is_some() {
        let line = if pad {
            hints(&[(ok, "type"), ("X", "delete"), ("Y", "space"), ("✓", "save"), (back, "cancel")])
        } else {
            hints(&[("enter", "save"), ("esc", "cancel")])
        };
        f.render_widget(Paragraph::new(line), area);
        return;
    }
    if app.menu.is_some() {
        let line = if pad {
            hints(&[(ok, "select"), (back, "close")])
        } else {
            hints(&[("enter", "select"), ("esc", "close")])
        };
        f.render_widget(Paragraph::new(line), area);
        return;
    }
    if app.osk.is_some() {
        f.render_widget(Paragraph::new(hints(&[(ok, "type"), ("X", "delete"), ("Y", "space"), (back, "done")])), area);
        return;
    }
    if let Some(s) = &app.search {
        let mut spans = vec![
            Span::styled(" / ", Style::new().fg(theme.accent).bold()),
            Span::raw(s.query.clone()),
            Span::styled("█  ", Style::new().fg(theme.accent)),
        ];
        let more = if pad {
            hints(&[(ok, "play"), (back, "exit search"), ("Select", "edit")])
        } else {
            hints(&[("enter", "play"), ("esc", "cancel")])
        };
        spans.extend(more.spans);
        f.render_widget(Paragraph::new(Line::from(spans)), area);
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
    let line = if pad {
        hints(&[
            (ok, "play"),
            (back, "back"),
            ("Y", "fav"),
            ("X", "random"),
            ("LB/RB", "system"),
            ("LT/RT", "A–Z"),
            ("Select", "search"),
            ("Start", "menu"),
        ])
    } else {
        hints(&[
            ("enter", "play"),
            ("tab", "system"),
            ("/", "search"),
            ("f", "fav"),
            ("r", "random"),
            ("o", "game"),
            ("m", "menu"),
            ("q", "quit"),
        ])
    };
    f.render_widget(Paragraph::new(line), area);
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
