//! Drawing. Only terminal palette colors and modifiers are used, so romi follows your theme.

use crate::art::Kind;
use crate::filter::ROWS as FILTER_ROWS;
use crate::sort::Sort;
use crate::state::now;
use crate::text;
use crate::tui::app::{App, Level, OSK, PadState, Pane, ViewKind};
use crate::tui::cover::Cover;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect, Size};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap};
use ratatui_image::Image;
use std::time::{Duration, Instant};

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
    if app.filter_panel.is_some() {
        draw_filter(f, app, &theme, body);
    }
    if app.options.is_some() {
        draw_options(f, app, &theme, body);
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

fn draw_filter(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
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
fn draw_options(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    use crate::tui::options::SETTINGS;
    let Some(sel) = app.options else { return };
    let lines: Vec<Line> = SETTINGS
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

fn draw_menu(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
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

/// The on-screen keyboard, docked at the bottom of the games pane so results stay visible above.
fn draw_osk(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let Some((row, col)) = app.osk else { return };
    let (title, prompt, query) = match (&app.tag_name, &app.cheevos) {
        (Some(name), _) => (" New tag ", " # ", name.as_str()),
        (None, Some(screen)) if screen.typing => (" Search ", " / ", screen.query.as_str()),
        _ => (" Search ", " / ", app.search.as_ref().map(|s| s.query.as_str()).unwrap_or_default()),
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(prompt, Style::new().fg(theme.accent).bold()),
            Span::raw(query.to_string()),
            Span::styled("█", Style::new().fg(theme.accent)),
        ]),
        Line::raw(""),
    ];
    for (r, keys) in OSK.iter().enumerate() {
        let spans = keys.iter().enumerate().map(|(c, k)| {
            let style = if (r, c) == (row, col) { theme.selected(true) } else { Style::new() };
            Span::styled(format!(" {k} "), style)
        });
        lines.push(Line::from(spans.collect::<Vec<_>>()).centered());
    }
    let h = lines.len() as u16 + 2;
    let w = (OSK[0].len() as u16 * 3 + 4).min(area.width);
    let rect = Rect::new(area.x + (area.width - w) / 2, (area.y + area.height).saturating_sub(h + 1).max(area.y), w, h);
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(theme.block(title, true)), rect);
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
                ViewKind::Tag(_) => ("# ", false),
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

fn draw_games(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
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
fn draw_grid(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
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

fn draw_details(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
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

fn draw_playing(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
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
fn draw_embedded(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let Some(embedded) = &mut app.embedded else { return };
    let block = theme.block(app.lib.games[embedded.game].name.clone(), true);
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    let [screen, hint] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    if let Some((cells, image)) = embedded.place(screen) {
        crate::emu::kitty::placeholders(f.buffer_mut(), cells, image);
        if let Some(layout) = embedded.place_toast(cells) {
            draw_toast(f, theme, &layout);
        }
    }
    let text = "Esc, or hold Select+Start, to stop";
    f.render_widget(Paragraph::new(Span::styled(text, Style::new().dim())).alignment(Alignment::Center), hint);
}

/// An achievement notice across the bottom of the game's picture, laid out by `notice`: its
/// badge, if it has one, in placeholder cells for the badge's image, and its lines beside it.
fn draw_toast(f: &mut Frame, theme: &Theme, layout: &crate::emu::notice::Layout) {
    use crate::emu::notice::Weight;
    f.render_widget(Clear, layout.area);
    f.render_widget(theme.block("", true), layout.area);
    if let Some(cells) = layout.badge {
        crate::emu::kitty::placeholders(f.buffer_mut(), cells, crate::emu::embedded::BADGE_IMAGE);
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

fn draw_empty(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
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

/// "retroarch · mgba" for libretro cores, otherwise just the program name.
pub fn runs_label(command: &str) -> String {
    let mut words = command.split_whitespace();
    let program = words.next().unwrap_or_default();
    let program = program.rsplit('/').next().unwrap_or(program);
    match words.find_map(|w| w.rsplit('/').next()?.strip_suffix("_libretro.so")) {
        Some(core) => format!("{program} · {core}"),
        None => program.to_string(),
    }
}

/// The stats screen: totals, play time by system and by game, and this year month by month.
fn draw_stats(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
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
fn draw_cheevos(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
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
                let mode = if c.earned_hardcore.is_some() { "hardcore, " } else { "" };
                notes.push(format!("earned {mode}{}", when.split(' ').next().unwrap_or(when)));
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
