use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::f64::consts::PI;
use std::mem;
use std::rc::Rc;

use egui::PointerButton::{Primary, Secondary};
use egui::ecolor::Hsva;
use gg_core::math::{self, EulerRot, Quat, Vec2, Vec3, Vec4};

use super::entities::{Tree, row_label};
use super::*;
use crate::reflect::tests::{Everything, Form, Inner, everything, same_snapshots, snapshot};
use crate::reflect::{self, Ident};
use crate::{Camera, Component, Entity, Projection, Sprite};

/// `entity`'s ID, which the Inspector picks it by and names it by.
fn id(world: &World, entity: Entity) -> EntityId {
    world.id(entity).expect("alive")
}

/// A text drawn, and where.
type Text = (String, egui::Rect);

/// Queues `pass`, as the panels do, and makes what it found at a tick, as the next one does.
fn make(inspector: &mut Inspector, world: &mut World, pass: Pass) {
    inspector.queue(pass);
    Layer::tick(inspector, world, Tick { number: 0 });
}

/// Draws the two panels side by side, each in a half of `ui` of its own, as the dock gives each
/// a rect of its own: below its rows, the Entities panel takes the rest of its half. First,
/// as a pass does, it makes the pick the link asked for.
fn panels(inspector: &mut Inspector, world: &World, ui: &mut egui::Ui, pass: &mut Pass) {
    inspector.follow(world);
    let (left, right) = ui.max_rect().split_left_right_at_fraction(0.5);
    let entities = egui::UiBuilder::new().id_salt("Entities").max_rect(left);
    ui.scope_builder(entities, |ui| inspector.entities(world, ui, pass));
    let inspect = egui::UiBuilder::new().id_salt("Inspector").max_rect(right);
    let pending = inspector.pending();
    ui.scope_builder(inspect, |ui| inspector.inspect(world, ui, pass, &pending));
}

/// A frame of the two panels' contents, on a screen big enough for all of them, given
/// `events`, whose edits are then set at a tick; returns its texts, and how many edits it made.
fn frame(
    inspector: &mut Inspector,
    world: &mut World,
    ctx: &egui::Context,
    time: f64,
    events: Vec<egui::Event>,
) -> (Vec<Text>, usize) {
    frame_ticking(inspector, world, ctx, time, events, true)
}

/// A frame as [`frame`] draws it, whose edits are set at a tick if `tick`, or else wait for
/// one, as at a frame rate above the simulation's.
fn frame_ticking(
    inspector: &mut Inspector,
    world: &mut World,
    ctx: &egui::Context,
    time: f64,
    events: Vec<egui::Event>,
    tick: bool,
) -> (Vec<Text>, usize) {
    let (shapes, made) = frame_shapes(inspector, world, ctx, time, events, tick);
    (texts_in(&shapes), made)
}

/// The texts `shapes` draw, and where, in the order they're drawn.
fn texts_in(shapes: &[egui::epaint::ClippedShape]) -> Vec<Text> {
    let painted = crate::testing::painted(shapes).into_iter();
    let texts = painted.filter_map(|shape| match shape {
        egui::Shape::Text(text) => {
            Some((text.galley.text().to_owned(), text.visual_bounding_rect()))
        }
        _ => None,
    });
    texts.collect()
}

/// The buttons of the components' menus that `shapes` paint, in the order they're painted:
/// each one's frame, and the bounds of its dots, the three circles of [`DOT`]'s radius painted
/// just after it.
fn find_menus(shapes: &[egui::epaint::ClippedShape]) -> Vec<(egui::Rect, egui::Rect)> {
    let mut frame = egui::Rect::NOTHING;
    let mut dots = Vec::new();
    for clipped in shapes {
        match &clipped.shape {
            egui::Shape::Rect(shape) => frame = shape.rect,
            egui::Shape::Circle(dot) if dot.radius == DOT => {
                let size = egui::Vec2::splat(2.0 * DOT);
                dots.push((frame, egui::Rect::from_center_size(dot.center, size)));
            }
            _ => {}
        }
    }
    let (menus, rest) = dots.as_chunks::<3>();
    assert!(rest.is_empty(), "three dots a menu: {dots:?}");
    menus
        .iter()
        .map(|[first, _, last]| (first.0, first.1.union(last.1)))
        .collect()
}

/// The menu on the row of `header`, a component's name: its button, and its dots.
fn menu_on(menus: &[(egui::Rect, egui::Rect)], header: egui::Rect) -> (egui::Rect, egui::Rect) {
    let on_row = menus
        .iter()
        .find(|(button, _)| button.y_range().contains(header.center().y));
    *on_row.unwrap_or_else(|| panic!("no menu on {header:?}'s row: {menus:?}"))
}

/// A frame as [`frame_ticking`] draws it; returns its shapes, and how many edits it made.
fn frame_shapes(
    inspector: &mut Inspector,
    world: &mut World,
    ctx: &egui::Context,
    time: f64,
    events: Vec<egui::Event>,
    tick: bool,
) -> (Vec<egui::epaint::ClippedShape>, usize) {
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 4000.0));
    let input = egui::RawInput {
        screen_rect: Some(screen),
        time: Some(time),
        events,
        ..egui::RawInput::default()
    };
    let mut made = 0;
    let mut output = ctx.run_ui(input, |ui| {
        let mut pass = Pass::default();
        panels(inspector, world, ui, &mut pass);
        made = pass.sets.len();
        if tick {
            make(inspector, world, pass);
        } else {
            inspector.queue(pass);
        }
    });
    output.textures_delta.clear();
    (output.shapes, made)
}

/// Three frames, the last's text: a grid lays itself out unseen in its first.
fn frames(inspector: &mut Inspector, world: &mut World, ctx: &egui::Context) -> Vec<String> {
    let mut texts = Vec::new();
    for time in 0..3 {
        let (shown, edits) = frame(inspector, world, ctx, f64::from(time), Vec::new());
        assert_eq!(edits, 0, "drawing edits nothing");
        texts = shown;
    }
    texts.into_iter().map(|(text, _)| text).collect()
}

#[test]
fn drawing_the_panels_changes_nothing() {
    let mut world = World::default();
    let turned = Transform {
        rotation: Quat::from_rotation_z(1.0),
        ..Transform::IDENTITY
    };
    let turned = world.spawn(turned);
    let lost = Transform {
        rotation: Quat::from_xyzw(f32::NAN, 0.0, 0.0, 1.0),
        ..Transform::IDENTITY
    };
    let lost = world.spawn(lost);
    // Transparent, so that its color is lost if alpha is multiplied in.
    let sprite = Sprite {
        color: Vec4::new(0.3, 0.6, 0.9, 0.0),
        ..Sprite::default()
    };
    let tinted = world.spawn((Transform::IDENTITY, sprite));
    let odd = Everything {
        count: 200,
        angle: f32::NAN,
        offset: Vec3::new(f32::INFINITY, 0.0, 0.0),
        maybe: Some(Inner {
            level: 0,
            name: "kept".to_owned(),
        }),
        form: Form::Box {
            size: Vec2::ONE,
            turn: 0.5,
        },
        list: vec![1, 2, 3],
        ..everything()
    };
    let odd = world.spawn(odd);
    let [first, second] = [0, 1].map(|_| world.spawn(Transform::IDENTITY));
    for child in [first, second] {
        world.set_parent(child, Some(turned)).expect("no cycle");
    }
    let before = snapshot(&world);
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let texts = frames(&mut inspector, &mut world, &ctx);
    // The rows of entities without names.
    for text in [
        "6 entities",
        "Entity",
        "Nothing picked: pick an entity in the Entities panel",
    ] {
        assert!(texts.iter().any(|shown| shown == text), "{text}: {texts:?}");
    }
    let shown: [(Entity, &[&str]); 4] = [
        (turned, &["Transform", "translation", "rotation", "scale"]),
        (lost, &["rotation", "[NaN, 0, 0, 1]"]),
        (
            tinted,
            &["Sprite", "color", "texture", "rect", "[1]", "LinearClamp"],
        ),
        (
            odd,
            &[
                "Everything",
                "count",
                "200",
                "angle",
                "NaN",
                "offset",
                "\u{2007}inf",
                "Box",
                "size",
                "maybe",
                "Some",
                "kept",
                "[2]",
                "level",
                "pair",
                "0",
                // Its integers' negative values, `inner.level`'s and the pair's, after a
                // minus sign as wide as a digit.
                "\u{2212}1",
                "\u{2212}4",
                "handle",
                "opaque",
            ],
        ),
    ];
    for (entity, names) in shown {
        inspector.pick(id(&world, entity));
        let texts = frames(&mut inspector, &mut world, &ctx);
        let header = format!("Entity {}", id(&world, entity));
        for name in names.iter().copied().chain([header.as_str()]) {
            assert!(texts.iter().any(|text| text == name), "{name}: {texts:?}");
        }
        let clash = texts.iter().find(|text| text.contains(" use of "));
        assert_eq!(clash, None, "egui saw an ID twice");
    }
    // A parent lists its children, and a child shows its parent, by their IDs.
    let [one, two] = [first, second].map(|child| id(&world, child));
    let children = format!("Children: {one}, {two}");
    let parent = id(&world, turned).to_string();
    for (entity, names) in [
        (turned, [children.as_str(), "Transform"]),
        (first, ["Parent", parent.as_str()]),
    ] {
        inspector.pick(id(&world, entity));
        let texts = frames(&mut inspector, &mut world, &ctx);
        for name in names {
            assert!(texts.iter().any(|text| text == name), "{name}: {texts:?}");
        }
    }
    assert!(same_snapshots(&snapshot(&world), &before));
}

#[test]
fn a_parent_lists_its_first_16_children() {
    let mut world = World::default();
    let parent = world.spawn(Transform::IDENTITY);
    let parent_id = id(&world, parent);
    assert_eq!(children(&world, parent_id), None);
    let kids: Vec<Entity> = (0..18).map(|_| world.spawn(Transform::IDENTITY)).collect();
    for &kid in &kids {
        world.set_parent(kid, Some(parent)).expect("no cycle");
    }
    let shown = kids[..16].iter().map(|&kid| id(&world, kid).to_string());
    let shown: Vec<String> = shown.collect();
    let shown = format!("Children: {}", shown.join(", "));
    let listed = children(&world, parent_id);
    assert_eq!(listed, Some(format!("{shown} and 2 more")));
    for &kid in &kids[16..] {
        world.set_parent(kid, None).expect("a root");
    }
    assert_eq!(children(&world, parent_id), Some(shown));
}

/// A click of `button` at `at`: the pointer there, pressed and released in one frame, as egui
/// takes a tap.
fn click(button: egui::PointerButton, at: egui::Pos2) -> Vec<egui::Event> {
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    vec![egui::Event::PointerMoved(at), button(true), button(false)]
}

fn enter() -> Vec<egui::Event> {
    vec![egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]
}

/// The pointer pressed at `from`, moved to `to` and released, each in a frame of its own.
fn drag(from: egui::Pos2, to: egui::Pos2) -> [Vec<egui::Event>; 3] {
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    [
        vec![egui::Event::PointerMoved(from), button(from, true)],
        vec![egui::Event::PointerMoved(to)],
        vec![button(to, false)],
    ]
}

/// Where the text `name` is, the first drawn at or after the text `after`.
fn find(texts: &[Text], after: &str, name: &str) -> egui::Rect {
    let start = texts.iter().position(|(text, _)| text == after);
    let start = start.unwrap_or_else(|| panic!("no {after}: {texts:?}"));
    let found = texts[start..].iter().find(|(text, _)| text == name);
    found.unwrap_or_else(|| panic!("no {name}: {texts:?}")).1
}

/// Where the last of `names` is, each the first drawn at or after the one before it, as
/// `["rect", "[1]", "X"]` finds the button of the x of a field's second element.
fn find_after(texts: &[Text], names: &[&str]) -> egui::Rect {
    let mut at = 0;
    for name in names {
        let found = texts[at..].iter().position(|(text, _)| text == name);
        at += found.unwrap_or_else(|| panic!("no {name} of {names:?}: {texts:?}"));
    }
    texts[at].1
}

/// What's drawn just right of `label`, on its row: the widget of its field.
fn next_to(texts: &[Text], label: egui::Rect) -> &Text {
    let row = |rect: &egui::Rect| (rect.center().y - label.center().y).abs() < 4.0;
    let right = texts.iter();
    let right = right.filter(|(_, rect)| row(rect) && rect.min.x > label.max.x);
    let nearest = right.min_by(|a, b| a.1.min.x.total_cmp(&b.1.min.x));
    nearest.expect("a widget beside the label")
}

/// The middle of what's drawn just right of `label`, on its row: the widget of its field.
fn beside(texts: &[Text], label: egui::Rect) -> egui::Pos2 {
    next_to(texts, label).1.center()
}

#[test]
fn the_find_field_picks_an_entity_by_a_pasted_id() {
    let mut world = World::default();
    let [first, second, gone] = [0, 1, 2].map(|_| world.spawn(Transform::IDENTITY));
    let [first, second, gone] = [first, second, gone].map(|entity| id(&world, entity));
    world
        .despawn(world.entity(gone).expect("alive"))
        .expect("alive");
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let mut time = 0.0;
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        let mut texts = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            time += 1.0;
            texts = frame(inspector, world, &ctx, time, events).0;
        }
        texts
    };
    let texts = run(&mut inspector, &mut world, Vec::new());
    let field = beside(&texts, find(&texts, "Find", "Find"));
    let bad = "`5F3A9C0E12B47D68` isn't an entity's ID: an ID is 16 lowercase hex digits, not \
               all zero, such as `5f3a9c0e12b47d68`";
    let missing = format!("no entity has ID {gone}");
    // Quoted, as a scene writes it; refused, keeping the pick; and another.
    for (typed, picked, says) in [
        (format!(" \"{first}\" "), first, None),
        ("5F3A9C0E12B47D68".to_owned(), first, Some(bad)),
        (gone.to_string(), first, Some(missing.as_str())),
        (second.to_string(), second, None),
    ] {
        inspector.find.clear();
        run(&mut inspector, &mut world, click(Primary, field));
        run(
            &mut inspector,
            &mut world,
            vec![egui::Event::Text(typed.clone())],
        );
        let texts = run(&mut inspector, &mut world, enter());
        assert_eq!(inspector.picked, Some(picked), "{typed}");
        let header = format!("Entity {picked}");
        assert!(texts.iter().any(|(text, _)| *text == header), "{texts:?}");
        let shown = |says: &str| texts.iter().any(|(text, _)| text == says);
        match says {
            Some(says) => assert!(shown(says), "{says}: {texts:?}"),
            None => assert!(!shown(bad) && !shown(&missing), "{texts:?}"),
        }
    }
}

fn called(name: &str) -> Name {
    Name(name.to_owned())
}

/// What `entity`'s row in the Entities panel says.
fn row_text(world: &World, entity: Entity) -> String {
    row_label(world, id(world, entity)).text().to_owned()
}

/// Names in a field and in an option, which a tuple struct of one field shows alone, and sets
/// at its own path or whole; and an enum whose variant has a field, which keeps its label.
#[derive(Clone, Component)]
struct Shaped {
    name: Name,
    maybe: Option<Name>,
    form: Form,
}

fn shaped() -> Shaped {
    Shaped {
        name: called("Bob"),
        maybe: Some(called("Cy")),
        form: Form::Circle(2.5),
    }
}

/// An entity's row shows its name alone, and one without a name, or with an empty one,
/// "Entity": neither its ID nor its components, which the Inspector shows, its heading naming
/// it and giving its ID.
#[test]
fn an_entitys_row_shows_its_name_alone_and_the_inspector_its_id() {
    let mut world = World::default();
    let named = world.spawn((called("Ada"), Transform::IDENTITY));
    let plain = world.spawn(Transform::IDENTITY);
    world.spawn((called(" "), Transform::IDENTITY));
    let [named_id, plain_id] = [named, plain].map(|entity| id(&world, entity));
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let texts = frames(&mut inspector, &mut world, &ctx);
    assert!(texts.iter().any(|text| text == "Ada"), "{texts:?}");
    let unnamed = texts.iter().filter(|text| *text == "Entity").count();
    assert_eq!(unnamed, 2, "{texts:?}");
    let ids = [named_id, plain_id].map(|id| id.to_string());
    let detailed = |text: &String| ids.iter().any(|id| text.contains(id.as_str()));
    assert!(!texts.iter().any(detailed), "{texts:?}");
    assert!(
        !texts.iter().any(|text| text.contains("Transform")),
        "{texts:?}"
    );
    for (picked, heading) in [
        (named_id, format!("Ada, entity {named_id}")),
        (plain_id, format!("Entity {plain_id}")),
    ] {
        inspector.pick(picked);
        let texts = frames(&mut inspector, &mut world, &ctx);
        assert!(texts.contains(&heading), "{heading}: {texts:?}");
    }
    // Despawned, a pick has no name left to show.
    inspector.pick(named_id);
    world.despawn(named).expect("alive");
    let texts = frames(&mut inspector, &mut world, &ctx);
    let despawned = format!("Entity {named_id} was despawned");
    assert!(texts.contains(&despawned), "{texts:?}");
}

#[test]
fn a_tuple_of_one_field_shows_its_field_without_a_label() {
    let mut world = World::default();
    let named = world.spawn(called("Ada"));
    let shape = world.spawn(shaped());
    let [parent, child] = [(); 2].map(|()| world.spawn(()));
    world.set_parent(child, Some(parent)).expect("no cycle");
    let parent = id(&world, parent).to_string();
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    // A name or a parent shows as its field alone, and so does a name in a field or an option;
    // the field of an enum's variant keeps its label, `0`.
    let picks: [(Entity, &[&str], usize); 3] = [
        (named, &["Name", "Ada"], 0),
        (child, &["Parent", parent.as_str()], 0),
        (shape, &["name", "Bob", "maybe", "Some", "Cy", "Circle"], 1),
    ];
    for (entity, names, zeros) in picks {
        inspector.pick(id(&world, entity));
        let texts = frames(&mut inspector, &mut world, &ctx);
        for name in names {
            assert!(texts.iter().any(|text| text == name), "{name}: {texts:?}");
        }
        let labels = texts.iter().filter(|text| *text == "0").count();
        assert_eq!(labels, zeros, "{texts:?}");
    }
}

/// A name typed into is set at its field's path, `0` as a component and `name.0` in a field,
/// and whole in an option.
#[test]
fn a_tuple_of_one_field_is_set_at_its_own_path_or_whole() {
    let mut world = World::default();
    let named = world.spawn(called("Ada"));
    let shape = world.spawn(shaped());
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let mut time = 0.0;
    // A frame given `events`, then two that settle what they did; the last one's texts.
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        let mut texts = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            time += 1.0;
            texts = frame(inspector, world, &ctx, time, events).0;
        }
        texts
    };
    // An x typed into each name as a person would: clicked left of its text, which puts the
    // cursor first, and typed.
    let mut texts = Vec::new();
    for (entity, after, name) in [
        (named, "Name", "Ada"),
        (shape, "Shaped", "Bob"),
        (shape, "maybe", "Cy"),
    ] {
        inspector.pick(id(&world, entity));
        let shown = run(&mut inspector, &mut world, Vec::new());
        let rect = find(&shown, after, name);
        run(
            &mut inspector,
            &mut world,
            click(Primary, egui::pos2(rect.min.x - 3.0, rect.center().y)),
        );
        let typed = vec![egui::Event::Text("x".to_owned())];
        texts = run(&mut inspector, &mut world, typed);
    }
    assert!(inspector.refusals.is_empty(), "{texts:?}");
    assert_eq!(world.get::<Name>(named), Some(&called("xAda")));
    let held = world.get::<Shaped>(shape).expect("shaped");
    assert_eq!(held.name, called("xBob"));
    assert_eq!(held.maybe, Some(called("xCy")));
    // The Entities panel, the screen's left half, shows the name as it is now.
    let row = |(text, rect): &Text| text == "xAda" && rect.max.x < 800.0;
    assert!(texts.iter().any(row), "{texts:?}");
}

/// A value refused for the field of a tuple struct shows under it, not in the row of the grid
/// it's in.
#[test]
fn a_value_refused_for_a_tuple_of_one_field_shows_under_it() {
    let mut world = World::default();
    let entity = world.spawn(shaped());
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let refused = Edit {
        component: "Shaped".to_owned(),
        path: "name.0".to_owned(),
        value: Value::UInt(1),
    };
    inspector.apply(&mut world, inspector.picked, refused);
    let says = format!(
        "can't set `Shaped.name.0` of entity {}: expected String, got UInt",
        id(&world, entity)
    );
    let mut time = 0.0;
    let mut texts = Vec::new();
    for _ in 0..3 {
        time += 1.0;
        texts = frame(&mut inspector, &mut world, &ctx, time, Vec::new()).0;
    }
    let (name, message) = (find(&texts, "name", "Bob"), find(&texts, "Bob", &says));
    assert!(message.min.y > name.max.y, "{texts:?}");
}

#[test]
fn widgets_set_the_fields_they_show() {
    let mut world = World::default();
    let odd = Everything {
        maybe: Some(Inner {
            level: 0,
            name: "kept".to_owned(),
        }),
        form: Form::Box {
            size: Vec2::ONE,
            turn: 0.5,
        },
        // Infinite, as code may leave it, which a number typed sets back.
        offset: Vec3::new(f32::INFINITY, -0.0, 3.0),
        ..everything()
    };
    let entity = world.spawn(odd.clone());
    // Transparent, so that its color is lost if alpha is multiplied in; one whose bits egui's
    // popup moves as it passes it through gamma.
    let tint = Vec4::new(0.3, 0.6, 0.9, 0.0);
    let sprite = Sprite {
        color: tint,
        ..Sprite::default()
    };
    let sprite = world.spawn(sprite);
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let mut time = 0.0;
    // A frame given `events`, then two that settle what they did; the last one's texts.
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        let mut texts = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            time += 1.0;
            texts = frame(inspector, world, &ctx, time, events).0;
        }
        texts
    };
    inspector.pick(id(&world, entity));
    let mut texts = run(&mut inspector, &mut world, Vec::new());
    // Each number typed in as a person would: clicked, which selects its text, typed over and
    // entered; a vector's beside the buttons of their axes.
    let typed: [(&[&str], &str); 5] = [
        (&["maybe", "level"], "1"),
        (&["rect", "[1]", "X"], "0.75"),
        (&["form", "turn"], "90"),
        (&["angle"], "999"),
        (&["offset", "X"], "1e39"),
    ];
    for (anchor, typed) in typed {
        let at = beside(&texts, find_after(&texts, anchor));
        run(&mut inspector, &mut world, click(Primary, at));
        let text = vec![egui::Event::Text(typed.to_owned())];
        run(&mut inspector, &mut world, text);
        texts = run(&mut inspector, &mut world, enter());
    }
    assert!(inspector.refusals.is_empty(), "{texts:?}");
    let all = world.get::<Everything>(entity).expect("everything");
    let Form::Box { turn, .. } = all.form else {
        panic!("still a box: {:?}", all.form);
    };
    assert!((turn - std::f32::consts::FRAC_PI_2).abs() < 1e-6, "{turn}");
    let expected = Everything {
        maybe: Some(Inner {
            level: 1,
            name: "kept".to_owned(),
        }),
        rect: [Vec2::ZERO, Vec2::new(0.75, 1.0)],
        form: Form::Box {
            size: Vec2::ONE,
            turn,
        },
        // Typed past its max, it stops there, at the f32 the field takes.
        angle: 3.2,
        // Past what an f32 holds, it stops at the most one does, rather than become infinite.
        offset: Vec3::new(f32::MAX, -0.0, 3.0),
        ..odd
    };
    assert_eq!(all, &expected);

    inspector.pick(id(&world, sprite));
    let texts = run(&mut inspector, &mut world, Vec::new());
    // Its color's button, in the column the texture's "None" starts: its popup opens.
    let at = egui::pos2(
        find(&texts, "texture", "None").min.x + 10.0,
        find(&texts, "Sprite", "color").center().y,
    );
    let opened = run(&mut inspector, &mut world, click(Primary, at));
    // Open a while, as egui passes it through gamma each frame.
    for _ in 0..3 {
        run(&mut inspector, &mut world, Vec::new());
    }
    let bits = |color: Vec4| color.to_array().map(f32::to_bits);
    let set = world.get::<Sprite>(sprite).expect("a sprite").color;
    assert_eq!(bits(set), bits(tint));
    // Closed by a click away from it, the sampler's choice is opened and picked from.
    let away = click(Primary, egui::pos2(1500.0, 3900.0));
    run(&mut inspector, &mut world, away);
    let at = beside(&texts, find(&texts, "Sprite", "sampler"));
    let texts = run(&mut inspector, &mut world, click(Primary, at));
    let at = find(&texts, "sampler", "NearestRepeat").center();
    run(&mut inspector, &mut world, click(Primary, at));
    let set = world.get::<Sprite>(sprite).expect("a sprite");
    assert_eq!(set.sampler, crate::Sampler::NearestRepeat);
    assert_eq!(bits(set.color), bits(tint));
    let copy = opened.iter().any(|(text, _)| text == "📋");
    assert!(copy, "the color's popup opened: {opened:?}");
}

#[test]
fn a_gray_keeps_the_hue_picked_for_it() {
    // White, as a sprite is untinted, and a gray whose value gamma's round trip moves; with a
    // tick each frame, and with frames that draw the sets waiting for one, as at frame rates
    // above the simulation's.
    for gray in [Vec4::ONE, Vec4::new(0.9, 0.9, 0.9, 1.0)] {
        for every_frame in [true, false] {
            a_gray_keeps_its_hue(gray, every_frame);
        }
    }
}

fn a_gray_keeps_its_hue(gray: Vec4, every_frame: bool) {
    let mut world = World::default();
    let sprite = Sprite {
        color: gray,
        ..Sprite::default()
    };
    let sprite = world.spawn(sprite);
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let mut time = 0.0;
    // A frame given `events`, then two that settle what they did, the last with a tick, if not
    // each; the last one's texts.
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        let mut texts = Vec::new();
        for (i, events) in [events, Vec::new(), Vec::new()].into_iter().enumerate() {
            time += 1.0;
            let tick = every_frame || i == 2;
            texts = frame_ticking(inspector, world, &ctx, time, events, tick).0;
        }
        texts
    };
    inspector.pick(id(&world, sprite));
    let texts = run(&mut inspector, &mut world, Vec::new());
    let at = egui::pos2(
        find(&texts, "texture", "None").min.x + 10.0,
        find(&texts, "Sprite", "color").center().y,
    );
    let opened = run(&mut inspector, &mut world, click(Primary, at));
    // The popup's hue slider, found by its tooltip below its first row, about halfway along.
    let copy = find(&opened, "📋", "📋");
    let x = copy.center().x + 70.0;
    let below = (0..100).map(|step| copy.max.y + 4.0 * step as f32);
    let hue = below.into_iter().find(|&y| {
        let moved = vec![egui::Event::PointerMoved(egui::pos2(x, y))];
        let texts = run(&mut inspector, &mut world, moved);
        texts.iter().any(|(text, _)| text == "Hue")
    });
    let hue = egui::pos2(x, hue.expect("the hue's slider") + 4.0);
    // Clicked, so that it's picked once: a drag would pick it again each frame it's held.
    run(&mut inspector, &mut world, click(Primary, hue));
    // A hue alone leaves it gray, though it may be one whose last bits gamma moves.
    let color = world.get::<Sprite>(sprite).expect("a sprite").color;
    let still = color.x == color.y && color.y == color.z;
    assert!(
        still && (color - gray).abs().max_element() < 1e-6,
        "{color}"
    );
    // Saturated from the square above the slider, it takes the hue picked, not red.
    let square = egui::pos2(x, hue.y - 250.0);
    for events in drag(square, square + egui::vec2(5.0, 0.0)) {
        run(&mut inspector, &mut world, events);
    }
    let [r, g, b, a] = world.get::<Sprite>(sprite).expect("a sprite").color.into();
    let picked = Hsva::from_rgba_unmultiplied(r, g, b, a);
    assert!(
        picked.s > 0.3 && (0.2..0.8).contains(&picked.h),
        "{gray}: {picked:?}"
    );
}

#[test]
fn angles_typed_stay_while_a_system_renormalizes_the_rotation() {
    let typed = Quat::from_rotation_z(math::to_radians(200.0));
    let mut world = World::default();
    let turned = Transform {
        rotation: typed,
        ..Transform::IDENTITY
    };
    let entity = world.spawn(turned);
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    // As the widget keeps them once 200° is typed: from the rotation alone, it shows -160°.
    let id = egui::Id::unique(("inspector rotation", "Transform", "rotation"));
    ctx.data_mut(|data| data.insert_temp(id, (typed, [0.0f32, 0.0, 200.0])));
    // A bit off in each component, as renormalizing it at each tick leaves it.
    let nudged = typed.to_array().map(|x| f32::from_bits(x.to_bits() + 1));
    world
        .get_mut::<Transform>(entity)
        .expect("a transform")
        .rotation = Quat::from_array(nudged).normalize();
    // egui draws the number apart from its suffix, after a figure space in place of a sign.
    let texts = frames(&mut inspector, &mut world, &ctx);
    assert!(
        texts.iter().any(|text| text == "\u{2007}200.0"),
        "{texts:?}"
    );
    // Turned by something else, it shows the new rotation's own angles.
    world
        .get_mut::<Transform>(entity)
        .expect("a transform")
        .rotation = Quat::from_rotation_z(math::to_radians(10.0));
    let texts = frames(&mut inspector, &mut world, &ctx);
    assert!(texts.iter().any(|text| text == "\u{2007}10.0"), "{texts:?}");
    // 200° typed again, and set at a tick that hasn't come yet: the frames before it draw the
    // rotation as the set will make it, and so keep the angles typed.
    ctx.data_mut(|data| data.insert_temp(id, (typed, [0.0f32, 0.0, 200.0])));
    let set = Edit {
        component: "Transform".to_owned(),
        path: "rotation".to_owned(),
        value: Value::Quat(typed),
    };
    inspector.queue(Pass {
        sets: vec![set],
        ..Pass::default()
    });
    frame_ticking(&mut inspector, &mut world, &ctx, 3.0, Vec::new(), false);
    Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
    let texts = frames(&mut inspector, &mut world, &ctx);
    assert!(
        texts.iter().any(|text| text == "\u{2007}200.0"),
        "{texts:?}"
    );
}

#[test]
fn an_edit_applies_at_the_next_tick_to_the_entity_picked_as_it_was_made() {
    let mut world = World::default();
    let [a, b] = [(); 2].map(|()| world.spawn(everything()));
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, a));
    let count = |value| Edit {
        component: "Everything".to_owned(),
        path: "count".to_owned(),
        value,
    };
    // A set, and then one refused, before another entity is picked.
    for value in [7, 200] {
        inspector.queue(Pass {
            sets: vec![count(Value::UInt(value))],
            ..Pass::default()
        });
    }
    inspector.pick(id(&world, b));
    let counts = |world: &World| {
        [a, b].map(|entity| world.get::<Everything>(entity).expect("everything").count)
    };
    assert_eq!(counts(&world), [42, 42]);
    Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
    assert_eq!(counts(&world), [7, 42]);
    assert_eq!(inspector.log.edits().len(), 1);
    // The refusal, of a field of the entity picked before, isn't shown beside the same field
    // of this one.
    assert!(inspector.refusals.is_empty());
}

#[test]
fn a_value_refused_shows_beside_its_field_until_one_is_set() {
    let mut world = World::default();
    let entity = world.spawn(everything());
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let count = |value| Edit {
        component: "Everything".to_owned(),
        path: "count".to_owned(),
        value,
    };
    inspector.apply(&mut world, inspector.picked, count(Value::UInt(200)));
    let refused = format!(
        "can't set `Everything.count` of entity {}: 200 is above the field's max, 100",
        id(&world, entity)
    );
    let texts = frames(&mut inspector, &mut world, &ctx);
    assert!(texts.contains(&refused), "{texts:?}");
    assert_eq!(
        world.get::<Everything>(entity).expect("everything").count,
        42
    );
    inspector.apply(&mut world, inspector.picked, count(Value::UInt(99)));
    assert_eq!(
        world.get::<Everything>(entity).expect("everything").count,
        99
    );
    let texts = frames(&mut inspector, &mut world, &ctx);
    assert!(!texts.contains(&refused), "{texts:?}");
    // Another entity picked starts without the last one's.
    inspector.apply(&mut world, inspector.picked, count(Value::UInt(200)));
    let other = world.spawn(everything());
    inspector.pick(id(&world, other));
    assert!(inspector.refusals.is_empty());
}

/// A pick despawned is named by the ID it was shown by, and an entity field holding a handle
/// whose entity is gone shows the handle, not the ID of the entity now in its slot.
#[test]
fn the_despawned_are_named_by_what_they_were_shown_by() {
    let mut world = World::default();
    let gone = world.spawn(Transform::IDENTITY);
    world.despawn(gone).expect("alive");
    // In the slot `gone` left, the last freed being taken first.
    let occupant = world.spawn(Transform::IDENTITY);
    let holder = world.spawn(Everything {
        target: Some(gone),
        ..everything()
    });
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, holder));
    let texts = frames(&mut inspector, &mut world, &ctx);
    assert!(texts.contains(&gone.to_string()), "{texts:?}");
    let taken = id(&world, occupant).to_string();
    assert!(!texts.contains(&taken), "{texts:?}");
    let picked = id(&world, holder);
    world.despawn(holder).expect("alive");
    let texts = frames(&mut inspector, &mut world, &ctx);
    let despawned = format!("Entity {picked} was despawned");
    assert!(texts.contains(&despawned), "{texts:?}");
}

/// `key` pressed with Ctrl, or Cmd on macOS, and with Shift too if `shift`.
fn shortcut(key: egui::Key, shift: bool) -> Vec<egui::Event> {
    let modifiers = match shift {
        true => Modifiers::COMMAND.plus(Modifiers::SHIFT),
        false => Modifiers::COMMAND,
    };
    vec![egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }]
}

/// The pointer pressed at `at`, moved right three times and released, each in a frame of its
/// own, so that a drag sets its field in several frames.
fn slide(at: egui::Pos2) -> Vec<Vec<egui::Event>> {
    let button = |pressed, pos| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    let to = |step: f32| at + egui::vec2(10.0 * step, 0.0);
    let mut gesture = vec![vec![egui::Event::PointerMoved(at), button(true, at)]];
    gesture.extend([1.0, 2.0, 3.0].map(|step| vec![egui::Event::PointerMoved(to(step))]));
    gesture.push(vec![button(false, to(3.0))]);
    gesture
}

#[test]
fn a_drag_is_one_edit_which_the_keys_undo_and_redo() {
    let mut world = World::default();
    let entity = world.spawn(everything());
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let mut time = 0.0;
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        time += 1.0;
        frame(inspector, world, &ctx, time, events)
    };
    // Shown above the fields until the drag's first set, so that they move as it goes on.
    inspector.not_done = Some("can't undo it".to_owned());
    let mut texts = Vec::new();
    for _ in 0..3 {
        texts = run(&mut inspector, &mut world, Vec::new()).0;
    }
    let at = beside(&texts, find(&texts, "Everything", "weight"));
    let weight = |world: &World| world.get::<Everything>(entity).expect("everything").weight;
    let mut sets = 0;
    for events in slide(at) {
        sets += run(&mut inspector, &mut world, events).1;
    }
    let dragged = weight(&world);
    assert!(sets > 1 && dragged != 2.5, "{sets} sets, to {dragged}");
    assert_eq!(inspector.log.edits().len(), 1, "one edit");
    for (keys, expected) in [
        (shortcut(egui::Key::Z, false), 2.5),
        (shortcut(egui::Key::Z, true), dragged),
        (shortcut(egui::Key::Z, false), 2.5),
        (shortcut(egui::Key::Y, false), dragged),
    ] {
        run(&mut inspector, &mut world, keys);
        assert_eq!(weight(&world).to_bits(), expected.to_bits());
    }
}

#[test]
fn frames_that_change_nothing_queue_nothing() {
    let mut world = World::default();
    let entity = world.spawn(everything());
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let mut time = 0.0;
    let mut run = |inspector: &mut Inspector, world: &mut World, events, tick| {
        time += 1.0;
        frame_ticking(inspector, world, &ctx, time, events, tick)
    };
    let mut texts = Vec::new();
    for _ in 0..3 {
        texts = run(&mut inspector, &mut world, Vec::new(), true).0;
    }
    assert!(inspector.queued.is_empty(), "drawing queues nothing");
    let at = beside(&texts, find(&texts, "Everything", "weight"));
    let weight = |world: &World| world.get::<Everything>(entity).expect("everything").weight;
    for _ in 0..2 {
        for events in slide(at) {
            run(&mut inspector, &mut world, events, true);
        }
        // Frames between the drags, which wait for a tick, and end no edit.
        for _ in 0..10 {
            run(&mut inspector, &mut world, Vec::new(), false);
        }
        assert!(inspector.queued.is_empty(), "idle frames queue nothing");
    }
    assert_eq!(inspector.log.edits().len(), 2, "two edits");
    for _ in 0..2 {
        run(
            &mut inspector,
            &mut world,
            shortcut(egui::Key::Z, false),
            true,
        );
    }
    assert_eq!(weight(&world).to_bits(), 2.5f64.to_bits());
}

#[test]
fn its_commands_are_values_that_queue_them_again() {
    let mut world = World::default();
    let entity = world.spawn(everything());
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let set = |path: &str, value| Edit {
        component: "Everything".to_owned(),
        path: path.to_owned(),
        value,
    };
    let target = Value::Option(Some(Box::new(Value::Entity(entity))));
    let sets = vec![set("count", Value::UInt(7)), set("target", target)];
    let pressed = || Cues {
        pressed: true,
        held: true,
        ..Cues::default()
    };
    inspector.queue(Pass {
        sets,
        cues: pressed(),
        ..Pass::default()
    });
    for step in [Step::Undo, Step::Redo] {
        inspector.queue(Pass {
            chosen: Some(step),
            ..Pass::default()
        });
    }
    // A set, and then operations, the first of which ends its edit, so that no seal does.
    inspector.queue(Pass {
        sets: vec![set("count", Value::UInt(8))],
        cues: pressed(),
        ..Pass::default()
    });
    let picked = id(&world, entity);
    let operations = vec![
        empty(None),
        empty(Some(picked)),
        Command::Duplicate(picked),
        Command::Delete(picked),
        Command::Add(picked, "Sprite".to_owned(), Sprite::default().to_value()),
        Command::Remove(picked, "Everything".to_owned()),
        Command::Revert(picked, "Everything".to_owned(), "count".to_owned()),
        Command::Unlink(picked),
    ];
    inspector.queue(Pass {
        operations,
        ..Pass::default()
    });
    let values = |queue: &Queue| (0..queue.len()).map(|i| queue.value(i)).collect::<Vec<_>>();
    let queued = values(&inspector.queued);
    let names: Vec<&str> = queued
        .iter()
        .map(|value| reflect::variant(value).expect("a variant").0)
        .collect();
    // The press began no edit, since none went on, and the release ended the one it began.
    assert_eq!(
        names,
        [
            "Set",
            "Set",
            "Seal",
            "Undo",
            "Redo",
            "Set",
            "Create",
            "Create",
            "Duplicate",
            "Delete",
            "Add",
            "Remove",
            "Revert",
            "Unlink"
        ]
    );
    let mut again = Queue::default();
    for value in &queued {
        again
            .push(value)
            .unwrap_or_else(|error| panic!("{value:?}: {error}"));
    }
    assert_eq!(values(&again), queued);
    let named = |name: &str, value| (Ident::Owned(name.to_owned()), value);
    let fields = |entity: &str| {
        vec![
            named("entity", Value::String(entity.to_owned())),
            named("component", Value::String("Everything".to_owned())),
            named("path", Value::String("count".to_owned())),
            named("value", Value::UInt(1)),
        ]
    };
    for (value, code) in [
        (
            Value::Variant("Set".into(), fields("5F3A9C0E12B47D68")),
            "ecs.bad_id",
        ),
        (
            Value::Variant("Set".into(), fields("")[1..].to_vec()),
            "reflect.missing_field",
        ),
        (
            Value::Variant("Seal".into(), fields("")),
            "reflect.no_field",
        ),
        (
            Value::Variant("Cut".into(), Vec::new()),
            "reflect.no_variant",
        ),
        (Value::UInt(1), "reflect.wrong_kind"),
    ] {
        let error = again.push(&value).expect_err("refused");
        assert_eq!(error.code(), code, "{value:?}: {error}");
    }
    let error = again.push(&Value::Variant("Cut".into(), Vec::new()));
    let error = error.expect_err("no such command").to_string();
    assert!(
        error.contains("`Create`, `Delete`, `Duplicate`, `Add`, `Remove`, `Revert` and `Unlink`"),
        "{error}"
    );
    // The operations' values, refused where they're wrong.
    let text = |text: &str| Value::String(text.to_owned());
    let create = |parent, components| {
        let fields = vec![named("parent", parent), named("components", components)];
        Value::Variant("Create".into(), fields)
    };
    let bad_id = Value::Option(Some(Box::new(text("5F3A9C0E12B47D68"))));
    let entity = picked.to_string();
    let add = vec![
        named("entity", text(&entity)),
        named("component", Value::UInt(1)),
        named("value", Value::Struct(Vec::new())),
    ];
    for (value, code, at) in [
        (
            create(text(&entity), Value::Struct(Vec::new())),
            "reflect.wrong_kind",
            "parent",
        ),
        (
            create(bad_id, Value::Struct(Vec::new())),
            "ecs.bad_id",
            "parent",
        ),
        (
            create(Value::Option(None), Value::List(Vec::new())),
            "reflect.wrong_kind",
            "components",
        ),
        (
            Value::Variant("Delete".into(), vec![named("entity", Value::UInt(1))]),
            "reflect.wrong_kind",
            "entity",
        ),
        (
            Value::Variant("Add".into(), add),
            "reflect.wrong_kind",
            "component",
        ),
        (
            Value::Variant("Remove".into(), fields(&entity)[..1].to_vec()),
            "reflect.missing_field",
            "",
        ),
    ] {
        let error = again.push(&value).expect_err("refused");
        assert_eq!(error.code(), code, "{value:?}: {error}");
        let says = format!("at `{at}`: ");
        assert_eq!(
            error.to_string().starts_with(&says),
            !at.is_empty(),
            "{error}"
        );
    }
    assert_eq!(again.len(), queued.len());
}

/// Adds and removes, queued as a recording replays them, are made at the next tick, through
/// the log, whose undo takes them back; a remove forgets what its component refused, as an add
/// does, and either clears a refusal shown above the components, as a set's of a component the
/// entity hasn't is. One refused shows above the components.
#[test]
fn components_are_added_and_removed_at_the_next_tick() {
    let mut world = World::default();
    let entity = world.spawn((Transform::IDENTITY, everything()));
    world.register::<Sprite>().expect("registered");
    let picked = id(&world, entity);
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(picked);
    let refused = |component: &str, path: &str, value| Edit {
        component: component.to_owned(),
        path: path.to_owned(),
        value,
    };
    // Above its bounds, and of a component it hasn't yet.
    let count = refused("Everything", "count", Value::UInt(200));
    inspector.apply(&mut world, Some(picked), count);
    let tint = refused("Sprite", "color", Value::F32(1.0));
    inspector.apply(&mut world, Some(picked), tint);
    assert_eq!(inspector.refusals.len(), 1);
    let says = inspector.not_done.clone().expect("refused");
    assert!(says.contains("has no `Sprite`"), "{says}");
    let named = |name: &str, value| (Ident::Owned(name.to_owned()), value);
    let text = |text: &str| Value::String(text.to_owned());
    let operation = |name: &str, component: &str, value: Option<Value>| {
        let mut fields = vec![
            named("entity", text(&picked.to_string())),
            named("component", text(component)),
        ];
        fields.extend(value.map(|value| named("value", value)));
        Value::Variant(name.to_owned().into(), fields)
    };
    let sprite = Sprite {
        color: Vec4::new(0.5, 0.25, 1.0, 1.0),
        ..Sprite::default()
    };
    let tick = |inspector: &mut Inspector, world: &mut World, values: &[Value]| {
        for value in values {
            let pushed = Commands::push(&mut inspector.queued, value);
            pushed.unwrap_or_else(|error| panic!("{value:?}: {error}"));
        }
        Layer::tick(inspector, world, Tick { number: 0 });
    };
    let added = operation("Add", "Sprite", Some(sprite.to_value()));
    let removed = operation("Remove", "Everything", None);
    tick(&mut inspector, &mut world, &[added, removed]);
    let names: Vec<&str> = world.components_of(entity).expect("alive").collect();
    assert_eq!(names, ["Transform", "Sprite"]);
    let color = world.get::<Sprite>(entity).expect("a sprite").color;
    assert_eq!(color, sprite.color);
    assert!(inspector.refusals.is_empty() && inspector.not_done.is_none());
    assert_eq!(inspector.log.edits().len(), 2);
    // Refused, since it has one, and said so above its components.
    let again = operation("Add", "Transform", Some(Transform::IDENTITY.to_value()));
    tick(&mut inspector, &mut world, &[again]);
    let says = inspector.not_done.clone().expect("refused");
    let starts = format!("can't add `Transform` to {picked}: ");
    assert!(says.starts_with(&starts), "{says}");
    let mut texts = Vec::new();
    for time in 0..3 {
        texts = frame(
            &mut inspector,
            &mut world,
            &ctx,
            f64::from(time),
            Vec::new(),
        )
        .0;
    }
    let heading = format!("Entity {picked}");
    assert!(find(&texts, &says, &says).max.y < find(&texts, &says, &heading).min.y);
    // Undone, each comes back as it was, and the refusal goes.
    for _ in 0..2 {
        inspector.queue(Pass {
            chosen: Some(Step::Undo),
            ..Pass::default()
        });
        Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
    }
    assert_eq!(inspector.not_done, None);
    assert_eq!(world.get::<Everything>(entity), Some(&everything()));
    assert!(world.get::<Sprite>(entity).is_none());
}

/// An entity an instance's prefab gives says which instance it's of, and marks the fields that
/// differ from its prefab, whose context menu reverts one at the next tick, as an edit the Edit
/// menu undoes; and an instance's root says its prefab, beside Unlink, which unlinks it, as one
/// edit, which an undo links again.
#[test]
fn an_instances_overrides_are_marked_and_reverted_and_its_root_unlinked() {
    let mut world = World::default();
    world.register::<Name>().expect("registered");
    world.register::<Transform>().expect("registered");
    let text = "(\n    entities: {\n        \"00000000000000a1\": (\n            Name: \
                (\"lamp\"),\n        ),\n        \"00000000000000b2\": (\n            Name: \
                (\"bulb\"),\n            Parent: (\"00000000000000a1\"),\n            Transform: \
                (\n                translation: (0.0, 1.0, 0.0),\n                rotation: (0.0, \
                0.0, 0.0, 1.0),\n                scale: (1.0, 1.0, 1.0),\n            ),\n        \
                ),\n    },\n)\n";
    let lamp = world.assets_mut().add_prefab(
        "lamp.prefab".into(),
        "lamp.prefab".into(),
        Some(text.into()),
    );
    let root = lamp
        .spawn(&mut world, Transform::IDENTITY)
        .expect("spawned");
    let bulb = world
        .children(root)
        .expect("alive")
        .next()
        .expect("its bulb");
    let up = Value::Vec3(Vec3::new(0.0, 5.0, 0.0));
    world
        .set_field(bulb, "Transform", "translation", &up)
        .expect("set");
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, bulb));
    let mut time = 0.0;
    let mut painted = |inspector: &mut Inspector, world: &mut World, events| {
        time += 1.0;
        frame_shapes(inspector, world, &ctx, time, events, true).0
    };
    let mut shapes = Vec::new();
    for _ in 0..3 {
        shapes = painted(&mut inspector, &mut world, Vec::new());
    }
    let mut shown = |inspector: &mut Inspector, world: &mut World, events| {
        texts_in(&painted(inspector, world, events))
    };
    let texts = texts_in(&shapes);
    let root_id = id(&world, root);
    let of = format!("Of the instance of lamp.prefab whose root is lamp, entity {root_id}");
    // The bars of the selection's color just left of the field and the component that differ.
    let selection = ctx.style_of(ctx.theme()).visuals.selection.bg_fill;
    let bars = crate::testing::painted(&shapes).into_iter();
    let bars: Vec<egui::Rect> = bars
        .filter_map(|shape| match shape {
            egui::Shape::Rect(bar) if bar.fill == selection && bar.rect.width() < 2.5 => {
                Some(bar.rect)
            }
            _ => None,
        })
        .collect();
    let marked = |label: egui::Rect| {
        let left = |bar: &egui::Rect| bar.max.x <= label.min.x + 0.5;
        let beside = |bar: &egui::Rect| bar.y_range().contains(label.center().y);
        bars.iter().any(|bar| left(bar) && beside(bar))
    };
    assert!(marked(find(&texts, &of, "translation")), "{bars:?}");
    assert!(
        marked(find(&texts, &of, "Transform")),
        "its header: {bars:?}"
    );
    assert!(
        !marked(find(&texts, &of, "rotation")),
        "as the prefab has it"
    );
    assert!(!marked(find(&texts, &of, "Name")), "as the prefab has it");
    let label = find(&texts, &of, "translation");
    shown(&mut inspector, &mut world, click(Secondary, label.center()));
    let texts = shown(&mut inspector, &mut world, Vec::new());
    let revert = find(&texts, &of, "Revert to Prefab");
    shown(&mut inspector, &mut world, click(Primary, revert.center()));
    let place = |world: &World| world.get::<Transform>(bulb).expect("alive").translation;
    assert_eq!(place(&world), Vec3::new(0.0, 1.0, 0.0), "the prefab's");
    assert_eq!(inspector.log.edits().len(), 1, "one edit");
    let undo = |inspector: &mut Inspector, world: &mut World| {
        inspector.queue(Pass {
            chosen: Some(Step::Undo),
            ..Pass::default()
        });
        Layer::tick(inspector, world, Tick { number: 0 });
    };
    undo(&mut inspector, &mut world);
    assert_eq!(place(&world), Vec3::new(0.0, 5.0, 0.0), "undone");

    inspector.pick(root_id);
    let mut texts = Vec::new();
    for _ in 0..2 {
        texts = shown(&mut inspector, &mut world, Vec::new());
    }
    let unlink = find(&texts, "Instance of lamp.prefab", "Unlink");
    shown(&mut inspector, &mut world, click(Primary, unlink.center()));
    assert_eq!(world.get::<crate::PrefabRoot>(root), None, "unlinked");
    undo(&mut inspector, &mut world);
    assert!(
        world.get::<crate::PrefabRoot>(root).is_some(),
        "linked again"
    );
}

#[test]
fn a_number_typed_is_one_edit() {
    // With a tick each frame, and every other one, as at frame rates above the simulation's,
    // whose frames between draw the sets waiting for one.
    for every in [1, 2] {
        a_number_is_typed(every);
    }
}

fn a_number_is_typed(every: u32) {
    let mut world = World::default();
    let entity = world.spawn(everything());
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let mut frames = 0;
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        frames += 1;
        let tick = frames % every == 0;
        frame_ticking(inspector, world, &ctx, f64::from(frames), events, tick)
    };
    let mut texts = Vec::new();
    for _ in 0..3 {
        texts = run(&mut inspector, &mut world, Vec::new()).0;
    }
    let at = beside(&texts, find(&texts, "Everything", "weight"));
    let weight = |world: &World| world.get::<Everything>(entity).expect("everything").weight;
    // Clicked, which selects its text, typed over a key a frame, and entered.
    run(&mut inspector, &mut world, click(Primary, at));
    let mut sets = 0;
    for typed in ["0", ".", "7", "5"] {
        let text = vec![egui::Event::Text(typed.to_owned())];
        sets += run(&mut inspector, &mut world, text).1;
    }
    sets += run(&mut inspector, &mut world, enter()).1;
    // Then a frame with a tick, if that one had none.
    run(&mut inspector, &mut world, Vec::new());
    assert!(sets > 1 && weight(&world) == 0.75, "{sets} sets");
    assert_eq!(inspector.log.edits().len(), 1, "one edit");
    run(&mut inspector, &mut world, shortcut(egui::Key::Z, false));
    run(&mut inspector, &mut world, Vec::new());
    assert_eq!(weight(&world).to_bits(), 2.5f64.to_bits());
}

/// What the window of its own that [`in_window`] gives the panels gets and gives: its pass's
/// time and events, and its texts.
#[derive(Default)]
struct Window {
    time: f64,
    events: Vec<egui::Event>,
    texts: Vec<Text>,
}

/// Runs `ctx`'s windows of their own as the engine runs a torn-off panel's: each in a pass of
/// its own inside the main window's, with its own input, which the returned `Window` gives.
fn in_window(ctx: &egui::Context) -> Rc<RefCell<Window>> {
    ctx.set_embed_viewports(false);
    let window = Rc::new(RefCell::new(Window::default()));
    let shared = window.clone();
    // The callback is the test thread's.
    egui::Context::set_immediate_viewport_renderer(move |ctx, viewport| {
        let egui::ImmediateViewport {
            ids,
            mut viewport_ui_cb,
            ..
        } = viewport;
        let input = {
            let mut window = shared.borrow_mut();
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 4000.0));
            let mut viewports = egui::ViewportIdMap::default();
            viewports.insert(ids.this, egui::ViewportInfo::default());
            egui::RawInput {
                viewport_id: ids.this,
                viewports,
                screen_rect: Some(screen),
                time: Some(window.time),
                events: mem::take(&mut window.events),
                ..egui::RawInput::default()
            }
        };
        let mut output = ctx.run_ui(input, |ui| viewport_ui_cb(ui));
        output.textures_delta.clear();
        shared.borrow_mut().texts = texts_in(&output.shapes);
    });
    window
}

/// A frame of the main window, in whose pass the two panels draw in `window`, a window of
/// their own, given `events`; returns that window's texts, and how many edits it made.
fn window_frame(
    inspector: &mut Inspector,
    world: &mut World,
    ctx: &egui::Context,
    window: &RefCell<Window>,
    time: f64,
    events: Vec<egui::Event>,
) -> (Vec<Text>, usize) {
    *window.borrow_mut() = Window {
        time,
        events,
        texts: Vec::new(),
    };
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0));
    let input = egui::RawInput {
        screen_rect: Some(screen),
        time: Some(time),
        ..egui::RawInput::default()
    };
    let mut made = 0;
    let id = egui::ViewportId::from_hash_of("the panels' window");
    let mut output = ctx.run_ui(input, |root| {
        let mut pass = Pass::default();
        let builder = egui::ViewportBuilder::default();
        root.ctx().show_viewport_immediate(id, builder, |ui, _| {
            panels(inspector, world, ui, &mut pass);
        });
        pass.cues.read(root.ctx());
        made = pass.sets.len();
        make(inspector, world, pass);
    });
    output.textures_delta.clear();
    let texts = mem::take(&mut window.borrow_mut().texts);
    (texts, made)
}

#[test]
fn a_torn_off_inspector_keeps_its_drags_and_keys() {
    let mut world = World::default();
    let entity = world.spawn(everything());
    let ctx = crate::egui_context();
    let window = in_window(&ctx);
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let mut time = 0.0;
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        time += 1.0;
        window_frame(inspector, world, &ctx, &window, time, events)
    };
    let mut texts = Vec::new();
    for _ in 0..3 {
        texts = run(&mut inspector, &mut world, Vec::new()).0;
    }
    let at = beside(&texts, find(&texts, "Everything", "weight"));
    let weight = |world: &World| world.get::<Everything>(entity).expect("everything").weight;
    let mut sets = 0;
    for events in slide(at) {
        sets += run(&mut inspector, &mut world, events).1;
    }
    let dragged = weight(&world);
    assert!(sets > 1 && dragged != 2.5, "{sets} sets, to {dragged}");
    assert_eq!(inspector.log.edits().len(), 1, "one edit");
    // Pressed in that window, the keys step through the edits.
    run(&mut inspector, &mut world, shortcut(egui::Key::Z, false));
    assert_eq!(weight(&world).to_bits(), 2.5f64.to_bits());
    run(&mut inspector, &mut world, shortcut(egui::Key::Z, true));
    assert_eq!(weight(&world).to_bits(), dragged.to_bits());
}

#[test]
fn each_click_is_an_edit_and_a_text_field_keeps_its_keys() {
    let mut world = World::default();
    let entity = world.spawn(everything());
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let mut time = 0.0;
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        time += 1.0;
        frame(inspector, world, &ctx, time, events).0
    };
    let mut texts = Vec::new();
    for _ in 0..3 {
        texts = run(&mut inspector, &mut world, Vec::new());
    }
    let on = |world: &World| world.get::<Everything>(entity).expect("everything").on;
    // Its checkbox, which has no text, in the column its neighbor's number starts.
    let checkbox = egui::pos2(
        find(&texts, "count", "42").min.x + 4.0,
        find(&texts, "Everything", "on").center().y,
    );
    // Clicked in frames one after the other, each click is an edit of its own.
    run(&mut inspector, &mut world, click(Primary, checkbox));
    run(&mut inspector, &mut world, click(Primary, checkbox));
    assert!(on(&world));
    assert_eq!(inspector.log.edits().len(), 2);
    // While the Find field has the keys, they undo its own text, not an edit.
    let field = beside(&texts, find(&texts, "Find", "Find"));
    run(&mut inspector, &mut world, click(Primary, field));
    run(&mut inspector, &mut world, shortcut(egui::Key::Z, false));
    assert_eq!(inspector.log.edits().len(), 2);
    let away = click(Primary, egui::pos2(1500.0, 3900.0));
    run(&mut inspector, &mut world, away);
    run(&mut inspector, &mut world, shortcut(egui::Key::Z, false));
    assert_eq!(inspector.log.edits().len(), 1);
    assert!(!on(&world));
    // Two drags while the Find field keeps the keys are two edits, a press beginning each.
    run(&mut inspector, &mut world, click(Primary, field));
    let weight = beside(&texts, find(&texts, "Everything", "weight"));
    for events in [slide(weight), slide(weight)].concat() {
        run(&mut inspector, &mut world, events);
    }
    assert_eq!(inspector.log.edits().len(), 3);
}

/// A frame of the Edit and Entity menus' items alone, as the open menus show them, given
/// `events`, and then the tick that makes what the one clicked chose; returns its texts.
fn menu(
    inspector: &mut Inspector,
    world: &mut World,
    ctx: &egui::Context,
    time: f64,
    events: Vec<egui::Event>,
) -> Vec<Text> {
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 400.0));
    let input = egui::RawInput {
        screen_rect: Some(screen),
        time: Some(time),
        events,
        ..egui::RawInput::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        let mut pass = Pass {
            chosen: inspector.edit_menu(ui, &inspector.pending()),
            ..Pass::default()
        };
        inspector.entity_menu(world, ui, &mut pass.operations);
        pass.cues.read(ui.ctx());
        inspector.duplicate_picked(world, &mut pass);
        make(inspector, world, pass);
    });
    output.textures_delta.clear();
    texts_in(&output.shapes)
}

#[test]
fn the_edit_menu_names_its_steps_and_a_refused_one_says_why() {
    let mut world = World::default();
    let entity = world.spawn(everything());
    let picked = id(&world, entity);
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(picked);
    let count = |world: &World| world.get::<Everything>(entity).expect("everything").count;
    let edit = Edit {
        component: "Everything".to_owned(),
        path: "count".to_owned(),
        value: Value::UInt(99),
    };
    inspector.apply(&mut world, inspector.picked, edit);
    let mut time = 0.0;
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        time += 1.0;
        menu(inspector, world, &ctx, time, events)
    };
    let undo = format!("Undo set `Everything.count` of {picked}");
    let redo = format!("Redo set `Everything.count` of {picked}");
    let texts = run(&mut inspector, &mut world, Vec::new());
    for item in [undo.as_str(), "Redo"] {
        assert!(
            texts.iter().any(|(text, _)| text == item),
            "{item}: {texts:?}"
        );
    }
    run(
        &mut inspector,
        &mut world,
        click(Primary, find(&texts, &undo, &undo).center()),
    );
    assert_eq!(count(&world), 42);
    let texts = run(&mut inspector, &mut world, Vec::new());
    for item in ["Undo", redo.as_str()] {
        assert!(
            texts.iter().any(|(text, _)| text == item),
            "{item}: {texts:?}"
        );
    }
    run(
        &mut inspector,
        &mut world,
        click(Primary, find(&texts, &redo, &redo).center()),
    );
    assert_eq!(count(&world), 99);
    // With its entity gone, the edit can't be undone, which the Inspector panel says.
    world.despawn(entity).expect("alive");
    run(&mut inspector, &mut world, shortcut(egui::Key::Z, false));
    assert_eq!(inspector.log.undoable(), None);
    let mut texts = Vec::new();
    for _ in 0..3 {
        time += 1.0;
        texts = frame(&mut inspector, &mut world, &ctx, time, Vec::new()).0;
    }
    let says = format!("can't undo set `Everything.count` of {picked}: ");
    let said = texts.iter().any(|(text, _)| text.starts_with(&says));
    assert!(said, "{texts:?}");
}

/// A world cleared, replaced or restored, as a layer below the Inspector's would at its tick,
/// is another, which the Inspector's next tick finds: it unpicks, closes every row, forgets
/// what was refused and clears its log, so that Undo names nothing; and drops what was queued
/// for the world before, rather than make it in this one, where a replace or a restore gives
/// the entity it named back under its ID, or have it refused, as the world cleared would.
/// Edits from then on are this world's.
#[test]
fn another_world_unpicks_and_drops_the_edits_made_to_the_one_before() {
    let ctx = crate::egui_context();
    let count = |value| Edit {
        component: "Everything".to_owned(),
        path: "count".to_owned(),
        value: Value::UInt(value),
    };
    let set = |value| Pass {
        sets: vec![count(value)],
        ..Pass::default()
    };
    let mut time = 0.0;
    for case in ["cleared", "replaced", "restored"] {
        let mut world = World::default();
        let [parent, child] = [(); 2].map(|()| world.spawn(everything()));
        world.set_parent(child, Some(parent)).expect("no cycle");
        let saved = world.save_scene(&[parent]).expect("saved");
        let snapshot = world.clone();
        let picked = id(&world, child);
        let mut inspector = Inspector::default();
        inspector.pick(picked);
        inspector.expanded.insert(id(&world, parent));
        make(&mut inspector, &mut world, set(7));
        // Refused, and shown beside its field.
        inspector.apply(&mut world, Some(picked), count(200));
        time += 1.0;
        let texts = menu(&mut inspector, &mut world, &ctx, time, Vec::new());
        let undo = format!("Undo set `Everything.count` of {picked}");
        let named = texts.iter().any(|(text, _)| *text == undo);
        assert!(named, "{case}: {texts:?}");
        inspector.not_done = Some("can't undo it".to_owned());
        inspector.not_found = Some("no entity has ID 0123456789abcdef".to_owned());
        // For the next tick: a delete, then a set of a gesture that goes on.
        inspector.queue(Pass {
            operations: vec![Command::Delete(picked)],
            ..Pass::default()
        });
        inspector.queue(Pass {
            cues: Cues {
                held: true,
                ..Cues::default()
            },
            ..set(9)
        });
        assert_eq!(inspector.queued.len(), 2, "{case}");
        match case {
            "cleared" => world.clear(),
            "replaced" => {
                world.replace_scene(&saved).expect("replaced");
            }
            _ => world.clone_from(&snapshot),
        }
        Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
        assert_eq!(inspector.log.undoable(), None, "{case}");
        assert_eq!(inspector.picked, None, "{case}");
        assert!(inspector.expanded.is_empty(), "{case}");
        assert!(inspector.refusals.is_empty(), "{case}");
        // Dropped, not refused, as the delete would be in the world cleared.
        assert_eq!(inspector.not_done, None, "{case}");
        assert_eq!(inspector.not_found, None, "{case}");
        assert!(
            inspector.queued.is_empty() && inspector.open.is_none(),
            "{case}"
        );
        time += 1.0;
        let texts = menu(&mut inspector, &mut world, &ctx, time, Vec::new());
        let unnamed = texts.iter().any(|(text, _)| text == "Undo");
        assert!(unnamed, "{case}: {texts:?}");
        if case == "cleared" {
            assert_eq!(world.entities(None).expect("every entity").len(), 0);
            continue;
        }
        // Given back under its ID, neither set nor deleted; and edited from then on.
        let entity = world.entity(picked).expect("given back");
        let counted = |world: &World| {
            let all = world.get::<Everything>(entity);
            all.expect("everything").count
        };
        assert_eq!(counted(&world), 42, "{case}");
        inspector.pick(picked);
        make(&mut inspector, &mut world, set(9));
        assert_eq!(counted(&world), 9, "{case}");
        assert!(inspector.log.undoable().is_some(), "{case}");
    }
}

/// The very world the Inspector left coming back, as when a layer swaps a copy in to play and
/// back to stop, finds its edits, its pick and its open rows as they were, and its undo undoes
/// in it; the copy's go, and so do those of a world left before, which never comes back.
#[test]
fn the_world_left_coming_back_finds_its_edits_pick_and_rows() {
    let mut world = World::default();
    let [parent, child] = [(); 2].map(|()| world.spawn(everything()));
    world.set_parent(child, Some(parent)).expect("no cycle");
    let picked = id(&world, child);
    let count = |value| Pass {
        sets: vec![Edit {
            component: "Everything".to_owned(),
            path: "count".to_owned(),
            value: Value::UInt(value),
        }],
        ..Pass::default()
    };
    let mut inspector = Inspector::default();
    inspector.pick(picked);
    inspector.expanded.insert(id(&world, parent));
    make(&mut inspector, &mut world, count(7));
    // Swapped for a copy, edited there, and back.
    let copy = world.clone();
    let mut left = mem::replace(&mut world, copy);
    Layer::tick(&mut inspector, &mut world, Tick { number: 1 });
    assert_eq!(inspector.log.undoable(), None, "the copy's own");
    assert_eq!(inspector.picked, None);
    inspector.pick(picked);
    make(&mut inspector, &mut world, count(8));
    mem::swap(&mut world, &mut left);
    Layer::tick(&mut inspector, &mut world, Tick { number: 2 });
    assert_eq!(inspector.picked, Some(picked));
    assert!(inspector.expanded.contains(&id(&world, parent)));
    let counted = |world: &World| world.get::<Everything>(child).expect("alive").count;
    assert_eq!(counted(&world), 7);
    inspector.queue(Pass {
        chosen: Some(Step::Undo),
        ..Pass::default()
    });
    Layer::tick(&mut inspector, &mut world, Tick { number: 3 });
    assert_eq!(counted(&world), 42, "undone in it");
    // A world cleared is another for good: neither it nor the copy comes back.
    world.clear();
    Layer::tick(&mut inspector, &mut world, Tick { number: 4 });
    mem::swap(&mut world, &mut left);
    Layer::tick(&mut inspector, &mut world, Tick { number: 5 });
    assert_eq!(inspector.log.undoable(), None, "the copy's went");
    assert_eq!(inspector.picked, None);
}

/// The very world the Inspector left coming back through `restored`, as the tool API's `stop`
/// brings back the world as edited, with no tick between, finds its edits, its pick and its open
/// rows as they were, and its undo undoes in it; and so does it after another copy has played,
/// and in a run whose world as edited never ticked.
#[test]
fn the_world_left_coming_back_through_restored_finds_its_edits_pick_and_rows() {
    let mut cx = Context::seeded(1);
    let world = cx.world_mut();
    let [parent, child] = [(); 2].map(|()| world.spawn(everything()));
    world.set_parent(child, Some(parent)).expect("no cycle");
    let [parent, picked] = [parent, child].map(|entity| id(world, entity));
    let count = |value| Pass {
        sets: vec![Edit {
            component: "Everything".to_owned(),
            path: "count".to_owned(),
            value: Value::UInt(value),
        }],
        ..Pass::default()
    };
    let counted = |cx: &Context| cx.world().get::<Everything>(child).expect("alive").count;
    let mut inspector = Inspector::default();
    inspector.pick(picked);
    inspector.expanded.insert(parent);
    make(&mut inspector, cx.world_mut(), count(7));
    for play in 1..=2 {
        // A copy plays in its place, whose first tick puts the world's aside.
        let copy = cx.world().clone();
        let edited = mem::replace(cx.world_mut(), copy);
        Layer::tick(&mut inspector, cx.world_mut(), Tick { number: 1 });
        assert_eq!(inspector.picked, None, "play {play}");
        inspector.pick(picked);
        make(&mut inspector, cx.world_mut(), count(8));
        *cx.world_mut() = edited;
        Layer::restored(&mut inspector, &mut cx);
        assert_eq!(inspector.picked, Some(picked), "play {play}");
        assert!(inspector.expanded.contains(&parent), "play {play}");
        assert_eq!(counted(&cx), 7, "play {play}");
    }
    inspector.queue(Pass {
        chosen: Some(Step::Undo),
        ..Pass::default()
    });
    Layer::tick(&mut inspector, cx.world_mut(), Tick { number: 2 });
    assert_eq!(counted(&cx), 42, "undone in it");
    // A stopped run's world as edited, which never ticks, picked and opened in after the
    // Inspector attached, or after it was restored, as a run restored from a snapshot starts.
    for start in ["attached", "restored"] {
        let mut inspector = Inspector::default();
        match start {
            "attached" => Layer::attach(&mut inspector, &mut cx),
            _ => Layer::restored(&mut inspector, &mut cx),
        }
        inspector.pick(picked);
        inspector.expanded.insert(parent);
        let copy = cx.world().clone();
        let edited = mem::replace(cx.world_mut(), copy);
        Layer::tick(&mut inspector, cx.world_mut(), Tick { number: 3 });
        assert_eq!(inspector.picked, None, "{start}: the copy's own");
        *cx.world_mut() = edited;
        Layer::restored(&mut inspector, &mut cx);
        assert_eq!(inspector.picked, Some(picked), "{start}");
        assert!(inspector.expanded.contains(&parent), "{start}");
    }
}

/// In a run whose ticks run again on a copy of the world (`--sync-test`), the Inspector's sets,
/// undos and redos run again on the copy as they ran in the world, through a copy of its log as
/// each tick started, so that the copy parts from the world at no tick; and the world keeps its
/// serial, and its log, whose undos and redos go on in it.
#[test]
fn a_sync_tests_copy_takes_the_inspectors_edits_undos_and_redos_as_the_world_did() {
    let mut cx = Context::seeded(1);
    let entity = cx.world_mut().spawn(everything());
    let picked = id(cx.world(), entity);
    let mut stack = crate::layer::Stack::default();
    stack.push_overlay(Inspector::default());
    stack.sync_test();
    stack.attach(&mut cx);
    let serial = cx.world().serial();
    let set = |count| {
        let edit = Edit {
            component: "Everything".to_owned(),
            path: "count".to_owned(),
            value: Value::UInt(count),
        };
        vec![Command::Set(picked, edit), Command::Seal]
    };
    let ticks = [
        (set(7), 7),
        (set(8), 8),
        (vec![Command::Step(Step::Undo)], 7),
        (vec![Command::Step(Step::Undo)], 42),
        (vec![Command::Step(Step::Redo)], 7),
    ];
    for (number, (commands, count)) in (0..).zip(ticks) {
        let queue = stack.commands_of(0).expect("its queue");
        for command in commands {
            let value = Queue(vec![command]).value(0);
            queue.push(&value).expect("queued");
        }
        assert!(stack.tick(&mut cx, Tick { number }));
        assert!(
            !cx.exiting(),
            "tick {number}: the copy parted from the world"
        );
        let counted = cx.world().get::<Everything>(entity).expect("alive").count;
        assert_eq!(counted, count, "tick {number}");
    }
    assert_eq!(cx.world().serial(), serial);
    stack.detach(&mut cx);
    assert!(!stack.failed());
}

/// Every row of the tree, the naive way: each root in turn, each followed, if its row is open,
/// by the rows of each of its children in turn.
fn flatten(world: &World, expanded: &HashSet<EntityId>) -> Vec<(EntityId, usize)> {
    fn rows(
        world: &World,
        expanded: &HashSet<EntityId>,
        (entity, depth): (Entity, usize),
        out: &mut Vec<(EntityId, usize)>,
    ) {
        out.push((id(world, entity), depth));
        if expanded.contains(&id(world, entity)) {
            for child in world.children(entity).expect("alive") {
                rows(world, expanded, (child, depth + 1), out);
            }
        }
    }
    let mut out = Vec::new();
    for root in world.roots() {
        rows(world, expanded, (root, 0), &mut out);
    }
    out
}

/// `n` children made for `parent`, each spawned a root and then given its parent.
fn children_of(world: &mut World, parent: Entity, n: usize) -> Vec<Entity> {
    let made = (0..n).map(|_| {
        let child = world.spawn(Transform::IDENTITY);
        world.set_parent(child, Some(parent)).expect("no cycle");
        child
    });
    made.collect()
}

/// The tree finds a page of its rows as a listing of every row does, wherever the page starts:
/// over 50,000 roots in several tables, open ones followed by their rows, with open children
/// among them; an open root without children; an open child under a closed parent, which adds
/// no rows; and an open entity despawned.
#[test]
fn the_tree_finds_a_page_as_a_listing_of_every_row_does() {
    let mut world = World::default();
    let plain: Vec<Entity> = (0..20_000)
        .map(|_| world.spawn(Transform::IDENTITY))
        .collect();
    // In a table of children, between the roots' tables.
    let top = children_of(&mut world, plain[5], 4);
    let middle = children_of(&mut world, top[0], 3);
    let bottom = children_of(&mut world, middle[1], 2);
    let named: Vec<Entity> = (0..20_000)
        .map(|i| world.spawn((called(&i.to_string()), Transform::IDENTITY)))
        .collect();
    let sprites: Vec<Entity> = (0..10_000)
        .map(|_| world.spawn(Sprite::default()))
        .collect();
    let last = *sprites.last().expect("sprites");
    children_of(&mut world, named[100], 3);
    let closed = children_of(&mut world, named[200], 2);
    children_of(&mut world, closed[0], 2);
    children_of(&mut world, last, 2);
    let gone = world.spawn(Transform::IDENTITY);
    let gone_id = id(&world, gone);
    world.despawn(gone).expect("alive");
    let open = [
        plain[5], top[0], middle[1], named[7], named[100], closed[0], last,
    ];
    let mut expanded: HashSet<EntityId> = open.iter().map(|&entity| id(&world, entity)).collect();
    expanded.insert(gone_id);
    let tree = Tree::new(&world, &expanded);
    let flat = flatten(&world, &expanded);
    assert!(flat.len() > 50_000, "{}", flat.len());
    assert!(flat.contains(&(id(&world, bottom[1]), 3)));
    assert_eq!(tree.spans.len(), 4, "the roots open");
    assert_eq!(tree.len, flat.len());
    assert_eq!(tree.page(&world, 0..flat.len()), flat);
    // Pages from every 97th row, and from each row about each open root's.
    let mut starts: Vec<usize> = (0..flat.len()).step_by(97).collect();
    let mut before = 0;
    for &(at, n) in &tree.spans {
        let start = at + before;
        starts.extend(start.saturating_sub(2)..(start + n + 3).min(flat.len()));
        before += n;
    }
    for start in starts {
        let end = (start + 40).min(flat.len());
        assert_eq!(tree.page(&world, start..end), flat[start..end], "{start}");
    }
}

/// A frame scrolled thousands of rows down draws the rows in view alone, as the tree lists
/// them, an open root's rows among them.
#[test]
fn a_frame_scrolled_over_thousands_of_roots_draws_the_rows_in_view() {
    let mut world = World::default();
    let roots: Vec<Entity> = (0..5000)
        .map(|i| world.spawn(called(&format!("root {i}"))))
        .collect();
    for i in 0..3 {
        let child = world.spawn(called(&format!("child {i}")));
        world.set_parent(child, Some(roots[130])).expect("no cycle");
    }
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.expanded.insert(id(&world, roots[130]));
    let flat = flatten(&world, &inspector.expanded);
    let rows: HashMap<String, usize> = flat
        .iter()
        .enumerate()
        .map(|(row, &(id, _))| (row_label(&world, id).text().to_owned(), row))
        .collect();
    let mut time = 0.0;
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        let mut texts = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            time += 1.0;
            texts = frame(inspector, world, &ctx, time, events).0;
        }
        texts
    };
    run(&mut inspector, &mut world, Vec::new());
    // As a trackpad scrolls, which egui doesn't smooth.
    let wheel = |phase, y| egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, y),
        phase,
        source: egui::MouseWheelSource::Unknown,
        modifiers: Modifiers::NONE,
    };
    let scrolled = vec![
        egui::Event::PointerMoved(egui::pos2(200.0, 600.0)),
        wheel(egui::TouchPhase::Start, 0.0),
        wheel(egui::TouchPhase::Move, -2500.0),
    ];
    let texts = run(&mut inspector, &mut world, scrolled);
    let shown: Vec<usize> = texts
        .iter()
        .filter_map(|(text, _)| rows.get(text).copied())
        .collect();
    let first = *shown.first().expect("rows shown");
    assert!(first > 100 && shown.len() < 300, "{shown:?}");
    assert_eq!(shown, (first..first + shown.len()).collect::<Vec<_>>());
    assert!((131..134).all(|child| shown.contains(&child)), "{shown:?}");
}

/// Where the arrows `shapes` draw are: their triangles.
fn arrows_in(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Rect> {
    let painted = crate::testing::painted(shapes).into_iter();
    let arrows = painted.filter_map(|shape| match shape {
        egui::Shape::Path(path) if path.closed && path.points.len() == 3 => {
            Some(path.visual_bounding_rect())
        }
        _ => None,
    });
    arrows.collect()
}

/// Only the rows of entities with children have arrows, which show and hide their children's
/// rows, under theirs and further in, rather than pick them; a list of the entities with a
/// component has neither arrows nor indents.
#[test]
fn rows_with_children_alone_have_arrows() {
    let mut world = World::default();
    let [parent, child, lone] = ["parent", "child", "lone"].map(|name| world.spawn(called(name)));
    world.set_parent(child, Some(parent)).expect("no cycle");
    let [parent_row, child_row, lone_row] = [parent, child, lone].map(|e| row_text(&world, e));
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let mut time = 0.0;
    // A frame given `events`, then two that settle what it did; the last one's texts and
    // arrows.
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        let (mut texts, mut arrows) = (Vec::new(), Vec::new());
        for events in [events, Vec::new(), Vec::new()] {
            time += 1.0;
            let (shapes, _) = frame_shapes(inspector, world, &ctx, time, events, true);
            (texts, arrows) = (texts_in(&shapes), arrows_in(&shapes));
        }
        (texts, arrows)
    };
    // The arrow on the row labeled `row`, left of its label, if it has one.
    let arrow = |(texts, arrows): &(Vec<Text>, Vec<egui::Rect>), row: &str| {
        let label = find(texts, row, row);
        let on_row = |arrow: &&egui::Rect| {
            (arrow.center().y - label.center().y).abs() < 4.0 && arrow.max.x <= label.min.x
        };
        arrows.iter().find(on_row).copied()
    };
    let shows = |(texts, _): &(Vec<Text>, Vec<egui::Rect>), row: &str| {
        texts.iter().any(|(text, _)| text == row)
    };
    let drawn = run(&mut inspector, &mut world, Vec::new());
    assert!(arrow(&drawn, &lone_row).is_none());
    assert!(!shows(&drawn, &child_row));
    let open = arrow(&drawn, &parent_row)
        .expect("the parent's arrow")
        .center();
    let drawn = run(&mut inspector, &mut world, click(Primary, open));
    assert_eq!(inspector.picked, None);
    let (above, under) = (
        find(&drawn.0, &parent_row, &parent_row),
        find(&drawn.0, &child_row, &child_row),
    );
    assert!(under.min.x > above.min.x && under.min.y > above.max.y);
    assert!(arrow(&drawn, &child_row).is_none());
    let drawn = run(&mut inspector, &mut world, click(Primary, open));
    assert!(!shows(&drawn, &child_row));
    inspector.with = Some("Name".to_owned());
    let drawn = run(&mut inspector, &mut world, Vec::new());
    for row in [&parent_row, &child_row, &lone_row] {
        assert!(arrow(&drawn, row).is_none(), "{row}");
    }
    // Within a glyph's bearing.
    let lefts = [&parent_row, &child_row].map(|row| find(&drawn.0, row, row).min.x);
    assert!((lefts[0] - lefts[1]).abs() < 4.0, "{lefts:?}");
}

/// Where `name` shows in a menu, over everything else.
fn in_menu(ctx: &egui::Context, texts: &[Text], name: &str) -> egui::Pos2 {
    let over = |(text, rect): &&Text| {
        let layer = ctx.layer_id_at(rect.center());
        text == name && layer.is_some_and(|layer| layer.order == egui::Order::Foreground)
    };
    let found = texts.iter().find(over);
    found
        .unwrap_or_else(|| panic!("a menu shows {name}: {texts:?}"))
        .1
        .center()
}

/// A world that has registered the components of an empty entity, as a bare one hasn't.
fn creating() -> World {
    let mut world = World::default();
    world.register::<Name>().expect("registered");
    world.register::<Transform>().expect("registered");
    world
}

/// The Entities panel's context menus, which a secondary click opens and egui sizes in a
/// frame of their own: below the rows, one makes a root; and on a row, one makes a child of
/// its entity, duplicates it or deletes it, with its descendants. Each is made at the next
/// tick, which picks what a create made, or the copy, and shows its row; and each is undone,
/// and the creates and the delete redone.
#[test]
fn context_menus_create_and_delete_entities() {
    let mut world = creating();
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let clock = Cell::new(0.0);
    let now = || {
        clock.set(clock.get() + 1.0);
        clock.get()
    };
    // A frame given `events`, then two that settle what it did; the last one's texts.
    let run = |inspector: &mut Inspector, world: &mut World, events| {
        let mut texts = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            texts = frame(inspector, world, &ctx, now(), events).0;
        }
        texts
    };
    let alive = |world: &World| world.entities(None).expect("every entity").len();
    // Every entity is an Empty Entity: where the Entities panel, the screen's left half,
    // shows their rows, top first.
    let rows = |texts: &[Text]| {
        let rows = texts
            .iter()
            .filter(|(text, rect)| text == "Empty Entity" && rect.max.x < 800.0);
        let mut rows: Vec<egui::Rect> = rows.map(|(_, rect)| *rect).collect();
        rows.sort_by(|a, b| a.min.y.total_cmp(&b.min.y));
        rows
    };
    // Opens the menu of what's at `at`, and clicks its item `item`.
    let choose = |inspector: &mut Inspector, world: &mut World, at: egui::Pos2, item: &str| {
        let texts = run(inspector, world, click(Secondary, at));
        let item = in_menu(&ctx, &texts, item);
        run(inspector, world, click(Primary, item))
    };
    run(&mut inspector, &mut world, Vec::new());
    let below = egui::pos2(200.0, 3000.0);
    let texts = run(&mut inspector, &mut world, click(Secondary, below));
    let item = in_menu(&ctx, &texts, "Create Empty Entity");
    let clicked = click(Primary, item);
    frame_ticking(&mut inspector, &mut world, &ctx, now(), clicked, false);
    assert_eq!(alive(&world), 0, "made at the next tick");
    Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
    let root = world.roots().next().expect("a root");
    assert_eq!(inspector.picked, Some(id(&world, root)));
    assert_eq!(world.get::<Name>(root), Some(&called("Empty Entity")));
    assert_eq!(world.get::<Transform>(root), Some(&Transform::IDENTITY));
    // A child, and a child of that, each shown under its parent's row, further in, as it's
    // picked.
    let mut parent = root;
    let mut made = Vec::new();
    for depth in 0..2 {
        let texts = run(&mut inspector, &mut world, Vec::new());
        let at = rows(&texts)[depth].center();
        let texts = choose(&mut inspector, &mut world, at, "Create Empty Child");
        let child = world
            .children(parent)
            .expect("alive")
            .next()
            .expect("a child");
        assert_eq!(inspector.picked, Some(id(&world, child)));
        let shown = rows(&texts);
        let (above, under) = (shown[depth], shown[depth + 1]);
        assert!(
            under.min.y > above.max.y && under.min.x > above.min.x,
            "{shown:?}"
        );
        made.push(id(&world, child));
        parent = child;
    }
    // The whole subtree duplicated from the root's row, the copy of the root picked, and
    // undone.
    let (undo, redo) = (shortcut(egui::Key::Z, false), shortcut(egui::Key::Z, true));
    let texts = run(&mut inspector, &mut world, Vec::new());
    let at = rows(&texts)[0].center();
    choose(&mut inspector, &mut world, at, "Duplicate Entity");
    assert_eq!(alive(&world), 6);
    let copy = world.roots().find(|&other| other != root).expect("a copy");
    assert_eq!(inspector.picked, Some(id(&world, copy)));
    run(&mut inspector, &mut world, undo.clone());
    assert_eq!(alive(&world), 3);
    inspector.pick(made[1]);
    // The whole subtree deleted from the root's row, the pick staying on what's gone.
    let texts = run(&mut inspector, &mut world, Vec::new());
    let at = rows(&texts)[0].center();
    let texts = choose(&mut inspector, &mut world, at, "Delete Entity");
    assert_eq!(alive(&world), 0);
    assert_eq!(inspector.picked, Some(made[1]));
    let despawned = format!("Entity {} was despawned", made[1]);
    assert!(
        texts.iter().any(|(text, _)| *text == despawned),
        "{texts:?}"
    );
    // Undone, it comes back, under its IDs; then each create is undone, and each redone.
    run(&mut inspector, &mut world, undo.clone());
    assert_eq!(alive(&world), 3);
    assert!(made.iter().all(|&made| world.entity(made).is_some()));
    for (keys, expected) in [
        (&undo, 2),
        (&undo, 1),
        (&undo, 0),
        (&redo, 1),
        (&redo, 2),
        (&redo, 3),
        (&redo, 0),
    ] {
        run(&mut inspector, &mut world, keys.clone());
        assert_eq!(alive(&world), expected);
    }
}

/// A context menu left open acts on the entity it opened on, though its row moves as tables
/// swap rows, and another entity's row comes to its place.
#[test]
fn a_context_menu_acts_on_its_entity_as_its_row_moves() {
    let mut world = World::default();
    let [a, b, c] = ["a", "b", "c"].map(|name| world.spawn((called(name), Transform::IDENTITY)));
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let mut time = 0.0;
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        let mut texts = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            time += 1.0;
            texts = frame(inspector, world, &ctx, time, events).0;
        }
        texts
    };
    let row = |world: &World, entity| row_text(world, entity);
    let texts = run(&mut inspector, &mut world, Vec::new());
    let first = find(&texts, &row(&world, a), &row(&world, a));
    run(&mut inspector, &mut world, click(Secondary, first.center()));
    // To the end of another table, the last of its own taking its row.
    world.insert(a, Sprite::default()).expect("alive");
    let texts = run(&mut inspector, &mut world, Vec::new());
    let moved_in = find(&texts, &row(&world, c), &row(&world, c));
    assert!(
        (moved_in.center().y - first.center().y).abs() < 4.0,
        "{texts:?}"
    );
    let item = in_menu(&ctx, &texts, "Delete Entity");
    run(&mut inspector, &mut world, click(Primary, item));
    assert!(!world.contains(a));
    assert!(world.contains(b) && world.contains(c));
}

/// The Entity menu makes a root; and, while the entity picked is alive, makes a child of it,
/// duplicates it, as Ctrl+D does, picking the copy, and deletes it; and unpicks it, despawned
/// too.
#[test]
fn the_entity_menu_creates_duplicates_deletes_and_unpicks() {
    let mut world = creating();
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let mut time = 0.0;
    // A frame given `events`, then one that settles what it did, since egui finds what's
    // clicked among the last frame's widgets; the last one's texts.
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        let mut texts = Vec::new();
        for events in [events, Vec::new()] {
            time += 1.0;
            texts = menu(inspector, world, &ctx, time, events);
        }
        texts
    };
    let texts = run(&mut inspector, &mut world, Vec::new());
    let item = |name: &str| click(Primary, find(&texts, name, name).center());
    // Without a pick, only Create Empty acts.
    let duplicate = shortcut(egui::Key::D, false);
    run(&mut inspector, &mut world, duplicate.clone());
    for name in ["Create Empty Child", "Duplicate", "Delete", "Unpick"] {
        run(&mut inspector, &mut world, item(name));
    }
    assert_eq!(world.entities(None).expect("every entity").len(), 0);
    run(&mut inspector, &mut world, item("Create Empty"));
    let root = world.roots().next().expect("a root");
    assert_eq!(inspector.picked, Some(id(&world, root)));
    run(&mut inspector, &mut world, item("Create Empty Child"));
    let child = world
        .children(root)
        .expect("alive")
        .next()
        .expect("a child");
    let child_id = id(&world, child);
    assert_eq!(inspector.picked, Some(child_id));
    assert!(inspector.expanded.contains(&id(&world, root)));
    // Each copy the last of the root's children, and picked; the second a copy of the first.
    for events in [item("Duplicate"), duplicate] {
        run(&mut inspector, &mut world, events);
        let children: Vec<Entity> = world.children(root).expect("alive").collect();
        let &copy = children.last().expect("a copy");
        assert_eq!(inspector.picked, Some(id(&world, copy)));
        assert_eq!(world.get::<Name>(copy), Some(&called("Empty Entity")));
    }
    assert_eq!(world.children(root).expect("alive").count(), 3);
    run(&mut inspector, &mut world, shortcut(egui::Key::Z, false));
    assert_eq!(world.children(root).expect("alive").count(), 2, "undone");
    // Drawn picked, so that Delete is enabled in the frame before the click, which egui finds
    // it among.
    inspector.pick(child_id);
    run(&mut inspector, &mut world, Vec::new());
    run(&mut inspector, &mut world, item("Delete"));
    assert!(!world.contains(child));
    // Still picked, for an undo to bring it back; but its pick despawned unpicks too.
    assert_eq!(inspector.picked, Some(child_id));
    run(&mut inspector, &mut world, item("Unpick"));
    assert_eq!(inspector.picked, None);
}

/// A click below the rows unpicks, forgetting the refusals of the entity picked.
#[test]
fn a_click_below_the_rows_unpicks() {
    let mut world = World::default();
    let entity = world.spawn(everything());
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let refused = Edit {
        component: "Everything".to_owned(),
        path: "count".to_owned(),
        value: Value::UInt(200),
    };
    inspector.apply(&mut world, inspector.picked, refused);
    let mut texts = Vec::new();
    let below = click(Primary, egui::pos2(200.0, 3000.0));
    for (time, events) in [Vec::new(), below, Vec::new(), Vec::new()]
        .into_iter()
        .enumerate()
    {
        texts = frame(&mut inspector, &mut world, &ctx, time as f64, events).0;
    }
    assert_eq!(inspector.picked, None);
    assert!(inspector.refusals.is_empty());
    let nothing = "Nothing picked: pick an entity in the Entities panel";
    assert!(texts.iter().any(|(text, _)| text == nothing), "{texts:?}");
}

/// A click across a row, well right of its label's text, picks its entity, in the tree and in
/// the list of those with a component; and each row is a line tall, as their paging takes it,
/// one whose label is wider than the panel too.
#[test]
fn a_click_across_a_row_picks_its_entity() {
    let mut world = World::default();
    let long = "long ".repeat(40);
    let [a, b, c] = ["a", long.as_str(), "c"].map(|name| world.spawn(called(name)));
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let mut time = 0.0;
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        let mut texts = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            time += 1.0;
            texts = frame(inspector, world, &ctx, time, events).0;
        }
        texts
    };
    let spacing = ctx.global_style().spacing.clone();
    let line = spacing.interact_size.y + spacing.item_spacing.y;
    for with in [None, Some("Name")] {
        inspector.with = with.map(str::to_owned);
        let texts = run(&mut inspector, &mut world, Vec::new());
        let row = |entity| {
            let label = row_text(&world, entity);
            find(&texts, &label, &label)
        };
        let rows = [a, b, c].map(row);
        let wide = rows[1];
        assert!(wide.width() > 800.0 && wide.height() < line, "{wide:?}");
        // Where the texts of the rows around it end below, which none of theirs hangs under,
        // as the g of "long" does: two lines apart.
        let apart = rows[2].max.y - rows[0].max.y;
        assert!((apart - 2.0 * line).abs() < 1.0, "{with:?}: {apart}");
        // Near the right edge of the Entities panel, the screen's left half.
        for (entity, label) in [(c, rows[2]), (a, rows[0])] {
            let at = egui::pos2(790.0, label.center().y);
            assert!(at.x > label.max.x + 400.0, "{label:?}");
            run(&mut inspector, &mut world, click(Primary, at));
            assert_eq!(inspector.picked, Some(id(&world, entity)), "{with:?}");
        }
    }
}

/// An entity found by its ID shows its row: the rows above it open.
#[test]
fn finding_an_entity_opens_the_rows_above_it() {
    let mut world = World::default();
    let [top, middle, bottom] = ["top", "middle", "bottom"].map(|name| world.spawn(called(name)));
    world.set_parent(middle, Some(top)).expect("no cycle");
    world.set_parent(bottom, Some(middle)).expect("no cycle");
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let mut time = 0.0;
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        let mut texts = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            time += 1.0;
            texts = frame(inspector, world, &ctx, time, events).0;
        }
        texts
    };
    let found = row_text(&world, bottom);
    let texts = run(&mut inspector, &mut world, Vec::new());
    assert!(!texts.iter().any(|(text, _)| *text == found));
    let field = beside(&texts, find(&texts, "Find", "Find"));
    run(&mut inspector, &mut world, click(Primary, field));
    let typed = vec![egui::Event::Text(id(&world, bottom).to_string())];
    run(&mut inspector, &mut world, typed);
    let texts = run(&mut inspector, &mut world, enter());
    assert_eq!(inspector.picked, Some(id(&world, bottom)));
    let open: HashSet<EntityId> = [top, middle].map(|entity| id(&world, entity)).into();
    assert_eq!(inspector.expanded, open);
    assert!(texts.iter().any(|(text, _)| *text == found), "{texts:?}");
}

/// What a component that follows an entity holds: its handle, of which there's no default.
#[derive(Clone, Component)]
struct Follower(Entity);

/// Opaque as a whole, as a mesh's draw is: no value makes one.
#[derive(Clone, Component)]
#[reflect(opaque)]
struct Sealed;

/// The Add Component menu lists the types the picked entity can be given, in the order they
/// registered: not those it has, nor a parent, nor those opaque as a whole. One picked is added
/// at the next tick, of its type's default, which an undo takes away; one without a default
/// shows disabled, saying why on hover, and adds nothing.
#[test]
fn add_component_adds_a_types_default() {
    let mut world = creating();
    world.register::<Camera>().expect("registered");
    let entity = world.spawn(Transform::IDENTITY);
    // Registered as they're spawned.
    world.spawn(Follower(entity));
    world.spawn(Sealed);
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let mut time = 0.0;
    // A frame given `events`, then two that settle what they did, as a popup draws unseen in
    // its first; the last one's texts.
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        let mut texts = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            time += 1.0;
            texts = frame(inspector, world, &ctx, time, events).0;
        }
        texts
    };
    let texts = run(&mut inspector, &mut world, Vec::new());
    let open = click(
        Primary,
        find(&texts, "Add Component", "Add Component").center(),
    );
    let texts = run(&mut inspector, &mut world, open.clone());
    let over = |(_, rect): &&Text| {
        let layer = ctx.layer_id_at(rect.center());
        layer.is_some_and(|layer| layer.order == egui::Order::Foreground)
    };
    let items: Vec<&str> = texts
        .iter()
        .filter(over)
        .map(|(text, _)| text.as_str())
        .collect();
    assert_eq!(items, ["Name", "Camera", "Follower"]);
    let follower = in_menu(&ctx, &texts, "Follower");
    let hovered = run(
        &mut inspector,
        &mut world,
        vec![egui::Event::PointerMoved(follower)],
    );
    let why = "can't make a default `Follower.0`: an entity has no default";
    let said = hovered.iter().any(|(text, _)| text.starts_with(why));
    assert!(said, "{hovered:?}");
    run(&mut inspector, &mut world, click(Primary, follower));
    assert!(world.get::<Follower>(entity).is_none());
    let texts = run(&mut inspector, &mut world, open);
    let camera = click(Primary, in_menu(&ctx, &texts, "Camera"));
    run(&mut inspector, &mut world, camera);
    let made = Camera {
        projection: Projection::Orthographic {
            height: 10.0,
            near: -1.0,
            far: 1.0,
        },
        primary: false,
    };
    assert_eq!(world.get::<Camera>(entity), Some(&made));
    assert_eq!(inspector.log.edits().len(), 1);
    run(&mut inspector, &mut world, shortcut(egui::Key::Z, false));
    assert_eq!(world.get::<Camera>(entity), None);
}

/// The menu beside each component's header removes it, at the next tick, and until then it
/// isn't drawn; an undo gives it back. A parent, which the world gives, has no menu.
#[test]
fn a_components_menu_removes_it() {
    let mut world = World::default();
    let parent = world.spawn(Transform::IDENTITY);
    let color = Vec4::new(0.5, 0.25, 1.0, 1.0);
    let sprite = Sprite {
        color,
        ..Sprite::default()
    };
    let entity = world.spawn((Transform::IDENTITY, sprite));
    world.set_parent(entity, Some(parent)).expect("no cycle");
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let mut time = 0.0;
    // A frame given `events`, then two that settle what they did, each with a tick if `tick`;
    // the last one's texts, and its components' menus.
    let mut run = |inspector: &mut Inspector, world: &mut World, events, tick| {
        let mut shapes = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            time += 1.0;
            shapes = frame_shapes(inspector, world, &ctx, time, events, tick).0;
        }
        (texts_in(&shapes), find_menus(&shapes))
    };
    let (texts, menus) = run(&mut inspector, &mut world, Vec::new(), true);
    // Its transform's and its sprite's.
    assert_eq!(menus.len(), 2, "{menus:?}");
    let (button, _) = menu_on(&menus, find(&texts, "Sprite", "Sprite"));
    let menu = click(Primary, button.center());
    let (texts, _) = run(&mut inspector, &mut world, menu, true);
    let remove = click(Primary, in_menu(&ctx, &texts, "Remove Component"));
    let (texts, _) = run(&mut inspector, &mut world, remove, false);
    assert!(!texts.iter().any(|(text, _)| text == "Sprite"), "{texts:?}");
    assert!(
        world.get::<Sprite>(entity).is_some(),
        "removed at the next tick"
    );
    Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
    assert!(world.get::<Sprite>(entity).is_none());
    run(
        &mut inspector,
        &mut world,
        shortcut(egui::Key::Z, false),
        true,
    );
    let back = world.get::<Sprite>(entity).map(|sprite| sprite.color);
    assert_eq!(back, Some(color));
}

/// A component's header is framed, as wide as the Inspector panel: a click on it, however far
/// right of its name, closes and opens it, while its menu, over its right end, its dots in the
/// middle of its button, opens from its button and leaves it as it was.
#[test]
fn a_header_spans_its_panel_under_its_menu() {
    let mut world = World::default();
    let entity = world.spawn(Sprite::default());
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let mut time = 0.0;
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        let mut shapes = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            time += 1.0;
            shapes = frame_shapes(inspector, world, &ctx, time, events, true).0;
        }
        (texts_in(&shapes), find_menus(&shapes))
    };
    let open = |texts: &[Text]| texts.iter().any(|(text, _)| text == "color");
    let (texts, menus) = run(&mut inspector, &mut world, Vec::new());
    assert!(open(&texts), "{texts:?}");
    let header = find(&texts, "Sprite", "Sprite");
    // On the header's row, at the right end of the Inspector panel, the screen's right half,
    // its dots in the middle of its button, both ways.
    let (menu, dots) = menu_on(&menus, header);
    assert!(menu.max.x > 1550.0 && menu.max.x <= 1600.0, "{menu:?}");
    let off = dots.center() - menu.center();
    assert!(off.length() < 0.5, "{dots:?} off the middle of {menu:?}");
    let beside = egui::pos2(menu.min.x - 40.0, header.center().y);
    assert!(beside.x > header.max.x + 500.0, "{header:?}");
    let (texts, _) = run(&mut inspector, &mut world, click(Primary, beside));
    assert!(!open(&texts), "closed: {texts:?}");
    let (texts, _) = run(&mut inspector, &mut world, click(Primary, beside));
    assert!(open(&texts), "opened: {texts:?}");
    let (texts, _) = run(&mut inspector, &mut world, click(Primary, menu.center()));
    in_menu(&ctx, &texts, "Remove Component");
    assert!(open(&texts), "left open: {texts:?}");
}

/// A component's menu shows, and opens from a click anywhere on its button, in an Inspector
/// panel narrower and shorter than its contents: their rows run past its right edge, cut off,
/// as a narrow one's axes do, and its scroll bar floats along that edge. It's at the end of
/// what shows of its header, short of the bar.
#[test]
fn a_components_menu_opens_in_a_panel_smaller_than_its_contents() {
    let mut world = World::default();
    let entity = world.spawn((Transform::IDENTITY, Sprite::default()));
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0));
    let panel = egui::Rect::from_min_max(egui::pos2(1380.0, 0.0), egui::pos2(1600.0, 150.0));
    let mut time = 0.0;
    // A frame given `events`, then two that settle what they did, as the dock draws the panel,
    // cut off at its edges, with the Inspector's own scroll area; the last one's texts, and
    // its components' menus.
    let mut run = |events: Vec<egui::Event>| {
        let mut shapes = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            time += 1.0;
            let input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                events,
                ..egui::RawInput::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let mut pass = Pass::default();
                let inspect = egui::UiBuilder::new().id_salt("Inspector").max_rect(panel);
                ui.scope_builder(inspect, |ui| {
                    ui.set_clip_rect(panel);
                    let scroll = egui::ScrollArea::vertical();
                    let pending = inspector.pending();
                    scroll.show(ui, |ui| inspector.inspect(&world, ui, &mut pass, &pending));
                });
                inspector.queue(pass);
            });
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        (texts_in(&shapes), find_menus(&shapes))
    };
    let (texts, menus) = run(Vec::new());
    // The rows of axes run past the panel's edge, translation's Z button cut off, undrawn.
    let row = |name: &str| texts.iter().position(|(text, _)| text == name);
    let (translation, rotation) = (row("translation").unwrap(), row("rotation").unwrap());
    let z = texts[translation..rotation]
        .iter()
        .any(|(text, _)| text == "Z");
    assert!(!z, "the axes within the panel: {texts:?}");
    let header = find(&texts, "Transform", "Transform");
    let (menu, _) = menu_on(&menus, header);
    assert!(panel.contains_rect(menu), "{menu:?} out of {panel:?}");
    assert!(
        menu.max.x > panel.max.x - 40.0,
        "{menu:?} at the panel's end"
    );
    let opened = |texts: &[Text]| texts.iter().any(|(text, _)| text == "Remove Component");
    let (texts, _) = run(click(Primary, menu.center()));
    in_menu(&ctx, &texts, "Remove Component");
    let (texts, _) = run(click(Primary, menu.center()));
    assert!(!opened(&texts), "closed: {texts:?}");
    // Its right end, a point in from its edge: the end toward the scroll bar.
    let end = egui::pos2(menu.max.x - 1.0, menu.center().y);
    let (texts, _) = run(click(Primary, end));
    in_menu(&ctx, &texts, "Remove Component");
}

/// An enum with fields switches its variant from a choice of them: to one it had while the
/// entity was drawn, with the fields it had then; to one it hadn't, with its fields' defaults;
/// and on another entity, to the defaults again.
#[test]
fn variants_switch_to_the_fields_they_had_or_their_defaults() {
    let mut world = World::default();
    let camera = |height| Camera {
        projection: Projection::Orthographic {
            height,
            near: -2.0,
            far: 2.0,
        },
        primary: false,
    };
    let [first, second] = [4.0, 6.0].map(|height| world.spawn(camera(height)));
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, first));
    let clock = Cell::new(0.0);
    // A frame given `events`, then two that settle what they did, as a popup draws unseen in
    // its first, each with a tick if `tick`; the last one's texts.
    let run = |inspector: &mut Inspector, world: &mut World, events, tick| {
        let mut texts = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            clock.set(clock.get() + 1.0);
            texts = frame_ticking(inspector, world, &ctx, clock.get(), events, tick).0;
        }
        texts
    };
    // Picks `to` from the projection's choice, which shows `from`, the variant it is.
    let switch = |inspector: &mut Inspector, world: &mut World, from: &str, to: &str| {
        let texts = run(inspector, world, Vec::new(), true);
        let choice = click(Primary, find(&texts, "projection", from).center());
        let texts = run(inspector, world, choice, true);
        run(
            inspector,
            world,
            click(Primary, in_menu(&ctx, &texts, to)),
            true,
        );
    };
    let projection =
        |world: &World, entity| world.get::<Camera>(entity).expect("a camera").projection;
    let orthographic = camera(4.0).projection;
    let perspective = |fov| Projection::Perspective { fov, near: 0.01 };
    let defaults = perspective(std::f32::consts::FRAC_PI_4);
    switch(&mut inspector, &mut world, "Orthographic", "Perspective");
    assert_eq!(projection(&world, first), defaults);
    switch(&mut inspector, &mut world, "Perspective", "Orthographic");
    assert_eq!(projection(&world, first), orthographic);
    switch(&mut inspector, &mut world, "Orthographic", "Perspective");
    assert_eq!(projection(&world, first), defaults);
    // Its field of view typed in; then switched away from, and back to.
    let texts = run(&mut inspector, &mut world, Vec::new(), true);
    let fov = beside(&texts, find(&texts, "projection", "fov"));
    run(&mut inspector, &mut world, click(Primary, fov), true);
    let typed = vec![egui::Event::Text("60".to_owned())];
    run(&mut inspector, &mut world, typed, true);
    run(&mut inspector, &mut world, enter(), true);
    // As a field in degrees takes what's typed.
    let sixty = perspective((60.0 / (180.0 / PI)) as f32);
    assert_eq!(projection(&world, first), sixty);
    switch(&mut inspector, &mut world, "Perspective", "Orthographic");
    assert_eq!(projection(&world, first), orthographic);
    switch(&mut inspector, &mut world, "Orthographic", "Perspective");
    assert_eq!(projection(&world, first), sixty);
    inspector.pick(id(&world, second));
    switch(&mut inspector, &mut world, "Orthographic", "Perspective");
    assert_eq!(projection(&world, second), defaults);
}

/// A variant that holds what's opaque, which no value makes, shows disabled in its enum's
/// choice, saying why on hover, and isn't switched to.
#[test]
fn a_variant_that_holds_whats_opaque_shows_disabled() {
    let mut world = World::default();
    let entity = world.spawn(everything());
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let mut time = 0.0;
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        let mut texts = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            time += 1.0;
            texts = frame(inspector, world, &ctx, time, events).0;
        }
        texts
    };
    let texts = run(&mut inspector, &mut world, Vec::new());
    // Its form's choice, which shows the variant it is.
    let choice = click(Primary, find(&texts, "form", "Point").center());
    let texts = run(&mut inspector, &mut world, choice);
    let held = in_menu(&ctx, &texts, "Held");
    let hovered = vec![egui::Event::PointerMoved(held)];
    let texts = run(&mut inspector, &mut world, hovered);
    let why = "at `0`: it's opaque: reflection keeps one, but doesn't make one";
    assert!(texts.iter().any(|(text, _)| text == why), "{texts:?}");
    run(&mut inspector, &mut world, click(Primary, held));
    let form = &world.get::<Everything>(entity).expect("everything").form;
    assert_eq!(form, &Form::Point);
    assert_eq!(inspector.log.edits().len(), 0);
}

/// Each axis's button sets its component alone to the default's, a click an edit: a
/// translation's x to 0, its type's; a scale's y to 1, the transform's; the x of a sprite's
/// rect's bottom right to 1, the sprite's; and a rotation's angle about z to the default
/// rotation's, keeping the others.
#[test]
fn axis_buttons_set_their_component_to_its_default() {
    let mut world = World::default();
    let transform = Transform {
        translation: Vec3::new(1.0, 2.0, 3.0),
        rotation: Quat::from_euler(EulerRot::ZYX, 0.3, 0.2, 0.1),
        scale: Vec3::new(2.0, 3.0, 4.0),
    };
    let sprite = Sprite {
        rect: [Vec2::new(0.1, 0.2), Vec2::new(0.5, 0.6)],
        ..Sprite::default()
    };
    let entity = world.spawn((transform, sprite));
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let mut time = 0.0;
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        let mut texts = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            time += 1.0;
            texts = frame(inspector, world, &ctx, time, events).0;
        }
        texts
    };
    let buttons: [&[&str]; 4] = [
        &["translation", "X"],
        &["scale", "Y"],
        &["rect", "[1]", "X"],
        &["rotation", "Z"],
    ];
    for button in buttons {
        let texts = run(&mut inspector, &mut world, Vec::new());
        let at = find_after(&texts, button).center();
        run(&mut inspector, &mut world, click(Primary, at));
    }
    let set = world.get::<Transform>(entity).expect("a transform");
    assert_eq!(set.translation, Vec3::new(0.0, 2.0, 3.0));
    assert_eq!(set.scale, Vec3::new(2.0, 1.0, 4.0));
    let (z, y, x) = set.rotation.to_euler(EulerRot::ZYX);
    let near = |a: f32, b: f32| (a - b).abs() < 1e-5;
    assert!(
        near(z, 0.0) && near(y, 0.2) && near(x, 0.1),
        "{x}, {y}, {z}"
    );
    let rect = world.get::<Sprite>(entity).expect("a sprite").rect;
    assert_eq!(rect, [Vec2::new(0.1, 0.2), Vec2::new(1.0, 0.6)]);
    assert_eq!(inspector.log.edits().len(), 4);
}

/// The axis buttons' labels draw in the family [`crate::BOLD`] names where the context binds
/// it, as the engine's does, and else in egui's proportional one, as in a context with egui's
/// own fonts, or a game's: never in a family that isn't bound.
#[test]
fn axis_buttons_are_bold_where_the_family_is_bound() {
    /// Each text `shapes` draw, by its sections, and its family.
    fn families(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::FontFamily)> {
        let mut found = Vec::new();
        for shape in crate::testing::painted(shapes) {
            if let egui::Shape::Text(text) = shape {
                for section in &text.galley.job.sections {
                    let bytes = &section.byte_range;
                    let part = &text.galley.text()[bytes.start.0..bytes.end.0];
                    found.push((part.to_owned(), section.format.font_id.family.clone()));
                }
            }
        }
        found
    }
    let mut world = World::default();
    let entity = world.spawn(Transform::IDENTITY);
    let bold = egui::FontFamily::Name(crate::BOLD.into());
    for (ctx, family) in [
        (crate::egui_context(), bold),
        (egui::Context::default(), egui::FontFamily::Proportional),
    ] {
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        let mut found = Vec::new();
        for time in 0..3 {
            let time = f64::from(time);
            let (shapes, _) =
                frame_shapes(&mut inspector, &mut world, &ctx, time, Vec::new(), true);
            found = families(&shapes);
        }
        for axis in ["X", "Y", "Z"] {
            let drawn = found.iter().filter(|(text, _)| text == axis);
            let drawn_in: Vec<&egui::FontFamily> = drawn.map(|(_, family)| family).collect();
            // A translation's, a rotation's and a scale's.
            assert_eq!(drawn_in, [&family; 3], "{axis}");
        }
    }
}

/// A drag of an axis sets that component alone, at its own path, so that a system moving
/// another at the ticks between the frames keeps what it moved; the drag is one edit, whose
/// undo takes back that component alone.
#[test]
fn an_axis_drag_sets_its_component_alone() {
    let mut world = World::default();
    let entity = world.spawn(Transform::IDENTITY);
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let mut time = 0.0;
    let mut run = |inspector: &mut Inspector, world: &mut World, events, tick| {
        time += 1.0;
        frame_ticking(inspector, world, &ctx, time, events, tick)
    };
    let mut texts = Vec::new();
    for _ in 0..3 {
        texts = run(&mut inspector, &mut world, Vec::new(), true).0;
    }
    let at = beside(&texts, find_after(&texts, &["translation", "X"]));
    let translation = |world: &World| {
        world
            .get::<Transform>(entity)
            .expect("a transform")
            .translation
    };
    let mut sets = 0;
    for events in slide(at) {
        sets += run(&mut inspector, &mut world, events, false).1;
        // The system, at the tick, after the frame drew it.
        let moved = world.get_mut::<Transform>(entity).expect("a transform");
        moved.translation.y += 1.0;
        Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
    }
    let dragged = translation(&world);
    assert!(sets > 1 && dragged.x != 0.0, "{sets} sets, to {dragged}");
    assert_eq!(dragged.y, 5.0);
    assert_eq!(inspector.log.edits().len(), 1, "one edit");
    run(
        &mut inspector,
        &mut world,
        shortcut(egui::Key::Z, false),
        true,
    );
    assert_eq!(translation(&world), Vec3::new(0.0, 5.0, 0.0));
}

/// Sets of a vector and of its components that wait for a tick draw it as they'll make it, in
/// the order they were made, as at frame rates above the simulation's; and the tick makes it
/// so.
#[test]
fn sets_of_a_vector_and_of_its_components_draw_in_order() {
    let mut world = World::default();
    let entity = world.spawn(Transform::IDENTITY);
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let set = |path: &str, value| Edit {
        component: "Transform".to_owned(),
        path: path.to_owned(),
        value,
    };
    let whole = |x, y, z| Value::Vec3(Vec3::new(x, y, z));
    let mut time = 0.0;
    for (sets, made) in [
        (
            vec![
                set("translation", whole(1.5, 2.5, 3.5)),
                set("translation.x", Value::F32(7.5)),
            ],
            Vec3::new(7.5, 2.5, 3.5),
        ),
        (
            vec![
                set("translation.y", Value::F32(8.5)),
                set("translation", whole(4.5, 5.5, 6.5)),
            ],
            Vec3::new(4.5, 5.5, 6.5),
        ),
    ] {
        inspector.queue(Pass {
            sets,
            ..Pass::default()
        });
        let mut texts = Vec::new();
        for _ in 0..3 {
            time += 1.0;
            let drawn = frame_ticking(&mut inspector, &mut world, &ctx, time, Vec::new(), false);
            texts = drawn.0;
        }
        // What each axis's drag shows, beside its button, after the figure space in place of a
        // sign.
        let shown = ["X", "Y", "Z"].map(|axis| {
            let (text, _) = next_to(&texts, find_after(&texts, &["translation", axis]));
            text.trim_start_matches('\u{2007}')
                .parse::<f32>()
                .unwrap_or_else(|_| panic!("a number: {text}"))
        });
        assert_eq!(Vec3::from(shown), made, "{texts:?}");
        Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
        let held = world.get::<Transform>(entity).expect("a transform");
        assert_eq!(held.translation, made);
    }
}

/// A vector's numbers keep their places as they change: each axis's field takes a share of the
/// row, however wide its number, and a number, centered in its field, keeps its width as its
/// digits change and its sign comes and goes, a minus sign or a figure space in its place; one
/// that shows as zero shows no minus.
#[test]
fn a_vectors_numbers_hold_still_as_they_change() {
    let mut world = World::default();
    let entity = world.spawn(Transform::IDENTITY);
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let mut time = 0.0;
    let mut numbers = Vec::new();
    let mut ys = Vec::new();
    for (x, shown) in [
        (0.0, "\u{2007}0.00"),
        (-1.5, "\u{2212}1.50"),
        // Ending in the same digit as the first two, so that where their ink ends compares
        // their widths.
        (9.5, "\u{2007}9.50"),
        (-12345.5, "\u{2212}12345.50"),
        (-1e-9, "\u{2007}0.00"),
    ] {
        let transform = world.get_mut::<Transform>(entity).expect("a transform");
        transform.translation.x = x;
        let mut texts = Vec::new();
        for _ in 0..3 {
            time += 1.0;
            texts = frame(&mut inspector, &mut world, &ctx, time, Vec::new()).0;
        }
        let (number, at) = next_to(&texts, find_after(&texts, &["translation", "X"]));
        assert_eq!(number, shown, "{x}");
        // Where its ink ends: where it starts leaves out a figure space's blank.
        numbers.push(at.max.x);
        ys.push(find_after(&texts, &["translation", "Y"]).min.x);
    }
    // The first three alike in width, and so in place.
    assert!(
        numbers[..3].iter().all(|&end| end == numbers[0]),
        "{numbers:?}"
    );
    assert!(ys.iter().all(|&y| y == ys[0]), "{ys:?}");
}

/// In an Inspector panel as narrow as the editor's first is, cut off at its edges in its scroll
/// area as the dock draws it, a vector's axes share what shows of their row: the last ends
/// within the panel, short of the scroll bar, and stays where it is whichever number the
/// first shows.
#[test]
fn a_vectors_axes_fit_a_panel_as_narrow_as_the_editors() {
    let mut world = World::default();
    let turned = Transform {
        translation: Vec3::new(-1.875, 0.0, -1.25),
        ..Transform::IDENTITY
    };
    let entity = world.spawn(turned);
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0));
    let panel = egui::Rect::from_min_max(egui::pos2(1280.0, 0.0), egui::pos2(1600.0, 900.0));
    let scroll = &ctx.global_style().spacing.scroll;
    let edge = panel.max.x - scroll.bar_width - scroll.bar_outer_margin;
    let mut time = 0.0;
    let mut zs = Vec::new();
    for x in [-1.875, 0.0] {
        let transform = world.get_mut::<Transform>(entity).expect("a transform");
        transform.translation.x = x;
        let mut texts = Vec::new();
        for _ in 0..4 {
            time += 1.0;
            let input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                ..egui::RawInput::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let mut pass = Pass::default();
                let inspect = egui::UiBuilder::new().id_salt("Inspector").max_rect(panel);
                ui.scope_builder(inspect, |ui| {
                    ui.set_clip_rect(panel);
                    let scroll = egui::ScrollArea::vertical();
                    let pending = inspector.pending();
                    scroll.show(ui, |ui| inspector.inspect(&world, ui, &mut pass, &pending));
                });
                inspector.queue(pass);
            });
            output.textures_delta.clear();
            texts = texts_in(&output.shapes);
        }
        let z = find_after(&texts, &["translation", "Z"]);
        let (number, at) = next_to(&texts, z);
        assert_eq!(number, "\u{2212}1.25");
        assert!(at.max.x < edge, "{at:?} past {edge}: {texts:?}");
        zs.push(z);
    }
    // Where Z is, whichever number X's field shows.
    assert_eq!(zs[0], zs[1]);
}

/// A value refused for a vector's component shows under the vector's row.
#[test]
fn a_value_refused_for_a_component_shows_under_its_vector() {
    let mut world = World::default();
    let entity = world.spawn(everything());
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let refused = Edit {
        component: "Everything".to_owned(),
        path: "offset.x".to_owned(),
        value: Value::UInt(1),
    };
    inspector.apply(&mut world, inspector.picked, refused);
    let says = format!(
        "can't set `Everything.offset.x` of entity {}: expected F32, got UInt",
        id(&world, entity)
    );
    let mut texts = Vec::new();
    for time in 0..3 {
        let drawn = frame(
            &mut inspector,
            &mut world,
            &ctx,
            f64::from(time),
            Vec::new(),
        );
        texts = drawn.0;
    }
    let offset = find(&texts, "Everything", "offset");
    let message = find(&texts, "offset", &says);
    assert!(message.min.y > offset.max.y, "{texts:?}");
}

/// The names of the commands `inspector` has queued, as a recording reads them.
fn queued_names(inspector: &Inspector) -> Vec<&'static str> {
    let queue = &inspector.queued;
    let names = (0..queue.len()).map(|i| match reflect::variant(&queue.value(i)) {
        Ok(("Set", _)) => "Set",
        Ok(("Seal", _)) => "Seal",
        Ok(("Undo", _)) => "Undo",
        Ok(("Create", _)) => "Create",
        other => panic!("not a set, a seal, an undo or a create: {other:?}"),
    });
    names.collect()
}

/// A gesture of the link's is one edit, whatever the panels' input says meanwhile, which a drag
/// in the game's view doesn't come through: across passes whose cues let go, press and
/// release, its sets queue as `Set`s, its seal as a `Seal`, and one undo takes it back. Its
/// first set ends an edit of the panels' going on, and a set of the panels' ends it.
#[test]
fn a_gesture_of_the_links_is_one_edit_whatever_the_panels_cues() {
    let mut world = World::default();
    let entity = world.spawn((Transform::IDENTITY, everything()));
    let picked = id(&world, entity);
    let mut inspector = Inspector::default();
    inspector.pick(picked);
    let link = inspector.link();
    let at = |x: f32| Value::Vec3(Vec3::new(x, 0.0, 0.0));
    let count = |n| Edit {
        component: "Everything".to_owned(),
        path: "count".to_owned(),
        value: Value::UInt(n),
    };
    let held = || Cues {
        held: true,
        ..Cues::default()
    };
    // A number being typed into the panel.
    inspector.queue(Pass {
        sets: vec![count(7)],
        cues: held(),
        ..Pass::default()
    });
    let cues = [
        Cues::default(),
        Cues {
            pressed: true,
            held: true,
            ..Cues::default()
        },
        Cues {
            released: true,
            ..Cues::default()
        },
    ];
    for (x, cues) in [1.0, 2.0, 3.0].into_iter().zip(cues) {
        link.set(picked, "Transform", "translation", at(x));
        inspector.queue(Pass {
            cues,
            ..Pass::default()
        });
    }
    link.seal();
    inspector.queue(Pass::default());
    let names = ["Set", "Seal", "Set", "Set", "Set", "Seal"];
    assert_eq!(queued_names(&inspector), names);
    Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
    let translation = |world: &World| world.get::<Transform>(entity).map(|at| at.translation);
    assert_eq!(translation(&world), Some(Vec3::new(3.0, 0.0, 0.0)));
    assert_eq!(
        inspector.log.edits().len(),
        2,
        "the count's, and the link's"
    );
    make(
        &mut inspector,
        &mut world,
        Pass {
            chosen: Some(Step::Undo),
            ..Pass::default()
        },
    );
    assert_eq!(translation(&world), Some(Vec3::ZERO));
    let typed = world.get::<Everything>(entity).map(|all| all.count);
    assert_eq!(typed, Some(7));
    // The panels' set ends a gesture of the link's going on.
    link.set(picked, "Transform", "translation", at(4.0));
    inspector.queue(Pass::default());
    inspector.queue(Pass {
        sets: vec![count(9)],
        cues: held(),
        ..Pass::default()
    });
    assert_eq!(queued_names(&inspector), ["Set", "Seal", "Set"]);
}

/// A set through the link is its entity's, whatever's picked as it's queued: here one picked
/// through the link in the same pass.
#[test]
fn a_set_through_the_link_is_its_entitys_whatever_is_picked() {
    let mut world = World::default();
    let [a, b] = [(); 2].map(|()| world.spawn(everything()));
    let [a_id, b_id] = [a, b].map(|entity| id(&world, entity));
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let link = inspector.link();
    link.set(a_id, "Everything", "count", Value::UInt(7));
    link.pick(Some(b_id));
    frame(&mut inspector, &mut world, &ctx, 1.0, Vec::new());
    let count = |entity| world.get::<Everything>(entity).map(|all| all.count);
    assert_eq!([count(a), count(b)], [Some(7), Some(42)]);
    assert_eq!((inspector.picked, link.picked()), (Some(b_id), Some(b_id)));
}

/// The link's sets wait from the frame they're made until a tick has made them: those of a
/// drag and the undo that follows them in one queue leave the world as it was, and nothing
/// waiting; and a refused one is dropped.
#[test]
fn the_links_sets_wait_until_a_tick_makes_them() {
    let mut world = World::default();
    let entity = world.spawn(Transform::IDENTITY);
    let picked = id(&world, entity);
    let mut inspector = Inspector::default();
    let link = inspector.link();
    let at = |x: f32| Value::Vec3(Vec3::new(x, 0.0, 0.0));
    assert!(!link.waiting());
    link.set(picked, "Transform", "translation", at(1.0));
    assert!(link.waiting(), "from the set");
    inspector.queue(Pass::default());
    link.set(picked, "Transform", "translation", at(2.0));
    inspector.queue(Pass {
        chosen: Some(Step::Undo),
        ..Pass::default()
    });
    assert!(link.waiting(), "until a tick");
    Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
    assert!(!link.waiting());
    assert_eq!(world.get::<Transform>(entity), Some(&Transform::IDENTITY));
    // A rotation that isn't a unit, which the world refuses.
    let skewed = Value::Quat(Quat::from_xyzw(0.0, 0.0, 2.0, 1.0));
    link.set(picked, "Transform", "rotation", skewed);
    make(&mut inspector, &mut world, Pass::default());
    assert!(!link.waiting());
    assert_eq!(world.get::<Transform>(entity), Some(&Transform::IDENTITY));
}

/// The Inspector panel draws a field as the link's set waiting for a tick will make it, from
/// the frame the set is made, after the sets queued before it.
#[test]
fn the_panel_draws_the_links_sets_from_the_frame_theyre_made() {
    let mut world = World::default();
    let entity = world.spawn(everything());
    let picked = id(&world, entity);
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(picked);
    let link = inspector.link();
    for time in 0..3 {
        frame(
            &mut inspector,
            &mut world,
            &ctx,
            f64::from(time),
            Vec::new(),
        );
    }
    let count = |n| Edit {
        component: "Everything".to_owned(),
        path: "count".to_owned(),
        value: Value::UInt(n),
    };
    inspector.queue(Pass {
        sets: vec![count(9)],
        ..Pass::default()
    });
    link.set(picked, "Everything", "count", Value::UInt(7));
    for time in 3..5 {
        let time = f64::from(time);
        let (texts, _) = frame_ticking(&mut inspector, &mut world, &ctx, time, Vec::new(), false);
        let (shown, _) = next_to(&texts, find(&texts, "Everything", "count"));
        assert_eq!(shown, "7", "{time}");
    }
    let held = world.get::<Everything>(entity).map(|all| all.count);
    assert_eq!(held, Some(42), "until a tick");
}

/// The link picks a living entity as the next pass starts, opening the rows above its own, and
/// unpicks for a despawned one's ID, one no entity had, or none; without a pick asked for, the
/// pick stays.
#[test]
fn the_link_picks_a_living_entity_and_unpicks_for_any_other() {
    let mut world = World::default();
    let [parent, child, gone] = ["parent", "child", "gone"].map(|name| world.spawn(called(name)));
    world.set_parent(child, Some(parent)).expect("no cycle");
    let [parent_id, child_id, gone_id] = [parent, child, gone].map(|entity| id(&world, entity));
    world.despawn(gone).expect("alive");
    let unknown: EntityId = "0123456789abcdef".parse().expect("an ID");
    assert_eq!(world.entity(unknown), None);
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let link = inspector.link();
    let mut time = 0.0;
    let mut pass = |inspector: &mut Inspector, world: &mut World| {
        time += 1.0;
        frame(inspector, world, &ctx, time, Vec::new());
    };
    link.pick(Some(child_id));
    pass(&mut inspector, &mut world);
    assert_eq!(
        (inspector.picked, link.picked()),
        (Some(child_id), Some(child_id))
    );
    assert!(inspector.expanded.contains(&parent_id));
    for asked in [Some(gone_id), Some(unknown), None] {
        inspector.pick(child_id);
        link.pick(asked);
        pass(&mut inspector, &mut world);
        assert_eq!((inspector.picked, link.picked()), (None, None), "{asked:?}");
    }
    inspector.pick(child_id);
    pass(&mut inspector, &mut world);
    assert_eq!(link.picked(), Some(child_id));
}

/// A world restored, which is another, drops the link's sets queued for the world before and
/// those no pass has taken yet, and the pick it asked for.
#[test]
fn another_world_drops_what_the_link_asked_of_the_one_before() {
    let mut world = World::default();
    let entity = world.spawn(Transform::IDENTITY);
    let picked = id(&world, entity);
    let snapshot = world.clone();
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let link = inspector.link();
    // So that it knows the world.
    Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
    let at = |x: f32| Value::Vec3(Vec3::new(x, 0.0, 0.0));
    link.set(picked, "Transform", "translation", at(1.0));
    inspector.queue(Pass::default());
    link.set(picked, "Transform", "translation", at(2.0));
    link.pick(Some(picked));
    world.clone_from(&snapshot);
    Layer::tick(&mut inspector, &mut world, Tick { number: 1 });
    assert!(!link.waiting() && link.pending().is_empty());
    assert_eq!(world.get::<Transform>(entity), Some(&Transform::IDENTITY));
    frame(&mut inspector, &mut world, &ctx, 1.0, Vec::new());
    assert_eq!((inspector.picked, link.picked()), (None, None));
}

/// The link counts the undos and redos the passes ask for, from the Edit menu or the keys.
#[test]
fn the_link_counts_the_undos_and_redos_asked_for() {
    let mut world = World::default();
    let entity = world.spawn(everything());
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let link = inspector.link();
    assert_eq!(link.stepped(), 0);
    inspector.queue(Pass {
        chosen: Some(Step::Undo),
        ..Pass::default()
    });
    assert_eq!(link.stepped(), 1);
    inspector.queue(Pass {
        cues: Cues {
            step: Some(Step::Redo),
            ..Cues::default()
        },
        ..Pass::default()
    });
    assert_eq!(link.stepped(), 2);
    inspector.queue(Pass::default());
    assert_eq!(link.stepped(), 2, "a pass without a step");
    frame(
        &mut inspector,
        &mut world,
        &ctx,
        1.0,
        shortcut(egui::Key::Z, false),
    );
    assert_eq!(link.stepped(), 3);
}

/// Between passes the link keeps the latest value of each field of its gesture alone, and a
/// seal keeps those before it apart; a second seal ends nothing more.
#[test]
fn the_link_keeps_the_latest_value_of_each_field_between_passes() {
    let link = Inspector::default().link();
    let [a, b]: [EntityId; 2] =
        ["0123456789abcdef", "fedcba9876543210"].map(|id| id.parse().expect("an ID"));
    let at = |x: f32| Value::Vec3(Vec3::new(x, 0.0, 0.0));
    for x in [1.0, 2.0] {
        link.set(a, "Transform", "translation", at(x));
        link.set(b, "Transform", "translation", at(x));
    }
    link.set(a, "Transform", "scale", at(5.0));
    link.seal();
    link.seal();
    link.set(a, "Transform", "translation", at(3.0));
    let set = |id, path: &str, x| Some((id, "Transform".to_owned(), path.to_owned(), at(x)));
    assert_eq!(
        link.pending(),
        [
            set(a, "translation", 2.0),
            set(b, "translation", 2.0),
            set(a, "scale", 5.0),
            None,
            set(a, "translation", 3.0)
        ]
    );
}

/// A debug build refuses a set through the link of the component whole, or inside a field of
/// another kind than a vector, which the panel couldn't draw as it'll be.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "a link sets a field of a component's own")]
fn a_link_refuses_a_set_inside_another_kind_of_field() {
    let link = Inspector::default().link();
    let id: EntityId = "0123456789abcdef".parse().expect("an ID");
    link.set(id, "Everything", "maybe.level", Value::UInt(1));
}

/// A debug build refuses, as the Inspector's next pass takes it, a set through the link of a
/// field whose insides the panels set at paths of their own, as a struct's, which the panel
/// couldn't draw beside theirs.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "got `inner` of `Everything`")]
fn a_link_refuses_a_set_of_a_struct_whole() {
    let mut world = World::default();
    let entity = world.spawn(everything());
    let mut inspector = Inspector::default();
    let link = inspector.link();
    let picked = id(&world, entity);
    link.set(picked, "Everything", "inner", Value::Bool(false));
    inspector.follow(&world);
}

/// A field's latest set through the link goes after the sets made since its last, so that a
/// vector set whole and one of its components still set in the order they came.
#[test]
fn the_links_latest_set_of_a_field_comes_after_those_made_since() {
    let link = Inspector::default().link();
    let id: EntityId = "0123456789abcdef".parse().expect("an ID");
    let whole = |x: f32| Value::Vec3(Vec3::splat(x));
    link.set(id, "Transform", "translation", whole(1.0));
    link.set(id, "Transform", "translation.x", Value::F32(5.0));
    link.set(id, "Transform", "translation", whole(2.0));
    let set = |path: &str, value| Some((id, "Transform".to_owned(), path.to_owned(), value));
    let expected = [
        set("translation.x", Value::F32(5.0)),
        set("translation", whole(2.0)),
    ];
    assert_eq!(link.pending(), expected);
}

/// A create through the link queues after the link's sets before it, ending their gesture, and
/// makes a root at the next tick, which the Inspector picks; one undo takes it back alone. One
/// the world refuses, of a type it hasn't, shows above the components, as the menu's does.
#[test]
fn a_create_through_the_link_makes_a_root_and_picks_it() {
    let mut world = World::default();
    world.register::<Name>().expect("registered");
    let entity = world.spawn(Transform::IDENTITY);
    let picked = id(&world, entity);
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let link = inspector.link();
    let at = Value::Vec3(Vec3::new(1.0, 0.0, 0.0));
    link.set(picked, "Transform", "translation", at);
    let made = Transform {
        translation: Vec3::new(0.0, 2.0, 0.0),
        ..Transform::IDENTITY
    };
    let components: Vec<(Ident, Value)> = vec![
        ("Name".into(), called("Rock").to_value()),
        ("Transform".into(), made.to_value()),
    ];
    link.create(components);
    assert!(!link.idle(), "until a pass takes it");
    assert_eq!(link.creates().len(), 1);
    inspector.queue(Pass::default());
    assert_eq!(queued_names(&inspector), ["Set", "Create"]);
    assert!(link.creates().is_empty());
    Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
    let mut named = world.query::<(Entity, &Name)>();
    let rock = named.find_map(|(entity, name)| (name.0 == "Rock").then_some(entity));
    let rock = rock.expect("made");
    assert_eq!(world.get::<Transform>(rock), Some(&made));
    assert_eq!(world.parent(rock).expect("alive"), None, "a root");
    assert_eq!(inspector.picked, Some(id(&world, rock)));
    make(
        &mut inspector,
        &mut world,
        Pass {
            chosen: Some(Step::Undo),
            ..Pass::default()
        },
    );
    assert_eq!(world.entity(id(&world, entity)), Some(entity));
    let translation = world.get::<Transform>(entity).map(|at| at.translation);
    assert_eq!(translation, Some(Vec3::X), "the set stays");
    assert_eq!(
        world.entities(None).expect("all").len(),
        1,
        "the create undone"
    );
    link.create(vec![("Nothing".into(), Value::Bool(true))]);
    // Refused at the first frame's tick, and shown from the next.
    frame(&mut inspector, &mut world, &ctx, 1.0, Vec::new());
    let (texts, _) = frame(&mut inspector, &mut world, &ctx, 2.0, Vec::new());
    let said = inspector.not_done.clone().expect("refused");
    assert!(texts.iter().any(|(text, _)| *text == said), "{texts:?}");
}

/// A set through the link of a component the picked entity hasn't, whose field the panel doesn't
/// draw, is refused above its components, as an operation is, rather than beside a field.
#[test]
fn a_links_set_of_a_component_the_entity_hasnt_shows_above_its_components() {
    let mut world = World::default();
    world.register::<Sprite>().expect("registered");
    let entity = world.spawn(Transform::IDENTITY);
    let picked = id(&world, entity);
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(picked);
    let link = inspector.link();
    link.set(picked, "Sprite", "color", Value::Vec4(Vec4::ONE));
    // Refused at the first frame's tick, and shown from the next.
    frame(&mut inspector, &mut world, &ctx, 1.0, Vec::new());
    let (texts, _) = frame(&mut inspector, &mut world, &ctx, 2.0, Vec::new());
    assert!(inspector.refusals.is_empty());
    let said = inspector.not_done.clone().expect("refused");
    assert!(said.contains("has no `Sprite`"), "{said}");
    let (_, under) = texts.iter().find(|(text, _)| *text == said).expect("shown");
    let (_, heading) = (texts.iter())
        .find(|(text, _)| *text == format!("Entity {picked}"))
        .expect("the heading");
    assert!(under.max.y <= heading.min.y, "above the components");
}

mod remote {
    use std::thread;
    use std::time::{Duration, Instant};

    use gg_api::Session;

    use super::*;
    use crate::api::Api;
    use crate::layer::Stack;
    use crate::main_loop::Stepper;

    /// How long a test waits for what should come in a few frames, past which it fails rather than
    /// hangs.
    const DEADLINE: Duration = Duration::from_secs(20);

    /// A game whose world its API serves on this thread, as its frames would.
    struct Game {
        api: Api,
        cx: Context,
        stack: Stack,
        stepper: Stepper,
    }

    impl Game {
        fn frame(&mut self) {
            self.api
                .serve(&mut self.cx, &mut self.stack, &mut self.stepper);
            self.stepper.step(Tick::DT, |_| true, || Duration::ZERO);
        }
    }

    /// A frame of a remote Inspector's panels, side by side as [`panels`] draws them, as its `ui`
    /// takes in the game's answers before them and sends the pass after them, then tells its link
    /// the pick and its place; `give` adds to the pass before it's sent. Returns the frame's texts.
    fn frame(
        inspector: &mut Inspector,
        ctx: &egui::Context,
        time: f64,
        give: impl FnOnce(&mut Pass),
    ) -> Vec<String> {
        let mut remote = inspector.remote.take().expect("a remote Inspector");
        remote.update(inspector);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 4000.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(time),
            ..egui::RawInput::default()
        };
        let mut give = Some(give);
        let mut output = ctx.run_ui(input, |ui| {
            let mut pass = Pass::default();
            inspector.follow(&remote);
            let (left, right) = ui.max_rect().split_left_right_at_fraction(0.5);
            let entities = egui::UiBuilder::new().id_salt("Entities").max_rect(left);
            ui.scope_builder(entities, |ui| inspector.entities(&remote, ui, &mut pass));
            let pending = remote.pending(inspector.picked);
            let inspect = egui::UiBuilder::new().id_salt("Inspector").max_rect(right);
            ui.scope_builder(inspect, |ui| {
                inspector.inspect(&remote, ui, &mut pass, &pending);
            });
            if let Some(give) = give.take() {
                give(&mut pass);
            }
            remote.send(pass, inspector);
        });
        output.textures_delta.clear();
        let linked = &mut *inspector.link.borrow_mut();
        linked.picked = inspector.picked;
        linked.placed = remote.placed(inspector.picked);
        inspector.remote = Some(remote);
        texts_in(&output.shapes)
            .into_iter()
            .map(|(text, _)| text)
            .collect()
    }

    /// Runs frames of `game` and of `inspector`, the first given `give`, until `until` holds of
    /// the Inspector's texts and the game; returns the texts.
    fn until(
        inspector: &mut Inspector,
        game: &mut Game,
        ctx: &egui::Context,
        time: &mut f64,
        give: impl FnOnce(&mut Pass),
        until: impl Fn(&[String], &Game) -> bool,
    ) -> Vec<String> {
        let start = Instant::now();
        let mut give = Some(give);
        loop {
            assert!(start.elapsed() < DEADLINE, "it never came");
            game.frame();
            *time += 1.0 / 60.0;
            let texts = frame(inspector, ctx, *time, |pass| {
                if let Some(give) = give.take() {
                    give(pass);
                }
            });
            if until(&texts, game) {
                return texts;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn x(game: &Game, entity: Entity) -> f32 {
        let transform = game.cx.world().get::<Transform>(entity);
        transform.expect("a transform").translation.x
    }

    /// A remote Inspector lists a game's entities through its API, as they come and go, shows the
    /// one picked, its components in the order they registered, sets a field in a gesture, which
    /// one undo takes back, its Edit menu naming the edit, and creates an entity, which it picks,
    /// as it does one its link creates; and a set through the link of a component the entity
    /// hasn't is refused above its components.
    #[test]
    fn a_remote_inspector_shows_and_edits_a_game_through_the_api() {
        let app = "Remote Inspector";
        let events: &'static gg_core::log::Events = Box::leak(Box::default());
        let api = Api::start(app, &[], None, events, None, None).expect("the API starts");
        let file = Session::file_name(app, std::process::id());
        let path = crate::beside_exe("sessions").join(file);
        let text = std::fs::read(path).expect("the session file");
        let session: Session = serde_json::from_slice(&text).expect("a session");
        let mut cx = Context::seeded(6);
        let world = cx.world_mut();
        let hero = world.spawn((Transform::IDENTITY, called("Hero")));
        let kid = world.spawn((Transform::IDENTITY, called("Kid")));
        world.set_parent(kid, Some(hero)).expect("no cycle");
        let (hero_id, kid_id) = (id(world, hero), id(world, kid));
        let mut stepper = Stepper::default();
        stepper.pause();
        let mut game = Game {
            api,
            cx,
            stack: Stack::default(),
            stepper,
        };
        let mut inspector = Inspector::remote("editor");
        inspector.link().reach(Some(session));
        let (ctx, mut time) = (crate::egui_context(), 0.0);
        let has = |texts: &[String], text: &str| texts.iter().any(|shown| shown == text);
        // The tree, its root closed.
        let texts = until(
            &mut inspector,
            &mut game,
            &ctx,
            &mut time,
            |_| {},
            |texts, _| has(texts, "Hero") && has(texts, "2 entities"),
        );
        assert!(!has(&texts, "Kid"), "{texts:?}");
        // Picked through the link, its components in the order they registered.
        inspector.link().pick(Some(hero_id));
        let heading = format!("Hero, entity {hero_id}");
        let texts = until(
            &mut inspector,
            &mut game,
            &ctx,
            &mut time,
            |_| {},
            |texts, _| has(texts, &heading) && has(texts, "Transform"),
        );
        assert!(has(&texts, &format!("Children: {kid_id}")), "{texts:?}");
        // Placed by the read that shows it, in the space of the world, by the Inspector's first
        // reach.
        let placed = inspector.link().placed().expect("placed");
        let expected = Placed {
            id: hero_id,
            local: Transform::IDENTITY,
            space: crate::Pose::IDENTITY,
            generation: 1,
        };
        assert_eq!(placed, expected);
        let own = game.cx.world().components_of(hero).expect("alive");
        let own: Vec<&str> = own.collect();
        let drawn: Vec<&str> = (texts.iter())
            .filter_map(|text| own.iter().find(|&&name| name == text).copied())
            .collect();
        assert_eq!(drawn, own, "in the order they registered");
        // Two sets in one gesture, then its end.
        let set = |x: f32| Edit {
            component: "Transform".to_owned(),
            path: "translation.x".to_owned(),
            value: Value::F32(x),
        };
        for x in [5.0, 6.0] {
            game.frame();
            time += 1.0 / 60.0;
            frame(&mut inspector, &ctx, time, |pass| {
                pass.sets.push(set(x));
                pass.cues.held = true;
            });
        }
        let edit = format!("set `Transform.translation.x` of {hero_id}");
        until(
            &mut inspector,
            &mut game,
            &ctx,
            &mut time,
            |pass| {
                pass.cues.released = true;
            },
            |_, game| x(game, hero) == 6.0,
        );
        until(
            &mut inspector,
            &mut game,
            &ctx,
            &mut time,
            |_| {},
            |_, _| true,
        );
        let labels = |inspector: &Inspector| {
            let remote = inspector.remote.as_ref().expect("remote");
            remote.pending(None).undo
        };
        let start = Instant::now();
        while labels(&inspector).as_deref() != Some(edit.as_str()) {
            assert!(
                start.elapsed() < DEADLINE,
                "the Edit menu never named the edit"
            );
            until(
                &mut inspector,
                &mut game,
                &ctx,
                &mut time,
                |_| {},
                |_, _| true,
            );
        }
        // One undo takes the gesture back whole.
        until(
            &mut inspector,
            &mut game,
            &ctx,
            &mut time,
            |pass| {
                pass.chosen = Some(Step::Undo);
            },
            |_, game| x(game, hero) == 0.0,
        );
        // The game despawns one, which goes from the list.
        game.cx.world_mut().despawn(kid).expect("alive");
        until(
            &mut inspector,
            &mut game,
            &ctx,
            &mut time,
            |_| {},
            |texts, _| has(texts, "1 entity"),
        );
        // An entity made, and picked.
        let texts = until(
            &mut inspector,
            &mut game,
            &ctx,
            &mut time,
            |pass| {
                pass.operations.push(empty(None));
            },
            |texts, _| {
                texts
                    .iter()
                    .any(|text| text.starts_with("Empty Entity, entity "))
            },
        );
        assert_ne!(inspector.picked, Some(hero_id), "{texts:?}");
        assert_eq!(game.cx.world().entities(None).expect("all").len(), 2);
        // One the link creates, picked too.
        let made = Transform {
            translation: Vec3::new(0.0, 3.0, 0.0),
            ..Transform::IDENTITY
        };
        inspector.link().create(vec![
            ("Name".into(), called("Rock").to_value()),
            ("Transform".into(), made.to_value()),
        ]);
        until(
            &mut inspector,
            &mut game,
            &ctx,
            &mut time,
            |_| {},
            |texts, _| texts.iter().any(|text| text.starts_with("Rock, entity ")),
        );
        let rock = inspector.picked.expect("picked");
        let world = game.cx.world();
        let placed = world
            .entity(rock)
            .and_then(|rock| world.get::<Transform>(rock));
        assert_eq!(placed, Some(&made));
        // Its sprite's color, which it hasn't.
        inspector
            .link()
            .set(rock, "Sprite", "color", Value::Vec4(Vec4::ONE));
        let refused = |text: &String| text.contains("has no `Sprite`");
        until(
            &mut inspector,
            &mut game,
            &ctx,
            &mut time,
            |_| {},
            |texts, _| texts.iter().any(refused),
        );
        assert!(inspector.refusals.is_empty());
    }
}
