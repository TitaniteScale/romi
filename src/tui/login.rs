//! Logging in to RetroAchievements from the options panel: with RetroArch's login if it has one, or
//! a username and password, on the on-screen keyboard if there's no other. Only the token the server
//! gives is kept.

use crate::emu::cheevos::Login;
use std::sync::mpsc::Receiver;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    /// RetroArch is logged in: use that, or another account.
    Found,
    Username,
    Password,
    Waiting,
    /// Already logged in: stay, or log out.
    LogOut,
}

pub struct LoginScreen {
    pub step: Step,
    pub username: String,
    pub password: String,
    /// Why the last try didn't work.
    pub problem: Option<String>,
    /// The option picked, where there are two.
    pub choice: usize,
    /// Whether the keyboard's letters are capitals.
    pub shift: bool,
    /// RetroArch's login, if it has one.
    pub retroarch: Option<Login>,
    /// The server's answer, while waiting for it.
    pub answer: Option<Receiver<Result<Login, String>>>,
}

impl LoginScreen {
    /// Whether keys go to the username or password.
    pub fn typing(&self) -> bool {
        matches!(self.step, Step::Username | Step::Password)
    }

    /// Types a character into the field being filled in, or deletes one with `None`.
    pub fn edit(&mut self, c: Option<char>) {
        let field = if self.step == Step::Username { &mut self.username } else { &mut self.password };
        match c {
            Some(c) => field.push(c),
            None => {
                field.pop();
            }
        }
        self.problem = None;
    }
}
