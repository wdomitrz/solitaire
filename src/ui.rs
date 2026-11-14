//! Klondike, as the shared page sees it.
//!
//! The board is [`crate::board_ui`] and the shell around it is [`crate::shell`];
//! both are carried byte for byte by Spider Solitaire. What is left here is the
//! only thing that is Klondike's own: how a [`Scene`] is built from this game,
//! and what each [`Intent`] means in its rules.
//!
//! No decision lives here. Every rule — what a move is worth, what may be
//! picked up, what the page says — is in [`crate::game`], which is where the
//! tests are.

use wasm_bindgen::prelude::*;

use crate::board::{Place, Scene};
use crate::board_ui::Intent;
use crate::card::deck;
use crate::game::{
    Difficulty, Game, Hint, Options, Pile, Report, Scoring, OPTIONS_KEY, STORAGE_KEY,
};
use crate::shell::{fresh_seed, Repeat, Rules};

/// Start the app.
///
/// # Errors
///
/// Fails only when there is no window, no document, or no board in the shell.
#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let board = document
        .get_element_by_id("board")
        .ok_or_else(|| JsValue::from_str("no #board"))?;
    crate::shell::Shell::start(&window, &document, board, Klondike::default())
}

/// The game, plus the only two things the page keeps beside it: the next deal's
/// options, and the hint being shown.
struct Klondike {
    game: Game,
    options: Options,
    hint: Option<Hint>,
    /// What the game last said, waiting to be shown.
    words: Option<String>,
}

impl Default for Klondike {
    fn default() -> Self {
        Self {
            game: Game::new(Options::default().difficulty, Options::default().scoring, 0),
            options: Options::default(),
            hint: None,
            words: None,
        }
    }
}

impl Klondike {
    /// The pile a board place means, if this game has one there.
    fn pile(place: Place) -> Option<Pile> {
        Pile::from_place(place)
    }

    /// Answer one gesture with the game's rules.
    fn answer(&mut self, intent: Intent) -> Report {
        match intent {
            Intent::Tap { place, index } => self.tap(place, index),
            Intent::DoubleTap { place, index } => self.double_tap(place, index),
            Intent::Drop { from, index, to } => self.drop(from, index, to),
            Intent::Back => self.game.clear_selection(),
            Intent::Cancelled => Report::quiet(""),
        }
    }

    fn tap(&mut self, place: Place, index: Option<usize>) -> Report {
        let Some(pile) = Self::pile(place) else {
            return Report::quiet("");
        };
        if pile == Pile::Stock {
            return self.game.draw();
        }
        match index {
            Some(index) => {
                // A face-down card is not a control, and with nothing in hand a
                // tap on one is a tap on the table: say nothing rather than
                // telling the player off for touching a card back. With a run
                // held, the tap is a request to drop it on this pile, which the
                // game answers for itself.
                if self.game.selection().is_none() && self.game.is_face_down(pile, index) {
                    return Report::quiet("");
                }
                self.game.tap(pile, index)
            }
            // An empty place: a press on it is a destination for whatever is in
            // hand, and nothing more.
            None if self.game.selection().is_some() => self.game.drop(pile),
            None => Report::quiet(""),
        }
    }

    fn double_tap(&mut self, place: Place, index: Option<usize>) -> Report {
        let (Some(pile), Some(index)) = (Self::pile(place), index) else {
            return Report::quiet("");
        };
        self.game.double_tap(pile, index)
    }

    /// A drag: take the run up in the rules, then put it down where the player
    /// did.
    ///
    /// The two steps are separate because they are separate questions — "may
    /// this run move" and "does this pile take it" — and the game answers each
    /// with the rule a tap uses, so a drag and a tap can never disagree about
    /// what is legal.
    fn drop(&mut self, from: Place, index: usize, to: Place) -> Report {
        let (Some(from), Some(to)) = (Self::pile(from), Self::pile(to)) else {
            return Report::quiet("");
        };
        self.game.select(from, index);
        self.game.drop(to)
    }

    /// One card of the automatic finish. The shell repeats this until it says
    /// the board is done, so no timer handle is ever held here.
    fn auto_step(&mut self) -> Repeat {
        let Some(report) = self.game.autocomplete_step() else {
            return Repeat::Once;
        };
        self.show(report);
        if self.game.won() {
            Repeat::Once
        } else {
            Repeat::Every(140)
        }
    }
}

impl Rules for Klondike {
    fn deck(&self) -> Vec<crate::card::Card> {
        deck()
    }

    fn scene(&self) -> Scene {
        crate::scene::scene(&self.game, self.hint.as_ref())
    }

    fn act(&mut self, intent: Intent) {
        let report = self.answer(intent);
        self.show(report);
    }

    fn lift(&self, place: Place, index: usize) -> usize {
        match Self::pile(place) {
            Some(pile) => self.game.run_len(pile, index).unwrap_or(0),
            None => 0,
        }
    }

    fn default_focus(&self) -> (Place, Option<usize>) {
        crate::scene::default_focus(&self.game)
    }

    fn hint(&mut self) -> Option<String> {
        if self.game.won() {
            return None;
        }
        let hint = self.game.hint();
        let words = hint.words().to_owned();
        self.hint = Some(hint);
        Some(words)
    }

    fn clear_hint(&mut self) {
        self.hint = None;
    }

    fn won(&self) -> bool {
        self.game.won()
    }

    fn result_words(&self) -> String {
        self.game.result_words()
    }

    fn can_undo(&self) -> bool {
        self.game.can_undo()
    }

    fn undo(&mut self) {
        let report = self.game.undo();
        self.show(report);
    }

    fn started(&self) -> bool {
        self.game.started()
    }

    fn elapsed(&self) -> u32 {
        self.game.elapsed()
    }

    fn tick(&mut self, seconds: u32) {
        self.game.tick(seconds);
    }

    fn readout(&self) -> (String, String, Option<String>) {
        let readout = self.game.readout();
        (readout.clock, readout.moves, readout.score)
    }

    fn variant(&self) -> String {
        self.game.variant()
    }

    fn summary(&self) -> String {
        self.game.summary()
    }

    fn action_label(&self) -> Option<&'static str> {
        Some("Finish")
    }

    fn action_enabled(&self) -> bool {
        self.game.can_autocomplete()
    }

    fn action(&mut self) -> Repeat {
        if !self.game.can_autocomplete() {
            return Repeat::Once;
        }
        self.auto_step()
    }

    fn take_words(&mut self) -> Option<String> {
        self.words.take()
    }

    fn option_values(&self) -> Vec<(&'static str, String)> {
        vec![
            ("difficulty", self.options.difficulty.slug().to_owned()),
            ("scoring", self.options.scoring.slug().to_owned()),
        ]
    }

    fn option_note(&self, id: &str) -> String {
        match id {
            "difficulty" => self.options.difficulty.sentence().to_owned(),
            "scoring" => self.options.scoring.sentence().to_owned(),
            _ => String::new(),
        }
    }

    fn set_option(&mut self, id: &str, value: &str) {
        match id {
            "difficulty" => {
                self.options.difficulty =
                    Difficulty::from_slug(value).unwrap_or(Options::default().difficulty);
            }
            "scoring" => {
                self.options.scoring = Scoring::from_slug(value).unwrap_or(Scoring::Standard);
            }
            _ => {}
        }
    }

    fn store_options(&self) -> String {
        self.options.to_stored()
    }

    fn load_options(&mut self, text: &str) {
        self.options = Options::from_stored(Some(text));
    }

    fn store_game(&self) -> Option<String> {
        self.game.to_stored()
    }

    fn load_game(&mut self, text: &str) {
        if let Some(game) = Game::from_stored(Some(text)) {
            self.game = game;
        } else {
            // An unreadable save is a fresh deal, not an error: a corrupt entry
            // must never stop the app from starting.
            self.game = Game::new(self.options.difficulty, self.options.scoring, fresh_seed());
        }
    }

    fn keys(&self) -> (&'static str, &'static str) {
        (STORAGE_KEY, OPTIONS_KEY)
    }

    fn new_game(&mut self) {
        self.game = Game::dealt(self.options.difficulty, self.options.scoring, fresh_seed());
    }

    fn restart(&mut self) {
        self.game = self.game.restart();
    }
}

impl Klondike {
    /// Say what the game says, then let the shell know the board moved.
    /// Keep whatever the game said, for the shell to put under the board.
    fn show(&mut self, report: Report) {
        if !report.words.is_empty() {
            self.words = Some(report.words);
        }
    }
}
