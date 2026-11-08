use gg_renderer::Textures;

use super::file::parse;
use super::*;

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
    let mut fonts = Fonts::new(&mut Textures::default());
    state.lay_out(&mut fonts, Vec2::new(view, 1080.0));
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
        state.event(&key(key_pressed), None, input)
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
    let event = state.event(&moved, over, &input);
    assert_eq!(event, Some(UiEvent::Focused("options".into())));
    assert_eq!(state.event(&button(true), over, &input), None);
    let options = state.named("options").expect("a node");
    assert_eq!(state.look(options), PRESSED);
    let event = state.event(&button(false), over, &input);
    assert_eq!(event, Some(UiEvent::Pressed("options".into())));
    assert_eq!(state.look(options), FOCUSED);
    // Let go over another button, it presses none.
    state.event(&button(true), over, &input);
    assert_eq!(
        state.event(&button(false), at(&state, "play"), &input),
        None
    );
    // A disabled button neither takes the focus nor a click.
    let credits = at(&state, "credits");
    assert_eq!(state.event(&moved, credits, &input), None);
    assert_eq!(state.event(&button(true), credits, &input), None);
    assert_eq!(state.event(&button(false), credits, &input), None);

    let key = |key, pressed, repeat| Event::Key {
        key,
        pressed,
        repeat,
    };
    let enter = state.event(&key(Key::Enter, true, false), None, &input);
    assert_eq!(enter, Some(UiEvent::Pressed("options".into())));
    assert_eq!(state.look(options), PRESSED, "held");
    assert_eq!(
        state.event(&key(Key::Enter, false, false), None, &input),
        None
    );
    assert_eq!(state.look(options), FOCUSED, "let go");
    assert_eq!(
        state.event(&key(Key::Space, true, true), None, &input),
        None
    );
    let back = state.event(&key(Key::Escape, true, false), None, &input);
    assert_eq!(back, Some(UiEvent::Back));

    let pad = |button, pressed| Event::PadButton {
        pad: crate::Pad(0),
        button,
        pressed,
    };
    let down = state.event(&pad(PadButton::DPadDown, true), None, &input);
    assert_eq!(down, Some(UiEvent::Focused("quit".into())));
    let south = state.event(&pad(PadButton::South, true), None, &input);
    assert_eq!(south, Some(UiEvent::Pressed("quit".into())));
    assert_eq!(
        state.event(&pad(PadButton::East, true), None, &input),
        Some(UiEvent::Back)
    );
    let stick = |x: f32, y: f32| Event::PadStick {
        pad: crate::Pad(0),
        stick: Stick::Left,
        value: Vec2::new(x, y),
    };
    let up = state.event(&stick(0.0, 0.8), None, &input);
    assert_eq!(up, Some(UiEvent::Focused("options".into())));
    // Held near the diagonal, wavering either side of it, it steps no more.
    for (x, y) in [(0.45, 0.62), (0.62, 0.45), (0.45, 0.62), (0.7, 0.68)] {
        assert_eq!(
            state.event(&stick(x, y), None, &input),
            None,
            "held at {x}, {y}"
        );
    }
    assert_eq!(
        state.event(&stick(0.2, 0.1), None, &input),
        None,
        "near rest"
    );
    let again = state.event(&stick(0.1, 0.8), None, &input);
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
