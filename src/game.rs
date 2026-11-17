//! Klondike as a value: the deal, the rules, the score, the undo history and
//! every word the screen says about them.
//!
//! No `web-sys`, no `js-sys`, no `wasm-bindgen`, so this module compiles and
//! runs natively under `cargo test`. The browser layer in `crate::ui` decides
//! nothing: it turns a tap into a [`Pile`] and an index, and paints what this
//! module reports.
//!
//! Three rules carry the design:
//!
//! * **Every mutation is a transaction.** A move or a draw is checked against a
//!   snapshot taken first, so undo is correct by construction rather than by
//!   remembering, for each feature, what it touched.
//! * **Every random deal is a seed.** The same seed deals the same game, which
//!   is what makes "Restart deal" honest and a saved game a few hundred bytes.
//! * **A move says nothing.** The board is the record of what happened, so a
//!   move, a draw, a recycle or an undo returns no words at all: a line of text
//!   under the table repeating what the player just watched themselves do is
//!   noise in the one place the cards want to be. A tap that *fails* is the
//!   exception, because the board then shows nothing and silence would read as
//!   a broken control.
//! * **Every word is written here.** [`Hint::words`], [`Game::readout`],
//!   [`Game::result_words`] and the refusals above are the only places that
//!   turn state into prose; the page never composes a sentence of its own.

use std::fmt::Write as _;

use crate::card::{deck, Card, Rank};

/// Key the game is saved under.
///
/// One prefix per app, so two PWAs on the same origin cannot read or overwrite
/// each other's state. The version lives in the stored value, not the key: a key
/// that changes loses every game in progress.
pub const STORAGE_KEY: &str = "solitaire.game.v1";

/// Key the next deal's options are saved under.
pub const OPTIONS_KEY: &str = "solitaire.options.v1";

/// How many moves can be taken back.
///
/// A snapshot is seven small vectors and five numbers, so this is a bounded
/// few hundred kilobytes at worst and never a slow leak in a tab left open all
/// day.
pub const HISTORY_LIMIT: usize = 500;

/// How many tableau columns a Klondike board has.
pub const COLUMNS: usize = 7;

/// How many foundations a Klondike board has.
pub const FOUNDATIONS: usize = 4;

/// The longest a game clock will hold: just under 100 hours.
const ELAPSED_LIMIT: u32 = 359_999;

/// How a deal is turned.
///
/// Difficulty in Klondike is not a hidden dice roll: it is how many cards the
/// stock gives up at once, and how many times the waste may be turned back
/// over. Those are the knobs every established implementation exposes, and
/// naming them plainly is more useful than a vague adjective alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Difficulty {
    /// One card at a time, unlimited passes. The gentlest deal.
    Easy,
    /// Three cards at a time, unlimited passes. The standard game.
    Medium,
    /// Three cards at a time, three passes through the stock.
    Hard,
}

impl Difficulty {
    /// Every difficulty, easiest first — the order the menu lists them in.
    pub const ALL: [Self; 3] = [Self::Easy, Self::Medium, Self::Hard];

    /// The word for this difficulty.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Easy => "Easy",
            Self::Medium => "Medium",
            Self::Hard => "Hard",
        }
    }

    /// The difficulty's name as stored and parsed, so a saved game survives a
    /// change to the visible wording.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Easy => "easy",
            Self::Medium => "medium",
            Self::Hard => "hard",
        }
    }

    /// A difficulty from its stored name.
    #[must_use]
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|level| level.slug() == slug)
    }

    /// How many cards the stock turns over at once.
    #[must_use]
    pub const fn draw(self) -> usize {
        match self {
            Self::Easy => 1,
            Self::Medium | Self::Hard => 3,
        }
    }

    /// The most times the waste may be turned back over, or `None` for no
    /// limit.
    ///
    /// Three passes means the first deal plus two redeals.
    #[must_use]
    pub const fn max_redeals(self) -> Option<u32> {
        match self {
            Self::Easy | Self::Medium => None,
            Self::Hard => Some(2),
        }
    }

    /// The rule in a few words, for the menu.
    #[must_use]
    pub const fn note(self) -> &'static str {
        match self {
            Self::Easy => "One at a time · unlimited passes",
            Self::Medium => "Three at a time · unlimited passes",
            Self::Hard => "Three at a time · three passes",
        }
    }
}

/// How the game is scored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scoring {
    /// The classic points table: waste and foundation moves score, turning a
    /// card scores, taking one back off a foundation costs.
    Standard,
    /// A 52-coin stake, five coins for every card sent home, one pass only.
    Vegas,
    /// No score at all: the board and the clock, and no arithmetic.
    Off,
}

impl Scoring {
    /// Every scoring style, in the order the menu lists them.
    pub const ALL: [Self; 3] = [Self::Standard, Self::Vegas, Self::Off];

    /// The word for this style.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Vegas => "Vegas",
            Self::Off => "No score",
        }
    }

    /// The style's stored name.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Vegas => "vegas",
            Self::Off => "off",
        }
    }

    /// A style from its stored name.
    #[must_use]
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|style| style.slug() == slug)
    }

    /// The style in a few words, for the menu.
    #[must_use]
    pub const fn note(self) -> &'static str {
        match self {
            Self::Standard => "Points for cards played and turned",
            Self::Vegas => "52-coin stake · 5 a card · one pass",
            Self::Off => "Game and clock · no points",
        }
    }
}

/// The next deal's settings: what the menu shows and what New game applies.
///
/// Deliberately separate from the running [`Game`]: changing a select is a
/// request for the *next* deal, and must not throw away the game in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// How the next deal turns cards.
    pub difficulty: Difficulty,
    /// How the next deal is scored.
    pub scoring: Scoring,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            difficulty: Difficulty::Medium,
            scoring: Scoring::Standard,
        }
    }
}

impl Options {
    /// Options from the stored line, falling back to the defaults.
    #[must_use]
    pub fn from_stored(raw: Option<&str>) -> Self {
        raw.and_then(Self::parse).unwrap_or_default()
    }

    /// The line to store.
    #[must_use]
    pub fn to_stored(self) -> String {
        format!("v1|{}|{}", self.difficulty.slug(), self.scoring.slug())
    }

    fn parse(text: &str) -> Option<Self> {
        let mut fields = text.split('|');
        if fields.next()? != "v1" {
            return None;
        }
        let difficulty = Difficulty::from_slug(fields.next()?)?;
        let scoring = Scoring::from_slug(fields.next()?)?;
        if fields.next().is_some() {
            return None;
        }
        Some(Self {
            difficulty,
            scoring,
        })
    }
}

/// One place on the board.
///
/// This is what the page sends back on a tap and what a move names, so it is
/// the whole vocabulary shared between the rules and the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pile {
    /// The facedown stock.
    Stock,
    /// The face-up waste.
    Waste,
    /// One of the four foundations.
    Foundation(usize),
    /// One of the seven tableau columns.
    Tableau(usize),
}

impl Pile {
    /// Every pile, in the order the page lays them out: the top row left to
    /// right, then the columns.
    pub const ALL: [Self; 2 + FOUNDATIONS + COLUMNS] = [
        Self::Stock,
        Self::Waste,
        Self::Foundation(0),
        Self::Foundation(1),
        Self::Foundation(2),
        Self::Foundation(3),
        Self::Tableau(0),
        Self::Tableau(1),
        Self::Tableau(2),
        Self::Tableau(3),
        Self::Tableau(4),
        Self::Tableau(5),
        Self::Tableau(6),
    ];

    /// The pile's name in running text.
    #[must_use]
    pub fn name(self) -> String {
        match self {
            Self::Stock => "the stock".to_owned(),
            Self::Waste => "the waste".to_owned(),
            Self::Foundation(index) => format!("foundation {}", index + 1),
            Self::Tableau(index) => format!("column {}", index + 1),
        }
    }

    /// The stable token the DOM carries and parses back.
    #[must_use]
    pub fn slug(self) -> String {
        match self {
            Self::Stock => "stock".to_owned(),
            Self::Waste => "waste".to_owned(),
            Self::Foundation(index) => format!("foundation-{index}"),
            Self::Tableau(index) => format!("tableau-{index}"),
        }
    }

    /// Where this pile sits on the shared board.
    ///
    /// The board numbers the top row left to right and the columns left to
    /// right; this is the only place the game's names and the board's places
    /// are tied together, so there is one answer rather than two.
    #[must_use]
    pub fn place(self) -> crate::board::Place {
        match self {
            Self::Stock => crate::board::Place::Top(0),
            Self::Waste => crate::board::Place::Top(1),
            Self::Foundation(index) => crate::board::Place::Top(2 + index),
            Self::Tableau(index) => crate::board::Place::Column(index),
        }
    }

    /// The pile a board place means, if this game has one there.
    #[must_use]
    pub fn from_place(place: crate::board::Place) -> Option<Self> {
        match place {
            crate::board::Place::Top(0) => Some(Self::Stock),
            crate::board::Place::Top(1) => Some(Self::Waste),
            crate::board::Place::Top(index) if index < 2 + FOUNDATIONS => {
                Some(Self::Foundation(index - 2))
            }
            crate::board::Place::Column(index) if index < COLUMNS => Some(Self::Tableau(index)),
            _ => None,
        }
    }

    /// A pile from its DOM token, rejecting anything out of range.
    #[must_use]
    pub fn from_slug(slug: &str) -> Option<Self> {
        if slug == "stock" {
            return Some(Self::Stock);
        }
        if slug == "waste" {
            return Some(Self::Waste);
        }
        if let Some(rest) = slug.strip_prefix("foundation-") {
            let index = rest.parse::<usize>().ok()?;
            return (index < FOUNDATIONS).then_some(Self::Foundation(index));
        }
        if let Some(rest) = slug.strip_prefix("tableau-") {
            let index = rest.parse::<usize>().ok()?;
            return (index < COLUMNS).then_some(Self::Tableau(index));
        }
        None
    }
}

/// One card picked up on the board: where it starts, and how far up its pile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// The pile the run is in.
    pub pile: Pile,
    /// The position of the run's first card within that pile.
    pub index: usize,
}

/// One tableau column: its cards, and how many at the bottom are face down.
#[derive(Debug, Clone)]
struct Column {
    cards: Vec<u16>,
    /// The first face-up card's index; `cards.len()` means none are face up.
    up: usize,
}

impl Column {
    fn new() -> Self {
        Self {
            cards: Vec::new(),
            up: 0,
        }
    }

    fn len(&self) -> usize {
        self.cards.len()
    }

    fn is_face_up(&self, index: usize) -> bool {
        index >= self.up && index < self.cards.len()
    }

    fn top(&self) -> Option<u16> {
        self.cards.last().copied()
    }
}

/// A state that undo returns to.
#[derive(Debug, Clone)]
struct Snapshot {
    stock: Vec<u16>,
    waste: Vec<u16>,
    foundations: [Vec<u16>; FOUNDATIONS],
    tableau: [Column; COLUMNS],
    score: i32,
    moves: u32,
    elapsed: u32,
    redeals: u32,
    won: bool,
}

/// What a tap, a draw or an undo says for itself, in the game's own words.
///
/// A mutation returns `None` when the board told the story already, which is
/// almost always: the cards moved, so there is nothing to say. `Some` is for a
/// refusal — a tap that changed nothing, where silence would read as a broken
/// control.
pub type Words = Option<String>;

/// A refusal: the tap could not do what it was aimed at, so it says why.
fn refuse(why: impl Into<String>) -> Words {
    Some(why.into())
}

/// What the game suggests.
///
/// A hint is a *picture*: the board rings the cards to pick up and the place
/// they land, which says "here, then there" without a sentence, and a sentence
/// beside it ("Try the nine of hearts to column 3.") is the board read aloud to
/// someone already looking at it. So a hint that has something to show says
/// nothing at all, and the one case with no picture to show — nothing to do —
/// is the one case that speaks ([`Hint::words`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    /// Move `count` cards beginning at `index` in `from` to `to`.
    Move {
        /// The source pile.
        from: Pile,
        /// The run's first card within the source.
        index: usize,
        /// The destination pile.
        to: Pile,
        /// How many cards the move carries.
        count: usize,
    },
    /// The stock has cards whose turn will expose something playable.
    Draw,
    /// The waste can be turned back over to expose something playable.
    Recycle,
    /// Nothing left that changes the deal.
    None,
}

/// What the line says when there is nothing to do, from either button.
pub const WORDS_STUCK: &str = "Stuck";

/// What the Stuck? button says when there is something left to play.
///
/// Only that button says it. "Not stuck" is the answer to a question the player
/// asked; from *Hint* it is noise — a player pressing Hint wants the move, and
/// the ring on the board is the move.
pub const WORDS_NOT_STUCK: &str = "Not stuck";

impl Hint {
    /// What the line under the board has to say, or `None` for silence.
    ///
    /// Silence is the normal case: the board is showing the answer. The one
    /// exception is a deal with nothing left in it, where there is no ring to
    /// draw and the player would otherwise get nothing at all.
    #[must_use]
    pub const fn words(&self) -> Option<&'static str> {
        match self {
            Self::None => Some(WORDS_STUCK),
            Self::Move { .. } | Self::Draw | Self::Recycle => None,
        }
    }
}

/// The facts the header shows, already formatted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readout {
    /// `m:ss`, or `h:mm:ss` past an hour.
    pub clock: String,
    /// The move count.
    pub moves: String,
    /// The score, or `None` when scoring is off.
    pub score: Option<String>,
}

/// A game in progress.
#[derive(Debug, Clone)]
pub struct Game {
    /// The immutable deck; a pile holds indexes into it.
    ///
    /// Building the deck once and moving indexes means a snapshot never copies
    /// a card, and the page has a stable element key per physical card.
    cards: Vec<Card>,
    stock: Vec<u16>,
    waste: Vec<u16>,
    foundations: [Vec<u16>; FOUNDATIONS],
    tableau: [Column; COLUMNS],
    difficulty: Difficulty,
    scoring: Scoring,
    seed: u64,
    score: i32,
    moves: u32,
    elapsed: u32,
    redeals: u32,
    won: bool,
    /// False until the player has done something, so an untouched first visit
    /// writes nothing.
    started: bool,
    /// True while the finish should not run by itself, because the player took
    /// a card back out of it. The deal is then theirs to play by hand, and the
    /// Finish button is how it is set going again.
    finish_held: bool,
    history: Vec<Snapshot>,
    selection: Option<Selection>,
}

impl Game {
    /// A fresh deal, ready for the page but not yet played.
    #[must_use]
    pub fn new(difficulty: Difficulty, scoring: Scoring, seed: u64) -> Self {
        let cards = deck();
        let mut order: Vec<u16> = (0..cards.len() as u16).collect();
        shuffle(&mut order, seed);

        let mut tableau: [Column; COLUMNS] = std::array::from_fn(|_| Column::new());
        let mut next = order.into_iter();
        // The conventional deal: one card to the first column, two to the
        // second, and so on, each deal row moving left to right. The shuffle
        // already made the cards random, so the geometry is all that matters.
        for (index, column) in tableau.iter_mut().enumerate() {
            for _ in 0..=index {
                if let Some(card) = next.next() {
                    column.cards.push(card);
                }
            }
            column.up = column.cards.len().saturating_sub(1);
        }
        let stock: Vec<u16> = next.collect();

        Self {
            cards,
            stock,
            waste: Vec::new(),
            foundations: std::array::from_fn(|_| Vec::new()),
            tableau,
            difficulty,
            scoring,
            seed,
            score: if scoring == Scoring::Vegas { -52 } else { 0 },
            moves: 0,
            elapsed: 0,
            redeals: 0,
            won: false,
            started: false,
            finish_held: false,
            history: Vec::new(),
            selection: None,
        }
    }

    /// A fresh deal the player asked for, which may be written to storage at
    /// once even if they have not moved yet.
    #[must_use]
    pub fn dealt(difficulty: Difficulty, scoring: Scoring, seed: u64) -> Self {
        let mut game = Self::new(difficulty, scoring, seed);
        game.started = true;
        game
    }

    /// A new deal with the same rules and the same card order.
    #[must_use]
    pub fn restart(&self) -> Self {
        Self::dealt(self.difficulty, self.scoring, self.seed)
    }

    /// The card behind a deck index.
    #[must_use]
    pub fn card(&self, index: u16) -> Card {
        self.cards[index as usize]
    }

    /// The pile's cards, bottom first.
    #[must_use]
    pub fn pile(&self, pile: Pile) -> &[u16] {
        match pile {
            Pile::Stock => &self.stock,
            Pile::Waste => &self.waste,
            Pile::Foundation(index) => &self.foundations[index],
            Pile::Tableau(index) => &self.tableau[index].cards,
        }
    }

    /// How many cards a pile holds.
    #[must_use]
    pub fn pile_len(&self, pile: Pile) -> usize {
        self.pile(pile).len()
    }

    /// How many cards are in the deck: 52 for a Klondike deal.
    #[must_use]
    pub fn deck_len(&self) -> usize {
        self.cards.len()
    }

    /// A pile's name and top card, for a screen reader. The page adds the
    /// position of an individual card; the pile's own facts come from here.
    #[must_use]
    pub fn pile_label(&self, pile: Pile) -> String {
        let len = self.pile_len(pile);
        if len == 0 {
            return format!("{}, empty", pile.name());
        }
        if pile == Pile::Stock {
            return format!("{}, {} cards", pile.name(), len);
        }
        match self.card_at(pile, len - 1) {
            Some(top) => format!("{}, {} cards, top card {}", pile.name(), len, top.name()),
            None => format!("{}, {} cards", pile.name(), len),
        }
    }

    /// The faint mark an empty place shows: what belongs there, or how the
    /// stock can be turned back over.
    #[must_use]
    pub fn slot_mark(&self, pile: Pile) -> &'static str {
        if !self.pile(pile).is_empty() {
            return "";
        }
        match pile {
            Pile::Stock if self.can_redeal() => "↻",
            Pile::Stock => "—",
            Pile::Waste => "",
            Pile::Foundation(_) => "A",
            Pile::Tableau(_) => "K",
        }
    }

    /// The run beginning at `index`, if it can be picked up as a unit.
    ///
    /// Public because a drag needs to lift the same cards a tap would: the page
    /// asks this question before it starts a gesture, and the answer must not
    /// be a second copy of the rule.
    #[must_use]
    pub fn run_len(&self, pile: Pile, index: usize) -> Option<usize> {
        self.run_len_inner(pile, index)
    }

    /// Put the held run back without moving it.
    ///
    /// Nothing is said: the run drops back to where it came from, which is the
    /// whole of the answer.
    pub fn clear_selection(&mut self) -> Words {
        self.selection = None;
        None
    }

    /// Whether a card in a pile is printed face down.
    ///
    /// The stock is face down; the waste and the foundations are not; a column
    /// knows its own boundary.
    #[must_use]
    pub fn is_face_down(&self, pile: Pile, index: usize) -> bool {
        match pile {
            Pile::Stock => index < self.stock.len(),
            Pile::Waste | Pile::Foundation(_) => false,
            Pile::Tableau(column) => !self.tableau[column].is_face_up(index),
        }
    }

    /// The rules of this deal.
    #[must_use]
    pub const fn difficulty(&self) -> Difficulty {
        self.difficulty
    }

    /// The scoring style of this deal.
    #[must_use]
    pub const fn scoring(&self) -> Scoring {
        self.scoring
    }

    /// The seed this deal was shuffled from.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// The turn in progress, if any.
    #[must_use]
    pub const fn selection(&self) -> Option<Selection> {
        self.selection
    }

    /// True once the player has done something.
    #[must_use]
    pub const fn started(&self) -> bool {
        self.started
    }

    /// True when the deal is complete.
    #[must_use]
    pub const fn won(&self) -> bool {
        self.won
    }

    /// The current score. Vegas may be negative; the other styles never are.
    #[must_use]
    pub const fn score(&self) -> i32 {
        self.score
    }

    /// The number of moves made, including draws and redeals.
    #[must_use]
    pub const fn moves(&self) -> u32 {
        self.moves
    }

    /// Seconds on the clock.
    #[must_use]
    pub const fn elapsed(&self) -> u32 {
        self.elapsed
    }

    /// Whether there is a move to take back.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    /// The deal's rules as one label, for the top of the menu.
    ///
    /// The header already says "Turn 3" and the two selects below say which
    /// settings are chosen, so this line is the one thing neither of them says:
    /// the passes left, and how the deal is scored.
    #[must_use]
    pub fn summary(&self) -> String {
        let passes = match self.redeal_limit() {
            None => "unlimited passes".to_owned(),
            Some(0) => "one pass".to_owned(),
            Some(max) => format!("{} passes left", max.saturating_sub(self.redeals)),
        };
        format!(
            "{} · {} · {}",
            self.difficulty.label(),
            passes,
            self.scoring.label()
        )
    }

    /// The short label the header shows beside the title.
    #[must_use]
    pub fn variant(&self) -> String {
        match self.scoring {
            Scoring::Vegas => format!("Vegas · turn {}", self.difficulty.draw()),
            _ => format!("Turn {}", self.difficulty.draw()),
        }
    }

    /// The formatted facts for the header.
    #[must_use]
    pub fn readout(&self) -> Readout {
        Readout {
            clock: clock_text(self.elapsed),
            moves: self.moves.to_string(),
            score: match self.scoring {
                Scoring::Standard => Some(grouped(i64::from(self.score))),
                Scoring::Vegas => Some(money(self.score)),
                Scoring::Off => None,
            },
        }
    }

    /// Is the player out of moves that do anything?
    ///
    /// "Stuck" is a question about the board in front of the player: whether
    /// *any* legal move changes it, rather than a shuffle whose reverse is the
    /// next move. A card that goes home, a run that turns a face-down card over
    /// or empties a column, a playable card in the waste, and a stock turn that
    /// can expose a playable card all change the deal; a queen moved off a king
    /// and back onto it does not, and neither does walking the stock from one
    /// end to the other and back.
    ///
    /// The answer is [`Game::hint`], and the two say the same thing because
    /// they *are* the same thing: one word ([`Hint::words`]) is what the line
    /// under the board shows either way, so a "not stuck" can never arrive with
    /// nothing to suggest — which is what a player who pressed both buttons
    /// would otherwise catch in a second.
    #[must_use]
    pub fn stuck(&self) -> bool {
        !self.won && matches!(self.hint(), Hint::None)
    }

    /// The three facts of a finished deal, for the victory panel.
    ///
    /// The panel already says "You won" twice over — a kicker and a heading —
    /// so the line under them is the numbers and nothing else: `"2:31 · 43
    /// moves · 520 points"`.
    #[must_use]
    pub fn result_words(&self) -> String {
        let moves = if self.moves == 1 {
            "1 move".to_owned()
        } else {
            format!("{} moves", self.moves)
        };
        let mut out = format!("{} · {}", clock_text(self.elapsed), moves);
        match self.scoring {
            Scoring::Standard => {
                let _ = write!(out, " · {} points", grouped(i64::from(self.score)));
            }
            Scoring::Vegas => {
                let _ = write!(out, " · {}", money(self.score));
            }
            Scoring::Off => {}
        }
        out
    }

    /// Advance the clock. A won game stops.
    pub fn tick(&mut self, seconds: u32) {
        if self.won {
            return;
        }
        self.elapsed = self.elapsed.saturating_add(seconds).min(ELAPSED_LIMIT);
    }

    /// Handle a tap on a pile card (or on an empty pile, where `index` is its
    /// length).
    ///
    /// The whole click grammar lives here: pick up, put down, put the picked-up
    /// run down, or switch the pick to another movable card.
    ///
    /// A tap that moves a card says nothing — the card is the sentence. A tap
    /// that cannot do what it was aimed at says why, because nothing moved and
    /// a control that answers nothing is a control the player thinks is broken.
    pub fn tap(&mut self, pile: Pile, index: usize) -> Words {
        if self.won {
            return None;
        }
        if pile == Pile::Stock {
            return self.draw();
        }
        if self.selection.is_none() && self.pile_len(pile) == index {
            // A tap on an empty place, with nothing in hand, is not a move and
            // not a mistake worth a sentence.
            return None;
        }
        if let Some(selection) = self.selection {
            if selection.pile == pile {
                if selection.index == index {
                    self.selection = None;
                    return None;
                }
                return self.select(pile, index);
            }
            if self.can_move(selection, pile) {
                return self.apply(selection, pile);
            }
            if self.can_pick(pile, index) {
                return self.select(pile, index);
            }
            return refuse("Can't go there");
        }
        self.select(pile, index)
    }

    /// Pick up the run beginning at `index`, if it is movable.
    pub fn select(&mut self, pile: Pile, index: usize) -> Words {
        if self.run_len_inner(pile, index).is_none() {
            if self.is_face_down(pile, index) && index < self.pile_len(pile) {
                return refuse("Face down");
            }
            return refuse("Can't move");
        }
        self.selection = Some(Selection { pile, index });
        None
    }

    /// Whether the run at `pile`/`index` can be picked up.
    #[must_use]
    pub fn can_pick(&self, pile: Pile, index: usize) -> bool {
        !self.won && self.run_len_inner(pile, index).is_some()
    }

    /// Drop the held run on `to`. The page calls this on a drag; it is the same
    /// transaction as tapping the destination.
    pub fn drop(&mut self, to: Pile) -> Words {
        let selection = self.selection?;
        if selection.pile == to {
            self.selection = None;
            return None;
        }
        if !self.can_move(selection, to) {
            return refuse("Can't go there");
        }
        self.apply(selection, to)
    }

    /// Send the picked-up card, or the tapped card, to the best place for it.
    ///
    /// A double tap is the shortcut every implementation has: home to a
    /// foundation when that is legal, otherwise onto the best column.
    pub fn double_tap(&mut self, pile: Pile, index: usize) -> Words {
        if self.won {
            return None;
        }
        let Some(card) = self.card_at(pile, index) else {
            return refuse("Can't move");
        };
        if self.run_len_inner(pile, index) != Some(1) {
            // Only a single card can go home; a run is a thing to move by hand.
            self.selection = None;
            return refuse("Part of a run");
        }
        if let Some(foundation) = self.foundation_for(card) {
            return self.apply(Selection { pile, index }, Pile::Foundation(foundation));
        }
        // Nothing moves, and the card is put back. A double tap means "send this
        // home", and when it cannot there is no second-best place to send it: a
        // card that jumps to a column the player did not choose is worse than a
        // card that stays where they left it.
        self.selection = None;
        refuse("Nowhere to go")
    }

    /// Turn the stock, or turn the waste back over when the stock is empty.
    pub fn draw(&mut self) -> Words {
        if self.won {
            return None;
        }
        if self.stock.is_empty() {
            return self.recycle();
        }
        self.push_history();
        self.selection = None;
        let count = self.difficulty.draw().min(self.stock.len());
        for _ in 0..count {
            if let Some(card) = self.stock.pop() {
                self.waste.push(card);
            }
        }
        self.moves = self.moves.saturating_add(1);
        self.started = true;
        None
    }

    /// Take back the last move, draw, recycle or automatic move.
    pub fn undo(&mut self) -> Words {
        let snapshot = self.history.pop()?;
        self.restore(snapshot);
        self.selection = None;
        // Taking a card back out of a deal that was finishing itself is the
        // player saying they want to play it by hand: it must not start
        // finishing again underneath them, or the undo does nothing at all.
        // The Finish button puts it back to work.
        if self.can_autocomplete() {
            self.finish_held = true;
        }
        None
    }

    /// True when every card is face up and the stock and waste are empty, so
    /// the rest of the deal is a formality.
    #[must_use]
    pub fn can_autocomplete(&self) -> bool {
        !self.won
            && self.stock.is_empty()
            && self.waste.is_empty()
            && self.tableau.iter().all(|column| column.up == 0)
    }

    /// True when the deal should finish itself now: every card is uncovered,
    /// and the player has not taken it back.
    ///
    /// The page asks this after every change, which is the whole of "the Finish
    /// button happens by itself": there is no timer, no second start and no
    /// button to press, and a deal that becomes a formality finishes wherever
    /// the move that made it one came from.
    #[must_use]
    pub fn finishing(&self) -> bool {
        self.can_autocomplete() && !self.finish_held
    }

    /// True when the finish has stopped and the player has to ask for it.
    #[must_use]
    pub const fn finish_held(&self) -> bool {
        self.finish_held
    }

    /// Let the automatic finish run again, after the player stopped it.
    pub fn resume_finish(&mut self) {
        self.finish_held = false;
    }

    /// Move one more card home, if the board is ready for it.
    ///
    /// Returns `false` when there is nothing left to send home *just now*, and
    /// the page stops until the deal changes again. That is not a verdict: every
    /// card being face up does not mean every card is reachable, and a run
    /// moved by hand can uncover the card that was in the way. So the finish
    /// asks again after the next move instead of deciding it is over, and it
    /// needs no switch of its own to do so.
    pub fn autocomplete_step(&mut self) -> bool {
        if !self.can_autocomplete() {
            return false;
        }
        // Lowest rank first, so aces and twos that are buried in different
        // columns are not raced past by a higher card that happens to be on top.
        let mut best: Option<(u8, usize, usize)> = None;
        for (column, pile) in self.tableau.iter().enumerate() {
            let Some(index) = pile.top().map(|_| pile.len() - 1) else {
                continue;
            };
            let Some(card) = self.card_at(Pile::Tableau(column), index) else {
                continue;
            };
            for foundation in 0..FOUNDATIONS {
                if self.accepts(Pile::Foundation(foundation), card) {
                    let rank = card.rank.value();
                    if best.is_none_or(|(best_rank, _, _)| rank < best_rank) {
                        best = Some((rank, column, foundation));
                    }
                }
            }
        }
        let Some((_, column, foundation)) = best else {
            return false;
        };
        let index = self.tableau[column].len() - 1;
        let _ = self.apply(
            Selection {
                pile: Pile::Tableau(column),
                index,
            },
            Pile::Foundation(foundation),
        );
        true
    }

    /// Suggest a move. The suggestion is a heuristic, not a solver: it prefers
    /// the move that turns over a hidden card, then one that clears a column,
    /// then anything that sends a card home.
    ///
    /// It suggests only a move that *changes* the deal. "This move is legal" is
    /// not enough to be worth pointing at: a card that can go onto another
    /// column can usually come straight back, so a hint that counts every legal
    /// move points at the card, then at the place it came from, then at the
    /// card again — the deal does not move, and the player pressing Hint is
    /// told to undo and redo the same move forever. What is worth pointing at
    /// is a move that turns a card over, empties a column, or uncovers a card
    /// that can go home; when those are all gone the hint says so, and the
    /// stock is the next thing to try.
    #[must_use]
    pub fn hint(&self) -> Hint {
        if self.won {
            return Hint::None;
        }

        let mut best: Option<(i32, Selection, Pile, usize)> = None;
        let mut consider = |score: i32, selection: Selection, to: Pile, count: usize| {
            if best
                .as_ref()
                .is_none_or(|(best_score, ..)| score > *best_score)
            {
                best = Some((score, selection, to, count));
            }
        };

        for column in 0..COLUMNS {
            let pile = Pile::Tableau(column);
            let cards = &self.tableau[column];
            for index in cards.up..cards.len() {
                let Some(count) = self.run_len_inner(pile, index) else {
                    continue;
                };
                let Some(card) = self.card_at(pile, index) else {
                    continue;
                };
                let gain = self.column_gain(column, index);
                if count == 1 {
                    for foundation in 0..FOUNDATIONS {
                        let to = Pile::Foundation(foundation);
                        if self.accepts(to, card) {
                            consider(gain + 800, Selection { pile, index }, to, count);
                        }
                    }
                }
                for destination in 0..COLUMNS {
                    let to = Pile::Tableau(destination);
                    if to == pile || !self.accepts(to, card) {
                        continue;
                    }
                    let into_room = self.pile_len(to) == 0;
                    // A whole column onto an empty one is the same board with a
                    // different number on it: it empties one column and fills
                    // another, and nothing about the deal has moved.
                    if into_room && index == 0 {
                        continue;
                    }
                    // Onto a card is worth a nudge over onto nothing, because a
                    // king in an empty column is one fewer place to put a king.
                    // A move that gains nothing is a shuffle whatever it lands
                    // on, so it is not considered at all.
                    if gain > 0 {
                        let score = if into_room { gain - 100 } else { gain + 40 };
                        consider(score, Selection { pile, index }, to, count);
                    }
                }
            }
        }

        // The waste is one card on top of a stack the player cannot reorder, so
        // it is worth more than the same card in a column.
        if let Some(index) = self.waste.len().checked_sub(1) {
            let Some(card) = self.card_at(Pile::Waste, index) else {
                return Hint::None;
            };
            if let Some(foundation) = self.foundation_for(card) {
                consider(
                    1_400,
                    Selection {
                        pile: Pile::Waste,
                        index,
                    },
                    Pile::Foundation(foundation),
                    1,
                );
            }
            for destination in 0..COLUMNS {
                let to = Pile::Tableau(destination);
                if !self.accepts(to, card) {
                    continue;
                }
                let mut score = 900;
                if self.pile_len(to) == 0 {
                    score -= 300;
                }
                consider(
                    score,
                    Selection {
                        pile: Pile::Waste,
                        index,
                    },
                    to,
                    1,
                );
            }
        }

        if let Some((_, selection, to, count)) = best {
            return Hint::Move {
                from: selection.pile,
                index: selection.index,
                to,
                count,
            };
        }
        // The stock is the last thing to try, and only if turning it can pay.
        // A stock that is walked to the end and back without exposing anything
        // playable is a loop, not a move, and pointing at it is how a hint
        // became an instruction to turn the same cards until the player gave
        // up — see `turning_pays`.
        if !self.stock.is_empty() {
            return if self.turning_pays() {
                Hint::Draw
            } else {
                Hint::None
            };
        }
        if !self.waste.is_empty() && self.can_redeal() {
            return if self.turning_pays() {
                Hint::Recycle
            } else {
                Hint::None
            };
        }
        // Nothing worth doing, and nothing left to turn over. Taking a card
        // back off a foundation is legal here and is not a suggestion: it is
        // how the player undoes their own progress, and the hint would then
        // send the same card home again.
        Hint::None
    }

    /// Whether turning the stock over can expose a card that plays.
    ///
    /// This is the answer to "the hint just keeps saying turn more cards". A
    /// turn that cannot produce a play ends exactly where it began — the whole
    /// stock goes through the waste and back into the stock unchanged — so
    /// pointing at the stock is a loop of the same kind as pointing at two
    /// columns alternately: press Hint, do what it says, press Hint, and get
    /// the same instruction back for ever.
    ///
    /// So the stock is walked here under the deal's own drawing rules and the
    /// deal's own pass limit, and the answer is whether any card it *exposes*
    /// — a waste top, never a card buried under one — can go to a foundation or
    /// onto a column at the moment it is seen. The walk stops when the stock
    /// comes back to a position it has already been in, which is the exact test
    /// for "this cycle repeats": with draw-three and a stock that is a multiple
    /// of three long, two thirds of the cards can never be reached at all, and
    /// a fixed number of passes would either miss a card or lie about one.
    ///
    /// Only cards in the stock can be judged this way, and that is enough: a
    /// card deeper in the waste can only be reached by playing the cards above
    /// it, and the caller has already established that nothing can be played.
    ///
    /// [`Game::plays_anywhere`] is the same test the hint applies to the waste
    /// top, on purpose: the promise this function makes is that some turn will
    /// put a card in the waste that the hint will then point at, so the two
    /// have to be one rule.
    fn turning_pays(&self) -> bool {
        let mut stock = self.stock.clone();
        let mut waste = self.waste.clone();
        let draw = self.difficulty.draw().max(1);
        // The passes the player still *has*, not the deal's whole allowance. A
        // walk that spends passes already used finds a playable card in a pass
        // the player cannot take, and that is the one way this answer can be
        // wrong: it says "not stuck" and rings the stock on a board where the
        // stock is the only thing left and it is spent.
        let mut recycles = match self.redeal_limit() {
            Some(max) => max.saturating_sub(self.redeals),
            None => u32::MAX,
        };
        loop {
            let start = stock.clone();
            while !stock.is_empty() {
                let take = draw.min(stock.len());
                for _ in 0..take {
                    if let Some(card) = stock.pop() {
                        waste.push(card);
                    }
                }
                let Some(&top) = waste.last() else {
                    return false;
                };
                if self.plays_anywhere(self.card(top)) {
                    return true;
                }
            }
            if waste.is_empty() || recycles == 0 {
                return false;
            }
            recycles -= 1;
            // The waste goes back into the stock the way the game turns it
            // over, and a stock that comes back exactly as it started is a
            // cycle with nothing new in it.
            stock = waste.drain(..).rev().collect();
            // The stock came back exactly as it was when this pass started, so
            // this pass is the previous one again: nothing new can be exposed.
            if stock == start {
                return false;
            }
        }
    }

    /// Whether this card has somewhere to go on the board as it stands.
    ///
    /// The same two destinations the hint considers: home, or a column. A card
    /// that can do neither is one the stock turn spent itself to look at.
    fn plays_anywhere(&self, card: Card) -> bool {
        self.foundation_for(card).is_some()
            || (0..COLUMNS).any(|column| self.accepts(Pile::Tableau(column), card))
    }

    /// What moving the run at `column`/`index` *to another column* is worth,
    /// and zero when it is a shuffle.
    ///
    /// Three things make a column-to-column move worth pointing at: it turns a
    /// face-down card over, it empties a column, or it uncovers a card that can
    /// now go home. A move that does none of them has not moved the deal on,
    /// only the picture — and because such a move is legal in both directions
    /// whenever both columns end in a matching rank and colour, suggesting it
    /// means suggesting the reverse of it next time Hint is pressed.
    fn column_gain(&self, column: usize, index: usize) -> i32 {
        let cards = &self.tableau[column];
        let mut gain = 0;
        if index == 0 {
            // The column the run leaves is left with nothing in it, which is a
            // place to put a king.
            gain += 700;
        }
        if index == cards.up && cards.up > 0 {
            // The run starts at the face-up boundary, so the card under it is
            // face down: moving the run is what turns it over.
            gain += 1_000;
        } else if index > cards.up {
            // The run has a face-up card under it, and that card becomes a
            // column top. It is worth something only if it can go home —
            // otherwise the next press of Hint is free to suggest moving the
            // run back onto it.
            if let Some(card) = self.card_at(Pile::Tableau(column), index - 1) {
                if self.foundation_for(card).is_some() {
                    gain += 400;
                }
            }
        }
        gain
    }

    /// The value stored for this game, or `None` when there is nothing worth
    /// keeping.
    #[must_use]
    pub fn to_stored(&self) -> Option<String> {
        if !self.started && self.moves == 0 && self.elapsed == 0 && !self.won {
            return None;
        }
        let mut out = String::new();
        let _ = write!(
            out,
            "v1|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.difficulty.slug(),
            self.scoring.slug(),
            self.seed,
            self.score,
            self.moves,
            self.elapsed,
            self.redeals,
            u8::from(self.won),
            u8::from(self.started),
            self.encode_cards(&self.stock),
            self.encode_cards(&self.waste),
        );
        for foundation in &self.foundations {
            out.push('|');
            out.push_str(&self.encode_cards(foundation));
        }
        for column in &self.tableau {
            let _ = write!(out, "|{}:{}", column.up, self.encode_cards(&column.cards));
        }
        Some(out)
    }

    /// Read a stored game back, or `None` if anything about it is unreadable.
    ///
    /// A corrupt entry must never be able to stop the app from starting, and a
    /// half-valid one must never put a card in two piles. So the parse checks
    /// more than the number of fields: every code is a real card, every card
    /// appears exactly once, and every foundation is a real ascending run.
    #[must_use]
    pub fn from_stored(raw: Option<&str>) -> Option<Self> {
        let raw = raw?;
        let fields: Vec<&str> = raw.split('|').collect();
        if fields.len() != 23 || fields[0] != "v1" {
            return None;
        }
        let difficulty = Difficulty::from_slug(fields[1])?;
        let scoring = Scoring::from_slug(fields[2])?;
        let seed = fields[3].parse::<u64>().ok()?;
        let score = fields[4].parse::<i32>().ok()?;
        let moves = fields[5].parse::<u32>().ok()?;
        let elapsed = fields[6].parse::<u32>().ok()?.min(ELAPSED_LIMIT);
        let redeals = fields[7].parse::<u32>().ok()?;
        let won = match fields[8] {
            "0" => false,
            "1" => true,
            _ => return None,
        };
        let started = match fields[9] {
            "0" => false,
            "1" => true,
            _ => return None,
        };

        let cards = deck();
        let mut seen = [false; 52];
        let mut decode = |text: &str| decode_cards(text, &cards, &mut seen);
        let stock = decode(fields[10])?;
        let waste = decode(fields[11])?;
        let mut foundations: [Vec<u16>; FOUNDATIONS] = std::array::from_fn(|_| Vec::new());
        for (index, foundation) in foundations.iter_mut().enumerate() {
            *foundation = decode(fields[12 + index])?;
            if !valid_foundation(foundation, &cards) {
                return None;
            }
        }
        let mut tableau: [Column; COLUMNS] = std::array::from_fn(|_| Column::new());
        for (index, column) in tableau.iter_mut().enumerate() {
            let field = fields[16 + index];
            let (up, pile) = field.split_once(':')?;
            let up = up.parse::<usize>().ok()?;
            let cards_in_pile = decode(pile)?;
            if up > cards_in_pile.len() {
                return None;
            }
            column.cards = cards_in_pile;
            column.up = up;
        }
        // Every physical card must be somewhere, exactly once. Duplicates are
        // caught by the `seen` array during decoding; this catches a save that
        // simply dropped one.
        if !seen.iter().all(|card| *card) {
            return None;
        }
        let mut game = Self {
            cards,
            stock,
            waste,
            foundations,
            tableau,
            difficulty,
            scoring,
            seed,
            score: match scoring {
                Scoring::Standard | Scoring::Off => score.max(0),
                Scoring::Vegas => score,
            },
            moves,
            elapsed,
            redeals,
            won,
            started,
            finish_held: false,
            history: Vec::new(),
            selection: None,
        };
        // `won` is derived, not trusted: a save that says so with an empty
        // foundation would otherwise lock the page on a victory screen.
        game.won = game.foundations.iter().map(Vec::len).sum::<usize>() == 52;
        Some(game)
    }

    /// How many times the waste may be turned back over: `None` for a deal
    /// with no limit, and zero for a Vegas stake, which allows one pass in all.
    #[must_use]
    pub fn redeal_limit(&self) -> Option<u32> {
        if self.scoring == Scoring::Vegas {
            Some(0)
        } else {
            self.difficulty.max_redeals()
        }
    }

    fn can_redeal(&self) -> bool {
        !self.waste.is_empty() && self.redeal_limit().is_none_or(|max| self.redeals < max)
    }

    fn recycle(&mut self) -> Words {
        if self.waste.is_empty() {
            // Nothing there at all, which the empty stock mark already says.
            return None;
        }
        if !self.can_redeal() {
            // The one refusal the board cannot show: the waste is not empty and
            // the stock will not take it, and nothing on screen says why.
            return refuse("No passes left");
        }
        self.push_history();
        self.selection = None;
        let mut turned = Vec::with_capacity(self.waste.len());
        while let Some(card) = self.waste.pop() {
            turned.push(card);
        }
        self.stock = turned;
        self.redeals = self.redeals.saturating_add(1);
        self.moves = self.moves.saturating_add(1);
        self.started = true;
        if self.scoring == Scoring::Standard {
            let penalty = if self.difficulty.draw() == 1 {
                -100
            } else {
                -20
            };
            self.score = (self.score + penalty).max(0);
        }
        None
    }

    fn run_len_inner(&self, pile: Pile, index: usize) -> Option<usize> {
        match pile {
            Pile::Stock => None,
            Pile::Waste => (index + 1 == self.waste.len()).then_some(1),
            Pile::Foundation(foundation) => {
                (index + 1 == self.foundations[foundation].len()).then_some(1)
            }
            Pile::Tableau(column) => {
                let pile = &self.tableau[column];
                if index >= pile.len() || !pile.is_face_up(index) {
                    return None;
                }
                for position in index..pile.len() - 1 {
                    let upper = self.card(pile.cards[position]);
                    let lower = self.card(pile.cards[position + 1]);
                    if upper.rank != lower.rank.up()? || upper.is_red() == lower.is_red() {
                        return None;
                    }
                }
                Some(pile.len() - index)
            }
        }
    }

    fn card_at(&self, pile: Pile, index: usize) -> Option<Card> {
        self.pile(pile).get(index).map(|&card| self.card(card))
    }

    fn foundation_for(&self, card: Card) -> Option<usize> {
        (0..FOUNDATIONS).find(|&foundation| self.accepts(Pile::Foundation(foundation), card))
    }

    fn accepts(&self, pile: Pile, card: Card) -> bool {
        match pile {
            Pile::Stock | Pile::Waste => false,
            Pile::Foundation(foundation) => match self.foundations[foundation].last() {
                None => card.rank == Rank::Ace,
                Some(&top) => {
                    let top = self.card(top);
                    top.suit == card.suit && top.rank.value() + 1 == card.rank.value()
                }
            },
            Pile::Tableau(column) => match self.tableau[column].top() {
                None => card.rank == Rank::King,
                Some(top) => {
                    let top = self.card(top);
                    top.rank.value() == card.rank.value() + 1 && top.is_red() != card.is_red()
                }
            },
        }
    }

    fn can_move(&self, selection: Selection, to: Pile) -> bool {
        if !self.can_pick(selection.pile, selection.index) {
            return false;
        }
        let Some(card) = self.card_at(selection.pile, selection.index) else {
            return false;
        };
        self.accepts(to, card)
    }

    fn apply(&mut self, selection: Selection, to: Pile) -> Words {
        let Some(count) = self.run_len_inner(selection.pile, selection.index) else {
            self.selection = None;
            return refuse("Can't move");
        };
        if to == selection.pile {
            self.selection = None;
            return None;
        }
        if to == Pile::Stock
            || to == Pile::Waste
            || (matches!(to, Pile::Foundation(_)) && count != 1)
        {
            return refuse("Can't go there");
        }
        let Some(card) = self.card_at(selection.pile, selection.index) else {
            return refuse("Can't move");
        };
        if !self.accepts(to, card) {
            return refuse("Can't go there");
        }

        self.push_history();
        let points = self.points_for(selection.pile, to, count);
        let mut moved = self.take(selection.pile, selection.index);
        self.put(to, &mut moved);

        self.score = match self.scoring {
            Scoring::Standard | Scoring::Off => (self.score + points).max(0),
            Scoring::Vegas => self.score + points,
        };

        if self.reveal(selection.pile) && self.scoring == Scoring::Standard {
            self.score += 5;
        }

        self.moves = self.moves.saturating_add(1);
        self.started = true;
        self.selection = None;
        if self.foundations.iter().map(Vec::len).sum::<usize>() == 52 {
            self.won = true;
        }
        // A move is the board's to tell: the cards have gone where they were
        // sent, and the result is the victory panel's, not a line of text under
        // it.
        None
    }

    fn points_for(&self, from: Pile, to: Pile, count: usize) -> i32 {
        let count = i32::try_from(count).unwrap_or(i32::MAX);
        match self.scoring {
            Scoring::Standard => match (from, to) {
                (Pile::Waste, Pile::Tableau(_)) => 5,
                (Pile::Waste | Pile::Tableau(_), Pile::Foundation(_)) => 10,
                (Pile::Foundation(_), Pile::Tableau(_)) => -15,
                _ => 0,
            },
            Scoring::Vegas => match (from, to) {
                (_, Pile::Foundation(_)) => 5 * count,
                (Pile::Foundation(_), _) => -5 * count,
                _ => 0,
            },
            Scoring::Off => 0,
        }
    }

    fn take(&mut self, pile: Pile, index: usize) -> Vec<u16> {
        match pile {
            Pile::Stock => Vec::new(),
            Pile::Waste => self.waste.split_off(index),
            Pile::Foundation(foundation) => self.foundations[foundation].split_off(index),
            Pile::Tableau(column) => self.tableau[column].cards.split_off(index),
        }
    }

    fn put(&mut self, pile: Pile, cards: &mut Vec<u16>) {
        match pile {
            Pile::Stock => self.stock.append(cards),
            Pile::Waste => self.waste.append(cards),
            Pile::Foundation(foundation) => self.foundations[foundation].append(cards),
            Pile::Tableau(column) => self.tableau[column].cards.append(cards),
        }
    }

    /// Turn over the newly exposed top card of a column, if it is face down.
    fn reveal(&mut self, pile: Pile) -> bool {
        let Pile::Tableau(column) = pile else {
            return false;
        };
        let column = &mut self.tableau[column];
        if !column.cards.is_empty() && column.up == column.cards.len() {
            column.up -= 1;
            true
        } else {
            false
        }
    }

    fn push_history(&mut self) {
        if self.history.len() == HISTORY_LIMIT {
            self.history.remove(0);
        }
        self.history.push(Snapshot {
            stock: self.stock.clone(),
            waste: self.waste.clone(),
            foundations: self.foundations.clone(),
            tableau: self.tableau.clone(),
            score: self.score,
            moves: self.moves,
            elapsed: self.elapsed,
            redeals: self.redeals,
            won: self.won,
        });
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.stock = snapshot.stock;
        self.waste = snapshot.waste;
        self.foundations = snapshot.foundations;
        self.tableau = snapshot.tableau;
        self.score = snapshot.score;
        self.moves = snapshot.moves;
        self.elapsed = snapshot.elapsed;
        self.redeals = snapshot.redeals;
        self.won = snapshot.won;
    }

    fn encode_cards(&self, cards: &[u16]) -> String {
        let mut out = String::with_capacity(cards.len() * 2);
        for &card in cards {
            out.push_str(&self.card(card).code());
        }
        out
    }
}

/// A pile parsed from a stored field, checking every card against the deck.
fn decode_cards(text: &str, cards: &[Card], seen: &mut [bool; 52]) -> Option<Vec<u16>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let mut pile = Vec::with_capacity(text.len() / 2);
    for chunk in text.as_bytes().chunks(2) {
        let code = std::str::from_utf8(chunk).ok()?;
        let card = Card::from_code(code)?;
        let index = cards.iter().position(|known| *known == card)?;
        if seen[index] {
            return None;
        }
        seen[index] = true;
        pile.push(index as u16);
    }
    Some(pile)
}

/// A foundation must be a real home pile: one suit, ascending from the ace.
fn valid_foundation(cards: &[u16], deck: &[Card]) -> bool {
    let Some(&first) = cards.first() else {
        return true;
    };
    let suit = deck[first as usize].suit;
    cards.iter().enumerate().all(|(index, &id)| {
        let card = deck[id as usize];
        card.suit == suit && usize::from(card.rank.value()) == index + 1
    })
}

/// `m:ss`, or `h:mm:ss` once the clock passes an hour.
#[must_use]
pub fn clock_text(seconds: u32) -> String {
    let displayed_seconds = seconds % 60;
    let minutes = (seconds / 60) % 60;
    let hours = seconds / 3_600;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{displayed_seconds:02}")
    } else {
        format!("{minutes}:{displayed_seconds:02}")
    }
}

/// Thousands separators, because a score is read at a glance.
#[must_use]
pub fn grouped(value: i64) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let digits = value.abs().to_string();
    let mut out = String::with_capacity(digits.len() + 2);
    out.push_str(sign);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn money(value: i32) -> String {
    if value < 0 {
        format!("-${}", grouped(i64::from(-value)))
    } else {
        format!("${}", grouped(i64::from(value)))
    }
}

/// A small, deterministic generator. The seed is the deal, so a game can be
/// restarted, saved and shared without storing the deck.
fn shuffle(cards: &mut [u16], seed: u64) {
    let mut state = seed;
    for index in (1..cards.len()).rev() {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        let pick = usize::try_from(z % (index as u64 + 1)).unwrap_or(0);
        cards.swap(index, pick);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::Suit;

    fn id(game: &Game, rank: Rank, suit: Suit) -> u16 {
        let card = Card::new(rank, suit);
        game.cards
            .iter()
            .position(|known| *known == card)
            .expect("the deck holds every card") as u16
    }

    fn empty_game(scoring: Scoring) -> Game {
        let mut game = Game::new(Difficulty::Medium, scoring, 7);
        game.stock.clear();
        game.waste.clear();
        for foundation in &mut game.foundations {
            foundation.clear();
        }
        for column in &mut game.tableau {
            column.cards.clear();
            column.up = 0;
        }
        game
    }

    fn put_column(game: &mut Game, column: usize, rank_suit: &[(Rank, Suit)], up: usize) {
        game.tableau[column].cards = rank_suit
            .iter()
            .map(|&(rank, suit)| id(game, rank, suit))
            .collect();
        game.tableau[column].up = up;
    }

    #[test]
    fn a_new_deal_has_the_standard_shape() {
        let game = Game::new(Difficulty::Medium, Scoring::Standard, 42);
        assert_eq!(game.stock.len(), 24);
        assert!(game.waste.is_empty());
        for (index, column) in game.tableau.iter().enumerate() {
            assert_eq!(column.len(), index + 1);
            assert_eq!(column.up, index);
            assert!(column.is_face_up(index));
        }
        let total: usize = game.stock.len()
            + game.waste.len()
            + game.foundations.iter().map(Vec::len).sum::<usize>()
            + game.tableau.iter().map(Column::len).sum::<usize>();
        assert_eq!(total, 52);
        assert!(!game.started());
        assert_eq!(game.to_stored(), None, "an untouched deal leaves no trace");
    }

    #[test]
    fn the_same_seed_deals_the_same_game_and_another_seed_does_not() {
        let one = Game::new(Difficulty::Medium, Scoring::Standard, 1234);
        let two = Game::new(Difficulty::Medium, Scoring::Standard, 1234);
        let other = Game::new(Difficulty::Medium, Scoring::Standard, 1235);
        let shape = |game: &Game| {
            game.tableau
                .iter()
                .map(|column| column.cards.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(shape(&one), shape(&two));
        assert_ne!(shape(&one), shape(&other));
    }

    #[test]
    fn a_run_of_alternating_colours_moves_as_a_unit() {
        let mut game = empty_game(Scoring::Standard);
        put_column(
            &mut game,
            0,
            &[(Rank::King, Suit::Spades), (Rank::Queen, Suit::Hearts)],
            0,
        );
        put_column(&mut game, 1, &[(Rank::King, Suit::Clubs)], 0);
        assert!(game.tap(Pile::Tableau(0), 1).is_none());
        assert!(game.tap(Pile::Tableau(1), 1).is_none());
        assert_eq!(game.pile_len(Pile::Tableau(1)), 2);
        assert_eq!(game.pile_len(Pile::Tableau(0)), 1);
        // Alternating colours on the way down, and a king that is not on an
        // empty column is refused.
        assert!(!game.accepts(Pile::Tableau(2), Card::new(Rank::Queen, Suit::Clubs)));
        assert!(game.accepts(Pile::Tableau(2), Card::new(Rank::King, Suit::Hearts)));
        assert!(!game.accepts(Pile::Tableau(2), Card::new(Rank::Ace, Suit::Hearts)));
    }

    /// A move is the board's to tell, and the page is not a log.
    ///
    /// The line under the table is for the two things the cards cannot say: a
    /// tap that was refused — nothing moved, so silence would read as a broken
    /// control — and a question the player asked. Everything the game does of
    /// its own accord is quiet, and the winning move is the quietest of all:
    /// the victory panel carries the result, and a sentence under it would be
    /// the same news twice.
    #[test]
    fn a_move_says_nothing_and_only_a_refusal_speaks() {
        let mut game = empty_game(Scoring::Standard);
        put_column(
            &mut game,
            0,
            &[(Rank::King, Suit::Spades), (Rank::Queen, Suit::Hearts)],
            0,
        );
        assert_eq!(game.tap(Pile::Tableau(0), 0), None, "picking a run up");
        assert_eq!(game.tap(Pile::Tableau(1), 0), None, "putting it down");
        assert_eq!(game.pile_len(Pile::Tableau(1)), 2);

        // A tap the board cannot answer for: the queen is in hand, and column
        // one is empty, so it will not take her.
        assert_eq!(game.tap(Pile::Tableau(1), 1), None);
        assert_eq!(
            game.tap(Pile::Tableau(0), 0).as_deref(),
            Some("Can't go there")
        );

        let mut ready = a_ready_board();
        for column in 0..4 {
            assert_eq!(ready.tap(Pile::Tableau(column), 0), None);
            assert_eq!(ready.tap(Pile::Foundation(column), 0), None);
        }
        assert!(ready.won());
        // Four kings home by the test's own taps: the panel gets the numbers,
        // the heading above them says "You won".
        assert_eq!(ready.result_words(), "0:00 · 4 moves · 40 points");
    }

    #[test]
    fn a_foundation_takes_the_ace_then_the_next_card_of_its_suit() {
        let mut game = empty_game(Scoring::Standard);
        let ace = id(&game, Rank::Ace, Suit::Spades);
        let two = id(&game, Rank::Two, Suit::Spades);
        let other_two = id(&game, Rank::Two, Suit::Hearts);
        game.foundations[0].push(ace);
        assert!(game.accepts(Pile::Foundation(0), game.card(two)));
        assert!(!game.accepts(Pile::Foundation(0), game.card(other_two)));
        assert!(!game.accepts(Pile::Foundation(1), game.card(two)));
        assert!(game.accepts(Pile::Foundation(1), game.card(ace)));
    }

    #[test]
    fn an_empty_column_takes_a_king_and_only_a_king() {
        let game = empty_game(Scoring::Standard);
        assert!(game.accepts(Pile::Tableau(0), Card::new(Rank::King, Suit::Clubs)));
        assert!(!game.accepts(Pile::Tableau(0), Card::new(Rank::Queen, Suit::Clubs)));
    }

    #[test]
    fn a_draw_turns_three_and_a_recycle_restores_the_order() {
        let mut game = empty_game(Scoring::Standard);
        game.stock = (0..7).collect();
        let original = game.stock.clone();
        assert!(game.draw().is_none());
        assert_eq!(game.waste.len(), 3);
        assert_eq!(game.stock.len(), 4);
        game.draw();
        game.draw();
        assert!(game.stock.is_empty());
        let mut reversed = original.clone();
        reversed.reverse();
        assert_eq!(game.waste, reversed);
        game.draw();
        assert!(game.waste.is_empty());
        assert_eq!(
            game.stock, original,
            "the recycle restores the original order"
        );
        assert_eq!(game.redeals, 1);
    }

    #[test]
    fn a_hard_deal_runs_out_of_passes() {
        let mut game = empty_game(Scoring::Standard);
        game.difficulty = Difficulty::Hard;
        game.stock = vec![0, 1];
        game.draw();
        assert!(game.stock.is_empty());
        game.draw();
        assert_eq!(game.redeals, 1);
        game.draw();
        assert!(game.can_redeal(), "one pass left, and cards in the waste");
        // Three passes through the stock is the whole of the deal's allowance:
        // the first, and two more.
        for _ in 0..2 {
            game.draw();
        }
        assert_eq!(game.redeals, 2);
        assert!(!game.can_redeal());
        assert_eq!(game.draw().as_deref(), Some("No passes left"));
        assert!(game.stock.is_empty());
    }

    #[test]
    fn vegas_allows_one_pass_whatever_the_difficulty() {
        let mut game = empty_game(Scoring::Vegas);
        game.difficulty = Difficulty::Easy;
        game.stock = vec![0];
        game.draw();
        assert_eq!(game.draw().as_deref(), Some("No passes left"));
        assert_eq!(game.score, -52);
    }

    #[test]
    fn turning_a_card_scores_five_and_the_undo_restores_it() {
        let mut game = empty_game(Scoring::Standard);
        put_column(
            &mut game,
            0,
            &[(Rank::Two, Suit::Hearts), (Rank::Ace, Suit::Spades)],
            1,
        );
        put_column(&mut game, 1, &[(Rank::King, Suit::Clubs)], 0);
        // Move the ace away, exposing the hidden two.
        game.tap(Pile::Tableau(0), 1);
        assert!(game.tap(Pile::Foundation(0), 0).is_none());
        assert_eq!(game.score, 15, "10 home and 5 for the turn");
        assert!(game.tableau[0].is_face_up(0));
        game.undo();
        assert_eq!(game.score, 0);
        assert!(!game.tableau[0].is_face_up(0));
        assert_eq!(game.tableau[0].up, 1);
    }

    #[test]
    fn a_standard_move_off_a_foundation_costs_fifteen() {
        let mut game = empty_game(Scoring::Standard);
        game.foundations[0].push(id(&game, Rank::Ace, Suit::Spades));
        game.tableau[0]
            .cards
            .push(id(&game, Rank::Two, Suit::Hearts));
        game.tableau[0].up = 0;
        game.tap(Pile::Foundation(0), 0);
        game.tap(Pile::Tableau(0), 1);
        assert_eq!(game.score, 0, "15 down, floored at zero");
    }

    #[test]
    fn vegas_starts_at_the_stake_and_pays_for_foundation_cards() {
        let mut game = empty_game(Scoring::Vegas);
        assert_eq!(game.score, -52);
        game.tableau[0]
            .cards
            .push(id(&game, Rank::Ace, Suit::Spades));
        game.tableau[0].up = 0;
        game.tap(Pile::Tableau(0), 0);
        game.tap(Pile::Foundation(0), 0);
        assert_eq!(game.score, -47);
    }

    #[test]
    fn undo_undoes_a_draw_too() {
        let mut game = Game::new(Difficulty::Medium, Scoring::Standard, 9);
        let stock = game.stock.clone();
        game.draw();
        assert_eq!(game.waste.len(), 3);
        game.undo();
        assert_eq!(game.waste.len(), 0);
        assert_eq!(game.stock, stock);
    }

    #[test]
    fn nothing_to_undo_says_so_rather_than_panicking() {
        let mut game = Game::new(Difficulty::Medium, Scoring::Standard, 9);
        assert!(game.undo().is_none());
    }

    #[test]
    fn the_automatic_finish_wins_a_ready_board() {
        let mut game = empty_game(Scoring::Standard);
        let mut remaining = Vec::new();
        for suit in Suit::ALL {
            for rank in Rank::ALL {
                if rank == Rank::King {
                    remaining.push((rank, suit));
                }
            }
        }
        for (foundation, suit) in Suit::ALL.into_iter().enumerate() {
            for rank in Rank::ALL {
                if rank != Rank::King {
                    game.foundations[foundation].push(id(&game, rank, suit));
                }
            }
        }
        for (column, &(rank, suit)) in remaining.iter().enumerate() {
            put_column(&mut game, column, &[(rank, suit)], 0);
        }
        assert!(game.can_autocomplete());
        for _ in 0..4 {
            assert!(game.autocomplete_step());
        }
        assert!(game.won());
        assert!(game.result_words().contains("moves"));
        assert!(!game.autocomplete_step());
    }

    /// A board with every card but the four kings home, and the kings on their
    /// own columns: the finish is the whole of what is left.
    fn a_ready_board() -> Game {
        let mut game = empty_game(Scoring::Standard);
        for (foundation, suit) in Suit::ALL.into_iter().enumerate() {
            for rank in Rank::ALL {
                if rank != Rank::King {
                    game.foundations[foundation].push(id(&game, rank, suit));
                }
            }
            put_column(&mut game, foundation, &[(Rank::King, suit)], 0);
        }
        game
    }

    #[test]
    fn a_board_every_card_of_which_is_uncovered_finishes_itself() {
        let game = a_ready_board();
        assert!(game.can_autocomplete());
        assert!(
            game.finishing(),
            "the finish has to want to run without being asked for"
        );
    }

    #[test]
    fn taking_a_card_back_out_of_the_finish_stops_it() {
        let mut game = a_ready_board();
        assert!(game.finishing());
        let _ = game.autocomplete_step();
        assert!(!game.won());
        let _ = game.undo();
        // The board is ready again, and that is exactly the problem: a finish
        // that ignored the undo would put the card straight back, for ever.
        assert!(game.can_autocomplete());
        assert!(!game.finishing());
        assert!(game.finish_held());
        game.resume_finish();
        assert!(game.finishing());
    }

    #[test]
    fn a_finish_with_nothing_to_send_home_does_nothing_at_all() {
        // Two cards that cannot go home and cannot be moved: every card is
        // uncovered, so the board *looks* like a formality and is not one. The
        // step must do nothing, and must not spend the player's deal by
        // declaring it over: a card uncovered by hand can make the next one
        // reachable.
        let mut game = empty_game(Scoring::Standard);
        put_column(
            &mut game,
            0,
            &[(Rank::Six, Suit::Hearts), (Rank::Five, Suit::Spades)],
            0,
        );
        assert!(game.can_autocomplete());
        assert!(game.finishing());
        assert!(!game.autocomplete_step());
        assert!(!game.won());
        assert_eq!(game.moves(), 0, "a step that moves nothing is not a move");
        assert!(!game.finish_held(), "the deal is the player's, not held");
    }

    /// A hint with something to show says nothing: the ring is the message.
    /// Only a hint with nothing to show speaks, and it says one word, without
    /// a stop.
    #[test]
    fn a_hint_speaks_only_when_there_is_nothing_to_ring() {
        assert_eq!(
            Hint::Move {
                from: Pile::Waste,
                index: 0,
                to: Pile::Tableau(0),
                count: 1,
            }
            .words(),
            None
        );
        assert_eq!(Hint::Draw.words(), None);
        assert_eq!(Hint::Recycle.words(), None);
        assert_eq!(Hint::None.words(), Some("Stuck"));
        assert_eq!(WORDS_STUCK, "Stuck");
        assert_eq!(WORDS_NOT_STUCK, "Not stuck");
        for word in [WORDS_STUCK, WORDS_NOT_STUCK] {
            assert!(
                !word.ends_with('.'),
                "a fragment does not end in a stop: {word}"
            );
        }
    }

    /// The check answers a question about the board, not about a deal it could
    /// become: a move that changes something is enough, and a deal whose only
    /// legal move puts back what another took out is stuck.
    #[test]
    fn stuck_means_no_move_left_but_a_shuffle() {
        // A fresh deal has the stock to turn, which always changes something.
        let fresh = Game::dealt(Difficulty::Easy, Scoring::Standard, 11);
        assert!(!fresh.stuck());

        // Two black cards on their own, with nothing to turn and nothing that
        // will take either of them: the only moves left are kings shuffling
        // between empty columns, and those are the same board renamed.
        let mut dead = empty_game(Scoring::Standard);
        put_column(&mut dead, 0, &[(Rank::King, Suit::Spades)], 0);
        put_column(&mut dead, 1, &[(Rank::Queen, Suit::Spades)], 0);
        assert!(dead.stuck());
        assert!(matches!(dead.hint(), Hint::None));

        // The same board with one card that can go home is not stuck.
        let mut playing = empty_game(Scoring::Standard);
        put_column(&mut playing, 0, &[(Rank::King, Suit::Spades)], 0);
        put_column(&mut playing, 1, &[(Rank::Ace, Suit::Hearts)], 0);
        assert!(!playing.stuck());

        // Nor is one where the only move is a run onto another column that
        // turns a card over: that is a change, even with the stock empty.
        let mut revealing = empty_game(Scoring::Standard);
        put_column(
            &mut revealing,
            0,
            &[(Rank::Nine, Suit::Spades), (Rank::Four, Suit::Hearts)],
            1,
        );
        put_column(&mut revealing, 1, &[(Rank::Five, Suit::Clubs)], 0);
        assert!(!revealing.stuck());
    }

    #[test]
    fn a_stored_game_round_trips_and_keeps_its_seed() {
        let mut game = Game::dealt(Difficulty::Hard, Scoring::Vegas, 77);
        game.draw();
        game.tap(Pile::Tableau(2), 2);
        let stored = game.to_stored().expect("a played game is stored");
        let restored = Game::from_stored(Some(&stored)).expect("the saved game parses");
        assert_eq!(restored.difficulty(), Difficulty::Hard);
        assert_eq!(restored.scoring(), Scoring::Vegas);
        assert_eq!(restored.seed(), 77);
        assert_eq!(restored.stock, game.stock);
        assert_eq!(restored.waste, game.waste);
        assert_eq!(restored.score, game.score);
        assert_eq!(restored.moves, game.moves);
        assert_eq!(restored.to_stored().as_deref(), Some(stored.as_str()));
    }

    #[test]
    fn corrupt_or_incomplete_saves_are_refused() {
        let game = Game::dealt(Difficulty::Medium, Scoring::Standard, 5);
        let stored = game.to_stored().expect("stored");
        for bad in [
            String::new(),
            "v1".to_owned(),
            stored.replace("v1|", "v2|"),
            stored.replace("|medium|", "|nonsense|"),
            stored.replace("v1|medium|standard|5|", "v1|medium|standard|x|"),
            // A duplicate card: the stock's first card is replaced by one that
            // is already in a tableau column.
            {
                let mut fields: Vec<String> = stored.split('|').map(str::to_owned).collect();
                let tableau_card = fields[16]
                    .split_once(':')
                    .map(|(_, cards)| cards[..2].to_owned())
                    .unwrap_or_default();
                fields[10] = format!("{tableau_card}{}", &fields[10][2..]);
                fields.join("|")
            },
            // A dropped card: one shorter stock.
            {
                let mut fields: Vec<String> = stored.split('|').map(str::to_owned).collect();
                fields[10] = fields[10].chars().skip(2).collect();
                fields.join("|")
            },
        ] {
            assert!(
                Game::from_stored(Some(&bad)).is_none(),
                "accepted a corrupt save: {bad}"
            );
        }
        assert!(Game::from_stored(None).is_none());
    }

    #[test]
    fn the_hint_prefers_the_move_that_turns_a_card_over() {
        let mut game = empty_game(Scoring::Standard);
        put_column(
            &mut game,
            0,
            &[(Rank::Nine, Suit::Spades), (Rank::Eight, Suit::Hearts)],
            1,
        );
        put_column(&mut game, 1, &[(Rank::Nine, Suit::Clubs)], 0);
        put_column(&mut game, 2, &[(Rank::Seven, Suit::Spades)], 0);
        game.stock = vec![0];
        let hint = game.hint();
        match hint {
            Hint::Move { from, to, .. } => {
                assert_eq!(from, Pile::Tableau(0));
                assert_eq!(to, Pile::Tableau(1));
            }
            other => panic!("expected a revealing move, got {other:?}"),
        }
    }

    /// The stock is suggested only when turning it can pay, and the whole
    /// stock is walked to decide — not just the next card.
    ///
    /// The orientation matters and is the game's own: a draw pops cards off the
    /// top of the stock, so in draw-three the *last* card of the vector is
    /// drawn first and the third one is the card left showing. A king at the
    /// bottom of a three-card stock is therefore on show after the first turn,
    /// and a king in the middle of it is never on show at all.
    #[test]
    fn the_hint_only_suggests_the_stock_when_a_turn_exposes_a_play() {
        let mut game = empty_game(Scoring::Standard);
        // Three cards, draw three, and the king is the one left showing. An
        // empty column takes it, so turning the stock pays.
        game.stock = vec![12, 3, 2];
        assert_eq!(game.hint(), Hint::Draw);
        assert!(!game.stuck());

        // The same stock without the king: three cards nothing will take,
        // walked round through the waste and back to where they started.
        game.stock = vec![5, 3, 2];
        assert_eq!(game.hint(), Hint::None);
        assert!(game.stuck());

        // Draw one reaches every card in the stock, so the same two cards are
        // two different answers under the two deals.
        game.difficulty = Difficulty::Easy;
        game.stock = vec![5, 12];
        assert_eq!(game.hint(), Hint::Draw);
        game.difficulty = Difficulty::Medium;
        game.stock = vec![12, 5];
        assert_eq!(
            game.hint(),
            Hint::Draw,
            "with two cards the first of them is left showing after one turn"
        );
        game.stock = vec![5, 12];
        assert_eq!(
            game.hint(),
            Hint::None,
            "the king is drawn first and buried under the six"
        );

        // The waste, likewise, only when turning it back over can expose a
        // card that plays — the recycle is the same walk by another name.
        game.difficulty = Difficulty::Easy;
        game.stock.clear();
        // An ace under a card nothing wants: the top is dead, and the recycle
        // is what turns it up.
        game.waste = vec![0, 5];
        assert_eq!(game.hint(), Hint::Recycle);
        game.waste = vec![5, 3];
        assert_eq!(game.hint(), Hint::None);
        game.waste.clear();
        assert_eq!(game.hint(), Hint::None);
    }

    /// The stock is named only when a turn the player can actually take brings
    /// a card that plays — and the passes already spent do not count.
    ///
    /// This is the one way the answer can be wrong, and it was: the walk used
    /// the deal's whole allowance instead of what was left, so on a Hard deal
    /// with three passes gone it found a card two passes away that the player
    /// could never reach. The board then said "not stuck" and rang the stock
    /// for as long as the player kept pressing, on a deal that was dead.
    ///
    /// The test is the promise itself, on the board's own terms: whenever the
    /// hint names the stock, a copy of the game that follows it — drawing and
    /// recycling exactly as the player would — must arrive at a hinted move
    /// before the stock is spent.
    #[test]
    fn a_stock_hint_always_leads_to_a_move() {
        for difficulty in [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard] {
            for seed in 0..60u64 {
                let mut game = Game::dealt(difficulty, Scoring::Standard, seed);
                let mut noise = seed.wrapping_mul(6_364_136_223_846_793_005);
                for _ in 0..40 {
                    if game.won() {
                        break;
                    }
                    match game.hint() {
                        Hint::Draw | Hint::Recycle => {
                            assert!(
                                draws_until_move(&game),
                                "{difficulty:?} seed {seed}: the hint names the stock, but \
                                 following it never reaches a move (passes {}/{})\n{}",
                                game.redeals,
                                game.redeal_limit()
                                    .map_or_else(|| "unlimited".to_owned(), |max| max.to_string()),
                                game.to_stored().unwrap_or_default()
                            );
                            game.draw();
                        }
                        _ => {
                            // Wander, so the board is not only where the hint
                            // would have led. Random taps need no legality
                            // check: the game refuses the ones that are not
                            // moves, and the board moves on either way.
                            noise ^= noise << 13;
                            noise ^= noise >> 7;
                            noise ^= noise << 17;
                            let pile = Pile::ALL[(noise % 12) as usize % Pile::ALL.len()];
                            let len = game.pile_len(pile);
                            let index = (noise >> 8) as usize % (len + 1);
                            if (noise >> 20) % 6 == 0 {
                                game.draw();
                            } else if let Some(held) = game.selection() {
                                let to = Pile::ALL[((noise >> 12) as usize) % Pile::ALL.len()];
                                let _ = held;
                                game.drop(to);
                            } else {
                                game.tap(pile, index);
                            }
                        }
                    }
                }
            }
        }
    }

    /// How many stock turns it takes to reach a hinted move, or `None` if the
    /// stock is spent first. The moves are the game's own, on a copy of it.
    fn draws_until_move(game: &Game) -> bool {
        let mut probe = game.clone();
        for _ in 0..60 {
            match probe.hint() {
                Hint::Move { .. } => return true,
                Hint::Draw | Hint::Recycle => {
                    let before = (probe.stock.clone(), probe.waste.clone());
                    probe.draw();
                    if (probe.stock.clone(), probe.waste.clone()) == before {
                        return false;
                    }
                }
                Hint::None => return false,
            }
        }
        false
    }

    /// Only a *move* is ringed. A stock hint is one press on one pile, and the
    /// keyboard goes there instead: a ring around a face-down stack says "this
    /// one" about cards that are not the thing to press, and the empty stock a
    /// recycle needs has no card to circle at all.
    #[test]
    fn a_hint_rings_a_move_and_nothing_else() {
        let mut game = empty_game(Scoring::Standard);
        // A run that can go onto another column: the one hint that is a move,
        // and therefore the one hint with a ring.
        put_column(
            &mut game,
            0,
            &[(Rank::Nine, Suit::Spades), (Rank::Eight, Suit::Hearts)],
            1,
        );
        put_column(&mut game, 1, &[(Rank::Nine, Suit::Clubs)], 0);
        let hint = game.hint();
        assert!(matches!(hint, Hint::Move { .. }), "{hint:?}");
        assert_eq!(
            crate::scene::scene(&game, Some(&hint)).hint,
            Some((Pile::Tableau(0).place(), 1, 1))
        );

        // A stock whose turn pays: no ring, because the press is the answer.
        let mut game = empty_game(Scoring::Standard);
        game.stock = vec![12, 3, 2];
        assert_eq!(game.hint(), Hint::Draw);
        assert_eq!(crate::scene::scene(&game, Some(&Hint::Draw)).hint, None);

        let mut game = empty_game(Scoring::Standard);
        game.difficulty = Difficulty::Easy;
        game.waste = vec![0, 5];
        assert_eq!(game.hint(), Hint::Recycle);
        assert_eq!(crate::scene::scene(&game, Some(&Hint::Recycle)).hint, None);
    }

    /// A stock cycle the turns cannot change is walked once and then
    /// recognised, which is what stops the search from going round for ever on
    /// a deal it cannot change.
    ///
    /// The recycle is what makes this exact rather than a guess: with nothing
    /// played between turns, a run through the stock leaves the waste as the
    /// stock exactly reversed, and turning the waste back over restores the
    /// stock to the position it started in. So a pass that exposes nothing
    /// playable is the whole of the answer — more passes are the same pass,
    /// whatever the deal allows — and in draw-three two thirds of the cards
    /// may never be on show at all.
    #[test]
    fn a_stock_the_turns_cannot_change_is_not_worth_turning() {
        let mut game = empty_game(Scoring::Standard);
        game.stock = vec![2, 12, 3];
        assert!(game.waste.is_empty());
        // The king is in the stock and can never be shown: the card left
        // showing is the first of the vector, every pass.
        assert_eq!(game.hint(), Hint::None);
        assert!(game.stuck());
    }

    #[test]
    fn options_round_trip_and_fall_back() {
        let options = Options {
            difficulty: Difficulty::Hard,
            scoring: Scoring::Off,
        };
        let stored = options.to_stored();
        assert_eq!(Options::from_stored(Some(&stored)), options);
        assert_eq!(Options::from_stored(Some("nonsense")), Options::default());
        assert_eq!(Options::from_stored(None), Options::default());
    }

    #[test]
    fn the_clock_and_the_score_are_formatted_for_reading() {
        assert_eq!(clock_text(0), "0:00");
        assert_eq!(clock_text(61), "1:01");
        assert_eq!(clock_text(3_600), "1:00:00");
        assert_eq!(grouped(1_204), "1,204");
        assert_eq!(grouped(-52), "-52");
        assert_eq!(money(-52), "-$52");
        assert_eq!(money(208), "$208");
    }

    /// Every action the rules allow, checked from the outside: the deck is
    /// conserved, no card is in two piles, and the score never goes below what
    /// the style allows.
    fn invariants(game: &Game) {
        let mut seen = vec![0u8; game.deck_len()];
        for pile in Pile::ALL {
            for &id in game.pile(pile) {
                seen[id as usize] += 1;
            }
        }
        assert!(
            seen.iter().all(|count| *count == 1),
            "a card was lost or duplicated: {seen:?}"
        );
        match game.scoring() {
            Scoring::Standard | Scoring::Off => assert!(game.score() >= 0, "{}", game.score()),
            Scoring::Vegas => assert!(game.score() >= -52 - 52 * 8),
        }
        for column in 0..COLUMNS {
            let pile = Pile::Tableau(column);
            let len = game.pile_len(pile);
            for index in 0..len {
                if !game.is_face_down(pile, index) {
                    assert!(
                        game.run_len(pile, index).is_some(),
                        "face-up card {index} in column {column} is not part of a run"
                    );
                    break;
                }
            }
        }
    }

    /// A double tap sends a card home and does nothing else.
    ///
    /// This is a deliberate narrowing. It used to fall back to "the best
    /// column", which meant a quick second tap could send a card somewhere the
    /// player never chose — and on a touch screen a quick second tap is what a
    /// player does when they are not sure the first one registered.
    #[test]
    fn a_double_tap_only_ever_sends_a_card_home() {
        let mut game = empty_game(Scoring::Standard);
        // A two with an empty foundation and an empty column: it can go on the
        // column, and it must not.
        game.tableau[0]
            .cards
            .push(id(&game, Rank::Two, Suit::Hearts));
        game.tableau[0].up = 0;
        assert_eq!(
            game.double_tap(Pile::Tableau(0), 0).as_deref(),
            Some("Nowhere to go"),
            "a double tap that sends nothing nowhere says why it did nothing"
        );
        assert_eq!(game.pile_len(Pile::Tableau(0)), 1, "the card stayed put");
        assert_eq!(game.pile_len(Pile::Tableau(1)), 0, "and did not jump");
        assert_eq!(game.selection(), None, "and nothing is left held");

        // An ace goes home.
        game.tableau[2]
            .cards
            .push(id(&game, Rank::Ace, Suit::Clubs));
        game.tableau[2].up = 0;
        assert!(
            game.double_tap(Pile::Tableau(2), 0).is_none(),
            "a card that goes home is the board's to report"
        );
        assert_eq!(game.pile_len(Pile::Tableau(2)), 0);
        assert_eq!(game.pile_len(Pile::Foundation(0)), 1);
    }

    /// A run is not a double-tap target: a double tap on the middle of a run
    /// must not split it.
    #[test]
    fn a_double_tap_leaves_a_run_alone() {
        let mut game = empty_game(Scoring::Standard);
        game.tableau[0]
            .cards
            .push(id(&game, Rank::Two, Suit::Hearts));
        game.tableau[0]
            .cards
            .push(id(&game, Rank::Ace, Suit::Spades));
        game.tableau[0].up = 0;
        assert_eq!(
            game.double_tap(Pile::Tableau(0), 0).as_deref(),
            Some("Part of a run")
        );
        assert_eq!(game.pile_len(Pile::Tableau(0)), 2);
    }

    /// A play-through of random legal actions on many deals.
    ///
    /// This is the test that catches a rule that is wrong in a way a hand-built
    /// board cannot show: it walks the whole grammar — draws, redeals, single
    /// cards, runs, aces home, cards off a foundation, undos — and checks the
    /// board's own invariants after every one.
    #[test]
    fn random_play_keeps_the_deck_conserved() {
        for seed in 0..40u64 {
            for (difficulty, scoring) in [
                (Difficulty::Easy, Scoring::Standard),
                (Difficulty::Medium, Scoring::Standard),
                (Difficulty::Hard, Scoring::Vegas),
                (Difficulty::Medium, Scoring::Off),
            ] {
                let mut game = Game::dealt(difficulty, scoring, seed);
                invariants(&game);
                let mut state = seed.wrapping_mul(6_364_136_223_846_793_005);
                for _ in 0..400 {
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    let pick = usize::try_from(state >> 33).unwrap_or(0);
                    match pick % 12 {
                        0 => {
                            game.draw();
                        }
                        1 => {
                            game.undo();
                        }
                        2 => {
                            if let Some(index) = game.waste.len().checked_sub(1) {
                                game.double_tap(Pile::Waste, index);
                            }
                        }
                        _ => {
                            let column = (pick / 12) % COLUMNS;
                            let pile = Pile::Tableau(column);
                            let len = game.pile_len(pile);
                            if len == 0 {
                                continue;
                            }
                            let index = (pick / 7) % len;
                            if game.run_len(pile, index).is_none() {
                                continue;
                            }
                            game.tap(pile, index);
                            if pick % 3 == 0 {
                                game.double_tap(pile, index);
                            } else {
                                let target = (pick / 5) % (COLUMNS + FOUNDATIONS);
                                let destination = if target < COLUMNS {
                                    Pile::Tableau(target)
                                } else {
                                    Pile::Foundation(target - COLUMNS)
                                };
                                game.tap(destination, game.pile_len(destination));
                            }
                        }
                    }
                    invariants(&game);
                    if game.won() {
                        break;
                    }
                }
            }
        }
    }

    /// A hint that says "do this" must actually be doable, and one that says
    /// "nothing to do" must be right about it.
    #[test]
    fn every_hint_is_a_move_that_works() {
        for seed in 0..25u64 {
            let mut game = Game::dealt(Difficulty::Medium, Scoring::Standard, seed);
            for _ in 0..30 {
                match game.hint() {
                    Hint::Move {
                        from, index, to, ..
                    } => {
                        let before = game.pile_len(from);
                        let count = game.run_len(from, index).expect("a hinted run is a run");
                        game.tap(from, index);
                        game.tap(to, game.pile_len(to));
                        assert_eq!(
                            game.pile_len(from),
                            before - count,
                            "the hinted move did not happen"
                        );
                    }
                    Hint::Draw => {
                        assert!(!game.stock.is_empty(), "a draw hint with an empty stock");
                        game.draw();
                    }
                    Hint::Recycle => {
                        assert!(game.can_redeal(), "a recycle hint with no passes left");
                        game.draw();
                    }
                    Hint::None => {
                        assert!(
                            !game.turning_pays(),
                            "a stock turn that pays was not suggested"
                        );
                        break;
                    }
                }
            }
        }
    }

    /// A hint that is followed must move the deal on, not move a card back and
    /// forth between two columns for as long as the player keeps pressing it.
    ///
    /// This is the regression the scoring rewrite exists for: a move between
    /// two columns is legal in both directions whenever both end in a matching
    /// rank and colour, so "is this legal" is not the question — "does anything
    /// about the deal change" is. Draws are excluded because turning the stock
    /// is allowed to reach a position it has been in before; a *move* is not.
    #[test]
    fn following_the_hint_never_walks_in_circles() {
        for seed in 0..25u64 {
            let mut game = Game::dealt(Difficulty::Medium, Scoring::Standard, seed);
            let mut seen: Vec<String> = Vec::new();
            for _ in 0..40 {
                let hint = game.hint();
                match &hint {
                    Hint::Move {
                        from, index, to, ..
                    } => {
                        let before = position(&game);
                        let _ = game.select(*from, *index);
                        let _ = game.drop(*to);
                        assert_ne!(
                            before,
                            position(&game),
                            "seed {seed}: the hint asked for a move that changed nothing"
                        );
                        let after = position(&game);
                        assert!(
                            !seen.contains(&after),
                            "seed {seed}: the hint walked back into a position it had \
                             already reached — {from:?}#{index} to {to:?}"
                        );
                        seen.push(after);
                    }
                    Hint::Draw | Hint::Recycle => {
                        // The stock is new information, so positions reached
                        // before it may legitimately be reached again.
                        seen.clear();
                        let _ = game.draw();
                    }
                    Hint::None => break,
                }
            }
        }
    }

    /// Every pile, its cards and which of them are face down: the deal's
    /// position, without the counters around it.
    fn position(game: &Game) -> String {
        let mut out = String::new();
        for pile in Pile::ALL {
            out.push_str(pile.slug().as_str());
            out.push(':');
            for (index, &card) in game.pile(pile).iter().enumerate() {
                out.push_str(&card.to_string());
                if game.is_face_down(pile, index) {
                    out.push('v');
                }
                out.push(',');
            }
            out.push('|');
        }
        out
    }

    #[test]
    fn the_clock_grows_an_hour_field_only_when_it_needs_one() {
        assert_eq!(clock_text(3_599), "59:59");
        assert_eq!(clock_text(3_600), "1:00:00");
    }
}

#[cfg(test)]
mod place_tests {
    use super::*;
    use crate::board::Place;

    #[test]
    fn every_pile_round_trips_through_its_board_place() {
        for pile in Pile::ALL {
            assert_eq!(Pile::from_place(pile.place()), Some(pile), "{pile:?}");
        }
    }

    #[test]
    fn a_place_this_game_does_not_have_is_refused() {
        assert_eq!(Pile::from_place(Place::Top(2 + FOUNDATIONS)), None);
        assert_eq!(Pile::from_place(Place::Column(COLUMNS)), None);
        assert_eq!(Pile::from_place(Place::Top(99)), None);
    }

    #[test]
    fn the_top_row_is_read_left_to_right_and_the_columns_after_it() {
        assert_eq!(Pile::Stock.place(), Place::Top(0));
        assert_eq!(Pile::Waste.place(), Place::Top(1));
        assert_eq!(Pile::Foundation(0).place(), Place::Top(2));
        assert_eq!(Pile::Tableau(0).place(), Place::Column(0));
        // The board's reading order is the top row then the columns, which is
        // what the keyboard walks.
        let mut places: Vec<Place> = Pile::ALL.iter().map(|pile| pile.place()).collect();
        places.sort_unstable();
        assert_eq!(places.first(), Some(&Place::Top(0)));
        assert_eq!(places.last(), Some(&Place::Column(COLUMNS - 1)));
    }
}
