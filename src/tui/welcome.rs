//! The title screen, and the few questions a first run asks after it. They only gather answers:
//! setting romi up is `romi init`'s job (see `cli::first_run`).
//!
//! A first run has no config and no launcher yet, so this draws for itself. After that the title
//! screen only comes back if it's turned on in the options (`"title_screen"`).

use crate::gamepad::{self, PadAction, PadEvent};
use anyhow::Result;
use image::DynamicImage;
use image::imageops::FilterType;
use ratatui::DefaultTerminal;
use ratatui::Frame;
use ratatui::crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind};
use ratatui::crossterm::event::{KeyEvent, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::layout::{Alignment, Constraint, Layout, Rect, Size};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui_image::Image;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::{FontSize, Resize};
use std::io::stdout;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

const LOGO: &[u8] = include_bytes!("../../Images/romi_logo.png");
/// The sky in the logo, for what's picked out around it.
const ACCENT: Color = Color::Rgb(120, 192, 248);
/// As wide as the logo is drawn, in cells, however wide the terminal.
const LOGO_WIDTH: u16 = 72;

/// What's asked of the person at the screen.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    Title,
    Roms,
    /// Only with RetroArch here: without it, there's only romi's own folder.
    Folders,
    Found,
}

/// A key or a controller's button, as the screens see them.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Input {
    Confirm,
    Back,
    Quit,
    Up,
    Down,
    Char(char),
    Backspace,
    Paste,
    /// Any other key or button, which only the title screen cares about.
    Other,
}

/// The logo at the size it was last drawn, as it's only made again when that changes.
struct Logo {
    image: Option<DynamicImage>,
    picker: Option<Picker>,
    drawn: Option<(Size, Protocol)>,
}

impl Logo {
    fn new(picker: Option<Picker>) -> Logo {
        Logo { image: image::load_from_memory(LOGO).ok(), picker, drawn: None }
    }

    /// How many cells tall the logo is at `width` cells, by the shape of the terminal's cells.
    fn height(&self, width: u16) -> u16 {
        let (cell_w, cell_h) = self.picker.as_ref().map_or((10, 20), |p| {
            let font: FontSize = p.font_size();
            (font.width, font.height)
        });
        let Some(image) = &self.image else { return 1 };
        let tall = u32::from(width) * u32::from(cell_w) * image.height() / image.width().max(1);
        (tall / u32::from(cell_h).max(1)).max(1) as u16
    }

    /// Draws it in the middle of `area`, or its name if it can't.
    fn draw(&mut self, f: &mut Frame, area: Rect) {
        let size = Size::new(area.width, area.height);
        if self.drawn.as_ref().is_none_or(|(at, _)| *at != size) {
            // Nearest neighbour: it's pixel art.
            let resize = Resize::Scale(Some(FilterType::Nearest));
            self.drawn = self.picker.as_ref().zip(self.image.clone()).and_then(|(picker, image)| {
                picker.new_protocol(image, size, resize).ok().map(|protocol| (size, protocol))
            });
        }
        match &self.drawn {
            Some((_, protocol)) => {
                let shown = protocol.size();
                let x = area.x + area.width.saturating_sub(shown.width) / 2;
                let y = area.y + area.height.saturating_sub(shown.height) / 2;
                f.render_widget(Image::new(protocol), Rect::new(x, y, shown.width, shown.height).intersection(area));
            }
            None => {
                let name = Line::from(Span::styled("romi", Style::new().fg(ACCENT).bold()));
                let y = area.y + area.height / 2;
                f.render_widget(Paragraph::new(name).alignment(Alignment::Center), Rect::new(area.x, y, area.width, 1));
            }
        }
    }
}

fn draw_title(f: &mut Frame, logo: &mut Logo, hint: &str) {
    let area = f.area();
    f.render_widget(Clear, area);
    let width = LOGO_WIDTH.min(area.width.saturating_sub(4)).max(1);
    let tall = logo.height(width).min(area.height.saturating_sub(4)).max(1);
    let [_, picture, _, tagline, _, press, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(tall),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    let [_, picture, _] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(width), Constraint::Fill(1)]).areas(picture);
    logo.draw(f, picture);
    let centered = |text: Span<'static>| Paragraph::new(Line::from(text)).alignment(Alignment::Center);
    f.render_widget(centered(Span::raw("Play your games in your terminal")), tagline);
    f.render_widget(centered(Span::styled(hint.to_string(), Style::new().dim())), press);
}

/// A column of text in the middle of the screen, as wide as reads comfortably, with a heading and,
/// at the bottom, what the keys do.
fn draw_page(f: &mut Frame, heading: &str, lines: Vec<Line<'_>>, scroll: u16, keys: &str) {
    let area = f.area();
    f.render_widget(Clear, area);
    let [_, column, _] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Max(92), Constraint::Fill(1)]).areas(area);
    let [_, title, _, body, _, foot, _] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(column.inner(ratatui::layout::Margin::new(2, 0)));
    f.render_widget(Paragraph::new(Span::styled(heading.to_string(), Style::new().fg(ACCENT).bold())), title);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((scroll, 0)), body);
    f.render_widget(Paragraph::new(Span::styled(keys.to_string(), Style::new().dim())), foot);
}

fn from_key(key: KeyEvent) -> Option<Input> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Some(Input::Quit);
    }
    Some(match key.code {
        KeyCode::Enter => Input::Confirm,
        KeyCode::Esc => Input::Back,
        KeyCode::Up => Input::Up,
        KeyCode::Down => Input::Down,
        KeyCode::Backspace => Input::Backspace,
        KeyCode::Char(c) => Input::Char(c),
        _ => Input::Other,
    })
}

fn from_pad(event: &PadEvent) -> Option<Input> {
    match event.action() {
        Some(PadAction::Confirm) => Some(Input::Confirm),
        Some(PadAction::Back) => Some(Input::Back),
        Some(PadAction::Up) => Some(Input::Up),
        Some(PadAction::Down) => Some(Input::Down),
        _ if matches!(event, PadEvent::Pressed { .. }) => Some(Input::Other),
        _ => None,
    }
}

/// The next thing pressed, or None if it's been a moment, so a resize gets drawn. Pasting comes back
/// in `pasted`, which is how a terminal hands over a folder dropped on it.
fn next_input(pads: &Receiver<PadEvent>, pasted: &mut String) -> Result<Option<Input>> {
    while let Ok(event) = pads.try_recv() {
        if let Some(input) = from_pad(&event) {
            return Ok(Some(input));
        }
    }
    if !event::poll(Duration::from_millis(30))? {
        return Ok(None);
    }
    Ok(match event::read()? {
        Event::Key(key) => from_key(key),
        Event::Paste(text) => {
            pasted.push_str(&text);
            Some(Input::Paste)
        }
        _ => None,
    })
}

/// The title screen by itself, as the option has it: there until something's pressed.
pub fn title_screen(terminal: &mut DefaultTerminal, picker: Option<Picker>, pads: &Receiver<PadEvent>) -> Result<()> {
    let mut logo = Logo::new(picker);
    let mut pasted = String::new();
    loop {
        terminal.draw(|f| draw_title(f, &mut logo, "Press any key"))?;
        if next_input(pads, &mut pasted)?.is_some() {
            terminal.clear()?;
            return Ok(());
        }
    }
}

/// What a first run was told.
#[derive(Debug, PartialEq)]
pub struct Answers {
    /// The ROM folder, or None to start without one.
    pub roms: Option<PathBuf>,
    /// BIOS files and saves go in romi's own folder, not RetroArch's.
    pub own_folders: bool,
}

/// The question of where BIOS files and saves go, with `own` as the one picked.
fn folders_lines(own: bool) -> Vec<Line<'static>> {
    let option = |picked: bool, name: &str| {
        let style = if picked { Style::new().fg(ACCENT).bold() } else { Style::new() };
        Line::from(Span::styled(format!("{} {name}", if picked { "  ›" } else { "   " }), style))
    };
    let about = |text: String| Line::from(Span::styled(format!("     {text}"), Style::new().dim()));
    let retroarch = crate::config::tilde(&crate::emucfg::retroarch_system_dir());
    let romi = crate::config::tilde(&crate::config::data_dir());
    vec![
        Line::raw("RetroArch is here too, so romi can share its folders or keep its own."),
        Line::raw(""),
        option(!own, "RetroArch's folders"),
        about(format!("BIOS files in {retroarch}, and in-game saves carry on in either one.")),
        Line::raw(""),
        option(own, "romi's own folder"),
        about(format!("BIOS files and saves in {romi}, apart from RetroArch.")),
        about("Copy over any BIOS files you already have.".to_string()),
    ]
}

/// What the ROM folder question has so far.
struct RomsQuestion {
    text: String,
    /// What it started with, which may not exist, and which it's fine to leave as it is.
    offered: String,
    found: bool,
    problem: Option<String>,
}

impl RomsQuestion {
    fn new(found: Option<&Path>) -> RomsQuestion {
        let offered = found.map_or("~/ROMs".to_string(), crate::config::tilde);
        RomsQuestion { text: offered.clone(), offered, found: found.is_some(), problem: None }
    }

    /// The folder to go on with, once it makes sense: Some(None) to start without one.
    fn answer(&mut self) -> Option<Option<PathBuf>> {
        let text = self.text.trim();
        let left_alone = text.is_empty() || (text == self.offered && !self.found);
        if left_alone {
            return Some(None);
        }
        let dir = crate::config::expand_path(text);
        if dir.is_dir() {
            return Some(Some(dir));
        }
        self.problem = Some(format!("{} isn't a folder", crate::config::tilde(&dir)));
        None
    }

    fn lines(&self, usual: &str) -> Vec<Line<'_>> {
        let mut lines = match self.found {
            true => vec![Line::raw("It looks like your games are here. Change it if they aren't.")],
            false => vec![
                Line::raw(format!("romi looked in {usual} and found none.")),
                Line::raw("Type the folder, or start without one: Steam games and anything you add"),
                Line::raw("with `romi add` still show, and `romi init` looks again later."),
            ],
        };
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled("  › ", Style::new().fg(ACCENT)),
            Span::raw(self.text.as_str()),
            Span::styled("▏", Style::new().fg(ACCENT)),
        ]));
        lines.push(Line::raw(""));
        lines.push(Line::raw("Each system goes in a folder of its own: ~/ROMs/gba, ~/ROMs/snes, and so on.").dim());
        if let Some(problem) = &self.problem {
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(problem.as_str(), Style::new().fg(Color::Red))));
        }
        lines
    }
}

/// Runs a first run's screens, with `found` as the ROM folder if one was found, and asking about
/// RetroArch's folders if `retroarch` is here. `preview` says what setting up with a folder and those
/// folders would find. None if it's turned down, so nothing's written.
pub fn first_run(
    found: Option<&Path>,
    usual: &str,
    retroarch: bool,
    preview: impl Fn(Option<&Path>, bool) -> String,
) -> Result<Option<Answers>> {
    let mut terminal = ratatui::init();
    let answers = ask(&mut terminal, found, usual, retroarch, preview);
    let _ = execute!(stdout(), DisableBracketedPaste);
    ratatui::restore();
    answers
}

fn ask(
    terminal: &mut DefaultTerminal,
    found: Option<&Path>,
    usual: &str,
    retroarch: bool,
    preview: impl Fn(Option<&Path>, bool) -> String,
) -> Result<Option<Answers>> {
    let (tx, pads) = mpsc::channel();
    gamepad::spawn(tx, Arc::new(AtomicBool::new(false)));
    let picker = match crate::terminal_answers() {
        true => Picker::from_query_stdio().ok(),
        false => Some(Picker::halfblocks()),
    };
    execute!(stdout(), EnableBracketedPaste)?;
    terminal.clear()?;
    let mut logo = Logo::new(picker);
    let mut step = Step::Title;
    let mut question = RomsQuestion::new(found);
    let mut roms: Option<PathBuf> = None;
    // RetroArch's, if it's here, so nothing changes for anyone who only presses on.
    let mut own_folders = !retroarch;
    let mut findings = String::new();
    let mut scroll = 0u16;
    let mut pasted = String::new();
    loop {
        terminal.draw(|f| match step {
            Step::Title => draw_title(f, &mut logo, "Press any key to get started, or Esc to leave"),
            Step::Roms => {
                draw_page(f, "Where are your ROMs?", question.lines(usual), 0, "Enter goes on  ·  Esc goes back");
            }
            Step::Folders => draw_page(
                f,
                "Where should romi keep BIOS files and saves?",
                folders_lines(own_folders),
                0,
                "↑↓ picks  ·  Enter goes on  ·  Esc goes back",
            ),
            Step::Found => {
                let lines = findings.lines().map(|l| Line::raw(l.to_string())).collect();
                let keys = "Enter sets this up  ·  ↑↓ scroll  ·  Esc goes back";
                draw_page(f, "Here's what romi found", lines, scroll, keys);
            }
        })?;
        let Some(input) = next_input(&pads, &mut pasted)? else { continue };
        if input == Input::Quit {
            return Ok(None);
        }
        let go_on = |roms: &Option<PathBuf>, own_folders: bool, findings: &mut String, scroll: &mut u16| {
            *findings = preview(roms.as_deref(), own_folders);
            *scroll = 0;
            Step::Found
        };
        match (step, input) {
            (Step::Title, Input::Back) => return Ok(None),
            (Step::Title, _) => {
                step = Step::Roms;
                terminal.clear()?;
            }
            (Step::Roms, Input::Back) => {
                step = Step::Title;
                terminal.clear()?;
            }
            (Step::Roms, Input::Confirm) => {
                if let Some(answer) = question.answer() {
                    roms = answer;
                    step = match retroarch {
                        true => Step::Folders,
                        false => go_on(&roms, own_folders, &mut findings, &mut scroll),
                    };
                }
            }
            (Step::Roms, Input::Char(c)) => {
                question.text.push(c);
                question.problem = None;
            }
            (Step::Roms, Input::Backspace) => {
                question.text.pop();
                question.problem = None;
            }
            (Step::Roms, Input::Paste) => {
                // A folder dropped on the terminal replaces what's there, without any quoting round it.
                question.text = std::mem::take(&mut pasted).trim().trim_matches(['\'', '"']).to_string();
                question.problem = None;
            }
            (Step::Folders, Input::Up | Input::Down) => own_folders = !own_folders,
            (Step::Folders, Input::Back) => step = Step::Roms,
            (Step::Folders, Input::Confirm) => step = go_on(&roms, own_folders, &mut findings, &mut scroll),
            (Step::Found, Input::Back) => step = if retroarch { Step::Folders } else { Step::Roms },
            (Step::Found, Input::Confirm) => return Ok(Some(Answers { roms, own_folders })),
            (Step::Found, Input::Up) => scroll = scroll.saturating_sub(1),
            (Step::Found, Input::Down) => scroll = scroll.saturating_add(1),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logo_is_built_in_and_keeps_its_shape() {
        let logo = Logo::new(None);
        let image = logo.image.as_ref().expect("the logo decodes");
        assert_eq!((image.width(), image.height()), (996, 378));
        // At the size it's guessed cells are, half as wide as tall.
        assert_eq!(logo.height(72), 13);
    }

    #[test]
    fn a_folder_that_isnt_there_is_turned_down_unless_it_was_only_offered() {
        let mut offered = RomsQuestion::new(None);
        assert_eq!(offered.answer(), Some(None), "left as it was, there's no folder yet");
        offered.text = "/nowhere/at/all".into();
        assert_eq!(offered.answer(), None);
        assert_eq!(offered.problem.as_deref(), Some("/nowhere/at/all isn't a folder"));
        offered.text.clear();
        assert_eq!(offered.answer(), Some(None), "cleared, it starts without one");

        let dir = std::env::temp_dir();
        let mut found = RomsQuestion::new(Some(&dir));
        assert_eq!(found.answer(), Some(Some(dir.clone())));
        found.text = dir.to_string_lossy().into_owned();
        assert_eq!(found.answer(), Some(Some(dir)));
    }

    #[test]
    fn keys_and_buttons_say_the_same_things() {
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(from_key(key(KeyCode::Enter)), Some(Input::Confirm));
        assert_eq!(from_key(key(KeyCode::Esc)), Some(Input::Back));
        assert_eq!(from_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)), Some(Input::Quit));
        let released = KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Release);
        assert_eq!(from_key(released), None);
        assert_eq!(from_pad(&PadEvent::Action(PadAction::Down)), Some(Input::Down));
        assert_eq!(from_pad(&PadEvent::Disconnected), None);
        let pressed = PadEvent::Pressed { pad: "pad".into(), button: Some("start"), action: Some(PadAction::Menu) };
        assert_eq!(from_pad(&pressed), Some(Input::Other));
    }
}
