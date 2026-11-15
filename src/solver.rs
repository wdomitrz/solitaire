//! Whether this deal can still be won, by walking the positions it can reach.
//!
//! "Stuck" is not a question about the next move, and it is not the hint's
//! question either. A deal is stuck when **no** sequence of legal moves from
//! here reaches a win — so the answer is what a search over positions says, and
//! there are three of them:
//!
//! * [`Verdict::Winnable`] — the search reached a won position.
//! * [`Verdict::Dead`] — every position reachable from here was visited, and
//!   none of them wins. The transposition table is what makes this honest: a
//!   deal whose moves only lead back to positions already seen is *stuck*, not
//!   "still being explored", and the table is what says so.
//! * [`Verdict::Unclear`] — the budget ran out first. Saying so is the point:
//!   a false "you cannot win this" would be worse than no answer at all.
//!
//! Nothing here knows about the score, the undo history or the page. It is
//! Klondike's rules in a form a machine can run: a compact, copyable copy of
//! the board, the moves it allows, and a depth-first walk with a visited set.
//!
//! The rules are written twice in this crate — once in [`crate::game`] for the
//! player, once here for the search — because the search copies a position tens
//! of thousands of times and a `Game` owns a dozen vectors it would drag along.
//! Two tests keep the copies from drifting apart, and they are what a `Dead`
//! verdict rests on: `the_search_plays_the_same_moves_as_the_game` plays every
//! move the search generates on a real [`Game`] and requires the two boards to
//! agree, and `the_search_offers_every_move_the_game_allows` walks every move
//! the *game* allows and requires the search to offer it. A search that quietly
//! skipped a move would call a winnable deal stuck.
//!
//! No `web-sys`, no `js-sys`: this compiles for the host and its tests run under
//! plain `cargo test`.

use crate::card::{Card, Suit};
use crate::game::{Game, Pile, COLUMNS, FOUNDATIONS};

/// The most cards one column can hold: thirteen descending face-up cards, and
/// the six the deal can put face down under them.
const MAX_COLUMN: usize = 19;

/// The most cards the stock — and, separately, the waste — can hold: 52 less
/// the 28 the deal lays out.
const MAX_PILE: usize = 24;

/// The most cards a deck has.
const DECK: u32 = 52;

/// How deep the walk may go before its own answer stops being trustworthy.
///
/// This is a valve on memory, not a rule of the game: a line this long is not
/// one a player would follow. Reaching it sets `cut`, which turns a would-be
/// "stuck" into "unclear" — an answer that is merely unhelpful rather than
/// wrong.
const MAX_DEPTH: usize = 2_048;

/// What a search concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// A winning line exists from the position the search started at.
    Winnable,
    /// Every reachable position was examined and none of them wins.
    Dead,
    /// Neither was established before the budget ran out.
    Unclear,
}

/// A card packed into one byte: suit × 13 + rank − 1.
///
/// The board is copied once per position examined, so a card has to be a byte
/// rather than the `Card` the rest of the app carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Slot(u8);

impl Slot {
    fn of(card: Card) -> Self {
        Self(suit_index(card.suit) * 13 + card.rank.value() - 1)
    }

    fn suit(self) -> usize {
        usize::from(self.0 / 13)
    }

    fn rank(self) -> u8 {
        self.0 % 13 + 1
    }

    fn is_red(self) -> bool {
        self.suit() == 1 || self.suit() == 2
    }
}

/// A suit's index in the packed form: the order of [`Suit::ALL`], so that red
/// is 1 and 2 as well as being a property of the suit itself.
const fn suit_index(suit: Suit) -> u8 {
    match suit {
        Suit::Clubs => 0,
        Suit::Diamonds => 1,
        Suit::Hearts => 2,
        Suit::Spades => 3,
    }
}

/// The rules a deal is played by, as far as winnability can tell: how many
/// cards one turn of the stock turns over, and how many times the waste may go
/// back.
#[derive(Debug, Clone, Copy)]
struct Rules {
    draw: u8,
    /// `None` is unlimited, which is also why it matters to the search: an
    /// unlimited deal's redeal count is not part of its position, or the walk
    /// would never come back to a place it had already been.
    redeal_limit: Option<u32>,
}

/// Where a card came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Waste,
    Column(u8),
}

/// One move the search will try.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Move {
    /// The top card of a pile goes home to its foundation.
    Home { from: Source },
    /// The run beginning at `index` moves onto column `to`.
    ToColumn { from: Source, index: u8, to: u8 },
    /// The top card of a foundation comes back onto column `to`.
    OffFoundation { suit: u8, to: u8 },
    /// Turn the stock over, or turn the waste back when the stock is empty.
    Turn,
}

/// A Klondike position, small enough to copy.
#[derive(Debug, Clone, Copy)]
struct Position {
    col: [[Slot; MAX_COLUMN]; COLUMNS],
    len: [u8; COLUMNS],
    /// How many cards at the bottom of each column are face down.
    down: [u8; COLUMNS],
    stock: [Slot; MAX_PILE],
    stock_len: u8,
    waste: [Slot; MAX_PILE],
    waste_len: u8,
    /// How far up each suit is home: 0 is nothing, 13 is the whole suit. The
    /// four foundations are this, in suit order — which pile a suit is home in
    /// does not matter to the game or to winnability, and forgetting it is what
    /// makes two boards that only differ by it the same position.
    home: [u8; FOUNDATIONS],
    /// How many times the waste has been turned back, for a deal that counts.
    redeals: u16,
    rules: Rules,
}

impl Position {
    /// The board a game is in, or `None` for a game whose piles are larger than
    /// the game's own rules allow — which a validated save cannot produce, and
    /// which this refuses rather than searching a truncated board.
    fn of(game: &Game) -> Option<Self> {
        let draw = game.difficulty().draw();
        if draw == 0 || draw > MAX_PILE {
            return None;
        }
        let rules = Rules {
            draw: draw as u8,
            redeal_limit: game.redeal_limit(),
        };
        let mut pos = Self {
            col: [[Slot(0); MAX_COLUMN]; COLUMNS],
            len: [0; COLUMNS],
            down: [0; COLUMNS],
            stock: [Slot(0); MAX_PILE],
            stock_len: 0,
            waste: [Slot(0); MAX_PILE],
            waste_len: 0,
            home: [0; FOUNDATIONS],
            redeals: 0,
            rules,
        };
        for column in 0..COLUMNS {
            let pile = Pile::Tableau(column);
            let cards = game.pile(pile);
            if cards.len() > MAX_COLUMN {
                return None;
            }
            for (index, &card) in cards.iter().enumerate() {
                pos.col[column][index] = Slot::of(game.card(card));
            }
            // The face-down cards are the bottom of the column, so the first
            // face-up one is the boundary the game calls `up`.
            let down = (0..cards.len())
                .find(|&index| !game.is_face_down(pile, index))
                .unwrap_or(cards.len());
            #[allow(clippy::cast_possible_truncation)]
            {
                pos.len[column] = cards.len() as u8;
                pos.down[column] = down as u8;
            }
        }
        for (index, &card) in game.pile(Pile::Stock).iter().enumerate() {
            if index >= MAX_PILE {
                return None;
            }
            pos.stock[index] = Slot::of(game.card(card));
        }
        #[allow(clippy::cast_possible_truncation)]
        {
            pos.stock_len = game.pile(Pile::Stock).len() as u8;
        }
        for (index, &card) in game.pile(Pile::Waste).iter().enumerate() {
            if index >= MAX_PILE {
                return None;
            }
            pos.waste[index] = Slot::of(game.card(card));
        }
        #[allow(clippy::cast_possible_truncation)]
        {
            pos.waste_len = game.pile(Pile::Waste).len() as u8;
        }
        for foundation in 0..FOUNDATIONS {
            let cards = game.pile(Pile::Foundation(foundation));
            let Some(&top) = cards.last() else {
                continue;
            };
            let suit = Slot::of(game.card(top)).suit();
            #[allow(clippy::cast_possible_truncation)]
            {
                pos.home[suit] = cards.len() as u8;
            }
        }
        #[allow(clippy::cast_possible_truncation)]
        {
            pos.redeals = game.redeals_used().min(u32::from(u16::MAX)) as u16;
        }
        Some(pos)
    }

    /// True when every card is home.
    fn won(&self) -> bool {
        self.home.iter().map(|&rank| u32::from(rank)).sum::<u32>() == DECK
    }

    #[allow(clippy::cast_possible_truncation)]
    fn top(&self, column: usize) -> Slot {
        self.col[column][self.len[column] as usize - 1]
    }

    fn last(&self, pile: &[Slot], len: u8) -> Option<Slot> {
        let index = usize::from(len).checked_sub(1)?;
        pile.get(index).copied()
    }

    /// Would `card` sit on column `to`?
    ///
    /// Empty columns are interchangeable, so only the first one is ever a
    /// destination: a move that lands on "an empty column" is one move, not one
    /// move per empty column.
    fn takes(&self, card: Slot, to: usize) -> bool {
        if self.len[to] == 0 {
            return card.rank() == 13 && self.first_empty() == Some(to);
        }
        let top = self.top(to);
        top.rank() == card.rank() + 1 && top.is_red() != card.is_red()
    }

    fn first_empty(&self) -> Option<usize> {
        (0..COLUMNS).find(|&column| self.len[column] == 0)
    }

    /// The lowest index in a column whose run to the top is a descending
    /// alternating sequence — the same question [`Game::run_len`] answers, for
    /// every run start in one pass down the column.
    fn first_run(&self, column: usize) -> usize {
        let len = usize::from(self.len[column]);
        let down = usize::from(self.down[column]);
        if len <= down {
            return len;
        }
        let mut start = len - 1;
        while start > down {
            let upper = self.col[column][start - 1];
            let lower = self.col[column][start];
            if lower.rank() + 1 != upper.rank() || lower.is_red() == upper.is_red() {
                break;
            }
            start -= 1;
        }
        start
    }

    /// Does the top of this suit's foundation go up one more rank?
    fn takes_home(&self, card: Slot) -> bool {
        u16::from(self.home[card.suit()]) + 1 == u16::from(card.rank())
    }

    fn may_redeal(&self) -> bool {
        self.rules
            .redeal_limit
            .is_none_or(|limit| u32::from(self.redeals) < limit)
    }

    /// Every move this position allows, in the order the walk should try them:
    /// home first, then free a card, then the shuffles.
    #[allow(clippy::cast_possible_truncation)]
    fn moves(&self, out: &mut Vec<Move>) {
        out.clear();

        // 1. Home, from the waste and from the tops of the columns.
        if let Some(card) = self.last(&self.waste, self.waste_len) {
            if self.takes_home(card) {
                out.push(Move::Home {
                    from: Source::Waste,
                });
            }
        }
        for column in 0..COLUMNS {
            if usize::from(self.len[column]) <= usize::from(self.down[column]) {
                continue;
            }
            if self.takes_home(self.top(column)) {
                out.push(Move::Home {
                    from: Source::Column(column as u8),
                });
            }
        }

        // 2. The waste's top card onto a column.
        if let Some(card) = self.last(&self.waste, self.waste_len) {
            for to in 0..COLUMNS {
                if self.takes(card, to) {
                    out.push(Move::ToColumn {
                        from: Source::Waste,
                        index: 0,
                        to: to as u8,
                    });
                }
            }
        }

        // 3. A run of a column onto another column.
        for column in 0..COLUMNS {
            let len = usize::from(self.len[column]);
            for index in self.first_run(column)..len {
                let card = self.col[column][index];
                for to in 0..COLUMNS {
                    if to == column || !self.takes(card, to) {
                        continue;
                    }
                    // A whole column onto an empty one is the same board with
                    // two columns renamed, so it is not a move: it is the
                    // position it started at.
                    if index == 0 && self.len[to] == 0 {
                        continue;
                    }
                    out.push(Move::ToColumn {
                        from: Source::Column(column as u8),
                        index: index as u8,
                        to: to as u8,
                    });
                }
            }
        }

        // 4. A card back off its foundation, when a column would take it. This
        //    is rarely the move a player makes, and it is here because a search
        //    that forbade it could call a winnable deal stuck: a card pulled
        //    back can be the pile a crowded column unloads onto.
        for suit in 0..FOUNDATIONS {
            let rank = self.home[suit];
            if rank == 0 {
                continue;
            }
            let card = Slot((suit as u8) * 13 + rank - 1);
            for to in 0..COLUMNS {
                if self.takes(card, to) {
                    out.push(Move::OffFoundation {
                        suit: suit as u8,
                        to: to as u8,
                    });
                }
            }
        }

        // 5. The stock, last: a walk that turns cards over before it has
        //    finished with the table is a walk that explores the same tableau
        //    once per arrangement of the waste.
        if self.stock_len > 0 || (self.waste_len > 0 && self.may_redeal()) {
            out.push(Move::Turn);
        }
    }

    /// Turn over the card a column has just uncovered, as the game does.
    fn reveal(&mut self, column: usize) {
        if self.len[column] == self.down[column] && self.len[column] > 0 {
            self.down[column] -= 1;
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    fn apply(&mut self, mv: Move) {
        match mv {
            Move::Home { from } => {
                let (card, column) = match from {
                    Source::Waste => {
                        self.waste_len -= 1;
                        (self.waste[usize::from(self.waste_len)], None)
                    }
                    Source::Column(column) => {
                        let index = usize::from(column);
                        self.len[index] -= 1;
                        (self.col[index][usize::from(self.len[index])], Some(index))
                    }
                };
                self.home[card.suit()] += 1;
                if let Some(column) = column {
                    self.reveal(column);
                }
            }
            Move::ToColumn { from, index, to } => {
                let to = usize::from(to);
                match from {
                    Source::Waste => {
                        self.waste_len -= 1;
                        let card = self.waste[usize::from(self.waste_len)];
                        let at = usize::from(self.len[to]);
                        self.col[to][at] = card;
                        self.len[to] += 1;
                    }
                    Source::Column(column) => {
                        let from = usize::from(column);
                        let start = usize::from(index);
                        let count = usize::from(self.len[from]) - start;
                        let at = usize::from(self.len[to]);
                        for offset in 0..count {
                            self.col[to][at + offset] = self.col[from][start + offset];
                        }
                        self.len[to] += count as u8;
                        self.len[from] = index;
                        self.reveal(from);
                    }
                }
            }
            Move::OffFoundation { suit, to } => {
                let suit = usize::from(suit);
                self.home[suit] -= 1;
                let card = Slot((suit as u8) * 13 + self.home[suit]);
                let to = usize::from(to);
                let at = usize::from(self.len[to]);
                self.col[to][at] = card;
                self.len[to] += 1;
            }
            Move::Turn => {
                if self.stock_len > 0 {
                    let count = self.rules.draw.min(self.stock_len);
                    for _ in 0..count {
                        self.stock_len -= 1;
                        let card = self.stock[usize::from(self.stock_len)];
                        self.waste[usize::from(self.waste_len)] = card;
                        self.waste_len += 1;
                    }
                } else {
                    while self.waste_len > 0 {
                        self.waste_len -= 1;
                        let card = self.waste[usize::from(self.waste_len)];
                        self.stock[usize::from(self.stock_len)] = card;
                        self.stock_len += 1;
                    }
                    self.redeals = self.redeals.saturating_add(1);
                }
            }
        }
    }

    /// The position's identity, for the visited set.
    ///
    /// FNV-1a over exactly the cards and counts that are in play — never the
    /// stale bytes above a column's top card, which would make two identical
    /// boards look different and turn the search inside out.
    fn key(&self) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |bytes: &[u8]| {
            for &byte in bytes {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        for column in 0..COLUMNS {
            let len = usize::from(self.len[column]);
            mix(&[self.len[column], self.down[column]]);
            for slot in &self.col[column][..len] {
                mix(&[slot.0]);
            }
        }
        mix(&[self.stock_len]);
        for slot in &self.stock[..usize::from(self.stock_len)] {
            mix(&[slot.0]);
        }
        mix(&[self.waste_len]);
        for slot in &self.waste[..usize::from(self.waste_len)] {
            mix(&[slot.0]);
        }
        mix(&self.home);
        if self.rules.redeal_limit.is_some() {
            mix(&self.redeals.to_le_bytes());
        }
        hash
    }

    /// The parts of a position that matter, for the tests that compare a
    /// searched board with the game's own.
    #[cfg(test)]
    fn same(&self, other: &Self) -> bool {
        if self.len != other.len
            || self.down != other.down
            || self.home != other.home
            || self.stock_len != other.stock_len
            || self.waste_len != other.waste_len
        {
            return false;
        }
        for column in 0..COLUMNS {
            let len = usize::from(self.len[column]);
            if self.col[column][..len] != other.col[column][..len] {
                return false;
            }
        }
        self.stock[..usize::from(self.stock_len)] == other.stock[..usize::from(other.stock_len)]
            && self.waste[..usize::from(self.waste_len)]
                == other.waste[..usize::from(other.waste_len)]
    }
}

/// The set of positions already examined, as an open-addressed table.
///
/// Not a `HashSet`: this is the search's inner loop, and the standard hasher is
/// built to resist an attacker who chooses the keys, which nothing here does —
/// the keys are the search's own hashes of its own boards. A table of its own is
/// a plain multiply-and-shift on an `u64`, two arrays, and no allocation per
/// probe.
struct Seen {
    keys: Vec<u64>,
    /// One flag per slot. A key can be any `u64`, including zero, so "this slot
    /// is empty" cannot be spelled with the key alone without one position in
    /// 2^64 being taken for another.
    used: Vec<bool>,
    count: usize,
}

impl Seen {
    fn new() -> Self {
        Self {
            keys: vec![0; 1 << 12],
            used: vec![false; 1 << 12],
            count: 0,
        }
    }

    /// Record `key`, and say whether it was new.
    fn insert(&mut self, key: u64) -> bool {
        if self.count * 10 >= self.used.len() * 7 {
            self.grow();
        }
        self.place(key)
    }

    fn place(&mut self, key: u64) -> bool {
        let mask = self.used.len() - 1;
        let mut slot = index(key, mask);
        loop {
            if !self.used[slot] {
                self.used[slot] = true;
                self.keys[slot] = key;
                self.count += 1;
                return true;
            }
            if self.keys[slot] == key {
                return false;
            }
            slot = (slot + 1) & mask;
        }
    }

    fn grow(&mut self) {
        let keys = std::mem::take(&mut self.keys);
        let used = std::mem::take(&mut self.used);
        let capacity = used.len() * 2;
        self.keys = vec![0; capacity];
        self.used = vec![false; capacity];
        self.count = 0;
        for (slot, &was_used) in used.iter().enumerate() {
            if was_used {
                self.place(keys[slot]);
            }
        }
    }
}

/// Where a key starts its probe: Fibonacci hashing, which spreads the low bits
/// of the key across the mask whatever the key's own structure is.
fn index(key: u64, mask: usize) -> usize {
    let mixed = key.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    ((mixed >> 32) as usize) & mask
}

/// One position on the walk, and how far through its moves it has got.
struct Frame {
    position: Position,
    moves: Vec<Move>,
    next: usize,
}

/// A search in progress.
///
/// The walk is depth first and keeps its own stack, so it can be stopped after
/// any number of positions and continued where it left off — which is what lets
/// the page run it in slices between frames instead of freezing the board for
/// the length of the answer.
pub struct Solver {
    stack: Vec<Frame>,
    visited: Seen,
    /// Positions examined so far, over the whole search.
    nodes: u64,
    /// How many will be examined before the search gives up.
    budget: u64,
    /// Set when a branch was cut short, which makes "stuck" no longer provable.
    cut: bool,
    answer: Option<Verdict>,
}

impl Solver {
    /// A search from the position the game is in.
    ///
    /// `budget` is the number of positions the whole search may examine. It is
    /// the caller's business because it is a question about patience, not about
    /// cards.
    #[must_use]
    pub fn new(game: &Game, budget: u64) -> Option<Self> {
        Some(Self::from_position(Position::of(game)?, budget))
    }

    fn from_position(position: Position, budget: u64) -> Self {
        let mut solver = Self {
            stack: Vec::new(),
            visited: Seen::new(),
            nodes: 0,
            budget: budget.max(1),
            cut: false,
            answer: None,
        };
        if position.won() {
            solver.answer = Some(Verdict::Winnable);
            return solver;
        }
        let _ = solver.visited.insert(position.key());
        let mut moves = Vec::new();
        position.moves(&mut moves);
        solver.stack.push(Frame {
            position,
            moves,
            next: 0,
        });
        solver
    }

    /// The answer, once there is one.
    #[must_use]
    pub const fn verdict(&self) -> Option<Verdict> {
        self.answer
    }

    /// How many positions have been examined.
    #[must_use]
    pub const fn nodes(&self) -> u64 {
        self.nodes
    }

    /// Examine up to `slice` more positions, and answer if the walk is over.
    pub fn step(&mut self, slice: u64) -> Option<Verdict> {
        if let Some(answer) = self.answer {
            return Some(answer);
        }
        let stop = self.nodes.saturating_add(slice.max(1));
        loop {
            if self.nodes >= self.budget {
                self.answer = Some(Verdict::Unclear);
                return self.answer;
            }
            if self.nodes >= stop {
                return None;
            }
            let Some(frame) = self.stack.last_mut() else {
                self.answer = Some(if self.cut {
                    Verdict::Unclear
                } else {
                    Verdict::Dead
                });
                return self.answer;
            };
            if frame.next == frame.moves.len() {
                self.stack.pop();
                continue;
            }
            let mv = frame.moves[frame.next];
            frame.next += 1;
            let mut child = frame.position;
            child.apply(mv);
            self.nodes += 1;
            if child.won() {
                self.answer = Some(Verdict::Winnable);
                return self.answer;
            }
            if !self.visited.insert(child.key()) {
                continue;
            }
            if self.stack.len() >= MAX_DEPTH {
                self.cut = true;
                continue;
            }
            let mut moves = Vec::new();
            child.moves(&mut moves);
            self.stack.push(Frame {
                position: child,
                moves,
                next: 0,
            });
        }
    }

    /// Run the whole search at once, however long it takes. For tests and for
    /// callers that do not need to stay responsive.
    pub fn run(&mut self) -> Verdict {
        loop {
            if let Some(answer) = self.step(1 << 20) {
                return answer;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Move, Position, Rules, Slot, Solver, Verdict};
    use crate::card::{Card, Rank, Suit};
    use crate::game::{Difficulty, Game, Pile, Scoring};

    fn card(suit: Suit, rank: Rank) -> Slot {
        Slot::of(Card::new(rank, suit))
    }

    const EASY: Rules = Rules {
        draw: 1,
        redeal_limit: None,
    };

    /// A position built by hand, for the shapes a scripted deal would take a
    /// long time to reach: `columns` is bottom first, and `down` says how many
    /// cards at the bottom of each column are face down.
    ///
    /// These boards are not full decks. The search does not need one — the only
    /// thing a deck's worth of cards buys is a position that could actually
    /// arise, and the tests that care about that build a real [`Game`] instead.
    fn position(
        columns: &[&[(Suit, Rank)]],
        down: &[usize],
        stock: &[(Suit, Rank)],
        waste: &[(Suit, Rank)],
        home: [u8; 4],
        rules: Rules,
    ) -> Position {
        let mut pos = Position {
            col: [[Slot(0); super::MAX_COLUMN]; super::COLUMNS],
            len: [0; super::COLUMNS],
            down: [0; super::COLUMNS],
            stock: [Slot(0); super::MAX_PILE],
            stock_len: 0,
            waste: [Slot(0); super::MAX_PILE],
            waste_len: 0,
            home,
            redeals: 0,
            rules,
        };
        for (index, cards) in columns.iter().enumerate() {
            for (at, &(suit, rank)) in cards.iter().enumerate() {
                pos.col[index][at] = card(suit, rank);
            }
            pos.len[index] = u8::try_from(cards.len()).unwrap();
            pos.down[index] = u8::try_from(down.get(index).copied().unwrap_or(0)).unwrap();
        }
        for (index, &(suit, rank)) in stock.iter().enumerate() {
            pos.stock[index] = card(suit, rank);
        }
        pos.stock_len = u8::try_from(stock.len()).unwrap();
        for (index, &(suit, rank)) in waste.iter().enumerate() {
            pos.waste[index] = card(suit, rank);
        }
        pos.waste_len = u8::try_from(waste.len()).unwrap();
        pos
    }

    fn run(pos: Position) -> Verdict {
        Solver::from_position(pos, 1 << 22).run()
    }

    fn moves_of(pos: &Position) -> Vec<Move> {
        let mut moves = Vec::new();
        pos.moves(&mut moves);
        moves
    }

    /// Every card home is a won position, and the search says so without
    /// walking anywhere.
    #[test]
    fn a_board_with_every_card_home_is_won() {
        let empty: &[(Suit, Rank)] = &[];
        let pos = position(&[empty; 7], &[0; 7], &[], &[], [13; 4], EASY);
        assert!(pos.won());
        assert_eq!(run(pos), Verdict::Winnable);
        assert_eq!(Solver::from_position(pos, 1).nodes(), 0);
    }

    /// A won save is read back as a search that has nothing to do.
    #[test]
    fn a_won_saved_game_needs_no_search() {
        let saved = concat!(
            "v1|easy|off|7|0|0|0|0|1|1||",
            "|AC2C3C4C5C6C7C8C9CTCJCQCKC|AD2D3D4D5D6D7D8D9DTDJDQDKD",
            "|AH2H3H4H5H6H7H8H9HTHJHQHKH|AS2S3S4S5S6S7S8S9STSJSQSKS",
            "|0:|0:|0:|0:|0:|0:|0:"
        );
        let game = Game::from_stored(Some(saved)).expect("a won save reads back");
        let solver = Solver::new(&game, 1).expect("a won board is searchable");
        assert_eq!(solver.verdict(), Some(Verdict::Winnable));
    }

    /// A whole deal that cannot be won, in the shape a real dead one takes:
    /// clubs and diamonds are home, both remaining aces are buried under a
    /// five, and every other spade and heart is face down with nothing on the
    /// table to move onto them. There is no move at all, so the answer is
    /// proved rather than guessed — and it is a full deck, so the position is
    /// one the game could really be in.
    #[test]
    fn a_deal_buried_under_its_own_cards_is_stuck() {
        let game = Game::from_stored(Some(
            "v1|easy|off|12|0|0|0|0|0|1|||AC2C3C4C5C6C7C8C9CTCJCQCKC|AD2D3D4D5D6D7D8D9DTDJDQDKD|||1:AS5S|1:AH5H|5:2S3S4S6S7S|6:8S9STSJSQSKS|5:2H3H4H6H7H|4:8H9HTHJH|2:QHKH",
        ))
        .expect("a buried deal reads back");
        let mut solver = Solver::new(&game, 1 << 16).expect("it is searchable");
        assert_eq!(solver.run(), Verdict::Dead);
    }

    /// One card left to send home, and the deal is won.
    #[test]
    fn a_deal_one_move_from_home_is_winnable() {
        let pos = position(
            &[&[(Suit::Spades, Rank::King)], &[], &[], &[], &[], &[], &[]],
            &[0; 7],
            &[],
            &[],
            [13, 13, 13, 12],
            EASY,
        );
        assert!(moves_of(&pos).contains(&Move::Home {
            from: super::Source::Column(0)
        }));
        assert_eq!(run(pos), Verdict::Winnable);
    }

    /// A card that is still in the stock is still reachable, so the deal is
    /// not stuck.
    #[test]
    fn a_card_in_the_stock_can_still_win() {
        let pos = position(
            &[&[], &[], &[], &[], &[], &[], &[]],
            &[0; 7],
            &[(Suit::Spades, Rank::King)],
            &[],
            [13, 13, 13, 12],
            EASY,
        );
        assert_eq!(run(pos), Verdict::Winnable);
    }

    /// A board with nothing to do is stuck, and is said to be stuck without
    /// inventing a move.
    #[test]
    fn a_board_with_no_moves_is_stuck() {
        let pos = position(
            &[
                &[(Suit::Hearts, Rank::Six), (Suit::Spades, Rank::Five)],
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
            ],
            &[0; 7],
            &[],
            &[],
            [0; 4],
            EASY,
        );
        assert!(moves_of(&pos).is_empty());
        assert_eq!(run(pos), Verdict::Dead);
    }

    /// The heart of the question: a deal whose only moves lead back to where
    /// they came from is dead, not "still going". A five and a six on a table
    /// with one empty column can each be put on the other, for ever.
    #[test]
    fn a_loop_between_two_positions_is_stuck() {
        let pos = position(
            &[
                &[(Suit::Hearts, Rank::Six)],
                &[(Suit::Spades, Rank::Five)],
                &[],
                &[],
                &[],
                &[],
                &[],
            ],
            &[0; 7],
            &[],
            &[],
            [0; 4],
            EASY,
        );
        let mut solver = Solver::from_position(pos, 1 << 22);
        assert_eq!(solver.run(), Verdict::Dead);
        // Three positions and no more: the start, the five on the six, and the
        // six on the five — which is a board already seen, so the walk stops.
        assert!(
            solver.nodes() <= 3,
            "the walk followed a loop for {} boards",
            solver.nodes()
        );
    }

    /// The count of passes is part of the position only when the deal limits
    /// it, and the limit is enforced: a card under the waste is reachable with
    /// passes to spare, and out of reach without them.
    ///
    /// The table is chosen so that nothing else is available — no empty column
    /// to take a king, and no column top that a foundation's card would sit on
    /// — which leaves the waste as the deal's only way forward.
    #[test]
    fn a_deal_out_of_passes_cannot_reach_under_the_waste() {
        let columns: &[(Suit, Rank)] = &[][..];
        let _ = columns;
        let singles = [
            &[(Suit::Clubs, Rank::Two)][..],
            &[(Suit::Diamonds, Rank::Four)][..],
            &[(Suit::Hearts, Rank::Six)][..],
            &[(Suit::Spades, Rank::Eight)][..],
            &[(Suit::Clubs, Rank::Ten)][..],
            &[(Suit::Diamonds, Rank::Queen)][..],
            &[(Suit::Hearts, Rank::King)][..],
        ];
        let waste = [(Suit::Spades, Rank::King), (Suit::Diamonds, Rank::Two)];
        let generous = position(&singles, &[0; 7], &[], &waste, [13, 13, 13, 12], EASY);
        assert_eq!(run(generous), Verdict::Winnable);

        let stingy = position(
            &singles,
            &[0; 7],
            &[],
            &waste,
            [13, 13, 13, 12],
            Rules {
                draw: 1,
                redeal_limit: Some(0),
            },
        );
        assert_eq!(run(stingy), Verdict::Dead);
    }

    /// A card can come back off a foundation, and taking it back puts it where
    /// it came from.
    #[test]
    fn a_card_comes_back_off_a_foundation() {
        // The queen of diamonds is home; a black king is on the table, so the
        // queen has somewhere to go and the search has to offer it.
        let pos = position(
            &[&[(Suit::Spades, Rank::King)], &[], &[], &[], &[], &[], &[]],
            &[0; 7],
            &[],
            &[],
            [0, 12, 0, 0],
            EASY,
        );
        let back = Move::OffFoundation { suit: 1, to: 0 };
        assert!(moves_of(&pos).contains(&back));

        let mut after = pos;
        after.apply(back);
        assert_eq!(after.home[1], 11);
        assert_eq!(after.len[0], 2);
        assert_eq!(after.top(0), card(Suit::Diamonds, Rank::Queen));
    }

    /// A fresh deal is not settled at once, and the search says so rather than
    /// guessing: with a budget that cannot finish it, the answer is `Unclear`.
    #[test]
    fn an_unfinished_search_says_it_does_not_know() {
        let game = Game::dealt(Difficulty::Hard, Scoring::Standard, 11);
        let mut solver = Solver::new(&game, 500).expect("a dealt board is searchable");
        assert_eq!(solver.step(1 << 20), Some(Verdict::Unclear));
    }

    /// Every move the search generates is a move the game makes, and it makes
    /// it the same way.
    ///
    /// This is the guard on the second copy of the rules: the search is a
    /// compact board for speed, and a compact board that quietly disagreed with
    /// the game would answer "stuck" about deals that are not.
    #[test]
    fn the_search_plays_the_same_moves_as_the_game() {
        for seed in 0..12_u64 {
            for difficulty in [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard] {
                // Four turns of the stock, then enough to empty it and turn the
                // waste back over: a redeal is a move too, and it is the one
                // whose order is easiest to get backwards.
                for draws in [4, 26] {
                    let mut game = Game::dealt(difficulty, Scoring::Standard, seed);
                    for _ in 0..draws {
                        game.draw();
                    }
                    check_agreement(&game, seed);
                }
            }
        }
    }

    fn copy_of(game: &Game) -> Game {
        Game::from_stored(game.to_stored().as_deref()).expect("a dealt game reads back")
    }

    fn foundation_of(game: &Game, suit: Suit) -> usize {
        (0..super::FOUNDATIONS)
            .find(|&foundation| {
                game.pile(Pile::Foundation(foundation))
                    .last()
                    .is_some_and(|&card| game.card(card).suit == suit)
            })
            .expect("the suit is home somewhere")
    }

    fn check_agreement(game: &Game, seed: u64) {
        let pos = Position::of(game).expect("a dealt board is searchable");
        let moves = moves_of(&pos);
        assert!(
            !moves.is_empty(),
            "seed {seed}: the search found nothing to do on a dealt board"
        );
        for mv in moves {
            let mut expected = pos;
            expected.apply(mv);
            let mut played = copy_of(game);
            match mv {
                Move::Home { from } => match from {
                    super::Source::Waste => {
                        let index = played.pile(Pile::Waste).len() - 1;
                        played.double_tap(Pile::Waste, index);
                    }
                    super::Source::Column(column) => {
                        let pile = Pile::Tableau(usize::from(column));
                        let index = played.pile(pile).len() - 1;
                        played.double_tap(pile, index);
                    }
                },
                Move::ToColumn { from, index, to } => {
                    let (from, index) = match from {
                        super::Source::Waste => (Pile::Waste, played.pile(Pile::Waste).len() - 1),
                        super::Source::Column(column) => {
                            (Pile::Tableau(usize::from(column)), usize::from(index))
                        }
                    };
                    played.select(from, index);
                    played.drop(Pile::Tableau(usize::from(to)));
                }
                Move::OffFoundation { suit, to } => {
                    let foundation =
                        Pile::Foundation(foundation_of(&played, Suit::ALL[usize::from(suit)]));
                    let index = played.pile(foundation).len() - 1;
                    played.select(foundation, index);
                    played.drop(Pile::Tableau(usize::from(to)));
                }
                Move::Turn => {
                    played.draw();
                }
            }
            let actual = Position::of(&played).expect("a played board is searchable");
            assert!(
                actual.same(&expected),
                "seed {seed}: the search and the game disagree about {mv:?}\n\
                 search: {expected:?}\n\ngame:   {actual:?}"
            );
        }
    }

    /// The search misses nothing the game allows.
    ///
    /// The other half of the guard above: that one proves every move the search
    /// offers is real, and this one proves every real move is offered — which is
    /// the half a "stuck" verdict rests on. A search that quietly skipped a move
    /// would answer "no way to win from here" about a deal that had one.
    ///
    /// Two moves are deliberately missing, and only because they are the same
    /// board under another name: a whole column moved onto an empty column, and
    /// a move onto an empty column that is not the first one. Both are exempted
    /// here with the reason attached, so a genuine omission cannot hide among
    /// them.
    #[test]
    fn the_search_offers_every_move_the_game_allows() {
        for seed in 0..8_u64 {
            for difficulty in [Difficulty::Easy, Difficulty::Hard] {
                let mut game = Game::dealt(difficulty, Scoring::Standard, seed);
                for _ in 0..3 {
                    game.draw();
                }
                check_completeness(&game, seed);
            }
        }
    }

    fn check_completeness(game: &Game, seed: u64) {
        let pos = Position::of(game).expect("a dealt board is searchable");
        let moves = moves_of(&pos);
        let first_empty = pos.first_empty();
        let destinations: Vec<Pile> = (0..super::COLUMNS)
            .map(Pile::Tableau)
            .chain((0..super::FOUNDATIONS).map(Pile::Foundation))
            .collect();
        for from in Pile::ALL {
            if from == Pile::Stock {
                continue;
            }
            for index in 0..game.pile(from).len() {
                for &to in &destinations {
                    if to == from {
                        continue;
                    }
                    let mut played = copy_of(game);
                    played.select(from, index);
                    played.drop(to);
                    if played.moves() == game.moves() {
                        continue; // the game does not allow it either
                    }
                    let mv = match (from, to) {
                        (Pile::Tableau(column), Pile::Tableau(onto)) => Move::ToColumn {
                            from: super::Source::Column(column as u8),
                            index: index as u8,
                            to: onto as u8,
                        },
                        (Pile::Tableau(_), Pile::Foundation(_)) => Move::Home {
                            from: super::Source::Column(from_index(from)),
                        },
                        (Pile::Waste, Pile::Tableau(onto)) => Move::ToColumn {
                            from: super::Source::Waste,
                            index: 0,
                            to: onto as u8,
                        },
                        (Pile::Waste, Pile::Foundation(_)) => Move::Home {
                            from: super::Source::Waste,
                        },
                        (Pile::Foundation(foundation), Pile::Tableau(onto)) => {
                            let top = *game
                                .pile(Pile::Foundation(foundation))
                                .last()
                                .expect("an empty foundation cannot move");
                            Move::OffFoundation {
                                suit: super::suit_index(game.card(top).suit),
                                to: onto as u8,
                            }
                        }
                        (from, to) => panic!("seed {seed}: unexpected move {from:?} to {to:?}"),
                    };
                    if let Pile::Tableau(onto) = to {
                        let empty = pos.len[onto] == 0;
                        let whole_column = matches!(
                            mv,
                            Move::ToColumn {
                                from: super::Source::Column(_),
                                index: 0,
                                ..
                            }
                        );
                        if empty && (Some(onto) != first_empty || whole_column) {
                            continue; // the same board with the columns renamed
                        }
                    }
                    assert!(
                        moves.contains(&mv),
                        "seed {seed}: the game allows {mv:?} and the search does not offer it"
                    );
                }
            }
        }
        let mut turned = copy_of(game);
        turned.draw();
        if turned.moves() != game.moves() {
            assert!(
                moves.contains(&Move::Turn),
                "seed {seed}: the game allows a turn of the stock and the search does not"
            );
        }
    }

    fn from_index(pile: Pile) -> u8 {
        let Pile::Tableau(column) = pile else {
            panic!("only a column has an index");
        };
        column as u8
    }

    /// The packed card agrees with the card it packs.
    #[test]
    fn a_packed_card_keeps_its_suit_and_rank() {
        for (index, suit) in Suit::ALL.into_iter().enumerate() {
            assert_eq!(usize::from(super::suit_index(suit)), index);
            for rank in Rank::ALL {
                let slot = Slot::of(Card::new(rank, suit));
                assert_eq!(slot.suit(), index);
                assert_eq!(slot.rank(), rank.value());
                assert_eq!(slot.is_red(), suit.is_red());
            }
        }
    }
}
