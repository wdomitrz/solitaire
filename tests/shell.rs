//! Invariants of the committed shell, and of what is and is not committed.
//!
//! Everything here reads committed files. `dist/` is build output and is
//! gitignored, so the release gate — which exports the candidate tree — never
//! has it and cannot build it: that needs the wasm target and a pinned
//! `wasm-bindgen` CLI. What covers the built output is running the two build
//! steps in the order `AGENTS.md` gives them, and `.github/workflows/build.yml`
//! does that on every push.
//!
//! What these tests protect is that the shell cannot quietly grow application
//! JavaScript again, cannot point anywhere absolute, cannot commit a generated
//! file, cannot lose the icon every install PNG is derived from, and cannot
//! precache a file the build does not write — that last one costs the whole
//! service worker, because `caches.addAll` rejects the entire install when any
//! listed URL 404s.

use std::path::Path;

/// The repository root, as the tests see it.
fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// A committed file, as text.
fn read(relative: &str) -> String {
    let path = root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("reading {relative}: {error}"))
}

/// The app shell, as committed. `build.rs` copies it into `dist/index.html`
/// byte for byte, so asserting on it asserts on what gets published.
fn shell() -> String {
    read("src/ui.html")
}

fn style() -> String {
    read("src/style.css")
}

fn worker_template() -> String {
    read("src/service-worker.js")
}

/// The two `wasm-bindgen` artefacts, and `build.rs` does not write them.
const WASM_BINDGEN_FILES: [&str; 2] = ["app.js", "app_bg.wasm"];

/// Tracked file names, or `None` outside a checkout.
///
/// The release gate exports the candidate as a bare directory with no `.git`,
/// so there is no index to ask. Callers decide what that means.
fn tracked_files() -> Option<String> {
    let inside = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(root())
        .output()
        .is_ok_and(|out| out.status.success());
    if !inside {
        return None;
    }
    let output = std::process::Command::new("git")
        .args(["ls-files"])
        .current_dir(root())
        .output()
        .expect("git ls-files");
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The entries of the worker's `ASSETS` allowlist, parsed out of the template.
fn precached_by_the_worker() -> Vec<String> {
    let worker = worker_template();
    let list = worker
        .split_once("const ASSETS = [")
        .expect("the worker declares an ASSETS allowlist")
        .1
        .split_once(']')
        .expect("the end of the ASSETS allowlist")
        .0;
    list.split('\'')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// The files `build.rs` publishes, read out of the script itself.
///
/// Derived, never duplicated: a hand-kept list is a second copy of the truth,
/// and it drifts. The shell table and the icon are both read from the code that
/// writes them, and the install PNGs from the sizes that are actually
/// rasterized.
fn published_by_build_script() -> Vec<String> {
    let build = read("build.rs");

    let (_, shell) = build
        .split_once("const SHELL: &[(&str, &str)] = &[")
        .expect("build.rs declares the files it copies");
    let mut names: Vec<String> = shell
        .split_once("];")
        .expect("the end of the SHELL table")
        .0
        .lines()
        .filter_map(|line| line.trim().strip_prefix("(\""))
        .filter_map(|rest| rest.split('"').next())
        .map(str::to_owned)
        .collect();

    // The icon's published name, read as the constant's *value*: a name built at
    // runtime would satisfy a search for the constant and still ship an SVG no
    // allowlist can fetch.
    let icon_out = build
        .lines()
        .find_map(|line| line.trim().strip_prefix("const ICON_OUT: &str = \""))
        .and_then(|rest| rest.split('"').next())
        .expect("build.rs declares the icon's published name")
        .to_owned();
    names.push(icon_out);

    let declared = build
        .lines()
        .find_map(|line| line.trim().strip_prefix("const ICON_SIZES: [u32; 2] = ["))
        .and_then(|rest| rest.split(']').next())
        .unwrap_or_default();
    for size in declared
        .split(',')
        .filter_map(|size| size.trim().parse::<u32>().ok())
    {
        names.push(format!("icon-{size}.png"));
    }

    if build.contains("\"manifest.webmanifest\"") {
        names.push("manifest.webmanifest".to_owned());
    }
    if build.contains("template.replace(\"__VERSION__\"") {
        names.push("service-worker.js".to_owned());
    }
    names.sort();
    names
}

/// Every URL the worker precaches is a file `build.rs` publishes, and every file
/// the build publishes is precached.
#[test]
fn the_worker_precaches_exactly_what_the_build_publishes() {
    let published = published_by_build_script();
    assert!(
        !published.is_empty(),
        "could not read the published files out of build.rs"
    );
    assert!(
        published.iter().any(|name| name == "index.html"),
        "build.rs no longer publishes the shell: {published:?}"
    );

    for entry in precached_by_the_worker() {
        if entry == "./" {
            continue; // the scope root, which the host serves as index.html
        }
        if WASM_BINDGEN_FILES.contains(&entry.as_str()) {
            assert!(
                !published.contains(&entry),
                "'{entry}' is written by wasm-bindgen, so build.rs must not also write it"
            );
            continue;
        }
        assert!(
            published.contains(&entry),
            "the worker precaches '{entry}' but build.rs does not publish it; \
             a 404 in caches.addAll rejects the entire install, so this costs the \
             app its offline support rather than one file. build.rs publishes: \
             {published:?}"
        );
    }

    for name in &published {
        if name == "service-worker.js" {
            continue; // a worker is fetched by the browser, never from its cache
        }
        assert!(
            precached_by_the_worker().contains(name),
            "build.rs publishes '{name}' but the worker does not precache it"
        );
    }
}

/// The manifest is what the browser reads to decide what an install *is*, so it
/// is the one precached URL the worker must never answer from the cache: a
/// cached copy would let a fresh install derive its identity from a manifest
/// older than the last change to it, and Cache Storage outlives an uninstall.
#[test]
fn the_worker_never_serves_the_manifest_from_the_cache() {
    let worker = worker_template();
    assert!(
        precached_by_the_worker()
            .iter()
            .any(|asset| asset == "manifest.webmanifest"),
        "the manifest must stay in ASSETS: a broken manifest must fail the install loudly"
    );
    let fetch = worker
        .split_once("addEventListener('fetch'")
        .expect("the worker must handle fetch")
        .1;
    assert!(
        fetch.contains("url === MANIFEST"),
        "the fetch handler must exempt the manifest before touching the cache"
    );
}

/// The page loads generated bindings, keeps one script tag, and declares no
/// hand-written wasm ABI.
#[test]
fn the_page_loads_generated_bindings_and_nothing_else() {
    let page = shell();
    assert!(page.contains("<!DOCTYPE html>"));
    assert!(page.contains("import('./app.js')"));
    assert!(page.contains(".then(m => m.default())"));
    assert_eq!(
        page.matches("<script").count(),
        1,
        "exactly one script tag in the shell"
    );
    for obsolete in [
        "instantiateStreaming",
        "alloc_buf",
        "wasm.exports",
        "fetch(",
    ] {
        assert!(!page.contains(obsolete), "{obsolete} in the static shell");
    }
}

/// The loader hands over and reports failure; it holds no state and no events.
#[test]
fn the_loader_does_nothing_but_load_and_report_failure() {
    let page = shell();
    let script = page
        .split_once("<script type=\"module\">")
        .expect("the module script")
        .1
        .split_once("</script>")
        .expect("the end of the module script")
        .0;
    assert!(script.contains("catch"));
    assert!(script.contains("getElementById('error')"));
    assert!(script.contains("could not start"));
    for forbidden in [
        "//",
        "/*",
        "addEventListener",
        "querySelector",
        "localStorage",
        "function",
        "fetch(",
    ] {
        assert!(
            !script.contains(forbidden),
            "{forbidden} in the loader:\n{script}"
        );
    }
}

/// The site is mounted under an arbitrary prefix, so every URL in it is
/// relative.
#[test]
fn the_shell_is_mountable_anywhere() {
    let page = shell();
    assert!(
        !page.contains("http://") && !page.contains("https://"),
        "an absolute URL would break the site outside its own origin"
    );
    for required in [
        "./app.js",
        "./icon.svg",
        "./icon-192.png",
        "./manifest.webmanifest",
    ] {
        assert!(
            page.contains(required),
            "the page must reference {required}"
        );
    }
    assert!(
        worker_template().contains("new URL('./', self.location.href)"),
        "the worker must resolve its cache from its own location"
    );
}

/// The worker template carries exactly one version placeholder, and the one
/// `skipWaiting` call is the answer to the page's ask.
#[test]
fn the_service_worker_updates_only_when_the_page_asks() {
    let worker = worker_template();
    assert_eq!(
        worker.matches("__VERSION__").count(),
        1,
        "the template must carry exactly one version placeholder"
    );
    let handler = worker
        .split("addEventListener('message'")
        .nth(1)
        .expect("the worker must listen for the page's handover message");
    let handler = &handler[..handler.find("});").expect("the message handler closes")];
    assert!(
        handler.contains("event.data === 'SKIP_WAITING'") && handler.contains("skipWaiting()"),
        "skipWaiting must be the worker's answer to the page's ask"
    );
    let outside: Vec<&str> = worker
        .split("addEventListener('message'")
        .next()
        .expect("the prefix before the handler")
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .filter(|line| line.contains("skipWaiting"))
        .collect();
    assert!(
        outside.is_empty(),
        "no skipWaiting outside the message handler: an update must never swap the \
         app under a live tab unasked, but found {outside:?}"
    );
}

/// The page asks a waiting worker in, and reloads on `controllerchange` — but
/// only when it was already controlled. A first visit claims its page as the
/// worker activates, and reloading that page would cost the reader a second
/// start for nothing.
///
/// The worker handshake is the *shell's*, shared with Spider Solitaire, so this
/// reads the shell rather than the app — which is the point of the split.
#[test]
fn the_page_takes_an_installed_worker_over_with_a_reload() {
    let code = strip_rust_comments(&read("src/shell.rs"));
    assert!(
        code.contains("registration.waiting()") && code.contains(".installing()"),
        "the page must look at both workers an update can leave behind"
    );
    assert!(
        code.contains("post_message(&JsValue::from_str(\"SKIP_WAITING\"))"),
        "the handover starts with the page's message"
    );
    assert!(
        code.contains("controllerchange") && code.contains("location().reload()"),
        "and ends with the page reloading as the worker takes over"
    );
    assert!(
        code.contains("container.controller().is_some()"),
        "the reload must be gated on the page having been controlled already"
    );
}

/// The worker only answers for URLs inside its own app's directory.
#[test]
fn the_worker_never_answers_outside_its_own_directory() {
    let worker = worker_template();
    assert!(
        worker.contains("const IS_OWN = url => url.startsWith(ROOT.href)"),
        "the directory guard must be a prefix test against the worker's own root"
    );
    let fetch = worker
        .split_once("addEventListener('fetch'")
        .expect("fetch handler")
        .1;
    assert!(
        fetch.contains("IS_OWN(url)"),
        "the fetch handler must check the request is inside this app's directory"
    );
}

/// The page states the worker's scope, and releases a wider registration left by
/// an earlier version.
#[test]
fn the_page_states_the_scope_and_releases_a_wider_one() {
    let ui = strip_rust_comments(&read("src/shell.rs"));
    assert!(
        ui.contains("const SCOPE: &str = \"./\";"),
        "the scope must be the app's own directory, relative"
    );
    assert!(ui.contains("register_with_options"));
    assert!(ui.contains("RegistrationOptions::new()") && ui.contains("set_scope(SCOPE)"));
    assert!(ui.contains("get_registrations") && ui.contains("unregister"));
    assert!(
        ui.contains("strip_suffix(\"service-worker.js\")"),
        "the script comparison must strip one filename, not a character set"
    );
    let calls: Vec<&str> = ui
        .lines()
        .filter(|line| {
            let code = line.split("//").next().unwrap_or(line);
            code.contains("trim_end_matches(")
        })
        .collect();
    assert!(
        calls.is_empty(),
        "`trim_end_matches` strips a character set and would tear down a sibling's \
         worker; found {calls:?}"
    );
}

/// No visible text may name the implementation.
///
/// A game that tells the reader it is "compiled to WebAssembly" is talking to
/// nobody: they came to play cards, and how the app is built is neither useful
/// nor actionable. Implementation vocabulary is fine in comments and in
/// `AGENTS.md`, and nowhere a reader can see it.
#[test]
fn no_visible_text_names_the_implementation() {
    let page = shell();
    let rendered = visible_text(&page);
    for banned in [
        "rust",
        "webassembly",
        "wasm",
        "bindings",
        "compiled",
        "compile",
    ] {
        assert!(
            !rendered.to_lowercase().contains(banned),
            "the rendered page says \"{banned}\"; say what the user gets or loses \
             instead. Visible text was:\n{rendered}"
        );
    }
    let outside_noscript = rendered.replace(&noscript_body(&rendered), "");
    assert!(
        !outside_noscript.to_lowercase().contains("javascript"),
        "only a <noscript> block may name JavaScript"
    );

    // The words the app writes itself are held to the same rule.
    for source in ["src/game.rs", "src/ui.rs"] {
        for literal in user_facing_strings(&read(source)) {
            let lower = literal.to_lowercase();
            for banned in ["rust", "webassembly", "wasm", "javascript", "compiled"] {
                assert!(
                    !lower.contains(banned),
                    "{source} shows the reader \"{banned}\": {literal}"
                );
            }
        }
    }
}

/// Every string literal in a source file that starts with a capital letter or is
/// a complete sentence — the ones a reader could plausibly be shown.
///
/// Deliberately over-inclusive: a literal with a space in it is checked whether
/// or not it is displayed, because deciding that by inspection is how a worded
/// string slips through.
fn user_facing_strings(source: &str) -> Vec<String> {
    let mut strings = Vec::new();
    let mut rest = source;
    while let Some(start) = rest.find('"') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('"') else {
            break;
        };
        let literal = &after[..end];
        rest = &after[end + 1..];
        let is_prose = literal.contains(' ')
            && !literal.contains("px;")
            && !literal.contains("translate(")
            && !literal.contains('{')
            && literal.len() > 8;
        if is_prose {
            strings.push(literal.to_owned());
        }
    }
    strings
}

/// Every id the shell renders is written by the app, or it is the failure box.
///
/// An element nothing ever touches can only ever show the text it shipped with,
/// which is decoration pretending to be information.
#[test]
fn every_element_the_shell_renders_is_written_by_the_app() {
    let page = shell();
    let ids: Vec<&str> = page
        .match_indices("id=\"")
        .filter_map(|(at, _)| {
            let rest = &page[at + 4..];
            rest.find('"').map(|end| &rest[..end])
        })
        .collect();
    assert!(ids.contains(&"board"), "the shell must carry the board");
    // Every element the page writes is written by the shared shell, by the
    // board, or by this game's own ui.rs. A test that looked in one file would
    // be checking the wrong copy of the truth.
    let ui = [
        read("src/ui.rs"),
        read("src/shell.rs"),
        read("src/board_ui.rs"),
    ]
    .join("\n");
    for id in ids {
        if matches!(
            id,
            "error" | "help" | "title" | "score-fact" | "won" | "won-title"
        ) {
            continue; // the failure box, the description, and structural hooks
        }
        // A menu note's id is built from its select's — `difficulty-note` from
        // `difficulty` — because the shell binds a select and writes its note
        // from one option list. Checking for the composed name would be asking
        // the test to know the concatenation.
        let checked = id.strip_suffix("-note").unwrap_or(id);
        assert!(
            ui.contains(&format!("\"{checked}\"")),
            "#{id} is rendered by the shell but nothing writes to it"
        );
    }
}

/// The row of tools carries no control that is dead on arrival.
///
/// Each button in `.tools` is one the shared shell binds and paints; the two
/// optional ones — the extra action and the game's own question — start hidden
/// in the markup, so a page whose app never loads shows three working tools
/// rather than five, two of which do nothing. The shell is what reveals them,
/// which is also what makes `hidden` in the markup honest: a button that the
/// script always hides could just as well not be in the file, and a button that
/// the script never reveals is one nobody can press.
#[test]
fn every_tool_in_the_row_is_bound_and_painted() {
    let page = shell();
    let row = page
        .split("<div class=\"tools\">")
        .nth(1)
        .and_then(|rest| rest.split("</div>").next())
        .expect("the page has a row of tools");
    let shell_rs = read("src/shell.rs");
    let mut buttons = 0;
    for chunk in row.split("<button").skip(1) {
        let opening = chunk.split('>').next().unwrap_or_default();
        let id = opening
            .split("id=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .expect("every tool in the row has an id");
        buttons += 1;
        assert!(
            shell_rs.contains(&format!("\"{id}\"")),
            "#{id} is in the row but the shared shell never binds or paints it"
        );
    }
    assert!(buttons >= 3, "the row lost its tools: {row}");

    // The two optional tools are hidden until the game asks for them.
    for id in ["action", "inspect"] {
        let opening = row
            .split("<button")
            .find(|chunk| chunk.contains(&format!("id=\"{id}\"")))
            .expect("the optional tool is in the row");
        let opening = opening.split('>').next().unwrap_or_default();
        let hidden_by_default = opening.contains(" hidden");
        assert_eq!(
            hidden_by_default,
            id == "action",
            "#{id}: the finish starts hidden because the game runs it by itself; \
             the question is there from the first deal"
        );
    }
}

/// A card's class list is built in exactly one place.
///
/// `.card.red` is the rule that paints a red rank red, so a class list that
/// omits the suit class draws a red card black. The drag rebuilt the string by
/// hand — `set_class_name("card dragging")` — and that is precisely what a
/// player saw: pick a red card up and it turned black under the finger, then
/// red again when it landed. Four callers each appended the states they knew
/// about, and they had already drifted: the drop-target pass silently dropped a
/// hint that was on the same card.
///
/// So the invariant is structural rather than a screenshot: no caller may spell
/// a card's class list itself. `board::card_classes` is the only builder, and it
/// is host-compiled, so its own test (`a_card_keeps_its_suit_class_in_every_state`)
/// runs under plain `cargo test` — `board_ui` is wasm-only and a test in it would
/// never run.
#[test]
fn a_cards_class_list_is_built_in_one_place() {
    let code = strip_rust_comments(&read("src/board_ui.rs"));
    let mut hand_built = Vec::new();
    for (number, line) in code.lines().enumerate() {
        let text = line.trim();
        if text.contains("set_class_name(\"card") || text.contains("format!(\"card ") {
            hand_built.push(format!("{}: {}", number + 1, text));
        }
    }
    assert!(
        hand_built.is_empty(),
        "a card's class list is built by `card_classes`, which is the only thing          that cannot forget the suit class — a red card with no suit class is \
         drawn black. Found: {hand_built:#?}"
    );
    assert!(
        code.contains("card_classes("),
        "board_ui must build card class lists with `card_classes`"
    );
    // And the builder is the shared board's, not a copy beside it.
    let board = strip_rust_comments(&read("src/board.rs"));
    assert!(
        board.contains("pub fn card_classes("),
        "the one builder lives in `board.rs`, which is host-compiled so its test can run"
    );
    assert!(
        !strip_rust_comments(&read("src/ui.rs")).contains("card_classes(")
            && !strip_rust_comments(&read("src/shell.rs")).contains("card_classes("),
        "nothing outside the board has any business building a card's classes"
    );
}

/// The board's own vocabulary is shared with the rules, not invented twice.
#[test]
fn the_page_and_the_rules_agree_on_the_piles() {
    // A place is a `Place`, and the board spells it while the game numbers it.
    // The two are tied together in exactly one place: `Pile::place`.
    // The scene is built in `scene.rs`, which is compiled for the host too —
    // so the arrangement of the board is something `cargo test` can check.
    let page = read("src/ui.rs") + &read("src/scene.rs");
    assert!(
        page.contains("pile.place()") || page.contains("Pile::place"),
        "the page must ask the rules where a pile sits on the board"
    );
    let game = read("src/game.rs");
    assert!(
        game.contains("fn place(self) -> crate::board::Place"),
        "the game owns the only mapping from a pile to a board place"
    );
    let board = read("src/board.rs");
    assert!(
        board.contains("fn from_slug(slug: &str) -> Option<Self>"),
        "the board owns the spelling of a place"
    );
    for token in ["\"stock\"", "\"waste\"", "\"foundation-\"", "\"tableau-\""] {
        assert!(
            game.contains(token),
            "the pile slugs and their parser live in one place; {token} is missing"
        );
    }
}

/// The shared modules are game-agnostic.
///
/// `board.rs`, `board_ui.rs`, `shell.rs` and `card.rs` are carried, byte for
/// byte, by Spider Solitaire. Nothing on this host enforces that — they are
/// separate repositories — so the enforcement has to be structural: a shared
/// file that names Klondike, or a foundation, or a run, cannot be shared, and
/// this test says so at the moment the mistake is made rather than the next
/// time somebody ports a change.
///
/// The words are the game's vocabulary, not English's: "column" and "card" are
/// the board's own, and a game says "foundation", "waste", "run" and "suit".
#[test]
fn the_shared_modules_name_neither_game() {
    let shared = [
        ("src/board.rs", read("src/board.rs")),
        ("src/board_ui.rs", read("src/board_ui.rs")),
        ("src/shell.rs", read("src/shell.rs")),
        ("src/card.rs", read("src/card.rs")),
    ];
    let banned = [
        // The games themselves.
        "klondike",
        "spider",
        // One game's piles and rules.
        "foundation",
        "waste",
        "redeal",
        "vegas",
        "suit sequence",
        "completed run",
    ];
    for (name, text) in &shared {
        // Comments explain what the board is *for*, and a comment that mentions
        // the games is honest documentation. Only code is held to the rule.
        let code = strip_rust_comments(text).to_lowercase();
        for word in banned {
            assert!(
                !code.contains(word),
                "{name} is shared with the other solitaire, and its code says \
                 \"{word}\". A shared module cannot name one game's piles or rules; \
                 keep those in the app's own ui.rs and game.rs."
            );
        }
    }
}

/// The shared files carry their copy's rule, in the place a copier will look.
///
/// They are carried, byte for byte, by the other solitaire, and nothing on this
/// host enforces that: they are separate repositories. So the *files* say so —
/// a reader editing `board.rs` has to be told it is also Spider's board — and
/// this test makes sure the note is not quietly deleted.
#[test]
fn the_shared_files_say_they_are_shared() {
    for name in [
        "src/board.rs",
        "src/board_ui.rs",
        "src/shell.rs",
        "src/card.rs",
    ] {
        let text = read(name);
        let header = &text[..text.len().min(2500)].to_lowercase();
        assert!(
            header.contains("both") && header.contains("solitaire"),
            "{name} is carried by both apps; its header must say so, or the next \
             reader edits one copy and leaves the other behind"
        );
        assert!(
            header.contains("byte for byte"),
            "{name} says how it is shared, not merely that it is"
        );
    }
}

/// The shared modules really are four files, not a copy in one app.
///
/// Each app's own `ui.rs` must be small: it is a `Rules` implementation, not a
/// second board. A rough size check is crude, but it catches the real failure —
/// somebody pasting the board back into the app "just for this one rule".
#[test]
fn each_app_keeps_only_its_own_rules() {
    let ui = read("src/ui.rs");
    let game = read("src/game.rs");
    let shared =
        read("src/board.rs").len() + read("src/board_ui.rs").len() + read("src/shell.rs").len();
    assert!(
        ui.len() < shared,
        "the app's ui.rs ({} bytes) should be a Rules implementation, not the board \
         (the shared modules are {shared} bytes)",
        ui.len()
    );
    assert!(
        !ui.contains("pointerdown") && !ui.contains("ResizeObserver"),
        "the app's ui.rs binds no DOM events: the shared shell does that"
    );
    assert!(
        game.len() > ui.len(),
        "the rules belong in game.rs, which is where the tests are"
    );
}

/// The icon is the committed SVG, one colour, and the family's colour.
///
/// The digest is pinned so that an "improved" icon is a deliberate act with a
/// visible diff rather than a silent one.
const ICON_SHA256: &str = "72d2662a9007089306d54b13e7489138f9f3e8d09949b993187d4bc3ae581fe6";

#[test]
fn the_icon_is_the_committed_svg() {
    let bytes = std::fs::read(root().join("assets/icon.svg")).expect("assets/icon.svg");
    assert_eq!(
        sha256_hex(&bytes),
        ICON_SHA256,
        "assets/icon.svg is the icon, and changing it must update ICON_SHA256 \
         deliberately: every install PNG is derived from it"
    );
    let icon = String::from_utf8_lossy(&bytes);
    // Read the fill attributes, not the file: the comments in this icon name the
    // colour too, and a substring search would pass on a file with no fill at
    // all. `fill="none"` is the card's outline, drawn by a stroke below rather
    // than filled, and is not a colour.
    let mut fills: Vec<&str> = icon
        .match_indices("fill=\"")
        .filter_map(|(at, _)| {
            let rest = &icon[at + 6..];
            rest.find('"').map(|end| &rest[..end])
        })
        .filter(|value| *value != "none")
        .collect();
    fills.sort_unstable();
    fills.dedup();
    assert_eq!(
        fills,
        ["#444444"],
        "the icon is one colour, and it is the family's pinned `#444444`"
    );
    // And the strokes too: the card outline is drawn, not filled.
    for stroke in icon.match_indices("stroke=\"") {
        let rest = &icon[stroke.0 + 8..];
        let value = &rest[..rest.find('"').unwrap_or(0)];
        assert_eq!(value, "#444444", "one colour across fills and strokes");
    }

    let Some(tracked) = tracked_files() else {
        return; // not a checkout: the gate's exported tree
    };
    assert!(tracked.lines().any(|line| line == "assets/icon.svg"));
    for artefact in ["assets/icon-192.png", "assets/icon-512.png"] {
        assert!(
            !tracked.lines().any(|line| line == artefact),
            "{artefact} is tracked; install icons are build output"
        );
        assert!(
            !root().join(artefact).exists(),
            "{artefact} exists in the source tree; it belongs in dist/ only"
        );
    }
}

/// Nothing generated may be tracked.
#[test]
fn no_build_artifact_is_committed() {
    let Some(tracked) = tracked_files() else {
        return;
    };
    for artefact in [
        "dist",
        "dist/app.js",
        "dist/app_bg.wasm",
        "dist/index.html",
        "dist/service-worker.js",
        "dist/style.css",
        "dist/icon-192.png",
        "dist/icon-512.png",
        "dist/manifest.webmanifest",
    ] {
        assert!(
            !tracked.lines().any(|line| line == artefact),
            "{artefact} is tracked; generated artefacts must never be committed"
        );
    }
}

/// `target/` and `dist/` have to be ignored, and the old JavaScript template's
/// files must not come back.
#[test]
fn build_output_is_ignored_and_the_javascript_template_is_gone() {
    let Some(tracked) = tracked_files() else {
        return;
    };
    for path in ["dist/", "target/"] {
        let ignored = std::process::Command::new("git")
            .args(["check-ignore", "-q", path])
            .current_dir(root())
            .status()
            .expect("git check-ignore")
            .success();
        assert!(ignored, "{path} must be in .gitignore");
    }
    for obsolete in [
        "app.js",
        "sw.js",
        "style.css",
        "index.html",
        "manifest.json",
        "Makefile",
        "eslint.config.mjs",
    ] {
        assert!(
            !tracked.lines().any(|line| line == obsolete),
            "{obsolete} is tracked; the app's sources are under src/"
        );
    }
}

/// The stylesheet carries its share of the contract.
#[test]
fn the_stylesheet_respects_the_family_rules() {
    let css = style();
    for required in [
        "prefers-color-scheme",
        "prefers-reduced-motion",
        "min-height: 44px",
        "outline: 3px solid",
        "isolation: isolate",
    ] {
        assert!(
            css.contains(required),
            "the stylesheet must keep {required}"
        );
    }
    assert!(
        css.contains(".card") && css.contains("--card-face"),
        "the card's own block lives here; it is the half of the shared card \
         format that Spider Solitaire also carries"
    );
    assert!(
        !css.contains("100vw") && !css.contains("font-size: 2v"),
        "normal UI text must not scale with the viewport"
    );
}

/// Every workflow reads the generator version from the lockfile rather than
/// naming one, and deploys Pages from its own workflow only.
#[test]
fn the_workflows_read_the_generator_version_from_the_lockfile() {
    let manifest = read("Cargo.toml");
    assert!(
        manifest
            .lines()
            .any(|line| line.trim() == "wasm-bindgen = \"0.2\""),
        "Cargo.toml must ask for `wasm-bindgen = \"0.2\"`"
    );
    for name in ["build.yml", "pages.yml"] {
        let text = strip_yaml_comments(&read(&format!(".github/workflows/{name}")));
        assert!(
            text.contains("version=\"$(awk") && text.contains("Cargo.lock"),
            "{name} must derive the generator version from Cargo.lock"
        );
        assert!(
            !text.contains("WASM_BINDGEN_VERSION"),
            "{name} must not declare a generator version of its own"
        );
        assert!(
            text.contains(
                "wasm-bindgen --target web --no-typescript --out-dir dist --out-name app"
            ),
            "{name} must generate bindings with `--out-name app`"
        );
        assert!(
            text.contains("target/wasm32-unknown-unknown/release/solitaire.wasm"),
            "{name} must name this crate's wasm"
        );
        assert!(
            text.contains("touch build.rs"),
            "{name} must touch build.rs, or the second build copies nothing"
        );
    }
    let pages = strip_yaml_comments(&read(".github/workflows/pages.yml"));
    assert!(
        pages.contains("github.ref == 'refs/heads/master' && !github.event.repository.fork"),
        "the Pages deploy must be gated on master and on this not being a fork"
    );
    assert!(
        !read(".github/workflows/build.yml").contains("pages: write"),
        "the build workflow must not hold Pages permissions; a fork PR has none"
    );
}

/// `sha256` of a byte slice, in hex.
fn sha256_hex(bytes: &[u8]) -> String {
    // FIPS 180-4, written out rather than pulled in: the digest is one assertion
    // in one test, and a dependency for it would cost more than it saves.
    const K: [u32; 64] = [
        0x428a_2f98,
        0x7137_4491,
        0xb5c0_fbcf,
        0xe9b5_dba5,
        0x3956_c25b,
        0x59f1_11f1,
        0x923f_82a4,
        0xab1c_5ed5,
        0xd807_aa98,
        0x1283_5b01,
        0x2431_85be,
        0x550c_7dc3,
        0x72be_5d74,
        0x80de_b1fe,
        0x9bdc_06a7,
        0xc19b_f174,
        0xe49b_69c1,
        0xefbe_4786,
        0x0fc1_9dc6,
        0x240c_a1cc,
        0x2de9_2c6f,
        0x4a74_84aa,
        0x5cb0_a9dc,
        0x76f9_88da,
        0x983e_5152,
        0xa831_c66d,
        0xb003_27c8,
        0xbf59_7fc7,
        0xc6e0_0bf3,
        0xd5a7_9147,
        0x06ca_6351,
        0x1429_2967,
        0x27b7_0a85,
        0x2e1b_2138,
        0x4d2c_6dfc,
        0x5338_0d13,
        0x650a_7354,
        0x766a_0abb,
        0x81c2_c92e,
        0x9272_2c85,
        0xa2bf_e8a1,
        0xa81a_664b,
        0xc24b_8b70,
        0xc76c_51a3,
        0xd192_e819,
        0xd699_0624,
        0xf40e_3585,
        0x106a_a070,
        0x19a4_c116,
        0x1e37_6c08,
        0x2748_774c,
        0x34b0_bcb5,
        0x391c_0cb3,
        0x4ed8_aa4a,
        0x5b9c_ca4f,
        0x682e_6ff3,
        0x748f_82ee,
        0x78a5_636f,
        0x84c8_7814,
        0x8cc7_0208,
        0x90be_fffa,
        0xa450_6ceb,
        0xbef9_a3f7,
        0xc671_78f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];
    let mut message = bytes.to_vec();
    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for block in message.chunks(64) {
        let mut schedule = [0u32; 64];
        for (index, word) in block.chunks(4).enumerate() {
            schedule[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for index in 16..64 {
            let s0 = schedule[index - 15].rotate_right(7)
                ^ schedule[index - 15].rotate_right(18)
                ^ (schedule[index - 15] >> 3);
            let s1 = schedule[index - 2].rotate_right(17)
                ^ schedule[index - 2].rotate_right(19)
                ^ (schedule[index - 2] >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(s1);
        }
        let mut working = state;
        for index in 0..64 {
            let s1 = working[4].rotate_right(6)
                ^ working[4].rotate_right(11)
                ^ working[4].rotate_right(25);
            let choice = (working[4] & working[5]) ^ (!working[4] & working[6]);
            let temp1 = working[7]
                .wrapping_add(s1)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(schedule[index]);
            let s0 = working[0].rotate_right(2)
                ^ working[0].rotate_right(13)
                ^ working[0].rotate_right(22);
            let majority =
                (working[0] & working[1]) ^ (working[0] & working[2]) ^ (working[1] & working[2]);
            let temp2 = s0.wrapping_add(majority);
            working[7] = working[6];
            working[6] = working[5];
            working[5] = working[4];
            working[4] = working[3].wrapping_add(temp1);
            working[3] = working[2];
            working[2] = working[1];
            working[1] = working[0];
            working[0] = temp1.wrapping_add(temp2);
        }
        for (slot, value) in state.iter_mut().zip(working) {
            *slot = slot.wrapping_add(value);
        }
    }
    state.iter().map(|word| format!("{word:08x}")).collect()
}

/// `src/ui.rs` with its comments removed.
///
/// Several assertions below are about code, and the doc comments in that file
/// quote the very strings they explain — `trim_end_matches`, `skipWaiting`, the
/// reload — so a search over the raw file would find the words in the prose
/// that says why they are not used.
fn strip_rust_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut in_block = false;
    for line in source.lines() {
        let mut text = line;
        if in_block {
            match text.find("*/") {
                Some(end) => {
                    text = &text[end + 2..];
                    in_block = false;
                }
                None => continue,
            }
        }
        if let Some(start) = text.find("/*") {
            if text[start..].contains("*/") {
                text = &text[..start];
            } else {
                text = &text[..start];
                in_block = true;
            }
        }
        if let Some(start) = text.find("//") {
            text = &text[..start];
        }
        out.push_str(text);
        out.push('\n');
    }
    out
}

/// A workflow with its comments removed, so commenting a line out cannot satisfy
/// an assertion about it.
fn strip_yaml_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| match line.find('#') {
            Some(index) => &line[..index],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The text inside the page's `<noscript>` element, or `""`.
fn noscript_body(page: &str) -> String {
    page.split_once("<noscript>")
        .and_then(|(_, rest)| rest.split_once("</noscript>"))
        .map_or(String::new(), |(body, _)| body.to_owned())
}

/// Everything a reader can actually see: the shell with its comments removed.
fn visible_text(page: &str) -> String {
    let mut out = String::with_capacity(page.len());
    let mut rest = page;
    loop {
        let Some(start) = rest.find("<!--") else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..start]);
        rest = rest[start..]
            .find("-->")
            .map_or("", |end| &rest[start + end + 3..]);
    }
}

#[cfg(test)]
mod digest_regression {
    use super::sha256_hex;

    #[test]
    fn the_digest_matches_a_known_answer() {
        // Without this, a broken implementation would give a stable and wrong
        // digest for the icon, and the pin would assert nothing at all.
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
