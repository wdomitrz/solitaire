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
use crate::board_ui::{BoardUi, Intent};
use crate::card::deck;
use crate::game::{
    stuck_words, Difficulty, Game, Hint, Options, Pile, Scoring, Words, OPTIONS_KEY, STORAGE_KEY,
};
use crate::shell::{fresh_seed, Inspect, Rules};

/// How long the automatic finish waits between cards, so it can be watched.
const FINISH_MS: i32 = 140;

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
    /// What the page has to say about the last action, waiting to be shown.
    ///
    /// `Some("")` is not the same as `None`: it is the instruction to clear the
    /// line, which is how the sentence from an old tap stops standing under a
    /// board it is no longer about.
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
    fn answer(&mut self, intent: Intent) -> Words {
        match intent {
            Intent::Tap { place, index } => self.tap(place, index),
            Intent::DoubleTap { place, index } => self.double_tap(place, index),
            Intent::Drop { from, index, to } => self.drop(from, index, to),
            Intent::Back => self.game.clear_selection(),
            Intent::Cancelled => None,
        }
    }

    fn tap(&mut self, place: Place, index: Option<usize>) -> Words {
        let pile = Self::pile(place)?;
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
                    return None;
                }
                self.game.tap(pile, index)
            }
            // An empty place: a press on it is a destination for whatever is in
            // hand, and nothing more.
            None if self.game.selection().is_some() => self.game.drop(pile),
            None => None,
        }
    }

    fn double_tap(&mut self, place: Place, index: Option<usize>) -> Words {
        let (Some(pile), Some(index)) = (Self::pile(place), index) else {
            return None;
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
    fn drop(&mut self, from: Place, index: usize, to: Place) -> Words {
        let (Some(from), Some(to)) = (Self::pile(from), Self::pile(to)) else {
            return None;
        };
        self.game.select(from, index);
        self.game.drop(to)
    }

    /// A deal that has become a formality finishes itself.
    ///
    /// The shell asks this after every change, so there is no button to press
    /// and no second start: the move that uncovers the last card is the move
    /// that begins the finish. When the player takes a card back out of it the
    /// game holds, and the Finish button is what sets it going again.
    fn finish_step(&mut self) -> Option<i32> {
        if !self.game.finishing() {
            return None;
        }
        if !self.game.autocomplete_step() {
            // Nothing can go home just now — every card is up, and a run still
            // has to be moved by hand before the next one can. The finish asks
            // again after the player's next move, and nothing is held, so it
            // takes over the moment a card is reachable again.
            return None;
        }
        // A card went home by itself, so any sentence on the line is about a
        // board that has moved on: clear it, the same as a move the player made.
        self.show(None);
        if self.game.won() {
            None
        } else {
            Some(FINISH_MS)
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
        let words = self.answer(intent);
        self.show(words);
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
        let words = self.game.undo();
        self.show(words);
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
        // The finish runs by itself, so its button exists for the one case that
        // needs it: the deal is a formality, the player took a card back out of
        // it, and now they want it done after all. It appears only when the
        // board is ready, so it is never a button that cannot do anything — a
        // control that is always there and almost never works teaches the
        // player that controls do nothing.
        (self.game.finish_held() && self.game.can_autocomplete()).then_some("Finish")
    }

    fn action_enabled(&self) -> bool {
        self.game.finish_held() && self.game.can_autocomplete()
    }

    fn action(&mut self) {
        self.game.resume_finish();
    }

    fn automatic(&mut self) -> Option<i32> {
        self.finish_step()
    }

    fn inspect_label(&self) -> Option<&'static str> {
        Some("Stuck?")
    }

    /// The question is about the board on screen, so it is answered in one
    /// go: there is no search to slice up, and `Inspect::Busy` — the page
    /// coming back for another frame — is a thing this game never needs.
    fn inspect(&mut self) -> Inspect {
        Inspect::Done(Some(stuck_words(self.game.stuck())))
    }

    fn take_words(&mut self) -> Option<String> {
        self.words.take()
    }

    fn decorate(&mut self, board: &BoardUi) {
        self.paint_hint(board);
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
        self.forget_words();
    }

    fn restart(&mut self) {
        self.game = self.game.restart();
        self.forget_words();
    }
}

impl Klondike {
    /// Mark where the hinted move goes.
    ///
    /// A ring around a card says "this one" and nothing else: the player is
    /// left to work out the destination from a sentence, and a sentence about
    /// "foundation 3" is a poor substitute for pointing at it. So the move is
    /// drawn as both halves — the run to pick up, which the board already
    /// rings, and the place it lands. A hint about the stock needs no second
    /// mark: the stock is both ends of it.
    fn paint_hint(&self, board: &BoardUi) {
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return;
        };
        let Some(mark) = document.get_element_by_id("hint-mark") else {
            return;
        };
        let Some(Hint::Move { to, .. }) = self.hint else {
            let _ = mark.set_attribute("hidden", "");
            return;
        };
        let place = to.place();
        let scene = board.scene();
        let landing = scene
            .slot(place)
            .and_then(|slot| slot.top().and_then(|card| board.layout().card(card.id)))
            .or_else(|| board.layout().place(place));
        let Some((x, y)) = landing else {
            let _ = mark.set_attribute("hidden", "");
            return;
        };
        let _ = mark.remove_attribute("hidden");
        let _ = mark.set_attribute(
            "style",
            &format!(
                "left:{x}px;top:{y}px;width:{}px;height:{}px;--cw:{}px",
                board.layout().card_w,
                board.layout().card_h,
                board.layout().card_w
            ),
        );
    }

    /// Clear the line when the board under it was thrown away: a sentence about
    /// a deal that no longer exists is worse than nothing at all.
    fn forget_words(&mut self) {
        self.words = Some(String::new());
    }

    /// Keep whatever this action said, for the shell to put under the board.
    ///
    /// Always something: an action that says nothing clears the line, so a
    /// hint from three moves ago cannot sit under a board it is not about. A
    /// won deal is the exception — the victory panel is the whole message, and
    /// whatever the line held is hidden behind it and cleared by the next deal.
    fn show(&mut self, words: Words) {
        self.words = if self.game.won() {
            None
        } else {
            Some(words.unwrap_or_default())
        };
    }
}
