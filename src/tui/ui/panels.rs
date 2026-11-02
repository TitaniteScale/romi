//! The main panels: the systems, the games as a list or a grid, the details, and a game playing.

use super::{Theme, draw_toast, runs_label};
use crate::art::Kind;
use crate::sort::Sort;
use crate::state::now;
use crate::text;
use crate::tui::app::{App, Pane, ViewKind};
use crate::tui::cover::Cover;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect, Size};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, Padding, Paragraph, Wrap};
use ratatui_image::Image;
use std::time::{Duration, Instant};

pub(super) fn draw_systems(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
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
                ViewKind::Tag(_) => ("# ", false),
                ViewKind::System(i) => ("  ", app.lib.systems[*i].problem.is_some()),
            };
            let count = format!("{:>3} ", v.games.len());
            // The name gives way to the marker and the count, so a long one doesn't push them off.
            let room = inner_width.saturating_sub(icon.chars().count() + count.len() + 3);
            let mut spans = vec![Span::raw(icon), Span::raw(text::truncate(&v.name, room))];
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

/// The games pane's title: the view (or search), its order when unusual, and the filter.
fn games_title(app: &App) -> String {
    let mut title = match (&app.search, app.current_view()) {
        (Some(s), _) => format!(" Search · {}", s.results.len()),
        (None, Some(v)) => format!(" {}", v.name),
        (None, None) => String::new(),
    };
    // Only orders other than the usual one are worth a label.
    let ordered = matches!(app.current_view().map(|v| &v.kind), Some(ViewKind::Favorites | ViewKind::Tag(_)));
    let usual = ordered.then_some(Sort::Added);
    if let Some(sort) = app.sort().filter(|&s| s != usual.unwrap_or(Sort::Name)) {
        title.push_str(&format!(" · by {}", sort.label()));
    }
    for label in app.filter.labels() {
        title.push_str(&format!(" · {label}"));
    }
    title.push(' ');
    title
}

pub(super) fn draw_games(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let focused = app.focus == Pane::Games || app.search.is_some();
    // Pseudo views and search mix systems, so label each row with its system.
    let mixed = app.search.is_some() || !matches!(app.current_view().map(|v| &v.kind), Some(ViewKind::System(_)));
    let ids = app.list().to_vec();
    let title = games_title(app);
    let position = if ids.is_empty() { String::new() } else { format!(" {}/{} ", app.game + 1, ids.len()) };
    let block = theme
        .block(title, focused)
        .title_bottom(Line::from(Span::styled(position, Style::new().dim())).right_aligned());
    let inner = block.inner(area);
    app.page = inner.height.saturating_sub(1) as usize;
    app.grid_cols = 0;

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
            let name = text::truncate(&game.name, room);
            let mut spans = vec![
                Span::styled(if fav { "★ " } else { "  " }, Style::new().fg(theme.accent)),
                if game.hidden {
                    Span::styled(name.clone(), Style::new().dim().italic())
                } else {
                    Span::raw(name.clone())
                },
            ];
            if let Some(sfx) = &game.suffix {
                let left = room.saturating_sub(name.chars().count() + 2);
                if left > 3 {
                    spans.push(Span::styled(format!("  {}", text::truncate(sfx, left)), Style::new().dim()));
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

/// Draws a game's cover centered in `area`. Returns the rows to leave for it: the whole area while
/// it has art (loading or not), so the text below stays put while scrolling; 0 if it has none.
fn draw_cover(f: &mut Frame, app: &mut App, game: usize, area: Rect) -> u16 {
    let Some(covers) = app.covers.as_mut().filter(|_| app.cfg.art.show) else { return 0 };
    match covers.get(&app.lib, game, Size::new(area.width, area.height)) {
        Cover::Ready(proto) => {
            draw_image(f, proto, area, false);
            area.height
        }
        Cover::Loading => area.height,
        Cover::None => 0,
    }
}

/// How long the details pane shows each picture before the next.
const ART_EVERY: Duration = Duration::from_secs(5);

/// A screen beside the cover needs at least this many columns to be worth showing.
const SIDE_MIN_WIDTH: u16 = 20;

/// The details pane's pictures. With room beside the cover, a title screen or screenshot goes
/// there, taking turns when there are both. Without, they take turns with the cover itself,
/// starting with the cover whenever the selection changes. Returns the rows used, like draw_cover.
fn draw_art(f: &mut Frame, app: &mut App, game: usize, area: Rect) -> u16 {
    let Some(covers) = app.covers.as_mut().filter(|_| app.cfg.art.show) else { return 0 };
    let screens: Vec<Kind> = Kind::wanted(&app.cfg)
        .iter()
        .copied()
        .filter(|&k| k != Kind::Cover && covers.path_kind(&app.lib, game, k).is_some())
        .collect();
    if screens.is_empty() {
        return draw_cover(f, app, game, area);
    }
    let since = match app.art_since {
        Some((g, at)) if g == game => at,
        _ => {
            app.art_since = Some((game, Instant::now()));
            Instant::now()
        }
    };
    let turn = (since.elapsed().as_secs() / ART_EVERY.as_secs()) as usize;
    let has_cover = covers.path(&app.lib, game).is_some();

    // Room for a cover of about 3 by 4, and whatever's left beside it for a screen.
    let (font_w, font_h) = covers.font_size();
    let cover_w = (u32::from(area.height) * u32::from(font_h) * 3 / (4 * u32::from(font_w))) as u16;
    let side_w = area.width.saturating_sub(cover_w + 1);
    // Beside the cover, the screens take turns there; otherwise everything takes turns in `area`.
    let beside = has_cover && side_w >= SIDE_MIN_WIDTH;
    let (screen_area, turns) = if beside {
        (Rect { x: area.x + cover_w + 1, width: side_w, ..area }, screens)
    } else {
        (area, if has_cover { [vec![Kind::Cover], screens].concat() } else { screens })
    };
    let kind = turns[turn % turns.len()];
    let screen_size = Size::new(screen_area.width, screen_area.height);
    // Ask for the next screen now, so it's ready when its turn comes.
    let next = turns[(turn + 1) % turns.len()];
    if next != kind && next != Kind::Cover {
        let _ = covers.get_kind(&app.lib, game, next, screen_size);
    }
    if kind != Kind::Cover
        && let Cover::Ready(proto) = covers.get_kind(&app.lib, game, kind, screen_size)
    {
        draw_image(f, proto, screen_area, false);
    }
    if beside {
        draw_cover(f, app, game, Rect { width: cover_w, ..area });
    } else if kind == Kind::Cover {
        draw_cover(f, app, game, area);
    }
    area.height
}

/// Draws a rendered cover centered across `area`, and centered or resting on its bottom edge.
fn draw_image(f: &mut Frame, proto: &ratatui_image::protocol::Protocol, area: Rect, bottom: bool) {
    let s = proto.size();
    let (w, h) = (s.width.min(area.width), s.height.min(area.height));
    let y = if bottom { area.y + area.height - h } else { area.y + (area.height - h) / 2 };
    f.render_widget(Image::new(proto), Rect::new(area.x + (area.width - w) / 2, y, w, h));
}

/// Covers in the grid are about this wide, in cells; the columns share out what's left over.
const GRID_COVER_WIDTH: u16 = 16;

/// The games as a grid of covers, each with its name underneath. Games without art get an
/// empty box with their system's name instead. The selected one is framed, and its details sit under the grid.
pub(super) fn draw_grid(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let focused = app.focus == Pane::Games || app.search.is_some();
    let ids = app.list().to_vec();
    let position = if ids.is_empty() { String::new() } else { format!(" {}/{} ", app.game + 1, ids.len()) };
    let mut block = theme
        .block(games_title(app), focused)
        .title_bottom(Line::from(Span::styled(position, Style::new().dim())).right_aligned());
    if let Some(g) = app.selected_game() {
        block = block.title_bottom(grid_caption(app, g, theme));
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    if ids.is_empty() {
        let msg = if app.search.is_some() { "No matches" } else { "Nothing here yet" };
        f.render_widget(Paragraph::new(msg).dim(), inner);
        app.grid_cols = 0;
        return;
    }

    // Each cell: a frame line, the cover, the name, a frame line; and a frame column either side.
    let cols = (inner.width / (GRID_COVER_WIDTH + 2)).max(1);
    let cell_w = inner.width / cols;
    let cover_w = cell_w.saturating_sub(2).max(1);
    // Box art is about 3 wide by 4 tall; cells are about twice as tall as wide.
    let (font_w, font_h) = app.covers.as_ref().map_or((1, 2), |c| c.font_size());
    let cover_h = (u32::from(cover_w) * u32::from(font_w) * 4 / (3 * u32::from(font_h))).clamp(2, 40) as u16;
    let cell_h = cover_h + 3;
    let rows = (inner.height / cell_h).max(1) as usize;
    let cols = cols as usize;
    app.grid_cols = cols;
    app.page = rows * cols;

    let row = app.game / cols;
    if row < app.grid_top {
        app.grid_top = row;
    } else if row >= app.grid_top + rows {
        app.grid_top = row + 1 - rows;
    }
    app.grid_top = app.grid_top.min(ids.len().div_ceil(cols).saturating_sub(rows));

    // Spread whatever width is left over between the columns.
    let spare = inner.width - cell_w * cols as u16;
    let first = app.grid_top * cols;
    for (i, &g) in ids.iter().enumerate().skip(first).take(rows * cols) {
        let (r, c) = ((i - first) / cols, (i - first) % cols);
        let x = inner.x + c as u16 * cell_w + spare / 2;
        let cell = Rect::new(x, inner.y + r as u16 * cell_h, cell_w, cell_h);
        let selected = i == app.game;
        if selected {
            let frame = Block::bordered().border_type(BorderType::Rounded).border_style(if focused {
                Style::new().fg(theme.accent)
            } else {
                Style::new().dim()
            });
            f.render_widget(frame, cell);
        }
        let art = Rect::new(cell.x + 1, cell.y + 1, cover_w, cover_h);
        let game = &app.lib.games[g];
        let fav = app.state.is_favorite(&game.key);
        let name = format!("{}{}", if fav { "★ " } else { "" }, game.name);
        let mut label = Style::new();
        if selected {
            label = label.fg(theme.accent).bold();
        }
        if game.hidden {
            label = label.dim().italic();
        }
        let label_area = Rect::new(art.x, art.y + cover_h, cover_w, 1);
        f.render_widget(Paragraph::new(text::truncate(&name, cover_w as usize)).style(label).centered(), label_area);

        let cover =
            app.covers.as_mut().filter(|_| app.cfg.art.show).map(|c| c.get(&app.lib, g, Size::new(cover_w, cover_h)));
        match cover {
            Some(Cover::Ready(proto)) => draw_image(f, proto, art, true),
            Some(Cover::Loading) => {}
            // No art: a box the size of a cover, saying what system the game is for.
            _ => {
                let boxed = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().dim());
                let text = Paragraph::new(game.system.clone())
                    .centered()
                    .wrap(Wrap { trim: true })
                    .block(boxed.padding(Padding::vertical(cover_h.saturating_sub(4) / 2)))
                    .dim();
                f.render_widget(text, art);
            }
        }
    }
}

/// A line about the selected game for under the grid, which has no room for its details.
fn grid_caption(app: &App, g: usize, theme: &Theme) -> Line<'static> {
    let game = &app.lib.games[g];
    let mut parts = vec![game.system.clone()];
    parts.extend(game.meta.year.map(|y| y.to_string()));
    parts.extend(game.meta.genres.first().cloned());
    Line::from(vec![
        Span::styled(format!(" {}", game.name), Style::new().fg(theme.accent).bold()),
        Span::styled(format!(" · {} ", parts.join(" · ")), Style::new().dim()),
    ])
}

pub(super) fn draw_details(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let block = theme.block(" Details ", false).padding(Padding::horizontal(1));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(g) = app.selected_game() else { return };

    // Box art is roughly square; terminal cells are about twice as tall as wide.
    let art_h = (inner.height * 11 / 20).min(inner.width * 3 / 5);
    let used = if art_h >= 4 { draw_art(f, app, g, Rect { height: art_h, ..inner }) } else { 0 };
    let text_area = if used > 0 {
        Rect { y: inner.y + used + 1, height: inner.height.saturating_sub(used + 1), ..inner }
    } else {
        inner
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
    let genre = (!m.genres.is_empty()).then(|| m.genres.join(", "));
    // Don't repeat the publisher when it's the same company.
    let publisher = m.publisher.clone().filter(|p| Some(p) != m.developer.as_ref());
    let players = m.players.map(|n| if n > 1 { format!("1–{n}") } else { "1".into() });
    for (k, v) in [
        ("Genre", genre),
        ("Developer", m.developer.clone()),
        ("Publisher", publisher),
        ("Released", m.year.map(|y| y.to_string())),
        ("Players", players),
        ("Series", m.franchise.clone()),
    ] {
        if let Some(v) = v {
            lines.push(Line::from(vec![label(k), Span::raw(v)]));
        }
    }
    if let Some(&secs) = app.state.playtime.get(&game.key).filter(|&&s| s > 0) {
        lines.push(Line::from(vec![label("Played"), Span::raw(text::duration(secs))]));
    }
    if let Some(&at) = app.state.last_played.get(&game.key) {
        lines.push(Line::from(vec![label("Last"), Span::raw(text::ago(now().saturating_sub(at)))]));
    }
    if game.hidden {
        lines.push(Line::from(vec![label("Hidden"), Span::raw("yes (o to unhide)")]));
    }
    if let Some(a) = app.achievements.games.get(&game.key) {
        let style = if a.supported { Style::new() } else { Style::new().dim() };
        lines.push(Line::from(vec![label("Cheevos"), Span::styled(a.summary(), style)]));
    }
    if !game.discs.is_empty() {
        lines.push(Line::from(vec![label("Discs"), Span::raw(game.discs.len().to_string())]));
    } else if let Some(file) = game.path.as_ref().and_then(|p| p.file_name()) {
        lines.push(Line::from(vec![label("File"), Span::raw(file.to_string_lossy().into_owned())]));
    }
    let runs = game.emulator.clone().unwrap_or_else(|| runs_label(&game.launch.command));
    lines.push(Line::from(vec![label("Runs"), Span::raw(runs)]));
    if let Some(desc) = &m.description {
        lines.push(Line::raw(""));
        lines.extend(desc.lines().map(|l| Line::raw(l.to_string())));
    }
    f.render_widget(Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }), text_area);
}

pub(super) fn draw_playing(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let Some(run) = &app.running else { return };
    let g = run.game;
    let game = &app.lib.games[g];
    let lines = vec![
        Line::from(Span::styled("▶ Now playing", Style::new().fg(theme.accent).bold())),
        Line::raw(""),
        Line::from(Span::styled(game.name.clone(), Style::new().bold())),
        Line::from(Span::styled(game.system.clone(), Style::new().dim())),
        Line::raw(""),
        Line::raw(text::clock(run.started.elapsed().as_secs())),
        Line::raw(""),
        Line::from(Span::styled("romi will be back when the game closes", Style::new().dim())),
    ];
    let has_art = app.cfg.art.show && app.covers.as_mut().is_some_and(|c| c.path(&app.lib, g).is_some());
    let text_h = lines.len() as u16;
    let art_h = if has_art { (area.height.saturating_sub(text_h + 4) * 3 / 5).min(16) } else { 0 };
    let h = text_h + 2 + if art_h >= 4 { art_h + 1 } else { 0 };
    let w = (game.name.chars().count() as u16 + 8).clamp(44, area.width);
    let [_, mid, _] = Layout::vertical([Constraint::Fill(1), Constraint::Length(h), Constraint::Fill(1)]).areas(area);
    let [_, rect, _] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(w), Constraint::Fill(1)]).areas(mid);
    let block = theme.block("", true);
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);
    let used = if art_h >= 4 { draw_cover(f, app, g, Rect { height: art_h, ..inner }) } else { 0 };
    let text_area = Rect { y: inner.y + used + u16::from(used > 0), height: text_h, ..inner };
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), text_area);
}

/// A game played inside romi: its picture, as big as fits, and how to stop it.
pub(super) fn draw_embedded(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let Some(embedded) = &mut app.embedded else { return };
    let block = theme.block(app.lib.games[embedded.game].name.clone(), true);
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    let [screen, hint] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    if let Some((cells, image)) = embedded.place(screen) {
        crate::emu::kitty::placeholders(f.buffer_mut(), cells, image);
        if let Some(layout) = embedded.place_tracker(cells) {
            draw_toast(f, theme, &layout);
        }
        if let Some(layout) = embedded.place_challenges(cells) {
            draw_toast(f, theme, &layout);
        }
        if let Some(layout) = embedded.place_toast(cells) {
            draw_toast(f, theme, &layout);
        }
    }
    let stop = "Esc, or hold Select+Start, to stop";
    // What the game says the player is doing goes before it, when both fit.
    let presence = embedded.presence();
    let both = format!("{presence} · {stop}");
    let text =
        if !presence.is_empty() && both.chars().count() <= hint.width as usize { both } else { stop.to_string() };
    f.render_widget(Paragraph::new(Span::styled(text, Style::new().dim())).alignment(Alignment::Center), hint);
}

pub(super) fn draw_empty(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    if app.filter.is_active() {
        let lines = vec![
            Line::from(Span::styled("Nothing matches the filter", Style::new().fg(theme.accent).bold())),
            Line::raw(""),
            Line::raw("Open the filter (Start → Filter, or F) and loosen it."),
        ];
        f.render_widget(Paragraph::new(lines).alignment(Alignment::Center).block(theme.block("", true)), area);
        return;
    }
    let lines = vec![
        Line::from(Span::styled("No games found", Style::new().fg(theme.accent).bold())),
        Line::raw(""),
        Line::raw(format!("ROMs are read from {}", app.cfg.roms_dir)),
        Line::raw(format!("Edit {} and press ctrl-r", crate::config::tilde(&app.cfg_path))),
        Line::raw("or run `romi check` to see what's going on."),
    ];
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center).block(theme.block("", true)), area);
}
