# Solitaire

Klondike solitaire that works offline: draw-one and draw-three deals, three
difficulty settings, standard, Vegas and no-score play, hints, unlimited undo
and an automatic finish. It installs as an app, and after one visit it plays
with the network off.

All of the rules, the scoring and the rendering are Rust; the page is a static
shell and `cargo build` writes the publishable site into `dist/`.

## Play

- Tap a card to pick it up, then tap where it should go. Drag works too, and a
  double tap sends a card to a foundation or onto the best column.
- Build the columns down in alternating colours; send aces to the foundations
  and continue upwards in suit. An empty column takes a king.
- **Undo** takes back anything, including a turn of the stock. **Hint** rings
  the card to pick up and the place it goes — and only ever suggests a move that
  changes something, so pressing it twice cannot send you round in circles.
  **Finish** appears once every card is face up.
- The menu holds the two choices that define the deal, and both apply to the
  *next* one so that changing your mind never discards a game in progress:

  | Difficulty | Draw | Passes through the stock |
  |-----------|------|--------------------------|
  | Easy      | one at a time | unlimited |
  | Medium    | three at a time | unlimited |
  | Hard      | three at a time | three |

  Scoring is **Standard** (the classic points table), **Vegas** (a 52-coin
  stake, five coins per card sent home, one pass) or **No score**.

- Held sideways there is no row of controls under the table: the buttons move to
  a rail beside it, so the cards get the whole height of the screen.
- Keyboard: arrow keys move around the board, Enter picks up or drops, `U`
  undoes, `H` hints, `D` turns the stock, Escape puts a held card back.

## Build

Two steps, because there are two targets. Nothing generated is committed.

```bash
rustup target add wasm32-unknown-unknown
# the generator must be the version Cargo.lock records
cargo install wasm-bindgen-cli --locked \
  --version "$(sed -n '/^name = "wasm-bindgen"$/{n;s/^version = "\(.*\)"$/\1/p;}' Cargo.lock)"

# 1. the site: compile to wasm and generate the JS bindings
cargo build --locked --lib --target wasm32-unknown-unknown --release
wasm-bindgen --target web --no-typescript --out-dir dist --out-name app \
  target/wasm32-unknown-unknown/release/solitaire.wasm

# 2. the rest of the site
touch build.rs
cargo build --release --locked
```

That leaves a publishable `dist/`. **The order matters**: step 2 derives the
service worker's cache name from the wasm, so running it first would pin that to
the previous build. The `touch` matters too — `build.rs` writes into the source
tree, so cargo would otherwise skip the second build and copy nothing.

`cargo test --locked` needs none of the above.

## Layout

    src/card.rs            the deck, the printed face and the stored code
    src/game.rs            the rules, the score, the undo history, the words
    src/ui.rs              the browser layer: the only file that touches the DOM
    src/ui.html            the static shell
    src/style.css          the styles, including the card format
    src/service-worker.js  install, activate, fetch; one __VERSION__ placeholder
    build.rs               writes six of the nine files in dist/, derives the rest
    assets/icon.svg        the icon, committed and authoritative
    tests/shell.rs         invariants of the committed shell
    AGENTS.md              the decisions, and how to verify them

## Card format

`src/card.rs` and the `.card` block in `src/style.css` are shared with
[Spider Solitaire](https://github.com/bot-git-ai/spider_solitaire): the same
rank and suit order, the same two-letter storage code, and the same printed
faces. The two are separate repositories, so the copy is kept in step by hand —
a change to one belongs in the other, or the two solitaires stop looking like
one family.

## Licence

AGPL-3.0-only. See `LICENSE`.
