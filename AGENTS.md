# AGENTS.md

Decisions and verification for this app. `INSTRUCTIONS.md` lives one level up,
in the Rust PWA template: this crate was cut from it, and every rule there
applies here. What follows is what is specific to this game.

## Shape

- `src/card.rs` — the deck: `Rank`, `Suit`, `Card`, the two-character storage
  code, and the printed short form. No `web-sys`, so `cargo test` runs it.
- `src/game.rs` — Klondike: the deal, the move grammar, the score, the undo
  history, the hint heuristic, the save format, and **every word the page
  shows**. Also `web-sys`-free.
- `src/scene.rs` — the game as the board draws it: the piles, how each fans,
  how many cells each occupies, where the keyboard starts. Compiled for the
  host too, so `cargo test` checks the board's *arrangement* rather than only
  its arithmetic.
- `src/solver.rs` — whether the deal can still be won: the positions it can
  reach, and the walk that visits them. No `web-sys` either, so the verdicts are
  tested natively.
- `src/ui.rs` — the browser half: a `Rules` implementation and nothing else.
  The board itself is [`src/board_ui.rs`], the page around it is
  [`src/shell.rs`], and both are shared with the other solitaire.
- `src/ui.html`, `src/style.css`, `src/service-worker.js`, `build.rs`,
  `assets/icon.svg` — the shell, the styles, the worker, the site writer and the
  icon, as the template describes them.

## Decisions

**Difficulty is the deal's rules, not a hidden dice roll.** Klondike has no
difficulty knob in its rules, so the three settings are the two that change how
the game plays: how many cards the stock turns over, and how many times the
waste may be turned back. Easy is draw-one unlimited; Medium is draw-three
unlimited; Hard is draw-three with three passes. The menu names the rule beside
the label (`Turn three cards at a time, with three passes.`), because "Hard" on
its own tells the reader nothing they can act on.

**Scoring is a choice of three, and "No score" is a real one.** Standard is the
classic table (waste→column 5, →foundation 10, foundation→column −15, turning a
card 5, recycling the waste −100 at draw-one and −20 at draw-three, floored at
zero). Vegas is a 52-coin stake, five coins a card, one pass. Off is the game and
the clock with no arithmetic. A player who does not care about points should not
have a number in their header pretending otherwise.

**Options apply to the next deal.** The menu's selects are a request, not a
command: changing difficulty mid-game would either discard the game (a
data-loss bug wearing a settings menu) or silently keep the old rules under a new
label. They are stored separately from the game, so they outlive "New game" and
survive the page being evicted from memory.

**One transaction per mutation.** Every move, draw and recycle takes a snapshot
first; undo is then correct by construction rather than by remembering, per
feature, what it changed. The history is capped at 500 entries — a snapshot is
seven small vectors and five numbers, so the cap is a bound, not a policy.

**Every deal is a seed.** "Restart deal" is honest because the same seed deals
the same game, and a save is a seed plus the piles rather than 52 card objects.

**A saved game is checked, not trusted.** `Game::from_stored` refuses a save
unless every card appears exactly once, every code is a real card, every
foundation is an ascending single-suit run, and the won flag agrees with the
foundations. A corrupt `localStorage` entry must never be able to stop the app
from starting, and a half-valid one must never put one card in two piles.

**Cards are placed with `left`/`top`, not `transform`.** A transform transition
is compositor-driven: a renderer that does not advance animations leaves every
card at the position it was first drawn at, which is a board that looks empty and
works. `left`/`top` animate on the main thread, so the card is in the right place
even at frame zero and the movement between frames is the decoration rather than
the fact. The board also carries `.instant` for its first paint, so the deal
arrives instantly instead of flying in from the corner.

**A pile that fans sideways is given the cells it fans into.** A fan spreads by
a fraction of the card's height, which is wider than a card's own cell, so an
unbounded fan leaves its cell and the pile beside it paints over it — which is
how three drawn cards came to hide under the first foundation. The waste
occupies two cells and the board is laid out by accumulated spans rather than by
pile index, so the top row still adds up to exactly the seven columns: one for
the stock, two for the waste, four for the foundations. The fan is then clamped
to its own cell, with a floor so three cards stay three cards at phone size.

**A pile's hit area is the union of its cards, not one card plus a height.** A
height is right for a column, which fans downwards, and wrong for a pile that
fans sideways: a three-card waste put two cards outside a one-card-wide box, so
a press on them hit no pile at all and the board felt dead exactly where a player
aims. The box is now the union of the cards the pile draws.

**A pile that shows only its last few cards answers with its top card.** Only the
right-hand waste card can be played, so a tap on a sliver of an earlier one
answers with its top and says something useful. Answering "that card cannot move"
about the card under the finger is technically true and feels like the board is
ignoring you. A column is different: it exposes every face-up card, because
picking a run up from its middle is the point of a tableau.

**A double tap only ever sends a card home, and does nothing otherwise.** It used
to fall back to "the best column", which meant a quick second tap could move a
card somewhere the player never chose — and on a touch screen a quick second tap
is what a player does when unsure the first one registered. A tap on a face-down
card with nothing in hand now says nothing at all: a card back is not a control.

**The board is its own stacking context (`isolation: isolate`).** A card's
`z-index` is what keeps a fanned column in order, and those numbers run into the
hundreds. Without the isolation a card paints over the menu sheet and over the
victory dialog, both of which are deliberately above the board.

**A card's class list is built in one place, and it always carries the suit
class.** `.card.red` is the *only* rule that paints a red rank red, so a class
list without the suit class is a red card drawn black. The drag built its own —
`set_class_name("card dragging")` — so picking a red card up turned it black
under the finger and red again when it landed. Four callers were each appending
the states they knew about, and they had already drifted the same way: the
repaint that marks a drop target silently dropped a hint sitting on the same
card.

`board::card_classes(suit, face_down, states)` is now the only builder, in
`board.rs` because that is compiled for the host — `board_ui.rs` is wasm-only, so
a test written in it would never run under `cargo test` (this family has been
bitten by that once already). Two tests hold it: `a_card_keeps_its_suit_class_in_every_state`
checks the builder against every state a card can be in, and
`a_cards_class_list_is_built_in_one_place` (in `tests/shell.rs`) refuses any
`set_class_name("card …")` or `format!("card …")` in `board_ui.rs`. Both fail on
the old code — the structural one names the offending line.

**One listener per board, not one per card.** A press is read off the element it
landed on; that element carries its pile and its position in the pile. Cards are
never rebuilt — a render moves them and rewrites their description — so a drag in
progress survives a repaint, and the transition has an element to travel with.

**The menu is a checkbox.** Opening it depends on `change` on a checkbox whose
label is the button, and the scrim and the Done control are labels for the same
box. A panel opened only by a `click` listener does not open on a phone; this
family has learned that once already.

**A hint marks both ends of the move, and only ever suggests a move that
changes the deal.** Two faults, and both made Hint useless in a way no test
noticed:

- It scored *legal* moves, and a move between two columns is legal in both
  directions whenever both end in a matching rank and colour. So the hint named
  a card, the player followed it, and the next press named the card it had just
  left — a card that cannot be improved where it is, and a column that gets
  nothing, pointed at alternately forever. A move is now worth considering only
  if it turns a face-down card over, empties a column, or uncovers a card that
  can go home (`Game::column_gain`); a whole column onto an empty one is
  excluded outright, because it empties one column and fills another and the
  deal has not moved. `following_the_hint_never_walks_in_circles` follows 25
  deals for 40 presses each and refuses a repeat, and it fails on the old
  scoring.
- It drew a ring around the card to pick up and said the destination in words.
  A ring says "this one" and cannot say "there", and "foundation 3" is a poor
  substitute for pointing at the third foundation. The destination now gets the
  same ring in the same colour (`#hint-mark`, painted by `Klondike::paint_hint`),
  and a hint about the stock needs no second mark because the stock is both ends
  of it.

The mark is `pointer-events: none` and a sibling of the board, not a child: the
destination must stay pressable underneath it, and the board's own stacking
context is what keeps its hundreds of card `z-index`es out of the page.

**The finish happens by itself.** The Finish button used to be a chore: every
card uncovered means the rest of the deal is a formality, and being asked to
press a button (then wait, then watch) for a result that is no longer in doubt is
work in place of play. So the page asks the game after every change whether it
is now finishing (`Game::finishing`), and a deal that is starts at once —
wherever the move that made it a formality came from, including a deal restored
mid-finish on page load.

Exactly one thing stops it: taking a card back out of a running finish with Undo
sets `finish_held`, because an undo the finish immediately redoes is a broken
undo. That leaves a **Finish** button in the row, and the button appears in
exactly that state — the board ready and the finish held — so it is never a
control that cannot do anything.

A board with every card up and nothing that can go home is *not* stopped, and
deliberately so. Every card being face up does not mean every card is reachable:
a run moved by hand can uncover the one that was in the way, so the finish asking
again after the next move is right and a flag saying "over" would be wrong. The
step returns nothing, the page stops its timer, and the player's own move is what
is on the status line — the finish takes over again the moment a card is
reachable.

**"Stuck?" answers a question the hint cannot, and says which answer it has.**
`src/solver.rs` searches the positions reachable from the current board for a
won one. The three answers are kept apart on purpose:

- **Not stuck** — a winning line exists. The search reached a won position.
- **Stuck** — every reachable position was visited and none of them wins.
- **Not sure** — the budget ran out first.

`Stuck` is a claim the search has *proved*, so it may only be said when the walk
finished; `Not sure` is the search admitting it did not get far enough. Printing
the first when it means the second would cost a player a game they could still
have won, so the two never share a sentence (`check_words`).

Loops are the whole reason this is a search and not a move count. A deal whose
only moves lead back to where they came from is dead, not "still exploring", and
the transposition table is what distinguishes the two: every position reached is
recorded by a hash of exactly the cards and counts in play, so a move back onto a
board already seen is dropped instead of followed for ever. The count of redeals
is part of that hash only when the deal limits it — with unlimited passes, a
board turned over twice and the same board turned over three times are the same
place to play from, and telling them apart is what would stop the walk from
ever closing.

Two moves are deliberately not generated, both because they are the same board
under another name: a whole column onto an empty column, and a card onto an empty
column that is not the first empty one. Every empty column is interchangeable, so
collapsing them is what keeps the search from re-walking the same position under
every numbering of the columns.

`Position` is a *second* copy of Klondike's rules, small and copyable because the
walk copies a board tens of thousands of times. Two tests hold the copies
together, and they are the reason the verdict can be trusted at all: one plays
every move the search generates on a real `Game` and requires the boards to
match, and the other enumerates every move the *game* allows and requires the
search to offer it. The second is the one a "Stuck" verdict rests on — a search
that silently skipped a move would call a winnable deal stuck.

The walk is depth first with its own stack, so it runs in slices between frames
rather than freezing the board for its whole length: 18 000 positions a slice, a
hard budget of 1 200 000, and the line under the board says what is happening
instead of leaving a dead button. Measured in the browser, the walk examines
about 1.7 million positions a second and the full budget takes 1.1s at 60fps;
most answers arrive long before it.

Measured against the 99 positions a random player actually gets stuck in, the
answer is `Not stuck` or `Stuck` for 96 of them, and the slowest of those takes
0.12s — because a game that has really run out of moves has a small graph to
close. The rare deal the budget runs out on says so. From a *fresh* deal it is
usually `Not sure`, which is the honest answer: a won position is fifty-odd moves
away, and about half of Easy deals are proven winnable within the budget, which
is not enough to justify waiting longer for the rest. The button is worth
pressing when the deal feels stuck, which is exactly when the search can close
the graph and answer.

**Controls move to a side rail on a wide short screen.** The board is laid out
from the *height* the page gives it, so every row of chrome above and below is a
row of cards lost. Held upright there is room to spare; held sideways the four
buttons and the status line were taking most of the height the cards got — 51px
cards on a rotated phone, 36px on a small one, half what the same deal got
upright. In a landscape shape (`min-width: 30rem and max-height: 34rem`) the pad
goes against the right edge, in a grid that wraps as the rail has room, with the
facts stacked above it in the same rail, and the table keeps the full height:
73px and 57px for those two sizes.

Nothing is folded behind a disclosure, and that was the decision rather than a
shortcut: a control that needs two taps to reach is a control a player mid-game
does not use, and the reason to fold them is height, which the rail already
reclaims. Every target stays 44px tall, because buying space by making the
buttons harder to hit has not solved anything. The condition is the *shape*, not
the device: a short wide window on a desktop gets the same treatment and a tall
one never does. Below `13rem` of height — a narrow split window, not a phone —
three stacked facts no longer fit the rail, so they take a row above the board
and read left to right instead.

**A tap is a pointer gesture, not a `click`.** `pointerdown` records a press,
`pointermove` past a slop threshold turns it into a drag, `pointerup` decides
between the two, and `pointercancel` abandons it. `click` is handled only for
keyboard activation (`event.detail === 0`), because handling both paths would
move every card twice.

## Verification

```bash
cargo test --locked                 # the rules, the save format, the shell, the card's classes
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --lib --target wasm32-unknown-unknown -- -D warnings
```

Then both build steps, in order, and check `dist/` — see `README.md`. The built
site was also driven in a headless browser against the real page: draw, undo,
hint, tap-to-move, drag-to-move, double-tap-to-foundation, the menu's difficulty
and scoring change, the automatic finish through to the victory dialog, the
keyboard, and the service worker registering and taking control. That is not a
committed test — the browser is not part of the toolchain here — and it is
written down so the next reader knows what "verified" covered.

What is *not* covered by a test: the layout's geometry. Card sizes and fan
offsets depend on the box the board gets, and the only honest check is to look at
it at a few sizes. The landscape rail was checked that way at 844x390, 667x375,
640x280, 568x320, 640x240, 480x200 and 1400x420, with portrait 390x844 and
360x640 as the controls that must not have changed, and by measuring the board's
box and one card's width in the browser rather than trusting the picture.

## Sharing the card format with Spider Solitaire

`src/card.rs` and the `.card` block of `src/style.css` are also carried by
[spider_solitaire](https://github.com/bot-git-ai/spider_solitaire). The two are
separate repositories, so nothing enforces that they agree; the rule is that a
change to either belongs in both. The deck, the codes and the printed faces are
the shared surface; the piles, the scoring and the layout are not.
