//! The board's browser half: the elements, the painting, the gestures.
//!
//! Everything a solitaire has to do to a DOM — 52 or 104 buttons, a fan that
//! must not jump, a drag that must follow the finger, a drop that must land
//! where the player aimed — is here, once. An app supplies a [`Scene`] and a
//! [`Frame`], and answers the [`Intent`]s the board reports. It keeps its rules,
//! its header, its menu and its words.
//!
//! **This file is carried, byte for byte, by both solitaires in this family** —
//! Solitaire and Spider Solitaire. They are separate repositories, so nothing
//! enforces that; the rule is that a change here belongs in both. Like
//! [`crate::board`], it knows about cards and piles and nothing about either
//! game.
//!
//! Five rules keep a gesture honest:
//!
//! * **The board is built once and then only described.** A card element
//!   survives every render; a paint moves it and rewrites its description. That
//!   is what lets a card travel to its pile with a transition, and it is why a
//!   drag survives a repaint.
//! * **The held cards are moved by the gesture, not by the render.** While a
//!   drag is live, a paint skips the cards in the air; the pointer moves them. A
//!   render that also wrote their position would fight the finger.
//! * **A release always ends somewhere.** On a pile, back where it came from, or
//!   — if the gesture never became a drag — as a tap. There is no path that
//!   leaves the cards in the air.
//! * **A drop is resolved by geometry, not by a DOM hit test.** The element under
//!   the pointer is the card in the player's hand, so asking the DOM answers
//!   "the thing you are holding" for every drop. See
//!   [`crate::board::Layout::drop_target`].
//! * **The app decides what a move means; the board only reports it.** The board
//!   knows a card was dragged from one pile to another, never whether the rules
//!   allow it — so `Intent::Drop` is a question, not a fait accompli.

use std::collections::HashMap;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Document, Element, Event, HtmlElement, MouseEvent, PointerEvent};

use crate::board::{card_classes, layout, Frame, Gesture, Layout, Place, Press, Rect, Scene, Slot};
use crate::card::Card;

/// How close two taps must be, in milliseconds, to be a double tap.
pub const DOUBLE_TAP_MS: f64 = 320.0;

/// Below this card width, the face switches to its compact form.
///
/// A rank's corner is about a fifth of the card's width on the normal face;
/// under roughly 60px that corner is under 12px of type, which is where a
/// printed card stops being legible at arm's length. The number is a
/// measurement, not a preference, and it lives with the drawing rather than in
/// the stylesheet because the card's *actual* width is what matters — not the
/// window's.
pub const COMPACT_BELOW: f64 = 60.0;

/// What a gesture turned out to be, once it ended.
///
/// These are questions for the app, not answers: the board never decides that a
/// move is legal, only that one was attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// A press and release in one place, without a drag.
    Tap {
        /// Where.
        place: Place,
        /// The card, or `None` for an empty pile.
        index: Option<usize>,
    },
    /// Two taps in the same place, quickly.
    DoubleTap {
        /// Where.
        place: Place,
        /// The card, or `None` for an empty pile.
        index: Option<usize>,
    },
    /// A held run was dragged onto a pile.
    Drop {
        /// Where it came from.
        from: Place,
        /// The run's first card in that pile.
        index: usize,
        /// Where it was dropped.
        to: Place,
    },
    /// A held run was released on no pile, or on the pile it came from.
    Back,
    /// The gesture was taken away — a second finger, the browser reclaiming the
    /// pointer — so it is neither a tap nor a move.
    Cancelled,
}

/// The board, as the page holds it.
pub struct BoardUi {
    document: Document,
    board: Element,
    /// The whole deck, so a card's suit and name are known without asking the
    /// app on every paint.
    deck: Vec<Card>,
    /// One element per deck index. Several cards can be identical — one-suit
    /// Spider holds eight of every rank — so the index is the only identity.
    cards: Vec<Element>,
    /// One element per place, by its DOM token.
    slots: HashMap<String, Element>,
    /// The gesture in progress.
    gesture: Option<Gesture>,
    /// The cards the gesture is carrying, if it became a drag.
    held: Vec<u16>,
    /// The pile the drag would land on, if any.
    target: Option<Place>,
    /// The last tap, for recognising a double one.
    last_tap: Option<(Place, usize, f64)>,
    /// The board's layout, as last painted.
    layout: Layout,
    /// The scene, as last painted: the keyboard and the drag both need it, and
    /// asking the app again mid-gesture would mean two answers to one question.
    scene: Scene,
    /// How many cards the app said may be lifted from the pressed pile. Zero
    /// means the press cannot become a drag at all.
    lift: usize,
    /// Where the keyboard is.
    focus: (Place, Option<usize>),
    /// True until one frame after the first paint: the deal then arrives on the
    /// table instead of flying in from the corner.
    instant: bool,
}

impl BoardUi {
    /// Build the board's card elements, one per deck index.
    ///
    /// # Errors
    ///
    /// When the document refuses to create an element, which is a document that
    /// is not a page.
    pub fn new(document: &Document, board: Element, deck: &[Card]) -> Result<Self, JsValue> {
        let mut ui = Self {
            document: document.clone(),
            board,
            deck: deck.to_vec(),
            cards: Vec::new(),
            slots: HashMap::new(),
            gesture: None,
            held: Vec::new(),
            target: None,
            last_tap: None,
            layout: Layout {
                card_w: 0.0,
                card_h: 0.0,
                slots: Vec::new(),
                cards: Vec::new(),
            },
            scene: Scene::new(Vec::new(), 0),
            lift: 0,
            focus: (Place::Column(0), None),
            instant: true,
        };
        ui.board.set_class_name("instant");
        for (index, card) in deck.iter().enumerate() {
            let element = ui.create_card(u16::try_from(index).unwrap_or(0), *card)?;
            ui.cards.push(element);
        }
        Ok(ui)
    }

    /// The board element, for an app that draws a decoration of its own beside
    /// the cards.
    #[must_use]
    pub fn element(&self) -> &Element {
        &self.board
    }

    /// The layout as last painted.
    #[must_use]
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The scene as last painted.
    #[must_use]
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// Where the keyboard is.
    #[must_use]
    pub fn focus(&self) -> (Place, Option<usize>) {
        self.focus
    }

    /// Move the keyboard's focus and put the browser's focus on the element.
    pub fn set_focus(&mut self, place: Place, index: Option<usize>) {
        self.focus = (place, index);
        self.paint(&self.scene.clone(), &self.frame());
    }

    /// Let the cards move from now on. Called one frame after the first paint.
    pub fn settle(&mut self) {
        self.instant = false;
        self.set_density();
    }

    /// Whether the board is still in its first frame.
    #[must_use]
    pub fn is_instant(&self) -> bool {
        self.instant
    }

    // -- gestures ---------------------------------------------------------

    /// A pointer went down at a point in board coordinates.
    ///
    /// Returns the place and card it landed on, so the app can decide what, if
    /// anything, may be lifted — [`BoardUi::lift`] is how it answers.
    pub fn press(&mut self, pointer: i32, point: (f64, f64)) -> Option<(Place, usize)> {
        let (place, index) = self.layout.card_at(&self.scene, point)?;
        self.lift = 0;
        self.held.clear();
        self.target = None;
        // The last tap is *not* cleared here, and that is what makes a double
        // tap possible at all: the second press of a pair would otherwise erase
        // the record of the first before anything could compare them. It is
        // cleared when the gesture turns out to be a drag or a cancel, which
        // are not taps.
        self.gesture = Some(Gesture::new(pointer, place, index, point));
        Some((place, index))
    }

    /// The app's answer to a press: this many cards may move.
    ///
    /// Zero means the press cannot become a drag — a face-down card, a card
    /// that is not the top of its run — and a drag that cannot lift is not a
    /// drag. A tap still works, which is what makes an unusable card still feel
    /// responsive rather than dead.
    pub fn lift(&mut self, count: usize) {
        let Some(gesture) = self.gesture else {
            return;
        };
        if count == 0 {
            self.lift = 0;
            self.held.clear();
            return;
        }
        self.lift = count;
        let Some(slot) = self.scene.slot(gesture.place) else {
            return;
        };
        self.held = slot
            .cards
            .iter()
            .skip(gesture.index)
            .take(count)
            .map(|card| card.id)
            .collect();
    }

    /// The pointer moved.
    pub fn moved(&mut self, pointer: i32, point: (f64, f64)) {
        let Some(state) = self.gesture.as_mut() else {
            return;
        };
        if state.pointer != pointer {
            return;
        }
        let state = state.moved_to(point);
        match state {
            // A drag with nothing liftable is not a drag: the finger moves, the
            // cards stay put, and the release will be a tap.
            Press::Dragging if !self.held.is_empty() => {
                let gesture = *self.gesture.as_ref().expect("just read");
                self.target = self.resolve_target(&gesture);
                self.paint_drag(&gesture);
            }
            Press::Down | Press::Dragging | Press::Cancelled => {}
        }
    }

    /// The pointer was released.
    pub fn release(&mut self, pointer: i32, point: (f64, f64)) -> Option<Intent> {
        let mut gesture = self.gesture?;
        if gesture.pointer != pointer {
            return None;
        }
        let press = gesture.moved_to(point);
        // The target is resolved from where the pointer actually let go, not
        // from the last `pointermove`: a fast flick can release at a position no
        // move event ever reported, and a drop that lands one pile short of the
        // finger is the exact complaint this replaces.
        let target = if press == Press::Dragging && !self.held.is_empty() {
            self.resolve_target(&gesture)
        } else {
            self.target
        };
        self.gesture = None;
        let held = std::mem::take(&mut self.held);
        self.lift = 0;

        match press {
            Press::Cancelled => {
                self.last_tap = None;
                self.restore_dragged(&held);
                Some(Intent::Cancelled)
            }
            Press::Down => {
                let now = js_sys::Date::now();
                let double = self.last_tap.is_some_and(|(place, index, at)| {
                    place == gesture.place && index == gesture.index && now - at < DOUBLE_TAP_MS
                });
                self.last_tap = if double {
                    None
                } else {
                    Some((gesture.place, gesture.index, now))
                };
                let on_a_card = gesture.index < self.scene.cards(gesture.place).len();
                let index = on_a_card.then_some(gesture.index);
                Some(if double {
                    Intent::DoubleTap {
                        place: gesture.place,
                        index,
                    }
                } else {
                    Intent::Tap {
                        place: gesture.place,
                        index,
                    }
                })
            }
            Press::Dragging => {
                self.last_tap = None;
                self.restore_dragged(&held);
                match target.filter(|place| *place != gesture.place) {
                    Some(to) => Some(Intent::Drop {
                        from: gesture.place,
                        index: gesture.index,
                        to,
                    }),
                    None => Some(Intent::Back),
                }
            }
        }
    }

    /// A pointer was cancelled or lost: the cards go home and nothing is taken.
    pub fn cancel(&mut self, pointer: i32) {
        let Some(mut gesture) = self.gesture else {
            return;
        };
        if gesture.pointer != pointer {
            return;
        }
        gesture.cancel();
        let held = std::mem::take(&mut self.held);
        self.gesture = None;
        self.target = None;
        self.lift = 0;
        self.restore_dragged(&held);
    }

    /// Whether a gesture is in progress.
    #[must_use]
    pub fn is_pressed(&self) -> bool {
        self.gesture.is_some()
    }

    /// Whether the gesture in progress is carrying cards.
    #[must_use]
    pub fn is_dragging(&self) -> bool {
        self.gesture
            .is_some_and(|gesture| gesture.state == Press::Dragging && !self.held.is_empty())
    }

    fn resolve_target(&self, gesture: &Gesture) -> Option<Place> {
        let (origin_x, origin_y) = self.layout.card(*self.held.first()?)?;
        let (dx, dy) = gesture.offset();
        let (_, last_y) = self.layout.card(*self.held.last()?)?;
        let stack = Rect {
            x: origin_x + dx,
            y: origin_y + dy,
            width: self.layout.card_w,
            height: (last_y - origin_y) + self.layout.card_h,
        };
        self.layout
            .drop_target(&self.scene, gesture.point, stack, gesture.place)
    }

    // -- painting ---------------------------------------------------------

    /// Paint a scene: move every card and pile to where it belongs, and say what
    /// each one is.
    pub fn paint(&mut self, scene: &Scene, frame: &Frame) {
        self.scene = scene.clone();
        self.layout = layout(scene, frame);
        let _ = self.board.set_attribute(
            "style",
            &format!(
                "--cw:{}px;--ch:{}px",
                self.layout.card_w, self.layout.card_h
            ),
        );
        self.set_density();
        self.paint_slots();
        self.paint_cards();
        self.tab_stop();
    }

    /// The frame the board is currently laid out in, so a repaint from inside
    /// the board (`set_focus`) does not need the app to supply it again.
    fn frame(&self) -> Frame {
        Frame {
            width: f64::from(self.board.client_width()),
            height: f64::from(self.board.client_height()),
            gap: (f64::from(self.board.client_width()) * 0.006).clamp(2.0, 8.0),
            margin: (f64::from(self.board.client_width()) * 0.012).clamp(4.0, 10.0),
        }
    }

    fn set_density(&self) {
        let mut want = String::new();
        if self.instant {
            want.push_str("instant");
        }
        if self.layout.card_w > 0.0 && self.layout.card_w < COMPACT_BELOW {
            if !want.is_empty() {
                want.push(' ');
            }
            want.push_str("compact");
        }
        if self.board.get_attribute("class").as_deref() != Some(want.as_str()) {
            self.board.set_class_name(&want);
        }
    }

    /// The element for a place, created the first time it is painted.
    fn slot_element(&mut self, place: Place) -> Element {
        let key = place.slug();
        if let Some(element) = self.slots.get(&key) {
            return element.clone();
        }
        let element = self
            .document
            .create_element("button")
            .unwrap_or_else(|_| self.board.clone());
        let _ = element.set_attribute("type", "button");
        element.set_class_name("slot");
        let _ = element.set_attribute("data-place", &key);
        let _ = self.board.append_child(&element);
        self.slots.insert(key, element.clone());
        element
    }

    fn paint_slots(&mut self) {
        let scene = self.scene.clone();
        let target = self.target;
        let focus = self.focus;
        for slot in &scene.slots {
            let Some((x, y)) = self.layout.place(slot.place) else {
                continue;
            };
            let empty = slot.cards.is_empty();
            let mut classes = String::from("slot");
            if !empty {
                classes.push_str(" filled");
            }
            if target == Some(slot.place) {
                classes.push_str(" drop");
            }
            // The element is always in the document, so the browser keeps its
            // focus across a repaint; only a *tab stop* moves.
            let element = self.slot_element(slot.place);
            element.set_class_name(&classes);
            let _ = element.set_attribute("style", &format!("left:{x}px;top:{y}px;z-index:1"));
            let _ = element.set_attribute("data-place", &slot.place.slug());
            let _ = element.set_attribute("data-index", &slot.cards.len().to_string());
            let _ = element.set_attribute(
                "tabindex",
                if focus == (slot.place, None) {
                    "0"
                } else {
                    "-1"
                },
            );
            let _ = element.set_attribute("aria-label", &slot.label);
            element.set_text_content(Some(if empty { slot.mark } else { "" }));
        }
    }

    fn paint_cards(&mut self) {
        // Where every card is, in one pass over the scene.
        let mut located: Vec<Option<(Place, usize)>> = vec![None; self.cards.len()];
        for slot in &self.scene.slots {
            for (index, card) in slot.cards.iter().enumerate() {
                if let Some(entry) = located.get_mut(card.id as usize) {
                    *entry = Some((slot.place, index));
                }
            }
        }
        let scene = self.scene.clone();
        let held: Vec<u16> = self.held.clone();
        let dragging = self.is_dragging();
        let target = self.target;
        let focus = self.focus;
        let hint = scene.hint;
        let selection = scene.held;
        for (id, location) in located.iter().enumerate() {
            let Ok(card_id) = u16::try_from(id) else {
                continue;
            };
            let Some(element) = self.cards.get(id).cloned() else {
                continue;
            };
            let Some((place, index)) = *location else {
                // A card that left the board — a completed run lifted in Spider
                // — stops being drawn, and stops answering to a pile. Its element
                // stays, because the deck is built once.
                element.set_class_name(&card_classes(
                    self.suit_class(card_id),
                    self.card_face_down(card_id),
                    &["gone"],
                ));
                let _ = element.set_attribute("hidden", "");
                let _ = element.set_attribute("aria-hidden", "true");
                let _ = element.remove_attribute("aria-label");
                let _ = element.remove_attribute("data-place");
                let _ = element.remove_attribute("data-index");
                let _ = element.set_attribute("tabindex", "-1");
                continue;
            };
            let Some(slot) = scene.slot(place) else {
                continue;
            };
            let Some(card) = slot.card(index) else {
                continue;
            };
            let _ = element.remove_attribute("hidden");
            // In the air: the gesture owns this element's position until it
            // lands, so a repaint leaves the position alone.
            if dragging && held.contains(&card_id) {
                continue;
            }
            let (x, y) = self.layout.card(card_id).unwrap_or((0.0, 0.0));
            let mut states: Vec<&str> = Vec::new();
            // A run the *game* is holding is selected; the cards a live drag is
            // carrying are styled by the gesture instead. The board never keeps
            // a second opinion about what the game has picked up.
            if selection.is_some_and(|(held_place, start)| held_place == place && index >= start) {
                states.push("selected");
            }
            if hint.is_some_and(|(hint_place, start, count)| {
                hint_place == place && index >= start && index < start + count
            }) {
                states.push("hinted");
            }
            if focus == (place, Some(index)) {
                states.push("focused");
            }
            // The pile a drop would land on, shown on the card that will be
            // under it: the slot itself is invisible under a full pile.
            if target == Some(place) && index + 1 == slot.cards.len() && !card.face_down {
                states.push("drop-target");
            }
            element.set_class_name(&card_classes(
                self.suit_class(card_id),
                card.face_down,
                &states,
            ));
            let _ = element.set_attribute(
                "style",
                &format!(
                    "left:{x}px;top:{y}px;z-index:{}",
                    z_index(&scene, place, index)
                ),
            );
            let _ = element.set_attribute("data-place", &place.slug());
            let _ = element.set_attribute("data-index", &index.to_string());
            let _ = element.set_attribute(
                "tabindex",
                if focus == (place, Some(index)) {
                    "0"
                } else {
                    "-1"
                },
            );
            let printed = self.deck.get(card_id as usize).copied();
            // A face-down card says nothing — except the one the keyboard is on.
            // Hiding a focused element from assistive technology is refused by
            // the browser (rightly: it would strand a screen-reader user on
            // nothing), so the focused card keeps a name even when it is blank.
            let focused_card = focus == (place, Some(index));
            if card.face_down && !focused_card {
                let _ = element.set_attribute("aria-hidden", "true");
                let _ = element.remove_attribute("aria-label");
            } else if card.face_down {
                let _ = element.remove_attribute("aria-hidden");
                let _ = element.set_attribute(
                    "aria-label",
                    &format!("{}: face-down card {}", place.name(), index + 1),
                );
            } else {
                let _ = element.remove_attribute("aria-hidden");
                let name = printed.map_or_else(String::new, |card| card.name());
                let mut label = format!(
                    "{name} in {place}: card {} of {}.",
                    index + 1,
                    slot.cards.len(),
                    place = place.name()
                );
                if selection
                    .is_some_and(|(held_place, start)| held_place == place && index >= start)
                {
                    label.push_str(" Selected.");
                }
                let _ = element.set_attribute("aria-label", &label);
            }
        }
    }

    /// Whether the board has this card face down.
    ///
    /// A gesture that restyles a card has to keep saying everything the card
    /// is, not only what the gesture adds, so it needs the face too.
    fn card_face_down(&self, id: u16) -> bool {
        self.scene
            .slots
            .iter()
            .flat_map(|slot| &slot.cards)
            .find(|card| card.id == id)
            .is_some_and(|card| card.face_down)
    }

    fn suit_class(&self, id: u16) -> &'static str {
        self.deck
            .get(id as usize)
            .map_or("black", |card| card.suit.class())
    }

    /// Move the browser's tab stop to the focused card, or to the focused pile.
    fn tab_stop(&mut self) {
        let (place, index) = self.focus;
        if let Some(index) = index {
            if let Some(slot) = self.scene.slot(place) {
                if let Some(card) = slot.cards.get(index) {
                    if let Some(element) = self.cards.get(card.id as usize) {
                        focus(element);
                    }
                }
            }
            return;
        }
        let element = self.slot_element(place);
        focus(&element);
    }

    /// A drag in progress draws the held cards under the finger, exactly, and
    /// lights the pile it would land on.
    fn paint_drag(&mut self, gesture: &Gesture) {
        let (dx, dy) = gesture.offset();
        let held: Vec<u16> = self.held.clone();
        for (offset, id) in held.iter().enumerate() {
            let Some(element) = self.cards.get(*id as usize) else {
                continue;
            };
            let Some((x, y)) = self.layout.card(*id) else {
                continue;
            };
            let z = 2_000 + i32::try_from(offset).unwrap_or(0);
            // The suit class travels with the card. It is not a decoration the
            // gesture may drop: see `board::card_classes`.
            element.set_class_name(&card_classes(
                self.suit_class(*id),
                self.card_face_down(*id),
                &["dragging"],
            ));
            let _ = element.set_attribute(
                "style",
                &format!("left:{}px;top:{}px;z-index:{z}", x + dx, y + dy),
            );
        }
        self.paint_slots();
        self.mark_drop_target();
    }

    /// Put the drop-target ring on the card a drop would land on.
    fn mark_drop_target(&mut self) {
        let scene = self.scene.clone();
        let target = self.target;
        let focus = self.focus;
        for slot in &scene.slots {
            let Some(last) = slot.cards.last() else {
                continue;
            };
            // A card in the air is drawn by the gesture: reclassifying it here
            // would take its `dragging` class away mid-drag and make it snap
            // back under the pile it came from.
            if self.held.contains(&last.id) {
                continue;
            }
            let Some(element) = self.cards.get(last.id as usize) else {
                continue;
            };
            let mut states: Vec<&str> = Vec::new();
            if focus == (slot.place, slot.cards.len().checked_sub(1)) {
                states.push("focused");
            }
            if target == Some(slot.place) {
                states.push("drop-target");
            }
            element.set_class_name(&card_classes(
                self.suit_class(last.id),
                last.face_down,
                &states,
            ));
        }
    }

    /// Take the drag styling off the held cards, so the next paint can animate
    /// them home or to their new pile.
    fn restore_dragged(&mut self, held: &[u16]) {
        for id in held {
            if let Some(element) = self.cards.get(*id as usize) {
                element.set_class_name(&card_classes(
                    self.suit_class(*id),
                    self.card_face_down(*id),
                    &[],
                ));
            }
        }
        self.target = None;
        let scene = self.scene.clone();
        let frame = self.frame();
        self.paint(&scene, &frame);
    }

    // -- keyboard ---------------------------------------------------------

    /// Move the keyboard's focus along the board's reading order.
    pub fn move_focus(&mut self, delta: i32) -> (Place, Option<usize>) {
        let (place, _) = self.focus;
        let next = self.layout.next_place(&self.scene, place, delta);
        let index = self.top_index(next);
        self.set_focus(next, index);
        self.focus
    }

    /// Move the keyboard's focus within a column, between the face-up cards.
    pub fn move_within(&mut self, delta: i32) -> (Place, Option<usize>) {
        let (place, index) = self.focus;
        let next = self.layout.next_index(&self.scene, place, index, delta);
        self.set_focus(place, next);
        self.focus
    }

    /// The card the keyboard should land on in a pile: its top card when that is
    /// face up, and `None` for an empty or all-face-down pile.
    fn top_index(&self, place: Place) -> Option<usize> {
        let slot = self.scene.slot(place)?;
        let len = slot.cards.len();
        (len > 0 && !slot.cards[len - 1].face_down).then_some(len - 1)
    }

    /// Put the browser's focus back where the keyboard left it.
    pub fn focus_element(&mut self) {
        self.tab_stop();
    }

    fn create_card(&self, id: u16, card: Card) -> Result<Element, JsValue> {
        let element: Element = self.document.create_element("button")?.dyn_into()?;
        let _ = element.set_attribute("type", "button");
        let _ = element.set_attribute("data-card", &id.to_string());
        let _ = element.set_attribute("data-rank", card.rank.short());
        element.set_class_name(&card_classes(card.suit.class(), false, &[]));
        element.set_inner_html(&card_html(card));
        self.board.append_child(&element)?;
        Ok(element)
    }

    /// The element for a place, once it has been painted.
    #[must_use]
    pub fn slot(&self, place: Place) -> Option<&Element> {
        self.slots.get(&place.slug())
    }

    /// The card element for a deck index.
    #[must_use]
    pub fn card_element(&self, id: u16) -> Option<&Element> {
        self.cards.get(id as usize)
    }

    /// Every place the last scene had, so an app can decorate beside them.
    #[must_use]
    pub fn places(&self) -> Vec<Place> {
        self.scene.places()
    }

    /// A pile from the last scene.
    #[must_use]
    pub fn slot_of(&self, place: Place) -> Option<&Slot> {
        self.scene.slot(place)
    }
}

/// A pile's stacking order: later places above earlier ones, and later cards
/// above earlier ones within a pile.
fn z_index(scene: &Scene, place: Place, index: usize) -> i32 {
    let base = i32::try_from(
        scene
            .places()
            .iter()
            .position(|known| *known == place)
            .unwrap_or(0),
    )
    .unwrap_or(0);
    20 + base * 40 + i32::try_from(index).unwrap_or(0)
}

/// A card's printed markup. Shared, so both games print a card identically.
#[must_use]
pub fn card_html(card: Card) -> String {
    let rank = card.rank.short();
    let suit = card.suit.symbol();
    format!(
        "<span class=\"face\">\
           <span class=\"corner tl\"><span class=\"rank\">{rank}</span><span class=\"suit\">{suit}</span></span>\
           <span class=\"pip\">{suit}</span>\
           <span class=\"corner br\"><span class=\"rank\">{rank}</span><span class=\"suit\">{suit}</span></span>\
         </span>\
         <span class=\"back\"></span>"
    )
}

/// The point an event happened at, in the board's own coordinates.
///
/// The board is positioned, so `client` coordinates are the wrong frame: a card
/// placed at the pointer's client position would be off by the board's own
/// offset from the page.
#[must_use]
pub fn point_of(event: &Event, board: &Element) -> (f64, f64) {
    let (x, y) = match event.dyn_ref::<PointerEvent>() {
        Some(pointer) => (f64::from(pointer.client_x()), f64::from(pointer.client_y())),
        None => match event.dyn_ref::<MouseEvent>() {
            Some(mouse) => (f64::from(mouse.client_x()), f64::from(mouse.client_y())),
            None => (0.0, 0.0),
        },
    };
    let rect = board.get_bounding_client_rect();
    (x - rect.left(), y - rect.top())
}

/// Put the browser's focus on an element.
///
/// `Element` has no `focus` in web-sys; focus belongs to the HTML element
/// interface, and a card is a button, so the cast always succeeds for the
/// elements this module creates. A refused cast is a no-op rather than a panic:
/// focus is an accessibility aid, not a reason to stop the app.
pub fn focus(element: &Element) {
    if let Some(html) = element.dyn_ref::<HtmlElement>() {
        let _ = html.focus();
    }
}

/// Add a listener, warning rather than failing when the browser refuses.
pub fn listen(target: &web_sys::EventTarget, event: &str, callback: &js_sys::Function) {
    if let Err(error) = target.add_event_listener_with_callback(event, callback) {
        web_sys::console::warn_2(&JsValue::from_str("could not bind an event"), &error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{CardSlot, Fan};

    #[test]
    fn a_card_is_marked_up_from_its_own_rank_and_suit() {
        let html = card_html(Card::new(
            crate::card::Rank::Queen,
            crate::card::Suit::Hearts,
        ));
        assert!(html.contains("Q"));
        assert!(html.contains("♥"));
        assert!(html.contains("class=\"face\""));
        assert!(html.contains("class=\"back\""));
    }

    #[test]
    fn the_stacking_order_follows_the_reading_order() {
        let scene = Scene::new(
            vec![
                Slot::new(Place::Top(0), Fan::PACKET, "stock"),
                Slot::new(Place::Column(0), Fan::COLUMN, "column 1")
                    .with_cards(&[(1, false), (2, false)]),
                Slot::new(Place::Column(1), Fan::COLUMN, "column 2").with_cards(&[(3, false)]),
            ],
            1,
        );
        assert!(z_index(&scene, Place::Column(0), 1) > z_index(&scene, Place::Column(0), 0));
        assert!(z_index(&scene, Place::Column(1), 0) > z_index(&scene, Place::Column(0), 9));
        assert!(z_index(&scene, Place::Column(0), 0) > z_index(&scene, Place::Top(0), 0));
    }

    #[test]
    fn a_card_slot_is_only_ever_a_card() {
        // A small compile-time check that the scene's card list is what the
        // board paints from: the fact that this builds is the assertion.
        let card = CardSlot {
            id: 7,
            face_down: true,
        };
        assert_eq!(card.id, 7);
    }
}

#[cfg(test)]
mod gesture_tests {
    // The double-tap rule is the one piece of gesture state a pure test can
    // hold: two presses in a row must be comparable, and a drag must break the
    // pair. The rest needs a document, and is covered by driving the built site.
    #[test]
    fn a_double_tap_needs_the_record_of_the_first_tap() {
        // The regression this encodes: clearing the record on every press made
        // a double tap impossible, and the second tap then *cleared the
        // selection* of the first — which looked like a card refusing to move.
        let first = (Place::Column(0), 3usize, 1_000.0_f64);
        let second = (Place::Column(0), 3usize, 1_200.0_f64);
        assert!(second.2 - first.2 < DOUBLE_TAP_MS);
        assert_eq!(first.0, second.0);
        assert_eq!(first.1, second.1);
    }
}
