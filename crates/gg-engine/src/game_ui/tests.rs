use std::cell::RefCell;

use gg_renderer::Textures;

use super::file::parse;
use super::layout::Rect;
use super::*;
use crate::{Key, MouseButton, PadButton, Stick};

thread_local! {
    /// The fonts each test's layouts and events measure text with, Inter baked once a thread.
    static FONTS: RefCell<Fonts> = RefCell::new(Fonts::new(&mut Textures::default()));
}

impl State {
    /// What `event` does to it, the cursor at `cursor` and the keys held as `input` has them, as
    /// [`State::event`] says, with the thread's fonts, laid out first for a view 1,920 units
    /// across, as [`GameUi::event`] lays it out.
    fn send(&mut self, event: &Event<'_>, cursor: Option<Vec2>, input: &Input) -> Option<UiEvent> {
        FONTS.with_borrow_mut(|fonts| {
            self.lay_out(fonts, Vec2::new(1920.0, 1080.0));
            self.event(event, cursor, input, fonts)
        })
    }

    /// Lays it out for a view `view` units across and 1,080 down, with the thread's fonts, again
    /// since it changed.
    fn lay_out_again(&mut self, view: f32) {
        self.laid_for = None;
        FONTS.with_borrow_mut(|fonts| self.lay_out(fonts, Vec2::new(view, 1080.0)));
    }
}

/// A menu of four buttons, the third disabled, in a column in the view's middle; a row along the
/// view's top, a button at each end of it and room between them; and a panel in the bottom right
/// corner, padded round a fixed stack: all of fixed sizes, so that where each goes is known.
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

/// A UI of `text`, its file's, laid out for a view `view` units across and 1,080 down.
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

/// A UI's file reads as its nodes, each after its parent and with the end of its subtree, each a
/// field left out taking its kind's default, a field that may be left out written as its value or
/// as `Some`, and the focus on the button it names.
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

/// A file that isn't a UI's is refused with the code of what's wrong, at the line and column
/// where the text is refused, which for a node's name is the name, and for a node's fields the
/// node's end.
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
        // Past their bounds, refused at the number rather than at the node's end.
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
    // The next parse knows none of the names the one before took.
    parse("(nodes: [Text(name: \"a\")])").expect("a name of its own");
}

/// The nodes lie where their sizes, fixed, fit and fill, their padding, gaps, margins, alignment
/// and anchors put them, in a view of 1,920 units across, and again in one of 1,440, the anchored
/// ones keeping to their edges.
#[test]
fn nodes_lie_where_their_sizes_and_alignment_put_them() {
    let state = laid(MENU, 1920.0);
    // Four buttons and three gaps, 380 high, in the view's middle.
    assert_eq!(rect(&state, "menu"), [760.0, 350.0, 400.0, 380.0]);
    assert_eq!(rect(&state, "play"), [760.0, 350.0, 400.0, 80.0]);
    assert_eq!(rect(&state, "quit"), [760.0, 650.0, 400.0, 80.0]);
    // Along the top, inside its margin, the room between its buttons filled.
    assert_eq!(rect(&state, "hud"), [40.0, 20.0, 1840.0, 60.0]);
    assert_eq!(rect(&state, "left"), [40.0, 20.0, 100.0, 60.0]);
    assert_eq!(rect(&state, "room"), [140.0, 50.0, 1640.0, 0.0]);
    assert_eq!(rect(&state, "right"), [1780.0, 20.0, 100.0, 60.0]);
    // In the bottom right corner, its padding round what it holds.
    assert_eq!(rect(&state, "corner"), [1770.0, 960.0, 120.0, 90.0]);
    assert_eq!(rect(&state, "inside"), [1780.0, 980.0, 100.0, 50.0]);

    let narrower = laid(MENU, 1440.0);
    assert_eq!(rect(&narrower, "menu"), [520.0, 350.0, 400.0, 380.0]);
    assert_eq!(rect(&narrower, "right"), [1300.0, 20.0, 100.0, 60.0]);
    assert_eq!(rect(&narrower, "corner"), [1290.0, 960.0, 120.0, 90.0]);

    // Fills share what's left by their shares, but none below its fit; and what's left without a
    // fill goes where the parent's alignment puts it, its children each placed across by their
    // own anchor or its alignment.
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
    // A fill no less than its fit, across a row and in a panel, overflowing round its anchor.
    assert_eq!(rect(&state, "short"), [0.0, 515.0, 10.0, 50.0]);
    assert_eq!(rect(&state, "tall"), [0.0, 490.0, 10.0, 100.0]);
    assert_eq!(rect(&state, "small"), [940.0, 1040.0, 40.0, 40.0]);
    assert_eq!(rect(&state, "big"), [930.0, 1040.0, 60.0, 40.0]);
}

/// A file of more nodes than a UI holds is refused as it's read, and one of as many reads; and a
/// row of many nodes each as large as it may be, its margins, its gaps and its padding too, lays
/// out finite, as the tool API lists it.
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

/// A hidden node takes no room, nor does what's under it, which is of no size where it is, and
/// shows nothing; shown again, it takes its room again.
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

/// The arrows move the focus to the nearest button that takes it the way they go, past a disabled
/// one, one level with it first, and no further than the last; Tab goes through them in the file's
/// order, coming round, Shift+Tab back; and a hidden button takes it no more.
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
    // As near the row's left button as its right, which comes after it; then the right one,
    // level with the left, though the menu's top is nearer.
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
    // With the focus on none, a move focuses the file's, here the first that takes it.
    assert_eq!(pressed(&mut state, Key::ArrowDown, &input), focused("play"));
    assert_eq!(pressed(&mut state, Key::Tab, &input), focused("quit"));
}

/// A click presses a button as it's let go over the button it went down on, which the cursor
/// focuses as it comes over it; Enter, Space and a pad's South press the focused button, not as a
/// key repeats; a disabled button takes neither; Escape and a pad's East ask for back; and a pad's
/// stick steps once a push, however it wavers, again only once it's come back near rest.
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
    // Let go over another button, it presses none.
    state.send(&button(true), over, &input);
    assert_eq!(state.send(&button(false), at(&state, "play"), &input), None);
    // A disabled button neither takes the focus nor a click.
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
    // Held near the diagonal, wavering either side of it, it steps no more.
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

/// A UI's file read again gives each UI of it its nodes: a node keeps the text code set of it,
/// where the node of its name still says something, and its visibility; and the focus stays on
/// the button of its name, wherever it's gone, or else goes to the file's.
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
    // Reordered, the score's text and the title's changed, and a button added.
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

/// The tool API's `ui` lists each UI's nodes laid out for the view, in the canvas's units and the
/// view's pixels, with what shows, what each says and each button's state.
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
    // 720 pixels high, two thirds of a pixel a unit, and as wide as the 1,920 units are.
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

/// A column of each widget in the view's middle, a slider, a toggle, a drop-down, a text field and
/// a button, each 400 units wide; and a list of six buttons along the view's right edge, 250 high,
/// which reach 530 down it: all of fixed sizes, so that where each goes is known.
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

/// A key's press, not a repeat.
fn press(key: Key) -> Event<'static> {
    Event::Key {
        key,
        pressed: true,
        repeat: false,
    }
}

/// A pad's button going down.
fn pad(button: PadButton) -> Event<'static> {
    Event::PadButton {
        pad: crate::Pad(0),
        button,
        pressed: true,
    }
}

/// The left mouse button going down or up.
fn click(pressed: bool) -> Event<'static> {
    Event::MouseButton {
        button: MouseButton::Left,
        pressed,
    }
}

/// The point of node `name`'s rectangle `along` its width and halfway down it.
fn on(state: &State, name: &str, along: f32) -> Option<Vec2> {
    let [x, y, width, height] = rect(state, name);
    Some(Vec2::new(x + width * along, y + height / 2.0))
}

/// The number a slider changed to, as `event` gives it.
fn number(event: Option<UiEvent>) -> f32 {
    match event {
        Some(UiEvent::Changed(_, UiValue::Number(number))) => number,
        other => panic!("a slider's change: {other:?}"),
    }
}

/// The change of widget `name` to `value`.
fn changed(name: &str, value: UiValue) -> Option<UiEvent> {
    Some(UiEvent::Changed(name.into(), value))
}

/// Widgets read from their file with their kinds' defaults, a list that fills its parent taking the
/// room it's given, and are refused with the codes of what's wrong with them: a widget without a
/// name, a field its kind doesn't take, a slider whose bounds or value are wrong, a drop-down
/// without options or whose choice is none of them, and a text field that holds more than it may.
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
    // A list that fills its parent down takes the room it's given, however much it holds.
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

/// A slider's value steps by its steps by the left and right arrows, as a repeat does, a pad's
/// D-pad and its stick, once a push, and goes to its ends by Home and End, no further; a click on
/// its track sets it where the cursor is, at the nearest step, and a drag goes on setting it, the
/// slider pressed until it's let go; each change gives its new value, and none that changes
/// nothing; and the up and down arrows move the focus.
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
    // Its track runs from 784 to 1136, inside its padding: 73% along is 0.7, at its step.
    let moved = Event::CursorMoved { x: 0.0, y: 0.0 };
    let at = on(&state, "volume", 0.0).map(|at| at + Vec2::new(24.0 + 352.0 * 0.73, 0.0));
    state.send(&moved, at, &input);
    near(number(state.send(&click(true), at, &input)), 0.7);
    let volume = state.named("volume").expect("a node");
    assert_eq!(state.look(volume), PRESSED, "held as it's dragged");
    // Dragged past its end, and up off it, it stays at its end.
    let past = at.map(|at| at + Vec2::new(400.0, -200.0));
    near(number(state.send(&moved, past, &input)), 1.0);
    assert_eq!(state.send(&moved, past, &input), None);
    assert_eq!(state.send(&click(false), past, &input), None);
    assert_eq!(state.look(volume), FOCUSED, "let go");
    // A drag ends as the slider's hidden or disabled under it, as a menu's page closes.
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

/// A slider whose value is off its steps' grid, from its `min`, steps to the grid's next point
/// either way, and its `max` past the grid's last is reached by keys and by a drag, as its `min`
/// is.
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
    // Its track runs from 784 to 1136: its end is its max, 97% along nearer it than 0.9, and its
    // start its min.
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

/// A toggle flips by Enter, Space and a pad's South, held pressed until they're let go, and by a
/// click let go over it, but not one let go off it; and the left and right arrows move the focus,
/// as a button's do.
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

/// A drop-down's choice steps by the left and right arrows within its options; Enter opens its
/// options over the rest, its choice lit, which take every event: the down arrow and a pad's stick
/// light the next, and Enter chooses the lit one, closing it, a change; Escape closes it as it was;
/// a click opens it, and a click on an option chooses it as it's let go; a click outside closes it,
/// and a pad's East.
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
    // The arrows light its options, and move the focus no more.
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
    // Its options open below it, each as tall as it: HARD is the third.
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

/// A drop-down hidden or disabled while its options are open closes them, the next event going to
/// the rest as if it had never opened; and its file read again closes them too.
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
    // The down arrow moves the focus, the drop-down's hidden, rather than lighting an option.
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

/// A text field takes the focus by a click, not by the cursor coming over it, its caret where it's
/// clicked; it takes what's typed at its caret, as many characters as it holds, and no control
/// character; the arrows move its caret and Shift selects, Home and End, Ctrl+A selecting all, and
/// what's typed or Backspace takes the selection's place; Backspace and Delete cut a character
/// either side; Space types, pressing nothing, Enter presses it, and the down arrow moves the focus
/// on.
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
    // The cursor passing over a button leaves the focus on the field, which goes on taking text.
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
    // Space types as its text follows, pressing nothing.
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
    // Shift+End selects to its end, and what's typed takes its place.
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
    // Ctrl+A selects it all, which Backspace cuts.
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
    // A click puts the caret between the characters nearest it.
    state.nodes[field].text = "iiiiwwww".to_owned();
    let start = on(&state, "player", 0.0).map(|at| at + Vec2::new(24.0, 0.0));
    state.send(&click(true), start, &input);
    assert_eq!(state.caret.at, 0, "at its start");
    let end = on(&state, "player", 0.99);
    state.send(&click(true), end, &input);
    assert_eq!(state.caret.at, 8, "past its end");
}

/// A text field takes what's pasted at its caret, in place of what's selected, its first line
/// without the characters no line holds, as many characters as it holds; Ctrl+C copies what's
/// selected, and Ctrl+X cuts it too, Cmd for Ctrl on macOS, nothing for a selection that's empty;
/// and Ctrl+V types nothing itself, as the paste follows it.
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
    // Shift+Home selects it all, which the paste takes the place of, 8 at most.
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

/// What a text field copies, through a UI a layer added, goes to the context, for the window to put
/// on the OS's clipboard, and what's pasted reaches the field.
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

/// A list's children lie down it from its top, clipped to it; the wheel over it scrolls them, no
/// further than they reach, and laid out again they lie scrolled up; a child scrolled out of it
/// takes no click; and the focus moving to a child by the keys scrolls it into view, while the
/// cursor coming over one at its edge doesn't.
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
    // Hidden, as a page of a menu is, and shown again, it keeps its scroll.
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
    // The cursor over save_2, half out of view at its bottom, focuses it without scrolling.
    let edge = Some(Vec2::new(1700.0, 660.0));
    let event = state.send(&Event::CursorMoved { x: 0.0, y: 0.0 }, edge, &input);
    assert_eq!(event, Some(UiEvent::Focused("save_2".into())));
    assert_eq!(state.nodes[list].scroll, 0.0);
    // Down by the keys, to save_3, scrolls it whole into view at the list's bottom.
    let event = state.send(&press(Key::ArrowDown), None, &input);
    assert_eq!(event, Some(UiEvent::Focused("save_3".into())));
    state.lay_out_again(1920.0);
    let [_, y, _, height] = rect(&state, "save_3");
    assert_eq!(y + height, 415.0 + 250.0, "at its bottom");
    // Tab round, past save_5 to the first widget, then back to save_0, scrolls it back into view
    // at the list's top.
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

/// Code reads and sets each widget's value by its name, a slider's within its bounds, refusing a
/// value of another widget's, and sets the focus by name, scrolling it into view; a value set by
/// code or input is kept as the UI's file is read again, a slider's within its new bounds, while a
/// drop-down's choice past its new options is the file's; and a list keeps its scroll.
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
    // Read again, the slider's bounds narrowed and the drop-down's options fewer.
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

/// What the mouse does over a UI is taken from the layers below, as [`State::takes`] says: its
/// buttons and wheel over a button, a widget, a disabled one too, or a list, and not over a panel,
/// a stack or nothing; a button let go after a press it holds, wherever; every button and the
/// wheel, wherever, while a drop-down's options are open; and nothing while it takes no input.
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
    // Held: a button let go away from where it went down, as a slider's drag ends outside it.
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

/// A drop-down's open options take the mouse everywhere only while the drop-down shows and isn't
/// disabled, as a game that hides or disables it with its options open doesn't lose its clicks.
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

/// A UI that didn't draw in the last frame, or takes no input, takes nothing and is over nothing,
/// as a menu no longer drawn doesn't swallow clicks over its invisible buttons.
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

/// While a text field has the focus, the keys and the text typed are taken from the layers below,
/// so that a name typed doesn't walk the player, and a pad's buttons aren't; without the focus, a
/// key isn't.
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
