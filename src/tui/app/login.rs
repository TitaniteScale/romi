//! The RetroAchievements login screen, opened from the options panel.

use super::input::Op;
use super::{App, Level};
use crate::emu::cheevos::{self, Client, Login};
use crate::tui::login::{LoginScreen, Step};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use std::sync::mpsc;
use std::time::Duration;

/// How long the server gets to answer, as `romi achievements login` gives it.
const WAIT: Duration = Duration::from_secs(30);

impl App {
    /// Opens the screen for where things stand: logged in, RetroArch logged in, or neither.
    pub(super) fn open_login(&mut self) {
        let saved = cheevos::saved_login();
        let retroarch = crate::emucfg::retroarch_login();
        let step = match (&saved, &retroarch) {
            (Some(_), _) => Step::LogOut,
            (None, Some(_)) => Step::Found,
            (None, None) => Step::Username,
        };
        let username = saved
            .map(|l| l.username)
            .or_else(|| self.cfg.retroachievements.username.clone())
            .or_else(|| retroarch.as_ref().map(|l| l.username.clone()))
            .unwrap_or_default();
        self.osk = (step == Step::Username).then_some((0, 0));
        self.login = Some(LoginScreen {
            step,
            username,
            password: String::new(),
            problem: None,
            choice: 0,
            shift: false,
            retroarch,
            answer: None,
        });
    }

    pub(super) fn close_login(&mut self) {
        self.login = None;
        self.osk = None;
    }

    pub(super) fn login_op(&mut self, op: Op) {
        let Some(screen) = &mut self.login else { return };
        match screen.step {
            Step::Username | Step::Password => self.osk_op(op),
            Step::Waiting if matches!(op, Op::Back | Op::Menu) => self.close_login(),
            Step::Waiting => {}
            Step::Found | Step::LogOut => match op {
                Op::Up | Op::Down => screen.choice = 1 - screen.choice,
                Op::Back | Op::Menu => self.close_login(),
                Op::Confirm => self.login_choose(),
                _ => {}
            },
        }
    }

    pub(super) fn login_key(&mut self, k: KeyEvent, ctrl: bool) {
        let Some(screen) = &mut self.login else { return };
        if screen.typing() {
            match k.code {
                KeyCode::Enter => self.login_next(),
                KeyCode::Esc => self.login_back(),
                KeyCode::Backspace => screen.edit(None),
                KeyCode::Char(c) if !ctrl => screen.edit(Some(c)),
                _ => {}
            }
            return;
        }
        let op = match k.code {
            KeyCode::Up | KeyCode::Char('k') => Op::Up,
            KeyCode::Down | KeyCode::Char('j') => Op::Down,
            KeyCode::Enter => Op::Confirm,
            KeyCode::Esc | KeyCode::Char('q') => Op::Back,
            _ => return,
        };
        self.login_op(op);
    }

    fn login_choose(&mut self) {
        let Some(screen) = &mut self.login else { return };
        match (screen.step, screen.choice) {
            (Step::Found, 0) => {
                if let Some(login) = screen.retroarch.clone() {
                    self.finish_login(login);
                }
            }
            (Step::Found, _) => {
                screen.step = Step::Username;
                self.osk = Some((0, 0));
            }
            // Staying logged in comes first, so a stray press doesn't log anyone out.
            (_, 0) => self.close_login(),
            (_, _) => {
                match cheevos::forget_login() {
                    Ok(_) => {
                        self.account = None;
                        self.set_status(Level::Info, "Logged out of RetroAchievements");
                    }
                    Err(e) => self.set_status(Level::Error, format!("Couldn't log out: {e:#}")),
                }
                self.close_login();
            }
        }
    }

    /// Goes on from the field being filled in, if there's something in it.
    pub(super) fn login_next(&mut self) {
        let Some(screen) = &mut self.login else { return };
        match screen.step {
            Step::Username if !screen.username.trim().is_empty() => {
                screen.username = screen.username.trim().to_string();
                screen.step = Step::Password;
                self.osk = Some((0, 0));
            }
            Step::Password if !screen.password.is_empty() => {
                let (username, password) = (screen.username.clone(), std::mem::take(&mut screen.password));
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let answer = Client::new(0).and_then(|c| c.login_with_password(&username, &password, WAIT));
                    let _ = tx.send(answer.map_err(|e| format!("{e:#}")));
                });
                screen.step = Step::Waiting;
                screen.problem = None;
                screen.answer = Some(rx);
                self.osk = None;
            }
            _ => {}
        }
    }

    /// Goes back a step from the field being filled in.
    pub(super) fn login_back(&mut self) {
        let Some(screen) = &mut self.login else { return };
        screen.problem = None;
        match screen.step {
            Step::Password => {
                screen.password.clear();
                screen.step = Step::Username;
                self.osk = Some((0, 0));
            }
            _ if screen.retroarch.is_some() => {
                screen.step = Step::Found;
                self.osk = None;
            }
            _ => self.close_login(),
        }
    }

    /// Takes the server's answer, once there is one.
    pub(super) fn poll_login(&mut self) {
        let Some(screen) = &mut self.login else { return };
        let Some(answer) = &screen.answer else { return };
        let answer = match answer.try_recv() {
            Ok(answer) => answer,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("the login stopped without an answer".to_string()),
        };
        screen.answer = None;
        match answer {
            Ok(login) => self.finish_login(login),
            Err(e) => {
                screen.problem = Some(e);
                screen.step = Step::Password;
                self.osk = Some((0, 0));
            }
        }
    }

    fn finish_login(&mut self, login: Login) {
        if let Err(e) = cheevos::save_login(&login) {
            if let Some(screen) = &mut self.login {
                screen.problem = Some(format!("Couldn't save the login: {e:#}"));
            }
            return;
        }
        self.set_status(Level::Info, format!("Logged in to RetroAchievements as {}", login.username));
        self.account = Some(login.username);
        self.close_login();
        self.load_achievements();
    }
}
