use std::cell::RefCell;

use gg_renderer::Textures;

use super::file::parse;
use super::layout::Rect;
use super::*;
use crate::{Key, MouseButton, PadButton, Stick};

thread_local! {
    /// The fonts tests measure text with, Inter baked once a thread.
    static FONTS: RefCell<Fonts> = RefCell::new(Fonts::new(&mut Textures::default()));
}

impl State {
    /// [`State::event`] with the thread's fonts, laid out first for a view 1,920 units across, as
    /// [`GameUi::event`] does.
    fn send(&mut self, event: &Event<'_>, cursor: Option<Vec2>, input: &Input) -> Option<UiEvent> {
        FONTS.with_borrow_mut(|fonts| {
            self.lay_out(fonts, Vec2::new(1920.0, 1080.0));
            self.event(event, cursor, input, fonts)
        })
    }

    /// Lays it out again for a view `view` units across and 1,080 down.
    fn lay_out_again(&mut self, view: f32) {
        self.laid_for = None;
        FONTS.with_borrow_mut(|fonts| self.lay_out(fonts, Vec2::new(view, 1080.0)));
    }
}

/// Four buttons, the third disabled, in a column in the view's middle; a row along the top with a
/// button at each end and room between; a panel in the bottom right corner padded round a fixed
/// stack. All sizes are fixed, so each position is known.
const MENU: &str = r#"(
    focus: "play",
    nodes: [
        Column(
            name: "menu",
            gap: 20.0,
            children: [
                Button(name: "play", text: "PLAY", width: Fixed(400.0), height: Fixed(80.0)),
                Button(name: "options", width: Fixed(400.0), height: Fixed(80.0)),
                Button(
                    name: "credits",
                    width: Fixed(400.0),
                    height: Fixed(80.0),
                    disabled: true,
                ),
                Button(name: "quit", width: Fixed(400.0), height: Fixed(80.0)),
            ],
        ),
        Row(
            name: "hud",
            anchor: (Center, Start),
            width: Fill(1.0),
            margin: (40.0, 20.0),
            children: [
                Button(name: "left", width: Fixed(100.0), height: Fixed(60.0)),
                Stack(name: "room", width: Fill(1.0)),
                Button(name: "right", width: Fixed(100.0), height: Fixed(60.0)),
            ],
        ),
        Panel(
            name: "corner",
            anchor: (End, End),
            margin: (30.0, 30.0),
            padding: (10.0, 20.0),
            children: [Stack(name: "inside", width: Fixed(100.0), height: Fixed(50.0))],
        ),
    ],
)"#;

/// A UI of file `text`, laid out for a view `view` units across and 1,080 down.
fn laid(text: &str, view: f32) -> State {
    let document = parse(text).unwrap_or_else(|error| panic!("{error:#}"));
    let mut state = State::new(0, "menu", &document);
    state.lay_out_again(view);
    state
}

/// Node `name`'s rectangle, `[x, y, width, height]`.
fn rect(state: &State, name: &str) -> [f32; 4] {
    let found = state
        .named(name)
        .unwrap_or_else(|| panic!("a node `{name}`"));
    let Rect { at, size } = state.laid.rects[found];
    [at.x, at.y, size.x, size.y]
}

/// A file reads as its nodes, each after its parent with its subtree's end; omitted fields take
/// their kind's defaults; an optional field reads written bare or as `Some`; the focus lands on
/// the button it names.
#[test]
fn a_ui_file_reads_as_its_tree_of_nodes() {
    let document = parse(MENU).unwrap_or_else(|error| panic!("{error:#}"));
    let names: Vec<_> = (document.nodes.iter())
        .map(|node| (node.name.as_deref(), node.kind, node.parent, node.end))
        .collect();
    let expected = [
        (Some("menu"), Kind::Column, None, 5),
        (Some("play"), Kind::Button, Some(0), 2),
        (Some("options"), Kind::Button, Some(0), 3),
        (Some("credits"), Kind::Button, Some(0), 4),
        (Some("quit"), Kind::Button, Some(0), 5),
        (Some("hud"), Kind::Row, None, 9),
        (Some("left"), Kind::Button, Some(5), 7),
        (Some("room"), Kind::Stack, Some(5), 8),
        (Some("right"), Kind::Button, Some(5), 9),
        (Some("corner"), Kind::Panel, None, 11),
        (Some("inside"), Kind::Stack, Some(9), 11),
    ];
    assert_eq!(names, expected);
    assert_eq!(document.focus, Some(1));
    let play = &document.nodes[1];
    assert_eq!(play.text, "PLAY");
    assert_eq!(play.padding, Vec2::new(32.0, 14.0), "a button's padding");
    assert_eq!((play.radius, play.text_size), (10.0, 34.0));
    assert!(document.nodes[3].disabled);
    assert_eq!(document.nodes[0].align, [Align::Center; 2]);
    assert_eq!(
        document.nodes[5].anchor,
        Some([Align::Center, Align::Start])
    );
    let explicit = r#"(focus: Some("b"), nodes: [Button(name: Some("b"), looks: (
        normal: (texture: "frame.png", color: (1.0, 0.0, 0.0, 1.0)),
        focused: (text_color: (0.0, 1.0, 0.0, 1.0)),
    ))])"#;
    let document = parse(explicit).unwrap_or_else(|error| panic!("{error:#}"));
    let looks = &document.nodes[0].looks;
    assert_eq!(looks[0].color, Vec4::new(1.0, 0.0, 0.0, 1.0));
    assert_eq!(looks[1].text, Vec4::new(0.0, 1.0, 0.0, 1.0));
    let textures: Vec<_> = (looks.iter())
        .map(|look| look.texture.as_ref().map(|texture| &*texture.name))
        .collect();
    assert_eq!(
        textures,
        [Some("frame.png"); 4],
        "each look the normal one's texture"
    );
}

/// A bad file is refused with its code, at the line and column of the refusal: a name's own for a
/// name, the node's end for its fields.
#[test]
fn a_malformed_ui_file_is_refused_with_its_line() {
    for (text, code, at) in [
        ("(nodes: [\n    Text(text: \"a\"\n])", "ui.syntax", "3:1"),
        (
            "(nodes: [\n    Text(colour: (1.0, 1.0, 1.0, 1.0)),\n])",
            "ui.syntax",
            "2:",
        ),
        (
            "(nodes: [Text(name: \"a\"),\n    Text(name: \"a\")])",
            "ui.duplicate_name",
            "2:19",
        ),
        ("(nodes: [\n    Text(name: \"a b\")])", "ui.bad_name", "2:"),
        (
            "(nodes: [\n    Row(),\n    Text(children: []),\n])",
            "ui.bad_field",
            "3:",
        ),
        (
            "(nodes: [\n    Button(text: \"go\"),\n])",
            "ui.unnamed_button",
            "2:",
        ),
        ("(nodes: [\n    Image()])", "ui.bad_field", "2:"),
        ("(nodes: [\n    Row(gap: -1.0)])", "ui.bad_value", "2:"),
        (
            "(nodes: [\n    Row(width: Fill(0.0))])",
            "ui.bad_value",
            "2:",
        ),
        ("(nodes: [\n    Text(size: 0.0)])", "ui.bad_value", "2:"),
        (
            "(nodes: [\n    Panel(slice: (0.6, 4.0))])",
            "ui.bad_value",
            "2:",
        ),
        // Out of bounds: refused at the number, not the node's end.
        ("(nodes: [Text(\n    size: 1e38,\n)])", "ui.bad_value", "2:"),
        (
            "(nodes: [Row(children: [Stack(\n    width: Fixed(3e38),\n)])])",
            "ui.bad_value",
            "2:",
        ),
        ("(nodes: [Row(\n    gap: 1e7,\n)])", "ui.bad_value", "2:"),
        (
            "(nodes: [Row(\n    margin: (0.0, inf),\n)])",
            "ui.bad_value",
            "2:",
        ),
    ] {
        let error = parse(text)
            .err()
            .unwrap_or_else(|| panic!("{text} is refused"));
        assert_eq!(error.code(), code, "{text}: {error:#}");
        assert!(error.to_string().starts_with(at), "{text}: {error:#}");
    }
    let focus = "(focus: \"a\", nodes: [Text(name: \"a\")])";
    let error = parse(focus).expect_err("a text takes no focus");
    assert_eq!(error.code(), "ui.no_node", "{error:#}");
    // A parse doesn't see the names of the one before.
    parse("(nodes: [Text(name: \"a\")])").expect("a name of its own");
}

/// Nodes lie where their sizes (fixed, fit, fill), padding, gaps, margins, alignment and anchors
/// put them, in a 1,920-unit view and again in a 1,440 one, anchored ones keeping to their edges.
#[test]
fn nodes_lie_where_their_sizes_and_alignment_put_them() {
    let state = laid(MENU, 1920.0);
    // Four buttons and three gaps, 380 high, centered.
    assert_eq!(rect(&state, "menu"), [760.0, 350.0, 400.0, 380.0]);
    assert_eq!(rect(&state, "play"), [760.0, 350.0, 400.0, 80.0]);
    assert_eq!(rect(&state, "quit"), [760.0, 650.0, 400.0, 80.0]);
    // Along the top inside its margin, the room between its buttons filled.
    assert_eq!(rect(&state, "hud"), [40.0, 20.0, 1840.0, 60.0]);
    assert_eq!(rect(&state, "left"), [40.0, 20.0, 100.0, 60.0]);
    assert_eq!(rect(&state, "room"), [140.0, 50.0, 1640.0, 0.0]);
    assert_eq!(rect(&state, "right"), [1780.0, 20.0, 100.0, 60.0]);
    // Bottom right, padded round its content.
    assert_eq!(rect(&state, "corner"), [1770.0, 960.0, 120.0, 90.0]);
    assert_eq!(rect(&state, "inside"), [1780.0, 980.0, 100.0, 50.0]);

    let narrower = laid(MENU, 1440.0);
    assert_eq!(rect(&narrower, "menu"), [520.0, 350.0, 400.0, 380.0]);
    assert_eq!(rect(&narrower, "right"), [1300.0, 20.0, 100.0, 60.0]);
    assert_eq!(rect(&narrower, "corner"), [1290.0, 960.0, 120.0, 90.0]);

    // Fills share what's left by share, none below its fit. Without a fill, the rest goes where
    // the parent's alignment puts it; children are placed across by their anchor or its alignment.
    let row = r#"(nodes: [
        Row(name: "shared", width: Fixed(600.0), gap: 10.0, anchor: (Start, Start), children: [
            Stack(name: "one", width: Fill(1.0)),
            Stack(name: "two", width: Fill(2.0)),
            Stack(name: "fixed", width: Fixed(100.0)),
        ]),
        Row(name: "held", width: Fixed(300.0), anchor: (Start, End), children: [
            Stack(name: "wide", width: Fill(1.0), children: [Stack(width: Fixed(250.0))]),
            Stack(name: "narrow", width: Fill(1.0)),
        ]),
        Panel(
            name: "aligned",
            width: Fixed(300.0),
            height: Fixed(200.0),
            padding: (10.0, 20.0),
            align: (End, End),
            anchor: (End, Start),
            children: [
                Stack(name: "end", width: Fixed(100.0), height: Fixed(50.0)),
                Stack(
                    name: "start",
                    anchor: (Start, Start),
                    width: Fixed(20.0),
                    height: Fixed(20.0),
                ),
                Stack(name: "filled", width: Fill(1.0), height: Fill(1.0)),
            ],
        ),
        Row(
            name: "short",
            height: Fixed(50.0),
            anchor: (Start, Center),
            children: [
                Stack(
                    name: "tall",
                    height: Fill(1.0),
                    children: [Stack(width: Fixed(10.0), height: Fixed(100.0))],
                ),
            ],
        ),
        Panel(
            name: "small",
            width: Fixed(40.0),
            height: Fixed(40.0),
            anchor: (Center, End),
            children: [
                Stack(
                    name: "big",
                    width: Fill(1.0),
                    height: Fill(1.0),
                    children: [Stack(width: Fixed(60.0), height: Fixed(20.0))],
                ),
            ],
        ),
    ])"#;
    let state = laid(row, 1920.0);
    assert_eq!(rect(&state, "one"), [0.0, 0.0, 160.0, 0.0]);
    assert_eq!(rect(&state, "two"), [170.0, 0.0, 320.0, 0.0]);
    assert_eq!(rect(&state, "fixed"), [500.0, 0.0, 100.0, 0.0]);
    assert_eq!(rect(&state, "wide"), [0.0, 1080.0, 250.0, 0.0]);
    assert_eq!(rect(&state, "narrow"), [250.0, 1080.0, 50.0, 0.0]);
    assert_eq!(rect(&state, "aligned"), [1620.0, 0.0, 300.0, 200.0]);
    assert_eq!(rect(&state, "end"), [1810.0, 130.0, 100.0, 50.0]);
    assert_eq!(rect(&state, "start"), [1630.0, 20.0, 20.0, 20.0]);
    assert_eq!(rect(&state, "filled"), [1630.0, 20.0, 280.0, 160.0]);
    // A fill is at least its fit, across a row and in a panel, overflowing round its anchor.
    assert_eq!(rect(&state, "short"), [0.0, 515.0, 10.0, 50.0]);
    assert_eq!(rect(&state, "tall"), [0.0, 490.0, 10.0, 100.0]);
    assert_eq!(rect(&state, "small"), [940.0, 1040.0, 40.0, 40.0]);
    assert_eq!(rect(&state, "big"), [930.0, 1040.0, 60.0, 40.0]);
}

/// One node over the limit is refused, the limit reads; and a row of many nodes at the largest
/// sizes, margins, gaps and padding lays out finite, as the tool API lists it.
#[test]
fn a_ui_is_bounded_in_its_nodes_and_numbers() {
    let stacks = |count: usize| format!("(nodes: [{}])", "Stack(),".repeat(count));
    let error = parse(&stacks(10_001)).expect_err("one too many");
    assert_eq!(error.code(), "ui.too_many_nodes", "{error:#}");
    let held = parse(&stacks(10_000)).expect("as many as a UI holds");
    assert_eq!(held.nodes.len(), 10_000);
    let large = "Stack(width: Fixed(1e6), height: Fixed(1e6), margin: (1e6, 1e6)),";
    let row = format!(
        "(nodes: [Row(gap: 1e6, padding: (1e6, 1e6), children: [{}])])",
        large.repeat(1_000)
    );
    let state = laid(&row, 1920.0);
    let finite = (state.laid.rects.iter()).all(|rect| rect.at.is_finite() && rect.size.is_finite());
    assert!(finite, "every rectangle finite");
}

/// A hidden node and what's under it take no room and show nothing; shown again, they do.
#[test]
fn a_hidden_node_takes_no_room() {
    let mut state = laid(MENU, 1920.0);
    let mut fonts = Fonts::new(&mut Textures::default());
    let options = state.named("options").expect("a node");
    state.nodes[options].visible = false;
    let corner = state.named("corner").expect("a node");
    state.nodes[corner].visible = false;
    state.laid_for = None;
    state.lay_out(&mut fonts, Vec2::new(1920.0, 1080.0));
    // Three buttons and two gaps, 280 high.
    assert_eq!(rect(&state, "menu"), [760.0, 400.0, 400.0, 280.0]);
    assert_eq!(rect(&state, "credits"), [760.0, 500.0, 400.0, 80.0]);
    assert_eq!(rect(&state, "options")[2..], [0.0, 0.0]);
    assert_eq!(rect(&state, "inside")[2..], [0.0, 0.0]);
    let inside = state.named("inside").expect("a node");
    assert!(!state.laid.shown[inside] && !state.laid.shown[options]);
    assert!(state.laid.shown[0]);
}

/// Arrows move the focus to the nearest focusable button their way, past a disabled one, level
/// ones first, and stop at the last. Tab goes in file order, wrapping; Shift+Tab back. A hidden
/// button takes it no more.
#[test]
fn the_focus_moves_spatially_and_by_tab() {
    let mut state = laid(MENU, 1920.0);
    let input = Input::default();
    let key = |key| Event::Key {
        key,
        pressed: true,
        repeat: false,
    };
    let pressed = |state: &mut State, key_pressed: Key, input: &Input| {
        state.send(&key(key_pressed), None, input)
    };
    let focused = |name: &str| Some(UiEvent::Focused(name.into()));
    assert_eq!(state.focused(), state.named("play"), "the file's focus");
    assert_eq!(
        pressed(&mut state, Key::ArrowDown, &input),
        focused("options")
    );
    assert_eq!(pressed(&mut state, Key::ArrowDown, &input), focused("quit"));
    assert_eq!(
        pressed(&mut state, Key::ArrowDown, &input),
        None,
        "the last"
    );
    assert_eq!(
        pressed(&mut state, Key::ArrowUp, &input),
        focused("options")
    );
    assert_eq!(pressed(&mut state, Key::ArrowUp, &input), focused("play"));
    // The row's left and right buttons are equally near; the left comes first. Then the right,
    // level with the left though the menu's top is nearer.
    assert_eq!(pressed(&mut state, Key::ArrowUp, &input), focused("left"));
    assert_eq!(
        pressed(&mut state, Key::ArrowRight, &input),
        focused("right")
    );

    assert_eq!(
        pressed(&mut state, Key::Tab, &input),
        focused("play"),
        "round"
    );
    assert_eq!(pressed(&mut state, Key::Tab, &input), focused("options"));
    assert_eq!(pressed(&mut state, Key::Tab, &input), focused("quit"));
    let mut shifted = Input::default();
    shifted.record(&key(Key::ShiftLeft));
    assert_eq!(pressed(&mut state, Key::Tab, &shifted), focused("options"));

    let options = state.named("options").expect("a node");
    state.nodes[options].visible = false;
    assert_eq!(state.focused(), None, "hidden");
    // With none focused, a move focuses the file's, here the first focusable.
    assert_eq!(pressed(&mut state, Key::ArrowDown, &input), focused("play"));
    assert_eq!(pressed(&mut state, Key::Tab, &input), focused("quit"));
}

/// A click presses the button it went down on when let go over it; the cursor focuses a button it
/// comes over. Enter, Space and pad South press the focused button, not on key repeat. A disabled
/// button takes neither. Escape and pad East ask for back. A stick steps once per push, however it
/// wavers, and again only after returning near rest.
#[test]
fn a_click_or_a_key_presses_a_button_by_its_name() {
    let mut state = laid(MENU, 1920.0);
    let input = Input::default();
    let at = |state: &State, name: &str| {
        let [x, y, width, height] = rect(state, name);
        Some(Vec2::new(x + width / 2.0, y + height / 2.0))
    };
    let moved = Event::CursorMoved { x: 0.0, y: 0.0 };
    let button = |pressed| Event::MouseButton {
        button: MouseButton::Left,
        pressed,
    };
    let over = at(&state, "options");
    let event = state.send(&moved, over, &input);
    assert_eq!(event, Some(UiEvent::Focused("options".into())));
    assert_eq!(state.send(&button(true), over, &input), None);
    let options = state.named("options").expect("a node");
    assert_eq!(state.look(options), PRESSED);
    let event = state.send(&button(false), over, &input);
    assert_eq!(event, Some(UiEvent::Pressed("options".into())));
    assert_eq!(state.look(options), FOCUSED);
    // Let go over another button, nothing is pressed.
    state.send(&button(true), over, &input);
    assert_eq!(state.send(&button(false), at(&state, "play"), &input), None);
    // A disabled button takes neither focus nor click.
    let credits = at(&state, "credits");
    assert_eq!(state.send(&moved, credits, &input), None);
    assert_eq!(state.send(&button(true), credits, &input), None);
    assert_eq!(state.send(&button(false), credits, &input), None);

    let key = |key, pressed, repeat| Event::Key {
        key,
        pressed,
        repeat,
    };
    let enter = state.send(&key(Key::Enter, true, false), None, &input);
    assert_eq!(enter, Some(UiEvent::Pressed("options".into())));
    assert_eq!(state.look(options), PRESSED, "held");
    assert_eq!(
        state.send(&key(Key::Enter, false, false), None, &input),
        None
    );
    assert_eq!(state.look(options), FOCUSED, "let go");
    assert_eq!(state.send(&key(Key::Space, true, true), None, &input), None);
    let back = state.send(&key(Key::Escape, true, false), None, &input);
    assert_eq!(back, Some(UiEvent::Back));

    let pad = |button, pressed| Event::PadButton {
        pad: crate::Pad(0),
        button,
        pressed,
    };
    let down = state.send(&pad(PadButton::DPadDown, true), None, &input);
    assert_eq!(down, Some(UiEvent::Focused("quit".into())));
    let south = state.send(&pad(PadButton::South, true), None, &input);
    assert_eq!(south, Some(UiEvent::Pressed("quit".into())));
    assert_eq!(
        state.send(&pad(PadButton::East, true), None, &input),
        Some(UiEvent::Back)
    );
    let stick = |x: f32, y: f32| Event::PadStick {
        pad: crate::Pad(0),
        stick: Stick::Left,
        value: Vec2::new(x, y),
    };
    let up = state.send(&stick(0.0, 0.8), None, &input);
    assert_eq!(up, Some(UiEvent::Focused("options".into())));
    // Held near the diagonal, wavering across it, it doesn't step again.
    for (x, y) in [(0.45, 0.62), (0.62, 0.45), (0.45, 0.62), (0.7, 0.68)] {
        assert_eq!(
            state.send(&stick(x, y), None, &input),
            None,
            "held at {x}, {y}"
        );
    }
    assert_eq!(
        state.send(&stick(0.2, 0.1), None, &input),
        None,
        "near rest"
    );
    let again = state.send(&stick(0.1, 0.8), None, &input);
    assert_eq!(again, Some(UiEvent::Focused("play".into())));
}

/// On reload, a node keeps the text and visibility code set, where the same-named node still has
/// text. The focus stays on its button by name wherever it moved, else goes to the file's.
#[test]
fn a_reload_keeps_the_focus_and_what_code_set_by_name() {
    let menu = r#"(focus: "play", nodes: [Column(children: [
        Button(name: "play", text: "PLAY"),
        Button(name: "options", text: "OPTIONS"),
        Text(name: "score", text: "0"),
        Text(name: "title", text: "MENU"),
    ])])"#;
    let mut uis = Uis::default();
    let file = uis.add_file(
        "menu".into(),
        "ui/menu.ui".into(),
        parse(menu).unwrap(),
        None,
    );
    let ui = uis.add(file, "menu");
    let state = uis.state(ui);
    let options = state.named("options").expect("a node");
    state.focus = Some(options);
    let score = state.named("score").expect("a node");
    state.nodes[score].text = "120".to_owned();
    state.set[score].text = true;
    let title = state.named("title").expect("a node");
    state.nodes[title].visible = false;
    state.set[title].visible = true;
    // Reordered; score and title texts changed; a button added.
    let edited = r#"(focus: "play", nodes: [Column(children: [
        Text(name: "title", text: "MAIN MENU"),
        Button(name: "play", text: "PLAY"),
        Button(name: "quit", text: "QUIT"),
        Button(name: "options", text: "SETTINGS"),
        Text(name: "score", text: "SCORE"),
    ])])"#;
    uis.reload(file, parse(edited).unwrap());
    let state = uis.state(ui);
    assert_eq!(state.focus, state.named("options"), "by its name");
    let score = state.named("score").expect("a node");
    assert_eq!(state.nodes[score].text, "120", "what code set");
    let options = state.named("options").expect("a node");
    assert_eq!(
        state.nodes[options].text, "SETTINGS",
        "what code didn't set"
    );
    let title = state.named("title").expect("a node");
    assert!(!state.nodes[title].visible);
    assert_eq!(state.nodes[title].text, "MAIN MENU");
    // The focused button gone, the focus goes to the file's.
    let without = r#"(focus: "play", nodes: [Button(name: "play")])"#;
    uis.reload(file, parse(without).unwrap());
    assert_eq!(uis.state(ui).focus, Some(0));
}

/// The tool API's `ui` lists each UI's nodes laid out, in canvas units and view pixels, with
/// visibility, text and button state.
#[cfg(feature = "dev")]
#[test]
fn the_tool_api_lists_each_uis_nodes() {
    let mut uis = Uis::default();
    let file = uis.add_file(
        "5f3c9a1e07b2d846".into(),
        "ui/menu.ui".into(),
        parse(MENU).unwrap(),
        None,
    );
    let ui = uis.add(file, "menu");
    let state = uis.state(ui);
    let quit = state.named("quit").expect("a node");
    state.pressed = Some((quit, Press::Key));
    state.focus = Some(quit);
    let mut fonts = Fonts::new(&mut Textures::default());
    // 720 pixels high: two thirds of a pixel a unit.
    let view = Viewport {
        origin: [10, 20],
        size: [1280, 720],
    };
    let listed = uis.listed(&mut fonts, Some(view));
    assert_eq!(listed.view, [1280, 720]);
    let [ui] = &listed.uis[..] else {
        panic!("one UI: {listed:?}");
    };
    assert_eq!(
        (&*ui.name, &*ui.asset, &*ui.path),
        ("menu", "5f3c9a1e07b2d846", "ui/menu.ui")
    );
    assert!(ui.input && !ui.drawn);
    assert_eq!(ui.focus.as_deref(), Some("quit"));
    let node = |name: &str| {
        let found = ui
            .nodes
            .iter()
            .find(|node| node.name.as_deref() == Some(name));
        found.unwrap_or_else(|| panic!("a node `{name}`"))
    };
    let play = node("play");
    assert_eq!(play.kind, gg_api::UiKind::Button);
    assert_eq!(play.parent, Some(0));
    assert_eq!(play.rect, [760.0, 350.0, 400.0, 80.0]);
    let pixels = [760.0, 350.0, 400.0, 80.0].map(|units: f32| units * (720.0 / 1080.0));
    assert_eq!(play.pixels, pixels);
    assert_eq!(play.text.as_deref(), Some("PLAY"));
    assert!(play.visible && !play.focused && !play.pressed && !play.disabled);
    let quit = node("quit");
    assert!(quit.focused && quit.pressed);
    assert!(node("credits").disabled);
    assert_eq!(node("room").text, None);
    let json = serde_json::to_value(&listed).expect("JSON");
    assert_eq!(json["uis"][0]["nodes"][1]["kind"], "button");
}

/// A centered column of a slider, toggle, drop-down, text field and button, each 400 wide; and a
/// list of six buttons on the right edge, 250 high, reaching 530 down. All sizes are fixed, so
/// each position is known.
const WIDGETS: &str = r#"(
    focus: "volume",
    nodes: [
        Column(
            name: "options",
            gap: 20.0,
            children: [
                Slider(name: "volume", step: 0.1, value: 0.5),
                Toggle(name: "sound", text: "SOUND", width: Fixed(400.0), height: Fixed(60.0)),
                Dropdown(
                    name: "difficulty",
                    options: ["EASY", "NORMAL", "HARD"],
                    choice: 1,
                    width: Fixed(400.0),
                    height: Fixed(60.0),
                ),
                Field(
                    name: "player",
                    hint: "NAME",
                    most: 8,
                    width: Fixed(400.0),
                    height: Fixed(60.0),
                ),
                Button(name: "done", text: "DONE", width: Fixed(400.0), height: Fixed(60.0)),
            ],
        ),
        List(
            name: "saves",
            anchor: (End, Center),
            width: Fixed(300.0),
            height: Fixed(250.0),
            gap: 10.0,
            children: [
                Button(name: "save_0", width: Fixed(300.0), height: Fixed(80.0)),
                Button(name: "save_1", width: Fixed(300.0), height: Fixed(80.0)),
                Button(name: "save_2", width: Fixed(300.0), height: Fixed(80.0)),
                Button(name: "save_3", width: Fixed(300.0), height: Fixed(80.0)),
                Button(name: "save_4", width: Fixed(300.0), height: Fixed(80.0)),
                Button(name: "save_5", width: Fixed(300.0), height: Fixed(80.0)),
            ],
        ),
    ],
)"#;

fn press(key: Key) -> Event<'static> {
    Event::Key {
        key,
        pressed: true,
        repeat: false,
    }
}

fn pad(button: PadButton) -> Event<'static> {
    Event::PadButton {
        pad: crate::Pad(0),
        button,
        pressed: true,
    }
}

fn click(pressed: bool) -> Event<'static> {
    Event::MouseButton {
        button: MouseButton::Left,
        pressed,
    }
}

/// The point `along` node `name`'s width, halfway down it.
fn on(state: &State, name: &str, along: f32) -> Option<Vec2> {
    let [x, y, width, height] = rect(state, name);
    Some(Vec2::new(x + width * along, y + height / 2.0))
}

fn number(event: Option<UiEvent>) -> f32 {
    match event {
        Some(UiEvent::Changed(_, UiValue::Number(number))) => number,
        other => panic!("a slider's change: {other:?}"),
    }
}

fn changed(name: &str, value: UiValue) -> Option<UiEvent> {
    Some(UiEvent::Changed(name.into(), value))
}

/// Widgets read with their kinds' defaults; a list filling its parent takes the room it's given.
/// Refused with their codes: a widget without a name, a field its kind doesn't take, bad slider
/// bounds or value, a drop-down without options or with a bad choice, an over-full text field.
#[test]
fn widgets_read_from_their_file_and_are_refused_with_their_codes() {
    let state = laid(WIDGETS, 1920.0);
    assert_eq!(
        rect(&state, "volume"),
        [760.0, 356.0, 400.0, 48.0],
        "a slider's size"
    );
    assert_eq!(rect(&state, "player"), [760.0, 584.0, 400.0, 60.0]);
    assert_eq!(rect(&state, "saves"), [1620.0, 415.0, 300.0, 250.0]);
    let volume = &state.nodes[state.named("volume").expect("a node")];
    let slider = Widget::Slider {
        min: 0.0,
        max: 1.0,
        step: 0.1,
        value: 0.5,
    };
    assert_eq!(volume.widget, slider);
    let player = &state.nodes[state.named("player").expect("a node")];
    assert_eq!(
        player.align,
        [Align::Start, Align::Center],
        "a field's text from its left"
    );
    assert!(
        state.nodes[state.named("saves").expect("a node")].clips,
        "a list clips"
    );
    assert_eq!(state.focused(), state.named("volume"), "the file's focus");
    // A list filling its parent takes the room given, however much it holds.
    let filled = r#"(nodes: [Column(height: Fixed(300.0), children: [
        Stack(height: Fixed(100.0)),
        List(name: "filled", height: Fill(1.0), children: [
            Stack(width: Fixed(50.0), height: Fixed(500.0)),
        ]),
    ])])"#;
    assert_eq!(rect(&laid(filled, 1920.0), "filled")[2..], [50.0, 200.0]);
    for (text, code) in [
        ("(nodes: [Slider()])", "ui.unnamed_widget"),
        (
            "(nodes: [Field(name: \"a\", options: [\"b\"])])",
            "ui.bad_field",
        ),
        ("(nodes: [Text(clip: true)])", "ui.bad_field"),
        (
            "(nodes: [Slider(name: \"a\", min: 1.0, max: 1.0)])",
            "ui.bad_value",
        ),
        ("(nodes: [Slider(name: \"a\", value: 2.0)])", "ui.bad_value"),
        ("(nodes: [Slider(name: \"a\", max: 1e7)])", "ui.bad_value"),
        ("(nodes: [Dropdown(name: \"a\")])", "ui.bad_field"),
        (
            "(nodes: [Dropdown(name: \"a\", options: [])])",
            "ui.bad_field",
        ),
        (
            "(nodes: [Dropdown(name: \"a\", options: [\"b\"], choice: 1)])",
            "ui.bad_value",
        ),
        ("(nodes: [Field(name: \"a\", most: 0)])", "ui.bad_value"),
        (
            "(nodes: [Field(name: \"a\", most: 2, text: \"abc\")])",
            "ui.bad_value",
        ),
        (
            "(nodes: [Field(name: \"a\", text: \"a\\nb\")])",
            "ui.bad_value",
        ),
    ] {
        let error = parse(text)
            .err()
            .unwrap_or_else(|| panic!("{text} is refused"));
        assert_eq!(error.code(), code, "{text}: {error:#}");
    }
    let focus = "(focus: \"a\", nodes: [Toggle(name: \"a\")])";
    parse(focus).expect("a toggle takes the focus");
}

/// A slider steps by its step with the left and right arrows (also on repeat), the D-pad and the
/// stick (once per push), and goes to its ends with Home and End, no further. A click on its
/// track sets the nearest step; a drag keeps setting it, the slider pressed until let go. Each
/// change gives its new value, none if nothing changes. Up and down arrows move the focus.
#[test]
fn a_slider_steps_by_keys_and_a_pad_and_follows_the_mouse() {
    let mut state = laid(WIDGETS, 1920.0);
    let input = Input::default();
    let near = |got: f32, want: f32| assert!((got - want).abs() < 1e-5, "{got}, not {want}");
    near(
        number(state.send(&press(Key::ArrowRight), None, &input)),
        0.6,
    );
    let repeat = Event::Key {
        key: Key::ArrowRight,
        pressed: true,
        repeat: true,
    };
    near(number(state.send(&repeat, None, &input)), 0.7);
    near(
        number(state.send(&pad(PadButton::DPadLeft), None, &input)),
        0.6,
    );
    let stick = |x: f32| Event::PadStick {
        pad: crate::Pad(0),
        stick: Stick::Left,
        value: Vec2::new(x, 0.0),
    };
    near(number(state.send(&stick(0.9), None, &input)), 0.7);
    assert_eq!(state.send(&stick(0.95), None, &input), None, "once a push");
    near(number(state.send(&press(Key::End), None, &input)), 1.0);
    assert_eq!(
        state.send(&press(Key::ArrowRight), None, &input),
        None,
        "its end"
    );
    near(number(state.send(&press(Key::Home), None, &input)), 0.0);
    // The track runs from 784 to 1136, inside its padding: 73% along is 0.7, a step.
    let moved = Event::CursorMoved { x: 0.0, y: 0.0 };
    let at = on(&state, "volume", 0.0).map(|at| at + Vec2::new(24.0 + 352.0 * 0.73, 0.0));
    state.send(&moved, at, &input);
    near(number(state.send(&click(true), at, &input)), 0.7);
    let volume = state.named("volume").expect("a node");
    assert_eq!(state.look(volume), PRESSED, "held as it's dragged");
    // Dragged past its end and off it vertically, it stays at its end.
    let past = at.map(|at| at + Vec2::new(400.0, -200.0));
    near(number(state.send(&moved, past, &input)), 1.0);
    assert_eq!(state.send(&moved, past, &input), None);
    assert_eq!(state.send(&click(false), past, &input), None);
    assert_eq!(state.look(volume), FOCUSED, "let go");
    // A drag ends if the slider is hidden or disabled, as when a menu page closes.
    for hide in [true, false] {
        state.send(&click(true), at, &input);
        match hide {
            true => state.nodes[volume].visible = false,
            false => state.nodes[volume].disabled = true,
        }
        let start = on(&state, "volume", 0.0);
        assert_eq!(state.send(&moved, start, &input), None, "hidden: {hide}");
        assert_eq!(state.pressed, None, "hidden: {hide}");
        (state.nodes[volume].visible, state.nodes[volume].disabled) = (true, false);
    }
    near(
        number(state.send(&press(Key::ArrowLeft), None, &input)),
        0.6,
    );
    let event = state.send(&press(Key::ArrowDown), None, &input);
    assert_eq!(event, Some(UiEvent::Focused("sound".into())));
}

/// A slider off its step grid (from `min`) steps to the next grid point either way. Keys and a
/// drag reach `max` past the last point, as they reach `min`.
#[test]
fn a_slider_steps_to_its_grid_and_reaches_its_ends() {
    let text = r#"(focus: "coarse", nodes: [
        Slider(name: "coarse", step: 0.3, value: 1.0),
    ])"#;
    let mut state = laid(text, 1920.0);
    let input = Input::default();
    let near = |got: f32, want: f32| assert!((got - want).abs() < 1e-5, "{got}, not {want}");
    let send = |state: &mut State, key| number(state.send(&press(key), None, &input));
    near(send(&mut state, Key::ArrowLeft), 0.9);
    near(send(&mut state, Key::ArrowLeft), 0.6);
    near(send(&mut state, Key::ArrowRight), 0.9);
    near(send(&mut state, Key::ArrowRight), 1.0);
    assert_eq!(
        state.send(&press(Key::ArrowRight), None, &input),
        None,
        "its max"
    );
    // The track runs from 784 to 1136: its end is `max`, 97% along is nearer it than 0.9, its
    // start is `min`.
    let moved = Event::CursorMoved { x: 0.0, y: 0.0 };
    let along = |state: &State, share: f32| {
        on(state, "coarse", 0.0).map(|at| at + Vec2::new(24.0 + 352.0 * share, 0.0))
    };
    near(
        number(state.send(&click(true), along(&state, 0.0), &input)),
        0.0,
    );
    near(number(state.send(&moved, along(&state, 0.5), &input)), 0.6);
    near(number(state.send(&moved, along(&state, 0.97), &input)), 1.0);
    near(number(state.send(&moved, along(&state, 0.92), &input)), 0.9);
    near(number(state.send(&moved, along(&state, 1.0), &input)), 1.0);
}

/// A toggle flips by Enter, Space and pad South, held pressed until let go, and by a click let go
/// over it, not off it. Left and right arrows move the focus, as a button's do.
#[test]
fn a_toggle_flips_by_a_press_or_a_click() {
    let mut state = laid(WIDGETS, 1920.0);
    let input = Input::default();
    state.send(&press(Key::ArrowDown), None, &input);
    assert_eq!(state.focused(), state.named("sound"));
    let event = state.send(&press(Key::Enter), None, &input);
    assert_eq!(event, changed("sound", UiValue::On(true)));
    let sound = state.named("sound").expect("a node");
    assert_eq!(state.look(sound), PRESSED);
    let up = Event::Key {
        key: Key::Enter,
        pressed: false,
        repeat: false,
    };
    state.send(&up, None, &input);
    assert_eq!(state.look(sound), FOCUSED);
    let event = state.send(&press(Key::Space), None, &input);
    assert_eq!(event, changed("sound", UiValue::On(false)));
    let event = state.send(&pad(PadButton::South), None, &input);
    assert_eq!(event, changed("sound", UiValue::On(true)));
    let over = on(&state, "sound", 0.5);
    assert_eq!(state.send(&click(true), over, &input), None);
    let event = state.send(&click(false), over, &input);
    assert_eq!(event, changed("sound", UiValue::On(false)));
    state.send(&click(true), over, &input);
    let off = on(&state, "done", 0.5);
    assert_eq!(
        state.send(&click(false), off, &input),
        None,
        "let go off it"
    );
    // To the list's first button, level with it.
    let event = state.send(&press(Key::ArrowRight), None, &input);
    assert_eq!(event, Some(UiEvent::Focused("save_0".into())));
}

/// Left and right arrows step a drop-down's choice within its options. Enter or a click opens the
/// options with the choice lit; they take every event. The arrows and the stick light another;
/// Enter, or letting go of a click over an option, chooses it, closing them with a change. Escape,
/// pad East or a click outside closes them unchanged.
#[test]
fn a_dropdown_steps_and_chooses_from_its_options() {
    let mut state = laid(WIDGETS, 1920.0);
    let input = Input::default();
    let dropdown = state.named("difficulty").expect("a node");
    state.focus = Some(dropdown);
    let event = state.send(&press(Key::ArrowRight), None, &input);
    assert_eq!(event, changed("difficulty", UiValue::Choice(2)));
    assert_eq!(
        state.send(&press(Key::ArrowRight), None, &input),
        None,
        "its last"
    );
    let event = state.send(&press(Key::ArrowLeft), None, &input);
    assert_eq!(event, changed("difficulty", UiValue::Choice(1)));
    assert_eq!(state.send(&press(Key::Enter), None, &input), None);
    let open = Open {
        node: dropdown,
        lit: 1,
        held: false,
    };
    assert_eq!(state.open, Some(open));
    assert_eq!(state.look(dropdown), PRESSED, "open");
    // The arrows light options and no longer move the focus.
    assert_eq!(state.send(&press(Key::ArrowUp), None, &input), None);
    assert_eq!(state.open.map(|open| open.lit), Some(0));
    let event = state.send(&press(Key::Enter), None, &input);
    assert_eq!(event, changed("difficulty", UiValue::Choice(0)));
    assert_eq!(state.open, None, "closed by its choice");
    state.send(&pad(PadButton::South), None, &input);
    let stick = Event::PadStick {
        pad: crate::Pad(0),
        stick: Stick::Left,
        value: Vec2::new(0.0, -0.9),
    };
    state.send(&stick, None, &input);
    assert_eq!(state.open.map(|open| open.lit), Some(1));
    assert_eq!(state.send(&press(Key::Escape), None, &input), None);
    assert_eq!(state.open, None, "closed as it was, without back");
    // Options open below, each as tall as it: HARD is the third.
    let over = on(&state, "difficulty", 0.5);
    state.send(&click(true), over, &input);
    assert_eq!(state.send(&click(false), over, &input), None);
    assert!(state.open.is_some(), "a click opens it");
    let hard = over.map(|at| at + Vec2::new(0.0, 60.0 * 3.0));
    state.send(&Event::CursorMoved { x: 0.0, y: 0.0 }, hard, &input);
    assert_eq!(
        state.open.map(|open| open.lit),
        Some(2),
        "lit under the cursor"
    );
    state.send(&click(true), hard, &input);
    let event = state.send(&click(false), hard, &input);
    assert_eq!(event, changed("difficulty", UiValue::Choice(2)));
    state.send(&press(Key::Enter), None, &input);
    let outside = Some(Vec2::new(100.0, 100.0));
    assert_eq!(state.send(&click(true), outside, &input), None);
    assert_eq!(state.open, None, "a click outside closes it");
    state.send(&press(Key::Enter), None, &input);
    assert_eq!(state.send(&pad(PadButton::East), None, &input), None);
    assert_eq!(state.open, None);
}

/// Hiding or disabling a drop-down, or reloading its file, closes its open options; the next
/// event goes to the rest as if they never opened.
#[test]
fn an_open_dropdown_closes_as_its_hidden_disabled_or_read_again() {
    let mut uis = Uis::default();
    let document = parse(WIDGETS).unwrap_or_else(|error| panic!("{error:#}"));
    let file = uis.add_file("widgets".into(), "widgets.ui".into(), document, None);
    let ui = uis.add(file, "widgets");
    let input = Input::default();
    let state = uis.state(ui);
    let dropdown = state.named("difficulty").expect("a node");
    let open = |state: &mut State| {
        state.focus = Some(dropdown);
        state.send(&press(Key::Enter), None, &input);
        assert!(state.open.is_some(), "open");
    };
    open(state);
    state.nodes[dropdown].visible = false;
    state.laid_for = None;
    // Hidden, the down arrow moves the focus instead of lighting an option.
    let event = state.send(&press(Key::ArrowDown), None, &input);
    assert_eq!(
        event,
        Some(UiEvent::Focused("volume".into())),
        "the file's focus"
    );
    assert_eq!(state.open, None);
    state.nodes[dropdown].visible = true;
    state.laid_for = None;
    open(state);
    state.nodes[dropdown].disabled = true;
    assert_eq!(
        state.send(&press(Key::Escape), None, &input),
        Some(UiEvent::Back)
    );
    assert_eq!(state.open, None);
    state.nodes[dropdown].disabled = false;
    open(state);
    uis.reload(
        file,
        parse(WIDGETS).unwrap_or_else(|error| panic!("{error:#}")),
    );
    assert_eq!(uis.state(ui).open, None, "read again");
}

/// A text field takes the focus by a click (not hover), caret where clicked. It takes typed text
/// at the caret, up to its limit, no control characters. Arrows, Home and End move the caret;
/// Shift selects; Ctrl+A selects all; typing or Backspace replaces the selection. Backspace and
/// Delete cut a character either side. Space types without pressing; Enter presses; the down
/// arrow moves the focus on.
#[test]
fn a_text_field_takes_typed_text_at_its_caret() {
    let mut state = laid(WIDGETS, 1920.0);
    let mut input = Input::default();
    let field = state.named("player").expect("a node");
    let moved = Event::CursorMoved { x: 0.0, y: 0.0 };
    let over = on(&state, "player", 0.9);
    assert_eq!(
        state.send(&moved, over, &input),
        None,
        "hovered, not focused"
    );
    let event = state.send(&click(true), over, &input);
    assert_eq!(event, Some(UiEvent::Focused("player".into())));
    state.send(&click(false), over, &input);
    let text = |state: &State| state.nodes[field].text.clone();
    let typed = |text: &str| changed("player", UiValue::Text(text.to_owned()));
    // The cursor passing over a button leaves the focus on the field.
    let done = on(&state, "done", 0.5);
    assert_eq!(
        state.send(&moved, done, &input),
        None,
        "the field keeps the focus"
    );
    assert_eq!(state.focused(), Some(field));
    assert_eq!(state.send(&Event::Text("ab"), None, &input), typed("ab"));
    assert_eq!(
        state.send(&Event::Text("\u{8}"), None, &input),
        None,
        "no control character"
    );
    // Space types, as its text event follows; it presses nothing.
    assert_eq!(state.send(&press(Key::Space), None, &input), None);
    assert_eq!(state.send(&Event::Text(" "), None, &input), typed("ab "));
    assert_eq!(
        state.send(&Event::Text("cdefgh"), None, &input),
        typed("ab cdefg"),
        "8 at most"
    );
    assert_eq!(state.send(&Event::Text("i"), None, &input), None, "full");
    state.send(&press(Key::Home), None, &input);
    state.send(&press(Key::ArrowRight), None, &input);
    assert_eq!(
        state.send(&press(Key::Delete), None, &input),
        typed("a cdefg")
    );
    assert_eq!(
        state.send(&press(Key::Backspace), None, &input),
        typed(" cdefg")
    );
    assert_eq!(
        state.send(&press(Key::Backspace), None, &input),
        None,
        "at its start"
    );
    // Shift+End selects to the end; typing replaces it.
    input.record(&press(Key::ShiftLeft));
    state.send(&press(Key::ArrowRight), None, &input);
    state.send(&press(Key::End), None, &input);
    assert_eq!((state.caret.from, state.caret.at), (0, 6));
    input.record(&Event::Key {
        key: Key::ShiftLeft,
        pressed: false,
        repeat: false,
    });
    assert_eq!(state.send(&Event::Text("é"), None, &input), typed("é"));
    assert_eq!(state.caret.at, 2, "after its two bytes");
    // Ctrl+A selects all; Backspace cuts it.
    state.send(&Event::Text("xy"), None, &input);
    input.record(&press(Key::ControlLeft));
    state.send(&press(Key::KeyA), None, &input);
    assert_eq!(state.send(&press(Key::Backspace), None, &input), typed(""));
    assert_eq!(text(&state), "");
    let event = state.send(&press(Key::Enter), None, &input);
    assert_eq!(event, Some(UiEvent::Pressed("player".into())));
    let event = state.send(&press(Key::ArrowDown), None, &input);
    assert_eq!(event, Some(UiEvent::Focused("done".into())));
    assert_eq!(
        state.send(&Event::Text("z"), None, &input),
        None,
        "not focused"
    );
    // A click puts the caret at the nearest boundary.
    state.nodes[field].text = "iiiiwwww".to_owned();
    let start = on(&state, "player", 0.0).map(|at| at + Vec2::new(24.0, 0.0));
    state.send(&click(true), start, &input);
    assert_eq!(state.caret.at, 0, "at its start");
    let end = on(&state, "player", 0.99);
    state.send(&click(true), end, &input);
    assert_eq!(state.caret.at, 8, "past its end");
}

/// A text field takes a paste at the caret over the selection: its first line minus control
/// characters, up to its limit. Ctrl+C copies the selection and Ctrl+X cuts it (Cmd on macOS),
/// nothing if empty. Ctrl+V types nothing itself; the paste follows.
#[test]
fn a_text_field_pastes_at_its_caret_and_copies_its_selection() {
    let mut state = laid(WIDGETS, 1920.0);
    let mut input = Input::default();
    let over = on(&state, "player", 0.9);
    state.send(&click(true), over, &input);
    state.send(&click(false), over, &input);
    let typed = |text: &str| changed("player", UiValue::Text(text.to_owned()));
    let paste = |text| Event::Paste(text);
    assert_eq!(
        state.send(&paste("ab\tc\r\nsecond"), None, &input),
        typed("abc")
    );
    // Shift+Home selects it all; the paste replaces it, 8 at most.
    input.record(&press(Key::ShiftLeft));
    state.send(&press(Key::Home), None, &input);
    let let_go = |key| Event::Key {
        key,
        pressed: false,
        repeat: false,
    };
    input.record(&let_go(Key::ShiftLeft));
    assert_eq!(
        state.send(&paste("0123456789"), None, &input),
        typed("01234567")
    );
    // 234 selected.
    state.send(&press(Key::Home), None, &input);
    state.send(&press(Key::ArrowRight), None, &input);
    state.send(&press(Key::ArrowRight), None, &input);
    input.record(&press(Key::ShiftLeft));
    for _ in 0..3 {
        state.send(&press(Key::ArrowRight), None, &input);
    }
    input.record(&let_go(Key::ShiftLeft));
    let command = if cfg!(target_os = "macos") {
        Key::SuperLeft
    } else {
        Key::ControlLeft
    };
    input.record(&press(command));
    assert_eq!(state.send(&press(Key::KeyC), None, &input), None);
    assert_eq!(state.copied.take().as_deref(), Some("234"));
    assert_eq!(state.send(&press(Key::KeyX), None, &input), typed("01567"));
    assert_eq!(state.copied.take().as_deref(), Some("234"));
    assert_eq!(state.send(&press(Key::KeyC), None, &input), None);
    assert_eq!(state.copied, None, "nothing selected");
    assert_eq!(state.send(&press(Key::KeyV), None, &input), None);
    assert_eq!(
        state.nodes[state.named("player").expect("a node")].text,
        "01567"
    );
}

/// A copy from a layer's UI goes to the context, for the window to put on the OS clipboard; a
/// paste reaches the field.
#[test]
fn a_ui_s_copies_go_to_the_context() {
    let scratch = crate::dirs::tests::Scratch::new("ui-copies");
    std::fs::create_dir_all(&scratch.0).expect("a directory");
    let path = scratch.0.join("widgets.ui");
    std::fs::write(&path, WIDGETS).expect("a UI");
    let mut cx = Context::seeded(0).root(scratch.0.clone());
    let ui = cx.add_ui("widgets", &path).expect("added");
    ui.set_focus(&mut cx, "player").expect("focused");
    let pasted = ui.event(&mut cx, &Event::Paste("code"));
    assert_eq!(pasted, changed("player", UiValue::Text("code".to_owned())));
    let command = if cfg!(target_os = "macos") {
        Key::SuperLeft
    } else {
        Key::ControlLeft
    };
    for key in [command, Key::ShiftLeft] {
        cx.record_input(&press(key));
    }
    assert_eq!(ui.event(&mut cx, &press(Key::Home)), None);
    ui.event(&mut cx, &press(Key::KeyC));
    assert_eq!(cx.copied.as_deref(), Some("code"));
}

/// A list's children lie down from its top, clipped to it. The wheel scrolls them no further than
/// they reach, and a relayout keeps the scroll. A child scrolled out takes no click. The focus
/// moving to a child by keys scrolls it into view; the cursor coming over one at the edge doesn't.
#[test]
fn a_list_scrolls_by_the_wheel_and_to_the_focus() {
    let mut state = laid(WIDGETS, 1920.0);
    let input = Input::default();
    assert_eq!(rect(&state, "save_0"), [1620.0, 415.0, 300.0, 80.0]);
    assert_eq!(rect(&state, "save_5"), [1620.0, 865.0, 300.0, 80.0]);
    let list = state.named("saves").expect("a node");
    let save_0 = state.named("save_0").expect("a node");
    let room = Rect {
        at: Vec2::new(1620.0, 415.0),
        size: Vec2::new(300.0, 250.0),
    };
    assert_eq!(state.laid.clips[save_0], Some(room));
    assert_eq!(state.laid.clips[list], None);
    let over = Some(Vec2::new(1700.0, 500.0));
    let wheel = |y: f32| Event::Scroll(crate::Scroll::Lines { x: 0.0, y });
    assert_eq!(state.send(&wheel(-2.0), over, &input), None);
    state.lay_out_again(1920.0);
    assert_eq!(rect(&state, "save_0")[1], 415.0 - 120.0, "two lines down");
    state.send(&wheel(-10.0), over, &input);
    state.lay_out_again(1920.0);
    // Hidden, like a menu page, then shown again, it keeps its scroll.
    state.nodes[list].visible = false;
    state.lay_out_again(1920.0);
    state.nodes[list].visible = true;
    state.lay_out_again(1920.0);
    assert_eq!(
        state.nodes[list].scroll, 280.0,
        "as far as they reach past it"
    );
    assert_eq!(rect(&state, "save_5")[1], 865.0 - 280.0);
    // save_0, scrolled out, takes no click.
    let hidden = on(&state, "save_0", 0.5);
    state.send(&click(true), hidden, &input);
    assert_eq!(state.send(&click(false), hidden, &input), None);
    state.send(&wheel(20.0), over, &input);
    state.lay_out_again(1920.0);
    assert_eq!(state.nodes[list].scroll, 0.0, "back at its top");
    // The cursor over save_2, half out of view at the bottom, focuses it without scrolling.
    let edge = Some(Vec2::new(1700.0, 660.0));
    let event = state.send(&Event::CursorMoved { x: 0.0, y: 0.0 }, edge, &input);
    assert_eq!(event, Some(UiEvent::Focused("save_2".into())));
    assert_eq!(state.nodes[list].scroll, 0.0);
    // Down by keys to save_3 scrolls it whole into view at the bottom.
    let event = state.send(&press(Key::ArrowDown), None, &input);
    assert_eq!(event, Some(UiEvent::Focused("save_3".into())));
    state.lay_out_again(1920.0);
    let [_, y, _, height] = rect(&state, "save_3");
    assert_eq!(y + height, 415.0 + 250.0, "at its bottom");
    // Tab round past save_5 to the first widget, then Shift+Tab back to save_0, scrolls it into
    // view at the top.
    for _ in 0..2 {
        state.send(&press(Key::Tab), None, &input);
    }
    let event = state.send(&press(Key::Tab), None, &input);
    assert_eq!(event, Some(UiEvent::Focused("volume".into())));
    let mut shifted = Input::default();
    shifted.record(&press(Key::ShiftLeft));
    for _ in 0..6 {
        state.send(&press(Key::Tab), None, &shifted);
    }
    assert_eq!(state.focused(), Some(save_0));
    state.lay_out_again(1920.0);
    assert_eq!(rect(&state, "save_0")[1], 415.0, "at its top");
}

/// Code reads and sets widget values by name (a slider's clamped; another widget's kind of value
/// refused) and sets the focus by name, scrolling it into view. A value set by code or input
/// survives a reload, a slider's clamped to its new bounds; a drop-down's choice past its new
/// options reverts to the file's. A list keeps its scroll.
#[test]
fn values_are_read_and_set_by_name_and_kept_through_a_reload() {
    let scratch = crate::dirs::tests::Scratch::new("ui-widgets");
    std::fs::create_dir_all(&scratch.0).expect("a directory");
    let path = scratch.0.join("widgets.ui");
    std::fs::write(&path, WIDGETS).expect("a UI");
    let mut cx = Context::seeded(0).root(scratch.0.clone());
    let ui = cx.add_ui("widgets", &path).expect("added");
    let value = |cx: &mut Context, name: &str| ui.value(cx, name).expect("a widget");
    assert_eq!(value(&mut cx, "volume"), UiValue::Number(0.5));
    assert_eq!(value(&mut cx, "difficulty"), UiValue::Choice(1));
    ui.set_value(&mut cx, "volume", UiValue::Number(3.0))
        .expect("set");
    assert_eq!(
        value(&mut cx, "volume"),
        UiValue::Number(1.0),
        "within its bounds"
    );
    ui.set_value(&mut cx, "sound", UiValue::On(true))
        .expect("set");
    ui.set_value(&mut cx, "difficulty", UiValue::Choice(2))
        .expect("set");
    let long = UiValue::Text("a name too long\nfor it".to_owned());
    ui.set_value(&mut cx, "player", long).expect("set");
    assert_eq!(
        value(&mut cx, "player"),
        UiValue::Text("a name t".to_owned())
    );
    for (name, wrong, code) in [
        ("volume", UiValue::On(true), "ui.bad_value"),
        ("volume", UiValue::Number(f32::NAN), "ui.bad_value"),
        ("difficulty", UiValue::Choice(3), "ui.bad_value"),
        ("done", UiValue::On(true), "ui.wrong_kind"),
        ("nobody", UiValue::On(true), "ui.no_node"),
    ] {
        let error = ui.set_value(&mut cx, name, wrong).expect_err("refused");
        assert_eq!(error.code(), code, "{name}: {error:#}");
    }
    ui.set_focus(&mut cx, "save_5").expect("focused");
    assert_eq!(ui.focused(&mut cx).as_deref(), Some("save_5"));
    let error = ui.set_focus(&mut cx, "options").expect_err("a column");
    assert_eq!(error.code(), "ui.wrong_kind");
    let parts = cx.ui_parts();
    let state = parts.uis.state(ui);
    state.lay_out_again(1920.0);
    let list = state.named("saves").expect("a node");
    assert_eq!(state.nodes[list].scroll, 280.0, "scrolled to its last");
    // Reloaded with the slider's bounds narrowed and the drop-down's options fewer.
    let edited = WIDGETS
        .replace("step: 0.1, value: 0.5", "max: 0.5")
        .replace("[\"EASY\", \"NORMAL\", \"HARD\"]", "[\"EASY\", \"HARD\"]");
    let document = parse(&edited).unwrap_or_else(|error| panic!("{error:#}"));
    let uis = cx.ui_parts().uis;
    uis.reload(0, document);
    assert_eq!(
        value(&mut cx, "volume"),
        UiValue::Number(0.5),
        "its new max"
    );
    assert_eq!(value(&mut cx, "sound"), UiValue::On(true));
    assert_eq!(
        value(&mut cx, "difficulty"),
        UiValue::Choice(1),
        "the file's"
    );
    assert_eq!(
        value(&mut cx, "player"),
        UiValue::Text("a name t".to_owned())
    );
    let state = cx.ui_parts().uis.state(ui);
    let list = state.named("saves").expect("a node");
    assert_eq!(state.nodes[list].scroll, 280.0, "kept");
}

/// [`State::takes`] takes mouse buttons and wheel from the layers below over a button, widget
/// (disabled too) or list, not over a panel, stack or nothing; a release after a press it holds,
/// anywhere; everything anywhere while options are open; nothing while it takes no input.
#[test]
fn the_mouse_over_a_button_a_widget_or_a_list_is_taken_from_the_layers_below() {
    let mut menu = laid(MENU, 1920.0);
    menu.drawn = Some(0);
    let scroll = Event::Scroll(crate::Scroll::Lines { x: 0.0, y: -1.0 });
    let takes = |state: &State, event: &Event<'_>, at: Option<Vec2>| state.takes(event, at, 0);
    for name in ["play", "credits", "left"] {
        let at = on(&menu, name, 0.5);
        assert!(takes(&menu, &click(true), at), "a press on {name}");
        assert!(takes(&menu, &click(false), at), "a release on {name}");
        assert!(menu.pointer_over(at.expect("a point")), "over {name}");
    }
    for name in ["room", "inside", "corner"] {
        let at = on(&menu, name, 0.5);
        assert!(!takes(&menu, &click(true), at), "a press on {name}");
        assert!(!takes(&menu, &scroll, at), "the wheel over {name}");
    }
    assert!(
        !takes(&menu, &click(true), None),
        "a press outside the view"
    );
    // Held: a release away from the press, as a slider's drag ends outside it.
    let _ = menu.send(&click(true), on(&menu, "play", 0.5), &Input::default());
    assert!(takes(&menu, &click(false), on(&menu, "room", 0.5)), "held");
    let _ = menu.send(&click(false), on(&menu, "room", 0.5), &Input::default());
    assert!(
        !takes(&menu, &click(false), on(&menu, "room", 0.5)),
        "let go"
    );
    menu.input = false;
    assert!(
        !takes(&menu, &click(true), on(&menu, "play", 0.5)),
        "no input"
    );

    let mut widgets = laid(WIDGETS, 1920.0);
    widgets.drawn = Some(0);
    assert!(
        takes(&widgets, &scroll, on(&widgets, "saves", 0.5)),
        "a list"
    );
    assert!(!takes(&widgets, &scroll, Some(Vec2::new(100.0, 100.0))));
    let at = on(&widgets, "difficulty", 0.5);
    let _ = widgets.send(&click(true), at, &Input::default());
    let _ = widgets.send(&click(false), at, &Input::default());
    assert!(widgets.open.is_some(), "the options are open");
    let far = Some(Vec2::new(100.0, 100.0));
    assert!(
        takes(&widgets, &click(true), far),
        "a click outside the options"
    );
    assert!(takes(&widgets, &scroll, far), "the wheel");
}

/// Open options take the mouse everywhere only while the drop-down shows and isn't disabled, so a
/// game that hides or disables it with options open doesn't lose its clicks.
#[test]
fn options_left_open_under_a_hidden_or_disabled_drop_down_take_nothing() {
    let mut state = laid(WIDGETS, 1920.0);
    state.drawn = Some(0);
    let at = on(&state, "difficulty", 0.5);
    let _ = state.send(&click(true), at, &Input::default());
    let _ = state.send(&click(false), at, &Input::default());
    let far = Some(Vec2::new(100.0, 100.0));
    assert!(state.takes(&click(true), far, 0), "open");
    assert!(state.pointer_over(Vec2::new(100.0, 100.0)));
    let difficulty = state.named("difficulty").expect("a node");
    state.nodes[difficulty].disabled = true;
    assert!(!state.takes(&click(true), far, 0), "disabled");
    assert!(!state.pointer_over(Vec2::new(100.0, 100.0)));
    state.nodes[difficulty].disabled = false;
    state.nodes[difficulty].visible = false;
    assert!(!state.takes(&click(true), far, 0), "hidden");
    assert!(!state.pointer_over(Vec2::new(100.0, 100.0)));
}

/// A UI that didn't draw in the last frame, or takes no input, takes nothing: a menu no longer
/// drawn doesn't swallow clicks over its invisible buttons.
#[test]
fn a_ui_that_didnt_draw_in_the_last_frame_takes_nothing() {
    let mut state = laid(MENU, 1920.0);
    let at = on(&state, "play", 0.5);
    assert!(!state.takes(&click(true), at, 0), "never drawn");
    state.drawn = Some(3);
    assert!(state.takes(&click(true), at, 3), "drawn this frame");
    assert!(state.takes(&click(true), at, 4), "drawn in the last");
    assert!(!state.takes(&click(true), at, 5), "not for two frames");
}

/// While a text field has the focus, keys and typed text are taken from the layers below, so a
/// typed name doesn't walk the player; pad buttons aren't. Without the focus, keys aren't.
#[test]
fn keys_and_text_are_taken_while_a_text_field_has_the_focus() {
    let mut state = laid(WIDGETS, 1920.0);
    state.drawn = Some(0);
    let typed = Event::Text("w");
    assert!(
        !state.takes(&press(Key::KeyW), None, 0),
        "the slider's focus"
    );
    assert!(!state.takes(&typed, None, 0));
    let at = on(&state, "player", 0.5);
    let _ = state.send(&click(true), at, &Input::default());
    let _ = state.send(&click(false), at, &Input::default());
    assert!(state.typing().is_some(), "the field has the focus");
    assert!(state.takes(&press(Key::KeyW), None, 0), "a key");
    assert!(state.takes(&typed, None, 0), "text");
    assert!(
        !state.takes(&pad(PadButton::South), None, 0),
        "a pad's button"
    );
}

/// Three cards in a row, 200 by 100 with 20 between, the middle one at x 860 to 1060 and y 490 to
/// 590 in a view 1,920 across; a list of two buttons at the top left, 150 high, reaching 200.
const CARDS: &str = r#"(
    nodes: [
        Row(
            name: "hand",
            gap: 20.0,
            children: [
                Button(name: "c0", width: Fixed(200.0), height: Fixed(100.0)),
                Button(name: "c1", width: Fixed(200.0), height: Fixed(100.0)),
                Button(name: "c2", width: Fixed(200.0), height: Fixed(100.0)),
            ],
        ),
        List(
            name: "shelf",
            anchor: (Start, Start),
            width: Fixed(300.0),
            height: Fixed(150.0),
            children: [
                Button(name: "s0", width: Fixed(300.0), height: Fixed(100.0)),
                Button(name: "s1", width: Fixed(300.0), height: Fixed(100.0)),
            ],
        ),
    ],
)"#;

/// A backing panel, then two buttons in a row: `a` at x 750 to 950, `b` at 970 to 1170, y 490 to
/// 590; then a veil over the left 1,070 units of the view, which holds `again`, at x 435 to 635.
/// The veil covers `a` wholly and `b` in part.
const COVER: &str = r#"(
    focus: "again",
    nodes: [
        Panel(name: "backing", width: Fixed(1000.0), height: Fixed(400.0)),
        Row(
            name: "hand",
            gap: 20.0,
            children: [
                Button(name: "a", width: Fixed(200.0), height: Fixed(100.0)),
                Button(name: "b", width: Fixed(200.0), height: Fixed(100.0)),
            ],
        ),
        Panel(
            name: "veil",
            anchor: (Start, Center),
            width: Fixed(1070.0),
            height: Fill(1.0),
            children: [Button(name: "again", width: Fixed(200.0), height: Fixed(60.0))],
        ),
    ],
)"#;

/// What a click on `name` at `along` its width does: its press and its release.
fn clicked(state: &mut State, name: &str, along: f32) -> [Option<UiEvent>; 2] {
    let at = on(state, name, along);
    let input = Input::default();
    [true, false].map(|down| state.send(&click(down), at, &input))
}

/// A panel drawn over a button takes the pointer there: the cursor focuses nothing and a click
/// presses nothing under it, though the veil's own button and the part of a button it leaves bare
/// work. Panels drawn before a button, as its backing, don't block it. A text doesn't cover. A
/// hidden panel covers nothing.
#[test]
fn a_panel_drawn_over_a_button_takes_the_pointer_there() {
    let mut state = laid(COVER, 1920.0);
    state.drawn = Some(0);
    let input = Input::default();
    let moved = Event::CursorMoved { x: 0.0, y: 0.0 };
    assert_eq!(
        state.send(&moved, on(&state, "a", 0.5), &input),
        None,
        "no focus by the cursor"
    );
    assert_eq!(state.focus, state.named("again"));
    assert_eq!(clicked(&mut state, "a", 0.5), [None, None], "covered whole");
    assert_eq!(
        clicked(&mut state, "b", 0.2),
        [None, None],
        "covered in part"
    );
    assert_eq!(state.hovered, None);
    let pressed = |name: &str| Some(UiEvent::Pressed(name.into()));
    let focused = |name: &str| Some(UiEvent::Focused(name.into()));
    assert_eq!(
        clicked(&mut state, "b", 0.9),
        [focused("b"), pressed("b")],
        "bare, though the backing is drawn before it"
    );
    assert_eq!(
        clicked(&mut state, "again", 0.5),
        [focused("again"), pressed("again")],
        "the veil's own"
    );

    // Taken from the layers below as a click on a button is, where a button is covered; not over
    // a veil with nothing under it.
    let covered = on(&state, "a", 0.5).expect("a point");
    assert!(state.pointer_over(covered));
    assert!(state.takes(&click(true), Some(covered), 0));
    assert!(!state.pointer_over(Vec2::new(500.0, 100.0)), "a veil alone");

    // A text, drawn after a button, covers nothing.
    let mut texts = laid(
        r#"(nodes: [
            Button(name: "a", width: Fixed(200.0), height: Fixed(100.0)),
            Text(name: "label", text: "x", width: Fixed(600.0), height: Fixed(600.0)),
        ])"#,
        1920.0,
    );
    let pressed_a = clicked(&mut texts, "a", 0.5);
    assert_eq!(pressed_a, [focused("a"), pressed("a")]);

    let veil = state.named("veil").expect("a node");
    state.nodes[veil].visible = false;
    state.lay_out_again(1920.0);
    assert_eq!(
        clicked(&mut state, "a", 0.5),
        [focused("a"), pressed("a")],
        "the veil hidden"
    );
}

/// A list's own panels don't cover it: the wheel over a panel row scrolls the list.
#[test]
fn a_list_of_panels_scrolls_by_the_wheel_over_them() {
    let mut state = laid(
        r#"(nodes: [
            List(
                name: "rows",
                width: Fixed(300.0),
                height: Fixed(200.0),
                children: [
                    Panel(name: "row_0", width: Fixed(300.0), height: Fixed(150.0)),
                    Panel(name: "row_1", width: Fixed(300.0), height: Fixed(150.0)),
                ],
            ),
        ])"#,
        1920.0,
    );
    let input = Input::default();
    let over = on(&state, "row_0", 0.5);
    let wheel = Event::Scroll(crate::Scroll::Lines { x: 0.0, y: -1.0 });
    state.send(&wheel, over, &input);
    state.lay_out_again(1920.0);
    let rows = state.named("rows").expect("a node");
    assert!(state.nodes[rows].scroll > 0.0, "scrolled over a row");
}

/// A panel covering a node wholly takes it out of the focus's moves by key or pad, Tab and the
/// file's focus; one covering it in part leaves it in. The focus the node already has stays.
#[test]
fn focus_moves_skip_a_button_a_panel_wholly_covers() {
    let mut state = laid(COVER, 1920.0);
    let input = Input::default();
    let focused = |name: &str| Some(UiEvent::Focused(name.into()));
    let key = |state: &mut State, key: Key, input: &Input| state.send(&press(key), None, input);
    assert_eq!(key(&mut state, Key::Tab, &input), focused("b"), "past a");
    assert_eq!(key(&mut state, Key::Tab, &input), focused("again"));
    let mut shifted = Input::default();
    shifted.record(&press(Key::ShiftLeft));
    assert_eq!(key(&mut state, Key::Tab, &shifted), focused("b"));
    assert_eq!(
        key(&mut state, Key::ArrowLeft, &input),
        focused("again"),
        "a is nearer, and covered"
    );
    assert_eq!(key(&mut state, Key::ArrowRight, &input), focused("b"));
    assert_eq!(
        state.send(&pad(PadButton::DPadLeft), None, &input),
        focused("again")
    );

    // With none focused, the file's focus if it's reachable, else the first that is.
    state.focus = None;
    assert_eq!(key(&mut state, Key::ArrowDown, &input), focused("again"));
    let edited = COVER.replace("focus: \"again\"", "focus: \"a\"");
    let mut state = laid(&edited, 1920.0);
    state.focus = None;
    assert_eq!(key(&mut state, Key::ArrowDown, &input), focused("b"));

    let a = state.named("a").expect("a node");
    state.focus = Some(a);
    assert_eq!(state.focused(), Some(a), "the focus it has stays");

    let veil = state.named("veil").expect("a node");
    state.nodes[veil].visible = false;
    state.lay_out_again(1920.0);
    assert_eq!(key(&mut state, Key::Tab, &input), focused("b"));
    assert_eq!(key(&mut state, Key::Tab, &input), focused("a"), "round");
}

/// Writes `files` (name and text) to a scratch directory and gives a context with the first added
/// as UI `name`, its view 1,280 by 720 from (100, 50) so the cursor's pixels are in a scale
/// of two thirds.
fn viewed(scratch: &str, name: &str, text: &str) -> (Context, GameUi) {
    let scratch = crate::dirs::tests::Scratch::new(scratch);
    std::fs::create_dir_all(&scratch.0).expect("a directory");
    let path = scratch.0.join(format!("{name}.ui"));
    std::fs::write(&path, text).expect("a UI");
    let mut cx = Context::seeded(0).root(scratch.0.clone());
    let ui = cx.add_ui(name, &path).expect("added");
    let view = Viewport {
        origin: [100, 50],
        size: [1280, 720],
    };
    cx.extract(&mut crate::render_thread::Extracted::default(), view);
    (cx, ui)
}

/// Moves the cursor to canvas point `(x, y)` of the view [`viewed`] makes.
fn point_at(cx: &mut Context, x: f64, y: f64) {
    let event = Event::CursorMoved {
        x: 100.0 + x * 2.0 / 3.0,
        y: 50.0 + y * 2.0 / 3.0,
    };
    cx.record_input(&event);
}

/// `hovered` names the button or widget under the cursor now, disabled or not, not what a panel
/// covers, and nothing once the cursor is off all of them, outside the view or gone, where
/// `focused` stays on the last the cursor came over.
#[test]
fn hovered_goes_with_the_cursor() {
    let (mut cx, ui) = viewed("ui-hovered", "cover", COVER);
    let hovered = |cx: &mut Context| ui.hovered(cx).map(|name| name.to_string());
    assert_eq!(hovered(&mut cx), None, "no cursor yet");
    point_at(&mut cx, 1150.0, 540.0);
    assert_eq!(hovered(&mut cx).as_deref(), Some("b"));
    point_at(&mut cx, 1010.0, 540.0);
    assert_eq!(hovered(&mut cx), None, "under the veil");
    point_at(&mut cx, 850.0, 540.0);
    assert_eq!(hovered(&mut cx), None, "under the veil");
    point_at(&mut cx, 535.0, 540.0);
    assert_eq!(hovered(&mut cx).as_deref(), Some("again"));
    point_at(&mut cx, 1150.0, 540.0);
    ui.set_disabled(&mut cx, "b", true).expect("disabled");
    assert_eq!(hovered(&mut cx).as_deref(), Some("b"), "disabled too");
    assert_eq!(ui.focused(&mut cx).as_deref(), Some("again"));
    point_at(&mut cx, 1500.0, 100.0);
    assert_eq!(hovered(&mut cx), None, "over nothing");
    assert_eq!(ui.focused(&mut cx).as_deref(), Some("again"), "focus stays");
    point_at(&mut cx, 1150.0, 540.0);
    cx.record_input(&Event::CursorLeft);
    assert_eq!(hovered(&mut cx), None, "the cursor left");
    cx.record_input(&Event::CursorMoved { x: 20.0, y: 20.0 });
    assert_eq!(hovered(&mut cx), None, "outside the view");
    // Hidden, a button is hovered no more.
    point_at(&mut cx, 1150.0, 540.0);
    ui.set_visible(&mut cx, "hand", false).expect("hidden");
    assert_eq!(hovered(&mut cx), None);
}

/// Settles `state`'s layout for a view 1,920 across if it isn't already.
fn settled(state: &mut State) {
    FONTS.with_borrow_mut(|fonts| state.lay_out(fonts, Vec2::new(1920.0, 1080.0)));
}

/// An offset moves a node and its subtree where they draw and where the pointer finds them, and
/// no other node or layout; offsets add down the tree; a list's clip moves with it; an offset
/// takes no new layout; code refuses a name no node has and an offset that isn't finite; and a
/// reload keeps it.
#[test]
fn an_offset_moves_where_a_node_draws_and_is_found_and_nothing_else() {
    let (mut cx, ui) = viewed("ui-offset", "cards", CARDS);
    let state = cx.ui_parts().uis.state(ui);
    settled(state);
    let before: Vec<_> = ["hand", "c0", "c1", "c2", "shelf", "s0", "s1"]
        .map(|name| rect(state, name))
        .into();
    assert_eq!(rect(state, "c1"), [860.0, 490.0, 200.0, 100.0]);

    ui.set_offset(&mut cx, "c1", Vec2::new(0.0, -40.0))
        .expect("set");
    let state = cx.ui_parts().uis.state(ui);
    assert_eq!(state.laid_for, Some(Vec2::new(1920.0, 1080.0)), "no layout");
    settled(state);
    let after: Vec<_> = ["hand", "c0", "c1", "c2", "shelf", "s0", "s1"]
        .map(|name| rect(state, name))
        .into();
    for (i, name) in ["hand", "c0", "c1", "c2", "shelf", "s0", "s1"]
        .iter()
        .enumerate()
    {
        if *name == "c1" {
            assert_eq!(after[i], [860.0, 450.0, 200.0, 100.0], "lifted");
        } else {
            assert_eq!(after[i], before[i], "{name} stays");
        }
    }
    // The pointer finds c1 where it draws: its old bottom strip is bare; its new top is c1.
    let (middle, strip, top) = (
        Vec2::new(960.0, 500.0),
        Vec2::new(960.0, 580.0),
        Vec2::new(960.0, 455.0),
    );
    assert_eq!(state.pointed_at(middle), state.named("c1"));
    assert_eq!(state.pointed_at(strip), None);
    assert_eq!(state.pointed_at(top), state.named("c1"));
    assert_eq!(
        state.pointed_at(Vec2::new(740.0, 540.0)),
        state.named("c0"),
        "its neighbors stay"
    );

    // The row's offset moves the cards with it, over the lift: they add.
    ui.set_offset(&mut cx, "hand", Vec2::new(10.0, 100.0))
        .expect("set");
    let state = cx.ui_parts().uis.state(ui);
    settled(state);
    assert_eq!(rect(state, "hand")[..2], [640.0 + 10.0, 490.0 + 100.0]);
    assert_eq!(rect(state, "c0")[..2], [650.0, 590.0]);
    assert_eq!(rect(state, "c1")[..2], [870.0, 550.0]);
    ui.set_offset(&mut cx, "hand", Vec2::ZERO).expect("set");
    let state = cx.ui_parts().uis.state(ui);
    settled(state);
    assert_eq!(rect(state, "c0"), before[1], "put back");

    // A list's clip moves with it: s1 reaches past the list's 150 high, and shows to there.
    let state = cx.ui_parts().uis.state(ui);
    assert_eq!(state.pointed_at(Vec2::new(150.0, 130.0)), state.named("s1"));
    assert_eq!(state.pointed_at(Vec2::new(150.0, 170.0)), None, "clipped");
    ui.set_offset(&mut cx, "shelf", Vec2::new(500.0, 20.0))
        .expect("set");
    let state = cx.ui_parts().uis.state(ui);
    settled(state);
    assert_eq!(state.pointed_at(Vec2::new(150.0, 130.0)), None, "moved");
    assert_eq!(state.pointed_at(Vec2::new(650.0, 40.0)), state.named("s0"));
    assert_eq!(state.pointed_at(Vec2::new(650.0, 150.0)), state.named("s1"));
    assert_eq!(state.pointed_at(Vec2::new(650.0, 190.0)), None, "clipped");

    for (name, offset, code) in [
        ("nobody", Vec2::ZERO, "ui.no_node"),
        ("c0", Vec2::new(f32::NAN, 0.0), "ui.bad_value"),
        ("c0", Vec2::new(0.0, f32::INFINITY), "ui.bad_value"),
    ] {
        let error = ui.set_offset(&mut cx, name, offset).expect_err("refused");
        assert_eq!(error.code(), code, "{name}: {error:#}");
    }

    // Kept by name through a reload, with nodes added.
    let edited = CARDS.replace("name: \"c0\"", "name: \"c9\"");
    let document = parse(&edited).unwrap_or_else(|error| panic!("{error:#}"));
    cx.ui_parts().uis.reload(0, document);
    let state = cx.ui_parts().uis.state(ui);
    settled(state);
    assert_eq!(rect(state, "c1")[..2], [860.0, 450.0], "kept");
    assert_eq!(
        rect(state, "c9")[..2],
        [640.0, 490.0],
        "a renamed node's not"
    );
    assert_eq!(rect(state, "s0")[..2], [500.0, 20.0]);
}

/// The tool API lists a node's own offset where it isn't zero, beside its rectangle as moved.
#[cfg(feature = "dev")]
#[test]
fn the_tool_api_lists_a_nodes_offset_when_it_has_one() {
    let mut uis = Uis::default();
    let file = uis.add_file(
        "5f3c9a1e07b2d846".into(),
        "ui/cards.ui".into(),
        parse(CARDS).unwrap(),
        None,
    );
    let ui = uis.add(file, "cards");
    let state = uis.state(ui);
    let hand = state.named("hand").expect("a node");
    state.nodes[hand].offset = Vec2::new(0.0, -30.0);
    state.unsettled = true;
    let mut fonts = Fonts::new(&mut Textures::default());
    let view = Viewport {
        origin: [0, 0],
        size: [1920, 1080],
    };
    let listed = uis.listed(&mut fonts, Some(view));
    let nodes = &listed.uis[0].nodes;
    let node = |name: &str| {
        let found = nodes.iter().find(|node| node.name.as_deref() == Some(name));
        found.unwrap_or_else(|| panic!("a node `{name}`"))
    };
    assert_eq!(node("hand").offset, [0.0, -30.0]);
    assert_eq!(node("c1").offset, [0.0, 0.0], "its own, not its parent's");
    assert_eq!(node("c1").rect, [860.0, 460.0, 200.0, 100.0], "moved");
    let json = serde_json::to_value(&listed).expect("JSON");
    assert_eq!(
        json["uis"][0]["nodes"][0]["offset"],
        serde_json::json!([0.0, -30.0])
    );
    assert!(
        json["uis"][0]["nodes"][1].get("offset").is_none(),
        "left out"
    );
}
