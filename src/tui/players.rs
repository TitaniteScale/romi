//! The players screen: press a button on each controller in turn, and that's the order they play
//! in, player 1's first. Saved in `state.json` (`State::players`), so it holds for every game until
//! it's changed again. Two controllers of the same kind are told apart by the order romi finds them.

#[derive(Default)]
pub struct Screen {
    /// The controllers pressed so far, in order.
    pub pressed: Vec<String>,
}

impl Screen {
    /// The connected controllers in player order: the ones pressed so far, then the rest in the
    /// order they had.
    pub fn order(&self, connected: &[String], saved: &[String]) -> Vec<String> {
        let mut rest: Vec<&String> = connected.iter().filter(|p| !self.pressed.contains(p)).collect();
        rest.sort_by_key(|p| saved.iter().position(|s| s == *p).unwrap_or(usize::MAX));
        self.pressed.iter().chain(rest).cloned().collect()
    }

    /// The order to save: `order`'s, then the controllers saved before that aren't connected now,
    /// so they keep their places among themselves when they're back.
    pub fn to_save(&self, connected: &[String], saved: &[String]) -> Vec<String> {
        let mut order = self.order(connected, saved);
        order.extend(saved.iter().filter(|s| !order.contains(s)).cloned().collect::<Vec<_>>());
        order
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pressed_go_first_and_the_rest_keep_their_order() {
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let connected = names(&["Xbox", "8BitDo", "Xbox #2"]);
        let saved = names(&["Old pad", "Xbox #2", "Xbox"]);
        let screen = Screen { pressed: names(&["8BitDo"]) };
        assert_eq!(screen.order(&connected, &saved), names(&["8BitDo", "Xbox #2", "Xbox"]));
        assert_eq!(screen.to_save(&connected, &saved), names(&["8BitDo", "Xbox #2", "Xbox", "Old pad"]));
    }
}
