//! The game as the board draws it: the scene, where the keyboard starts, and
//! the tests that pin both.
//!
//! No `web-sys` here, on purpose. Building a scene is pure — a `Game` in, a
//! [`Scene`] out — and it is where the board's *arrangement* is decided: which
//! pile owns how many cells, how its cards fan, what it says to a screen reader.
//! A test that could not run would not be worth writing, and everything in this
//! file runs under plain `cargo test`.
//!
//! The app's `ui.rs` is the browser half; this is the half that can be checked.

use crate::board::{Fan, Place, Scene, Slot};
use crate::game::{Game, Hint, Pile, COLUMNS, FOUNDATIONS};

/// How many column cells the top row holds, and therefore how many the board
/// is laid out in.
///
/// One for the stock, **two for the waste**, and one per foundation. Two matters:
/// the waste's cards fan sideways, and a pile that fans must be given the cells
/// it fans into — one cell puts the third drawn card underneath the first
/// foundation, which paints over it.
pub const TOP_CELLS: usize = 1 + 2 + FOUNDATIONS;

/// The game as the board draws it.
#[must_use]
pub fn scene(game: &Game, hint: Option<&Hint>) -> Scene {
    let mut slots = Vec::with_capacity(Pile::ALL.len());
    for pile in Pile::ALL {
        let cards: Vec<(u16, bool)> = game
            .pile(pile)
            .iter()
            .enumerate()
            .map(|(index, &id)| (id, game.is_face_down(pile, index)))
            .collect();
        let mut slot = Slot::new(pile.place(), fan_for(game, pile), game.pile_label(pile))
            .with_mark(game.slot_mark(pile))
            .with_cards(&cards);
        if pile == Pile::Stock {
            slot = slot.pressable();
        }
        if pile == Pile::Waste {
            // The room its fan needs; see `TOP_CELLS`.
            slot = slot.spanning(2);
        }
        slots.push(slot);
    }
    let mut scene = Scene::new(slots, TOP_CELLS);
    scene.held = game.selection().map(|held| (held.pile.place(), held.index));
    // The ring goes on the pile to press, which is not a card: the stock for a
    // turn, the waste for turning it back over. Ringing the stock for a recycle
    // was a hint that pointed at the wrong pile — and with the words reduced to
    // one, the picture is all the player has to go on.
    scene.hint = hint.and_then(|hint| match hint {
        Hint::Move {
            from, index, count, ..
        } => Some((from.place(), *index, *count)),
        Hint::Draw => Some((
            Pile::Stock.place(),
            game.pile_len(Pile::Stock).saturating_sub(1),
            1,
        )),
        Hint::Recycle => Some((
            Pile::Waste.place(),
            game.pile_len(Pile::Waste).saturating_sub(1),
            1,
        )),
        Hint::None => None,
    });
    scene
}

/// How a pile is drawn.
#[must_use]
pub fn fan_for(game: &Game, pile: Pile) -> Fan {
    match pile {
        Pile::Tableau(_) => Fan::COLUMN,
        // The cards a draw-three turn exposed are fanned so the player can see
        // and reach each; everything deeper is under them.
        Pile::Waste => Fan::tail(game.difficulty().draw().clamp(1, 3)),
        Pile::Stock | Pile::Foundation(_) => Fan::PACKET,
    }
}

/// Where the keyboard should be: the stock while it has cards, else the first
/// column with a card that can be picked up.
#[must_use]
pub fn default_focus(game: &Game) -> (Place, Option<usize>) {
    if game.pile_len(Pile::Stock) > 0 {
        return (Pile::Stock.place(), Some(game.pile_len(Pile::Stock) - 1));
    }
    for column in 0..COLUMNS {
        let pile = Pile::Tableau(column);
        let len = game.pile_len(pile);
        if let Some(index) = (0..len)
            .rev()
            .find(|&index| !game.is_face_down(pile, index))
        {
            return (pile.place(), Some(index));
        }
    }
    (Pile::Tableau(0).place(), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Frame;
    use crate::game::{Difficulty, Scoring};

    /// The top row adds up to exactly the board's width.
    ///
    /// That sum is not decoration. Wider than the columns and the last
    /// foundation hangs off the board; narrower than two cells for the waste and
    /// its fan goes under the first foundation.
    #[test]
    fn the_top_row_spans_exactly_the_columns() {
        let game = Game::new(Difficulty::Hard, Scoring::Standard, 5);
        let board = scene(&game, None);
        let cells: usize = board
            .slots
            .iter()
            .filter(|slot| matches!(slot.place, Place::Top(_)))
            .map(Slot::cells)
            .sum();
        assert_eq!(cells, COLUMNS, "the top row must fill the board exactly");
        assert_eq!(board.columns(), COLUMNS);
        assert_eq!(
            board.slot(Pile::Waste.place()).map(Slot::cells),
            Some(2),
            "the waste needs two cells for its fan"
        );
    }

    /// The reported defect, at the board level: three drawn cards hid partially
    /// under the first foundation.
    ///
    /// A fan spreads by a fraction of the card's height, which is wider than a
    /// card's cell, so the third card's right edge landed inside the
    /// foundation's cell — and the foundation, later in the row, painted over
    /// it. Both halves of the fix are checked here: the waste's own cell is two
    /// cells wide, and the fan is clamped to the cell it is in.
    #[test]
    fn three_drawn_cards_stay_out_of_the_foundations_cell() {
        let game = Game::new(Difficulty::Medium, Scoring::Standard, 9);
        // The state a draw-three turn leaves: the top three stock cards are on
        // the waste, and nowhere else. Taking real ids out of the stock is what
        // makes the positions below the *waste's* — reusing ids that are also
        // in a column would read the column's position instead.
        let mut board = scene(&game, None);
        let drawn: Vec<u16> = board
            .slot(Pile::Stock.place())
            .map(|slot| {
                slot.cards
                    .iter()
                    .rev()
                    .take(3)
                    .map(|card| card.id)
                    .collect()
            })
            .expect("a stock");
        for slot in &mut board.slots {
            if slot.place == Pile::Stock.place() {
                slot.cards.retain(|card| !drawn.contains(&card.id));
            }
            if slot.place == Pile::Waste.place() {
                slot.cards = drawn
                    .iter()
                    .map(|id| crate::board::CardSlot {
                        id: *id,
                        face_down: false,
                    })
                    .collect();
            }
        }
        for (width, height) in [(1280.0, 800.0), (1000.0, 700.0), (390.0, 844.0)] {
            let layout = crate::board::layout(
                &board,
                &Frame {
                    width,
                    height,
                    gap: (width * 0.008).clamp(3.0, 10.0),
                    margin: (width * 0.012).clamp(4.0, 12.0),
                },
            );
            let first = layout.card(drawn[0]).expect("the first drawn card");
            let third = layout.card(drawn[2]).expect("the third drawn card");
            let (foundation, _) = layout
                .place(Pile::Foundation(0).place())
                .expect("a foundation");
            assert!(
                third.0 + layout.card_w <= foundation + 0.01,
                "at {width}x{height} the fan reaches {:.1} but the foundation starts at \
                 {foundation:.1}",
                third.0 + layout.card_w
            );
            // And the three are still distinguishable, or the fix would have
            // turned a fan into a packet.
            assert!(
                third.0 > first.0 + layout.card_w * 0.1,
                "the fan still shows three separate cards at {width}x{height}"
            );
            // Nothing else moved: the first two cards are on the same spot.
            let second = layout.card(drawn[1]).expect("the second drawn card");
            assert!(second.0 > first.0 && second.0 < third.0, "in order");
        }
    }

    #[test]
    fn the_waste_fans_by_the_difficulty_and_a_packet_does_not() {
        let easy = Game::new(Difficulty::Easy, Scoring::Standard, 1);
        let hard = Game::new(Difficulty::Hard, Scoring::Standard, 1);
        assert_eq!(fan_for(&easy, Pile::Waste).fan_tail, 1);
        assert_eq!(fan_for(&hard, Pile::Waste).fan_tail, 3);
        assert!(fan_for(&easy, Pile::Stock).stacked);
        assert!(fan_for(&easy, Pile::Foundation(0)).stacked);
        assert!(!fan_for(&easy, Pile::Tableau(0)).stacked);
    }

    #[test]
    fn every_pile_has_a_place_and_every_place_has_a_pile() {
        for pile in Pile::ALL {
            assert_eq!(Pile::from_place(pile.place()), Some(pile), "{pile:?}");
        }
        assert_eq!(Pile::from_place(Place::Column(COLUMNS)), None);
    }

    #[test]
    fn the_keyboard_starts_on_the_stock_while_there_is_one() {
        let game = Game::new(Difficulty::Medium, Scoring::Standard, 3);
        assert_eq!(default_focus(&game).0, Pile::Stock.place());
    }

    #[test]
    fn a_held_run_and_a_hint_are_part_of_the_scene() {
        let mut game = Game::new(Difficulty::Medium, Scoring::Standard, 4);
        // Column 6 has seven cards, so its top card is a run of one.
        game.select(Pile::Tableau(6), 6);
        let board = scene(&game, None);
        assert_eq!(
            board.held,
            Some((Pile::Tableau(6).place(), 6)),
            "the board draws what the game is holding"
        );
        let hint = game.hint();
        let board = scene(&game, Some(&hint));
        assert!(board.hint.is_some(), "a hint lights something");
    }
}
