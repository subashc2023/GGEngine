use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::f64::consts::PI;
use std::mem;
use std::rc::Rc;

use egui::PointerButton::{Primary, Secondary};
use egui::ecolor::Hsva;
use gg_core::math::{self, DVec3, EulerRot, Quat, Vec2, Vec3, Vec4};

use super::entities::{Tree, row_label};
use super::*;
use crate::reflect::tests::{Everything, Form, Inner, everything, same_snapshots, snapshot};
use crate::reflect::{self, Ident};
use crate::{Camera, Component, Entity, Projection, Sprite};

fn id(world: &World, entity: Entity) -> EntityId {
    world.id(entity).expect("alive")
}

type Text = (String, egui::Rect);

/// Queues `pass` as the panels do, then makes what it found at a tick.
fn make(inspector: &mut Inspector, world: &mut World, pass: Pass) {
    inspector.queue(pass);
    Layer::tick(inspector, world, Tick { number: 0 });
}

/// Draws both panels side by side, each in its own half of `ui` as the dock does; the Entities
/// panel takes the rest of its half below its rows. First it makes the link's pick, as a pass does.
fn panels(inspector: &mut Inspector, world: &World, ui: &mut egui::Ui, pass: &mut Pass) {
    inspector.follow(world);
    let (left, right) = ui.max_rect().split_left_right_at_fraction(0.5);
    let entities = egui::UiBuilder::new().id_salt("Entities").max_rect(left);
    ui.scope_builder(entities, |ui| inspector.entities(world, ui, pass));
    let inspect = egui::UiBuilder::new().id_salt("Inspector").max_rect(right);
    let pending = inspector.pending();
    ui.scope_builder(inspect, |ui| inspector.inspect(world, ui, pass, &pending));
}

/// Returns the texts and the edit count; the edits are then set at a tick.
fn frame(
    inspector: &mut Inspector,
    world: &mut World,
    ctx: &egui::Context,
    time: f64,
    events: Vec<egui::Event>,
) -> (Vec<Text>, usize) {
    frame_ticking(inspector, world, ctx, time, events, true)
}

/// Like [`frame`], but its edits are set at a tick only if `tick`: else they wait, as above the
/// simulation's frame rate.
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

/// The component menus' buttons in `shapes`, in painting order: each frame, and the bounds of
/// its dots (three circles of [`DOT`]'s radius painted just after it).
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

/// The menu on the row of `header`, a component's name: its button and dots.
fn menu_on(menus: &[(egui::Rect, egui::Rect)], header: egui::Rect) -> (egui::Rect, egui::Rect) {
    let on_row = menus
        .iter()
        .find(|(button, _)| button.y_range().contains(header.center().y));
    *on_row.unwrap_or_else(|| panic!("no menu on {header:?}'s row: {menus:?}"))
}

/// Like [`frame_ticking`], but returns its shapes and edit count.
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

/// Three frames, the last's text: a grid lays out unseen in its first.
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
    // Transparent: premultiplying alpha would lose its color.
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
                // Negative integers (`inner.level`, the pair's) follow a minus sign as wide as
                // a digit.
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

/// Pressed and released in one frame (egui's tap).
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

/// Press, move and release, each in its own frame.
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

/// The first `name` drawn at or after the text `after`.
fn find(texts: &[Text], after: &str, name: &str) -> egui::Rect {
    let start = texts.iter().position(|(text, _)| text == after);
    let start = start.unwrap_or_else(|| panic!("no {after}: {texts:?}"));
    let found = texts[start..].iter().find(|(text, _)| text == name);
    found.unwrap_or_else(|| panic!("no {name}: {texts:?}")).1
}

/// Where the last of `names` is, each the first at or after the previous: `["rect", "[1]", "X"]`
/// finds the x button of a field's second element.
fn find_after(texts: &[Text], names: &[&str]) -> egui::Rect {
    let mut at = 0;
    for name in names {
        let found = texts[at..].iter().position(|(text, _)| text == name);
        at += found.unwrap_or_else(|| panic!("no {name} of {names:?}: {texts:?}"));
    }
    texts[at].1
}

/// What's drawn just right of `label` on its row: its field's widget.
fn next_to(texts: &[Text], label: egui::Rect) -> &Text {
    let row = |rect: &egui::Rect| (rect.center().y - label.center().y).abs() < 4.0;
    let right = texts.iter();
    let right = right.filter(|(_, rect)| row(rect) && rect.min.x > label.max.x);
    let nearest = right.min_by(|a, b| a.1.min.x.total_cmp(&b.1.min.x));
    nearest.expect("a widget beside the label")
}

/// The middle of [`next_to`]'s widget.
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
    let texts = run(
        &mut inspector,
        &mut world,
        click(Primary, magnifier(&texts)),
    );
    let field = find_field(&texts);
    let bad = "no entity's name has `5F3A9C0E12B47D68`, and `5F3A9C0E12B47D68` isn't an entity's \
               ID: an ID is 16 lowercase hex digits, not all zero, such as `5f3a9c0e12b47d68`";
    let missing = format!("no entity has ID {gone}");
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

fn row_text(world: &World, entity: Entity) -> String {
    row_label(world, id(world, entity)).text().to_owned()
}

/// A name with line breaks shows on one line in its row, as the rows' heights take each for one;
/// its `Name` keeps them.
#[test]
fn a_row_shows_a_name_with_line_breaks_on_one_line() {
    let mut world = World::default();
    let names = [
        ("two\nlines", "two lines"),
        ("a\r\nb\rc", "a b c"),
        ("one", "one"),
    ];
    for (name, shown) in names {
        let entity = world.spawn(called(name));
        assert_eq!(row_text(&world, entity), shown);
        assert_eq!(world.name(entity), Some(name));
    }
}

/// Names in a field and in an option: a one-field tuple struct shows alone and sets at its path
/// or whole. An enum variant's field keeps its label.
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

/// The Inspector's heading, not the row, names the entity and gives its ID.
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
    // An enum variant's field keeps its label, `0`.
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

/// Paths: `0` as a component, `name.0` in a field.
#[test]
fn a_tuple_of_one_field_is_set_at_its_own_path_or_whole() {
    let mut world = World::default();
    let named = world.spawn(called("Ada"));
    let shape = world.spawn(shaped());
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
    // Clicked left of the text (cursor first), then typed.
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
    let row = |(text, rect): &Text| text == "xAda" && rect.max.x < 800.0;
    assert!(texts.iter().any(row), "{texts:?}");
}

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
        // Infinite, as code may leave it; a typed number sets it back.
        offset: Vec3::new(f32::INFINITY, -0.0, 3.0),
        ..everything()
    };
    let entity = world.spawn(odd.clone());
    // Transparent: premultiplying alpha would lose its color. Its bits move as egui's popup
    // passes it through gamma.
    let tint = Vec4::new(0.3, 0.6, 0.9, 0.0);
    let sprite = Sprite {
        color: tint,
        ..Sprite::default()
    };
    let sprite = world.spawn(sprite);
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
    inspector.pick(id(&world, entity));
    let mut texts = run(&mut inspector, &mut world, Vec::new());
    // Typed as a person would: clicked (selecting its text), typed over, entered.
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
        // Typed past its max, it stops at the f32 the field takes.
        angle: 3.2,
        // Past f32's range, it stops at the f32 maximum, not infinity.
        offset: Vec3::new(f32::MAX, -0.0, 3.0),
        ..odd
    };
    assert_eq!(all, &expected);

    inspector.pick(id(&world, sprite));
    let texts = run(&mut inspector, &mut world, Vec::new());
    let at = egui::pos2(
        find(&texts, "texture", "None").min.x + 10.0,
        find(&texts, "Sprite", "color").center().y,
    );
    let opened = run(&mut inspector, &mut world, click(Primary, at));
    // Open a while: egui passes it through gamma each frame.
    for _ in 0..3 {
        run(&mut inspector, &mut world, Vec::new());
    }
    let bits = |color: Vec4| color.to_array().map(f32::to_bits);
    let set = world.get::<Sprite>(sprite).expect("a sprite").color;
    assert_eq!(bits(set), bits(tint));
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
    // White (untinted) and a gray gamma's round trip moves; with a tick each frame, and with
    // frames whose sets wait for one.
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
    // The popup's hue slider: found by its tooltip, below the first row, about halfway along.
    let copy = find(&opened, "📋", "📋");
    let x = copy.center().x + 70.0;
    let below = (0..100).map(|step| copy.max.y + 4.0 * step as f32);
    let hue = below.into_iter().find(|&y| {
        let moved = vec![egui::Event::PointerMoved(egui::pos2(x, y))];
        let texts = run(&mut inspector, &mut world, moved);
        texts.iter().any(|(text, _)| text == "Hue")
    });
    let hue = egui::pos2(x, hue.expect("the hue's slider") + 4.0);
    // Clicked: a drag would pick again each frame it's held.
    run(&mut inspector, &mut world, click(Primary, hue));
    let color = world.get::<Sprite>(sprite).expect("a sprite").color;
    let still = color.x == color.y && color.y == color.z;
    // A hue alone leaves it gray, though gamma may move its last bits.
    assert!(
        still && (color - gray).abs().max_element() < 1e-6,
        "{color}"
    );
    // Saturated from the square above the slider: it takes the picked hue, not red.
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
    // The widget keeps the typed 200°; from the rotation alone, it shows -160°.
    let id = egui::Id::unique(("inspector rotation", "Transform", "rotation"));
    ctx.data_mut(|data| data.insert_temp(id, (typed, [0.0f32, 0.0, 200.0])));
    // A bit off in each component, as renormalizing each tick leaves it.
    let nudged = typed.to_array().map(|x| f32::from_bits(x.to_bits() + 1));
    world
        .get_mut::<Transform>(entity)
        .expect("a transform")
        .rotation = Quat::from_array(nudged).normalize();
    // egui draws the number apart from its suffix, after a figure space for a sign.
    let texts = frames(&mut inspector, &mut world, &ctx);
    assert!(
        texts.iter().any(|text| text == "\u{2007}200.0"),
        "{texts:?}"
    );
    world
        .get_mut::<Transform>(entity)
        .expect("a transform")
        .rotation = Quat::from_rotation_z(math::to_radians(10.0));
    let texts = frames(&mut inspector, &mut world, &ctx);
    assert!(texts.iter().any(|text| text == "\u{2007}10.0"), "{texts:?}");
    // Before the tick, frames draw the rotation as the set will make it: the typed angles stay.
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
    // The earlier entity's refusal isn't shown beside this one's same field.
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
    inspector.apply(&mut world, inspector.picked, count(Value::UInt(200)));
    let other = world.spawn(everything());
    inspector.pick(id(&world, other));
    assert!(inspector.refusals.is_empty());
}

/// A field's handle to a gone entity shows the handle, not the ID of the entity now in its slot.
#[test]
fn the_despawned_are_named_by_what_they_were_shown_by() {
    let mut world = World::default();
    let gone = world.spawn(Transform::IDENTITY);
    world.despawn(gone).expect("alive");
    // Takes the slot `gone` left: the last freed is reused first.
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

/// Ctrl (Cmd on macOS) with `key`, and Shift if `shift`.
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

/// Press at `at`, three moves right, release, each in its own frame.
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
    // Shown above the fields until the drag's first set, so they move as it goes on.
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
        // Frames between drags wait for a tick and end no edit.
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
    // A set, then operations, the first ending its edit, so no seal does.
    inspector.queue(Pass {
        sets: vec![set("count", Value::UInt(8))],
        cues: pressed(),
        ..Pass::default()
    });
    let picked = id(&world, entity);
    let operations = vec![
        empty(None),
        empty(Some(picked)),
        Command::Duplicate(vec![picked]),
        Command::Delete(vec![picked]),
        Command::Add(picked, "Sprite".to_owned(), Sprite::default().to_value()),
        Command::Remove(picked, "Everything".to_owned()),
        Command::Revert(picked, "Everything".to_owned(), "count".to_owned()),
        Command::Unlink(picked),
        Command::Reparent(picked, Some(picked)),
        Command::Reparent(picked, None),
        Command::Rename(picked, "Renamed".to_owned()),
    ];
    inspector.queue(Pass {
        operations,
        ..Pass::default()
    });
    let values = |queue: &Queue| (0..queue.len()).map(|i| queue.value(i)).collect::<Vec<_>>();
    let queued = values(&inspector.queued);
    // The press began no edit (none went on); the release ended the one it began.
    let names: Vec<&str> = queued
        .iter()
        .map(|value| reflect::variant(value).expect("a variant").0)
        .collect();
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
            "Unlink",
            "Reparent",
            "Reparent",
            "Rename"
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
        error.contains(
            "`Create`, `Delete`, `Duplicate`, `Add`, `Remove`, `Revert`, `Unlink`, `Reparent` and \
             `Rename`"
        ),
        "{error}"
    );
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
        (
            Value::Variant(
                "Reparent".into(),
                vec![
                    named("entity", text(&entity)),
                    named("parent", text(&entity)),
                ],
            ),
            "reflect.wrong_kind",
            "parent",
        ),
        (
            Value::Variant(
                "Rename".into(),
                vec![
                    named("entity", text(&entity)),
                    named("name", Value::UInt(1)),
                ],
            ),
            "reflect.wrong_kind",
            "name",
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

/// - Adds and removes, queued as a recording replays them, are made at the next tick through
///   the log; undo takes them back.
/// - A remove, like an add, forgets what its component refused.
/// - Either clears a refusal shown above the components. One refused shows there too.
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
    // Above its bounds, on a component it lacks.
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
    // Refused (it has one), and said so above its components.
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

/// - An entity from an instance's prefab says which instance it's of and marks fields that
///   differ from the prefab. Their context menu reverts one at the next tick, as an edit the
///   Edit menu undoes.
/// - An instance's root says its prefab, beside Unlink: one edit, which an undo reverses.
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
    let up = Value::DVec3(DVec3::new(0.0, 5.0, 0.0));
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
    assert_eq!(place(&world), DVec3::new(0.0, 1.0, 0.0), "the prefab's");
    assert_eq!(inspector.log.edits().len(), 1, "one edit");
    let undo = |inspector: &mut Inspector, world: &mut World| {
        inspector.queue(Pass {
            chosen: Some(Step::Undo),
            ..Pass::default()
        });
        Layer::tick(inspector, world, Tick { number: 0 });
    };
    undo(&mut inspector, &mut world);
    assert_eq!(place(&world), DVec3::new(0.0, 5.0, 0.0), "undone");

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
    // With a tick each frame, and every other (the others draw sets waiting for one).
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
    run(&mut inspector, &mut world, click(Primary, at));
    let mut sets = 0;
    for typed in ["0", ".", "7", "5"] {
        let text = vec![egui::Event::Text(typed.to_owned())];
        sets += run(&mut inspector, &mut world, text).1;
    }
    sets += run(&mut inspector, &mut world, enter()).1;
    run(&mut inspector, &mut world, Vec::new());
    assert!(sets > 1 && weight(&world) == 0.75, "{sets} sets");
    assert_eq!(inspector.log.edits().len(), 1, "one edit");
    run(&mut inspector, &mut world, shortcut(egui::Key::Z, false));
    run(&mut inspector, &mut world, Vec::new());
    assert_eq!(weight(&world).to_bits(), 2.5f64.to_bits());
}

/// What [`in_window`]'s window gets and gives: its pass's time and events, and its texts.
#[derive(Default)]
struct Window {
    time: f64,
    events: Vec<egui::Event>,
    texts: Vec<Text>,
}

/// Runs `ctx`'s windows as the engine runs a torn-off panel's: each in its own pass inside the
/// main window's, with its own input (the returned `Window`'s).
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

/// Both panels draw in `window`; returns its texts and edit count.
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
    // The checkbox has no text: find it in the column where its neighbor's number starts.
    let checkbox = egui::pos2(
        find(&texts, "count", "42").min.x + 4.0,
        find(&texts, "Everything", "on").center().y,
    );
    run(&mut inspector, &mut world, click(Primary, checkbox));
    run(&mut inspector, &mut world, click(Primary, checkbox));
    assert!(on(&world));
    assert_eq!(inspector.log.edits().len(), 2);
    // With the Find field holding the keys, undo hits its text, not an edit.
    run(
        &mut inspector,
        &mut world,
        click(Primary, magnifier(&texts)),
    );
    let field = find_field(&run(&mut inspector, &mut world, Vec::new()));
    run(&mut inspector, &mut world, click(Primary, field));
    run(&mut inspector, &mut world, shortcut(egui::Key::Z, false));
    assert_eq!(inspector.log.edits().len(), 2);
    let away = click(Primary, egui::pos2(1500.0, 3900.0));
    run(&mut inspector, &mut world, away);
    run(&mut inspector, &mut world, shortcut(egui::Key::Z, false));
    assert_eq!(inspector.log.edits().len(), 1);
    assert!(!on(&world));
    // Still two edits: a press begins each.
    run(&mut inspector, &mut world, click(Primary, field));
    let weight = beside(&texts, find(&texts, "Everything", "weight"));
    for events in [slide(weight), slide(weight)].concat() {
        run(&mut inspector, &mut world, events);
    }
    assert_eq!(inspector.log.edits().len(), 3);
}

/// A frame of the open Edit and Entity menus' items alone, then the tick that makes the clicked
/// item's choice. Returns its texts.
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
    // With its entity gone, the edit can't be undone, and the Inspector panel says so.
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

/// A world cleared, replaced or restored by a lower layer is another; the Inspector's next tick
/// finds it and:
/// - unpicks, closes every row, forgets refusals and clears its log, so Undo names nothing;
/// - drops what was queued for the old world, neither making it here (a replace or restore
///   gives the named entity back under its ID) nor having it refused, as a cleared world would.
///
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
        inspector.apply(&mut world, Some(picked), count(200));
        time += 1.0;
        let texts = menu(&mut inspector, &mut world, &ctx, time, Vec::new());
        let undo = format!("Undo set `Everything.count` of {picked}");
        let named = texts.iter().any(|(text, _)| *text == undo);
        assert!(named, "{case}: {texts:?}");
        inspector.not_done = Some("can't undo it".to_owned());
        inspector.not_found = Some("no entity has ID 0123456789abcdef".to_owned());
        // For the next tick: a delete, then a set of an ongoing gesture.
        inspector.queue(Pass {
            operations: vec![Command::Delete(vec![picked])],
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
        // Dropped, not refused, as the delete would be in a cleared world.
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
        // Given back under its ID, neither set nor deleted.
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

/// When a layer swaps a copy in to play and back to stop, the returning world finds its edits,
/// pick and open rows as they were, and its undo works. The copy's go, and so do an earlier
/// world's, which never returns.
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
    // A cleared world is another for good: neither it nor the copy returns.
    world.clear();
    Layer::tick(&mut inspector, &mut world, Tick { number: 4 });
    mem::swap(&mut world, &mut left);
    Layer::tick(&mut inspector, &mut world, Tick { number: 5 });
    assert_eq!(inspector.log.undoable(), None, "the copy's went");
    assert_eq!(inspector.picked, None);
}

/// Through `restored`, as the tool API's `stop` brings back the world as edited with no tick
/// between, the returning world finds its edits, pick and open rows as they were, and its undo
/// works. So it does after another copy has played, and in a run whose edited world never ticked.
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
        // A copy plays in its place; its first tick puts the world's aside.
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
    // A run restored from a snapshot starts stopped: its edited world never ticks, yet is picked
    // and opened in after the Inspector attached or was restored.
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

/// With ticks rerun on a copy of the world (`--sync-test`), the Inspector's sets, undos and
/// redos rerun on the copy through a copy of the log as each tick started: the copy never
/// parts from the world. The world keeps its serial and log; its undos and redos go on.
#[test]
fn a_sync_tests_copy_takes_the_inspectors_edits_undos_and_redos_as_the_world_did() {
    let mut cx = Context::seeded(1);
    let entity = cx.world_mut().spawn(everything());
    let picked = id(cx.world(), entity);
    let mut stack = crate::layer::Stack::default();
    stack.push_overlay(Inspector::default());
    stack.sync_test();
    stack.attach(&mut cx).expect("loaded");
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

/// Every row of the tree, the naive way: each root, then if open each child's rows in turn.
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

/// `n` children of `parent`, each spawned a root then parented.
fn children_of(world: &mut World, parent: Entity, n: usize) -> Vec<Entity> {
    let made = (0..n).map(|_| {
        let child = world.spawn(Transform::IDENTITY);
        world.set_parent(child, Some(parent)).expect("no cycle");
        child
    });
    made.collect()
}

/// A page of the tree's rows matches the full listing wherever it starts. Covers 50,000+ roots
/// in several tables, open ones (open children among them), an open root without children, an
/// open child under a closed parent (adds no rows) and an open entity despawned.
#[test]
fn the_tree_finds_a_page_as_a_listing_of_every_row_does() {
    let mut world = World::default();
    let plain: Vec<Entity> = (0..20_000)
        .map(|_| world.spawn(Transform::IDENTITY))
        .collect();
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
    // Pages from every 97th row, and from each row near an open root's.
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

/// A frame scrolled thousands of rows down draws only the rows in view, as the tree lists them.
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

/// The triangles of the arrows `shapes` draw.
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

/// Only rows of entities with children have arrows; they show and hide the children's rows
/// (indented under theirs) rather than pick. A list of the entities with a component has
/// neither arrows nor indents.
#[test]
fn rows_with_children_alone_have_arrows() {
    let mut world = World::default();
    let [parent, child, lone] = ["parent", "child", "lone"].map(|name| world.spawn(called(name)));
    world.set_parent(child, Some(parent)).expect("no cycle");
    let [parent_row, child_row, lone_row] = [parent, child, lone].map(|e| row_text(&world, e));
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let mut time = 0.0;
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        let (mut texts, mut arrows) = (Vec::new(), Vec::new());
        for events in [events, Vec::new(), Vec::new()] {
            time += 1.0;
            let (shapes, _) = frame_shapes(inspector, world, &ctx, time, events, true);
            (texts, arrows) = (texts_in(&shapes), arrows_in(&shapes));
        }
        (texts, arrows)
    };
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

/// A world with an empty entity's components registered, unlike a bare one.
fn creating() -> World {
    let mut world = World::default();
    world.register::<Name>().expect("registered");
    world.register::<Transform>().expect("registered");
    world
}

/// The Entities panel's context menus (a secondary click opens each; egui sizes it in a frame
/// of its own):
/// - Below the rows: make a root.
/// - On a row: make a child of its entity, duplicate it or delete it, descendants included.
///
/// Each is made at the next tick, which picks what a create made, or the copy, and shows its
/// row. Each is undone; the creates and the delete are redone.
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
    let run = |inspector: &mut Inspector, world: &mut World, events| {
        let mut texts = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            texts = frame(inspector, world, &ctx, now(), events).0;
        }
        texts
    };
    let alive = |world: &World| world.entities(None).expect("every entity").len();
    let rows = |texts: &[Text]| {
        let rows = texts
            .iter()
            .filter(|(text, rect)| text == "Empty Entity" && rect.max.x < 800.0);
        let mut rows: Vec<egui::Rect> = rows.map(|(_, rect)| *rect).collect();
        rows.sort_by(|a, b| a.min.y.total_cmp(&b.min.y));
        rows
    };
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
    // The pick stays on what's gone.
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

/// A context menu left open acts on the entity it opened on, though its row moves and another
/// entity's takes its place.
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
    // Moves it to the end of another table; the last of its own takes its row.
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

/// The Entity menu makes a root. While the picked entity is alive it makes a child, duplicates
/// it (like Ctrl+D) picking the copy, and deletes it; a despawned pick is unpicked.
#[test]
fn the_entity_menu_creates_duplicates_deletes_and_unpicks() {
    let mut world = creating();
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let mut time = 0.0;
    // One settling frame only: egui finds a click among the last frame's widgets.
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
    // Each copy is the root's last child and picked; the second copies the first.
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
    // Drawn picked first: Delete must be enabled in the frame before the click.
    inspector.pick(child_id);
    run(&mut inspector, &mut world, Vec::new());
    run(&mut inspector, &mut world, item("Delete"));
    assert!(!world.contains(child));
    // Still picked, for an undo to bring it back; a despawned pick unpicks too.
    assert_eq!(inspector.picked, Some(child_id));
    run(&mut inspector, &mut world, item("Unpick"));
    assert_eq!(inspector.picked, None);
}

/// A click below the rows unpicks, forgetting the picked entity's refusals.
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

/// A click across a row, well right of its label, picks its entity, in the tree and in the
/// component list. Each row is a line tall, as paging assumes, even with a label wider than the
/// panel.
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
        // Neighbors' texts end two lines apart (none hangs below the line, as the g of "long").
        let apart = rows[2].max.y - rows[0].max.y;
        assert!((apart - 2.0 * line).abs() < 1.0, "{with:?}: {apart}");
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
    let texts = run(
        &mut inspector,
        &mut world,
        click(Primary, magnifier(&texts)),
    );
    let field = find_field(&texts);
    run(&mut inspector, &mut world, click(Primary, field));
    let typed = vec![egui::Event::Text(id(&world, bottom).to_string())];
    run(&mut inspector, &mut world, typed);
    let texts = run(&mut inspector, &mut world, enter());
    assert_eq!(inspector.picked, Some(id(&world, bottom)));
    let open: HashSet<EntityId> = [top, middle].map(|entity| id(&world, entity)).into();
    assert_eq!(inspector.expanded, open);
    assert!(texts.iter().any(|(text, _)| *text == found), "{texts:?}");
}

/// `key` pressed alone.
fn press(key: egui::Key) -> Vec<egui::Event> {
    vec![egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]
}

/// Where the text `name` is drawn in the Entities panel, the screen's left half.
fn in_entities(texts: &[Text], name: &str) -> egui::Rect {
    let found = texts
        .iter()
        .find(|(text, rect)| text == name && rect.max.x < 800.0);
    found.unwrap_or_else(|| panic!("no {name}: {texts:?}")).1
}

/// How far left of a header text button's text the preceding glyph button's middle is, in
/// egui's default spacing: the text button's padding 8 points, the widget gap 8, and half the
/// glyph button (square, as tall as a text's) 10.
const GLYPH_BEFORE: f32 = 26.0;

/// The Entities panel's magnifier, which opens its Find field: at the start of the header,
/// before the component filter.
fn magnifier(texts: &[Text]) -> egui::Pos2 {
    let filter = in_entities(texts, "All");
    egui::pos2(filter.min.x - GLYPH_BEFORE, filter.center().y)
}

/// Where the Find field is, once the magnifier has opened it, while its hint shows.
fn find_field(texts: &[Text]) -> egui::Pos2 {
    in_entities(texts, "ID or name").center()
}

/// - The Find field matches part of a name, ignoring case: each Enter finds the next match,
///   wrapping, and opens the rows above it.
/// - It says so when no name matches and the text isn't an ID.
/// - The magnifier opens it in place of the rest of the header; × closes it, emptied.
#[test]
fn the_find_field_finds_the_next_entity_whose_name_has_whats_typed() {
    let mut world = World::default();
    let [first, rock, second] = ["Lamp A", "rock", "lamp b"].map(|name| world.spawn(called(name)));
    world.set_parent(second, Some(rock)).expect("no cycle");
    let [first, rock, second] = [first, rock, second].map(|entity| id(&world, entity));
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
    let texts = run(
        &mut inspector,
        &mut world,
        click(Primary, magnifier(&texts)),
    );
    let filter = |(text, rect): &Text| text == "All" && rect.max.x < 800.0;
    assert!(!texts.iter().any(filter), "covered: {texts:?}");
    let field = find_field(&texts);
    // Open, it has the keys.
    run(
        &mut inspector,
        &mut world,
        vec![egui::Event::Text("LAMP".to_owned())],
    );
    for expected in [first, second, first] {
        run(&mut inspector, &mut world, enter());
        assert_eq!(inspector.picked, Some(expected));
        run(&mut inspector, &mut world, click(Primary, field));
    }
    assert!(inspector.expanded.contains(&rock), "found under it before");
    inspector.find.clear();
    run(
        &mut inspector,
        &mut world,
        vec![egui::Event::Text("zzz".to_owned())],
    );
    let texts = run(&mut inspector, &mut world, enter());
    assert_eq!(inspector.picked, Some(first), "kept");
    let says = |(text, _): &Text| text.starts_with("no entity's name has `zzz`, and `zzz` isn't");
    assert!(texts.iter().any(says), "{texts:?}");
    let close = in_entities(&texts, "×").center();
    let texts = run(&mut inspector, &mut world, click(Primary, close));
    assert!(!inspector.finding && inspector.find.is_empty());
    assert!(!texts.iter().any(says), "{texts:?}");
    in_entities(&texts, "All");
}

/// F2 (while the panel has the keys) and a row's Rename rename its entity in its row. Typing
/// replaces the selected name; Enter applies it at the next tick, as a set or an add, which an
/// undo takes back. Escape leaves it as it was.
#[test]
fn f2_and_a_rows_rename_name_its_entity_in_its_row() {
    let mut world = creating();
    let named = world.spawn((called("a"), Transform::IDENTITY));
    let nameless = world.spawn(Transform::IDENTITY);
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
    let typed = |text: &str| vec![egui::Event::Text(text.to_owned())];
    let texts = run(&mut inspector, &mut world, Vec::new());
    run(
        &mut inspector,
        &mut world,
        click(Primary, in_entities(&texts, "a").center()),
    );
    assert_eq!(inspector.picked, Some(id(&world, named)));
    for (left, name) in [(press(egui::Key::Escape), "a"), (enter(), "Apple")] {
        run(&mut inspector, &mut world, press(egui::Key::F2));
        run(&mut inspector, &mut world, typed("Apple"));
        let texts = run(&mut inspector, &mut world, left);
        assert_eq!(world.get::<Name>(named), Some(&called(name)));
        in_entities(&texts, name);
    }
    let texts = run(&mut inspector, &mut world, Vec::new());
    let row = in_entities(&texts, "Entity").center();
    let texts = run(&mut inspector, &mut world, click(Secondary, row));
    let item = in_menu(&ctx, &texts, "Rename");
    run(&mut inspector, &mut world, click(Primary, item));
    assert_eq!(inspector.picked, Some(id(&world, nameless)));
    run(&mut inspector, &mut world, typed("Bee"));
    run(&mut inspector, &mut world, enter());
    assert_eq!(world.get::<Name>(nameless), Some(&called("Bee")));
    run(&mut inspector, &mut world, shortcut(egui::Key::Z, false));
    assert_eq!(world.get::<Name>(nameless), None, "undone");
}

/// - While the panel has the keys (from a press in it until a press elsewhere), Delete deletes
///   the picked entity and F asks the link to frame it.
/// - A double click on a row frames and picks its entity, as does its menu's Frame in View,
///   which shows once a layer takes frames.
/// - Its Copy ID copies its ID.
#[test]
fn the_panels_keys_its_double_clicks_and_its_menus_delete_and_frame() {
    let mut world = World::default();
    let [a, b, c] = ["a", "b", "c"].map(|name| world.spawn(called(name)));
    let [a_id, b_id, c_id] = [a, b, c].map(|entity| id(&world, entity));
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let link = inspector.link();
    let clock = Cell::new(0.0);
    let now = || {
        clock.set(clock.get() + 1.0);
        clock.get()
    };
    let run = |inspector: &mut Inspector, world: &mut World, events| {
        let mut texts = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            texts = frame(inspector, world, &ctx, now(), events).0;
        }
        texts
    };
    let row = |texts: &[Text], name| in_entities(texts, name).center();
    let texts = run(&mut inspector, &mut world, Vec::new());
    let menu = run(
        &mut inspector,
        &mut world,
        click(Secondary, row(&texts, "c")),
    );
    assert!(
        !menu.iter().any(|(text, _)| text == "Frame in View"),
        "{menu:?}"
    );
    run(&mut inspector, &mut world, press(egui::Key::Escape));
    // A press in the panel gives it the keys.
    run(&mut inspector, &mut world, click(Primary, row(&texts, "a")));
    run(&mut inspector, &mut world, press(egui::Key::Delete));
    assert!(!world.contains(a));
    // A press elsewhere, in the Inspector panel, takes them.
    let texts = run(&mut inspector, &mut world, click(Primary, row(&texts, "b")));
    run(
        &mut inspector,
        &mut world,
        click(Primary, egui::pos2(1200.0, 3000.0)),
    );
    run(&mut inspector, &mut world, press(egui::Key::Delete));
    run(&mut inspector, &mut world, press(egui::Key::F));
    assert!(world.contains(b));
    assert_eq!(link.framed(), None);
    run(&mut inspector, &mut world, click(Primary, row(&texts, "b")));
    run(&mut inspector, &mut world, press(egui::Key::F));
    assert_eq!(link.framed(), Some(b_id));
    assert_eq!(link.framed(), None, "taken");
    let twice = [
        click(Primary, row(&texts, "c")),
        click(Primary, row(&texts, "c")),
    ]
    .concat();
    run(&mut inspector, &mut world, twice);
    assert_eq!((inspector.picked, link.framed()), (Some(c_id), Some(c_id)));
    // Frame in View shows once a layer takes frames, as it has.
    inspector.pick(b_id);
    let menu = run(
        &mut inspector,
        &mut world,
        click(Secondary, row(&texts, "c")),
    );
    run(
        &mut inspector,
        &mut world,
        click(Primary, in_menu(&ctx, &menu, "Frame in View")),
    );
    assert_eq!((inspector.picked, link.framed()), (Some(c_id), Some(c_id)));
    let menu = run(
        &mut inspector,
        &mut world,
        click(Secondary, row(&texts, "c")),
    );
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 4000.0));
    let input = egui::RawInput {
        screen_rect: Some(screen),
        time: Some(now()),
        events: click(Primary, in_menu(&ctx, &menu, "Copy ID")),
        ..egui::RawInput::default()
    };
    let output = ctx.run_ui(input, |ui| {
        panels(&mut inspector, &world, ui, &mut Pass::default());
    });
    let copy = egui::OutputCommand::CopyText(c_id.to_string());
    let commands = output.platform_output.commands;
    assert!(commands.contains(&copy), "{commands:?}");
    assert_ne!(a_id, c_id);
}

/// - Ctrl-click (Cmd on macOS) adds a row's entity to the picks as the active one, or takes it
///   out, the previous pick becoming active.
/// - Shift-click picks the rows from the active one's to it.
/// - The Inspector panel says how many are picked.
/// - Delete deletes them all; a picked row's Duplicate Entity copies them all, picking the
///   copies. Each is one edit, one undo; a pick under another picked goes with it.
#[test]
fn clicks_with_ctrl_and_shift_pick_several_which_delete_and_copy_as_one_edit() {
    let mut world = World::default();
    let [a, b, c, d] = ["a", "b", "c", "d"].map(|name| world.spawn(called(name)));
    world.set_parent(d, Some(c)).expect("no cycle");
    let ids = [a, b, c, d].map(|entity| id(&world, entity));
    let [a_id, b_id, c_id, d_id] = ids;
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.expanded.insert(c_id);
    let mut time = 0.0;
    // `modifiers` are held for the first frame and let go for the last.
    let mut run = |inspector: &mut Inspector, world: &mut World, events: Vec<_>, modifiers| {
        let mut texts = Vec::new();
        let held = [egui::Event::ModifiersChanged(modifiers)]
            .into_iter()
            .chain(events);
        let frames = [held.collect(), Vec::new()];
        let let_go = [vec![egui::Event::ModifiersChanged(Modifiers::NONE)]];
        for events in frames.into_iter().chain(let_go) {
            time += 1.0;
            texts = frame(inspector, world, &ctx, time, events).0;
        }
        texts
    };
    let held = |modifiers, at| {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: Primary,
            pressed,
            modifiers,
        };
        vec![egui::Event::PointerMoved(at), button(true), button(false)]
    };
    let picks = |inspector: &Inspector| inspector.picks().collect::<Vec<_>>();
    let texts = run(&mut inspector, &mut world, Vec::new(), Modifiers::NONE);
    let row = |name| in_entities(&texts, name).center();
    let none = Modifiers::NONE;
    run(&mut inspector, &mut world, held(none, row("a")), none);
    let ctrl = Modifiers::COMMAND;
    run(&mut inspector, &mut world, held(ctrl, row("c")), ctrl);
    assert_eq!(
        (picks(&inspector), inspector.picked),
        (vec![a_id, c_id], Some(c_id))
    );
    run(&mut inspector, &mut world, held(ctrl, row("c")), ctrl);
    assert_eq!(
        (picks(&inspector), inspector.picked),
        (vec![a_id], Some(a_id)),
        "the active one taken out, the one before it active"
    );
    let shift = Modifiers::SHIFT;
    let texts = run(&mut inspector, &mut world, held(shift, row("d")), shift);
    assert_eq!(picks(&inspector), ids, "the rows from a's to d's");
    let said = "4 entities picked: the last one picked is below";
    assert!(texts.iter().any(|(text, _)| text == said), "{texts:?}");
    // The clicks gave the panel the keys; d goes with c.
    run(&mut inspector, &mut world, press(egui::Key::Delete), none);
    assert!([a, b, c, d].iter().all(|&entity| !world.contains(entity)));
    run(
        &mut inspector,
        &mut world,
        shortcut(egui::Key::Z, false),
        none,
    );
    assert!(
        [a, b, c, d].iter().all(|&entity| world.contains(entity)),
        "one edit"
    );
    let menu = run(&mut inspector, &mut world, click(Secondary, row("a")), none);
    let duplicate = in_menu(&ctx, &menu, "Duplicate Entity");
    run(&mut inspector, &mut world, click(Primary, duplicate), none);
    let roots: Vec<Entity> = world.roots().collect();
    assert_eq!(roots.len(), 6, "a copy of each root picked, d's under c's");
    let copies: Vec<EntityId> = roots[3..].iter().map(|&copy| id(&world, copy)).collect();
    assert_eq!(picks(&inspector), copies, "the copies picked, c's active");
    assert_eq!(
        world.children(roots[5]).expect("alive").count(),
        1,
        "d's copy"
    );
    run(
        &mut inspector,
        &mut world,
        shortcut(egui::Key::Z, false),
        none,
    );
    assert_eq!(world.roots().count(), 3, "one edit");
    assert_ne!(b_id, d_id);
}

/// Commands on several entities are values like one's; recordings from before multiselect
/// still read.
#[test]
fn deletes_and_duplicates_of_one_entity_as_recordings_had_them_read() {
    let id: EntityId = "0123456789abcdef".parse().expect("an ID");
    let mut queue = Queue::default();
    for name in ["Delete", "Duplicate"] {
        let entity = ("entity".into(), Value::String(id.to_string()));
        queue
            .push(&Value::Variant(name.into(), vec![entity]))
            .expect("one entity's");
    }
    let [Command::Delete(deleted), Command::Duplicate(copied)] = &queue.0[..] else {
        panic!("a delete and a duplicate");
    };
    assert_eq!((deleted, copied), (&vec![id], &vec![id]));
    let entities = Value::List(vec![Value::String(id.to_string())]);
    let written = Value::Variant("Delete".into(), vec![("entities".into(), entities)]);
    assert_eq!(queue.value(0), written);
}

/// A row dragged onto another makes its entity that one's last child, opening its row; onto the
/// space below the rows, a root; onto one of its descendants, nothing.
#[test]
fn a_row_dragged_onto_another_makes_its_entity_that_ones_child() {
    let mut world = World::default();
    let [a, b] = ["a", "b"].map(|name| world.spawn(called(name)));
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let clock = Cell::new(0.0);
    let run = |inspector: &mut Inspector, world: &mut World, events| {
        let mut texts = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            clock.set(clock.get() + 1.0);
            texts = frame(inspector, world, &ctx, clock.get(), events).0;
        }
        texts
    };
    let drop = |inspector: &mut Inspector, world: &mut World, from, to| {
        let mut texts = Vec::new();
        for events in drag(from, to) {
            texts = run(inspector, world, events);
        }
        texts
    };
    let row = |texts: &[Text], name| in_entities(texts, name).center();
    let parent = |world: &World, entity| world.parent(entity).expect("alive");
    let texts = run(&mut inspector, &mut world, Vec::new());
    let texts = drop(
        &mut inspector,
        &mut world,
        row(&texts, "b"),
        row(&texts, "a"),
    );
    assert_eq!(parent(&world, b), Some(a));
    assert!(inspector.expanded.contains(&id(&world, a)));
    let texts = drop(
        &mut inspector,
        &mut world,
        row(&texts, "a"),
        row(&texts, "b"),
    );
    assert_eq!((parent(&world, a), parent(&world, b)), (None, Some(a)));
    let below = egui::pos2(200.0, 3000.0);
    drop(&mut inspector, &mut world, row(&texts, "b"), below);
    assert_eq!(parent(&world, b), None);
}

/// The + menu creates an empty root, or an empty child of the entity picked, picking it; and
/// Collapse All hides the rows under every root.
#[test]
fn the_plus_menu_creates_and_collapse_all_hides_every_roots_rows() {
    let mut world = creating();
    let top = world.spawn((called("top"), Transform::IDENTITY));
    let kid = world.spawn((called("kid"), Transform::IDENTITY));
    world.set_parent(kid, Some(top)).expect("no cycle");
    let top_id = id(&world, top);
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.expanded.insert(top_id);
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
    in_entities(&texts, "kid");
    let plus = in_entities(&texts, "+");
    let collapse = egui::pos2(plus.min.x - GLYPH_BEFORE, plus.center().y);
    let texts = run(&mut inspector, &mut world, click(Primary, collapse));
    assert!(inspector.expanded.is_empty());
    assert!(!texts.iter().any(|(text, _)| text == "kid"), "{texts:?}");
    let plus = in_entities(&texts, "+").center();
    for (item, parent) in [
        ("Create Empty Entity", None),
        ("Create Empty Child", Some(top)),
    ] {
        if parent.is_some() {
            inspector.pick(top_id);
        }
        let menu = run(&mut inspector, &mut world, click(Primary, plus));
        run(
            &mut inspector,
            &mut world,
            click(Primary, in_menu(&ctx, &menu, item)),
        );
        let made = inspector
            .picked
            .and_then(|made| world.entity(made))
            .expect("made");
        assert_eq!(world.get::<Name>(made), Some(&called("Empty Entity")));
        assert_eq!(world.parent(made).expect("alive"), parent);
    }
}

/// A component that follows an entity: it holds its handle, which has no default.
#[derive(Clone, Component)]
struct Follower(Entity);

/// Opaque as a whole, like a mesh's draw: no value makes one.
#[derive(Clone, Component)]
#[reflect(opaque)]
struct Sealed;

/// The Add Component menu lists, in registration order, the types the picked entity can be
/// given: not those it has, a parent, or those opaque as a whole. A pick is added at the next
/// tick as its type's default; undo removes it. A type without a default shows disabled,
/// saying why on hover, and adds nothing.
#[test]
fn add_component_adds_a_types_default() {
    let mut world = creating();
    world.register::<Camera>().expect("registered");
    let entity = world.spawn(Transform::IDENTITY);
    world.spawn(Follower(entity));
    world.spawn(Sealed);
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let mut time = 0.0;
    // Two settling frames: a popup draws unseen in its first.
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

/// The menu beside a component's header removes it at the next tick; undo gives it back. A
/// parent, which the world gives, has no menu.
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
    let mut run = |inspector: &mut Inspector, world: &mut World, events, tick| {
        let mut shapes = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            time += 1.0;
            shapes = frame_shapes(inspector, world, &ctx, time, events, tick).0;
        }
        (texts_in(&shapes), find_menus(&shapes))
    };
    let (texts, menus) = run(&mut inspector, &mut world, Vec::new(), true);
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

/// A component's header is framed, as wide as the Inspector panel. A click on it, however far
/// right of its name, closes and opens it; its menu (over the right end, dots mid-button) opens
/// from its button without toggling it.
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

/// In an Inspector panel narrower and shorter than its contents, rows run past the right edge,
/// cut off, and the scroll bar floats along it. A component's menu still shows, at the end of
/// the visible header short of the bar, and opens from a click anywhere on its button.
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
    // The panel is drawn as the dock does: cut off at its edges, in the Inspector's scroll area.
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
    // Axis rows run past the edge: translation's Z button is cut off, undrawn.
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
    let end = egui::pos2(menu.max.x - 1.0, menu.center().y);
    let (texts, _) = run(click(Primary, end));
    in_menu(&ctx, &texts, "Remove Component");
}

/// An enum with fields switches variant from a choice: to one it had while the entity was
/// drawn, with the fields it had then; to a new one, with defaults; on another entity, with
/// defaults again.
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
    let run = |inspector: &mut Inspector, world: &mut World, events, tick| {
        let mut texts = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            clock.set(clock.get() + 1.0);
            texts = frame_ticking(inspector, world, &ctx, clock.get(), events, tick).0;
        }
        texts
    };
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
    let texts = run(&mut inspector, &mut world, Vec::new(), true);
    let fov = beside(&texts, find(&texts, "projection", "fov"));
    run(&mut inspector, &mut world, click(Primary, fov), true);
    let typed = vec![egui::Event::Text("60".to_owned())];
    run(&mut inspector, &mut world, typed, true);
    run(&mut inspector, &mut world, enter(), true);
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

/// An environment's sky switches between its clear color and Earth's atmosphere in its choice.
/// An image sky, whose texture has no default, shows disabled there, saying why on hover.
#[test]
fn an_environments_sky_switches_to_the_atmosphere() {
    let mut world = World::default();
    let entity = world.spawn(crate::Environment::default());
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
    let sky = |world: &World| world.get::<crate::Environment>(entity).expect("one").sky;
    let texts = run(&mut inspector, &mut world, Vec::new());
    let choice = click(Primary, find(&texts, "sky", "ClearColor").center());
    let texts = run(&mut inspector, &mut world, choice);
    let image = in_menu(&ctx, &texts, "Image");
    let texts = run(
        &mut inspector,
        &mut world,
        vec![egui::Event::PointerMoved(image)],
    );
    let why = (texts.iter()).any(|(text, _)| text.contains("a texture has no default"));
    assert!(why, "{texts:?}");
    let atmosphere = in_menu(&ctx, &texts, "Atmosphere");
    run(&mut inspector, &mut world, click(Primary, atmosphere));
    let earth = crate::Sky::Atmosphere(crate::Atmosphere::default());
    assert_eq!(sky(&world), earth);
    let texts = run(&mut inspector, &mut world, Vec::new());
    let choice = click(Primary, find(&texts, "sky", "Atmosphere").center());
    let texts = run(&mut inspector, &mut world, choice);
    let clear = in_menu(&ctx, &texts, "ClearColor");
    run(&mut inspector, &mut world, click(Primary, clear));
    assert_eq!(sky(&world), crate::Sky::ClearColor);
}

/// An option's checkbox turns it on and off, each an edit undo reverses. An environment's
/// reflections turn on with defaults; after tuning and an off-on cycle, with what they last
/// held; on another entity, with defaults. A sprite's texture, with no default, shows its
/// checkbox disabled, saying why on hover, and stays off.
#[test]
fn options_turn_on_with_what_they_last_held_or_their_defaults() {
    let mut world = World::default();
    let [first, second] = [0, 1].map(|_| world.spawn(crate::Environment::default()));
    let sprite = world.spawn(Sprite::default());
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, first));
    let mut time = 0.0;
    let mut run = |inspector: &mut Inspector, world: &mut World, events| {
        let mut texts = Vec::new();
        for events in [events, Vec::new(), Vec::new()] {
            time += 1.0;
            texts = frame(inspector, world, &ctx, time, events).0;
        }
        texts
    };
    let reflections = |world: &World, entity| {
        let environment = world.get::<crate::Environment>(entity).expect("one");
        environment.reflections
    };
    let defaults = Some(crate::Reflections::default());
    let turn = |texts: &[Text], label| click(Primary, find(texts, "reflections", label).center());
    let texts = run(&mut inspector, &mut world, Vec::new());
    let texts = run(&mut inspector, &mut world, turn(&texts, "None"));
    assert_eq!(reflections(&world, first), defaults);
    let distance = beside(&texts, find(&texts, "reflections", "distance"));
    run(&mut inspector, &mut world, click(Primary, distance));
    let typed = vec![egui::Event::Text("30".to_owned())];
    run(&mut inspector, &mut world, typed);
    let texts = run(&mut inspector, &mut world, enter());
    let tuned = Some(crate::Reflections {
        distance: 30.0,
        ..crate::Reflections::default()
    });
    assert_eq!(reflections(&world, first), tuned);
    let texts = run(&mut inspector, &mut world, turn(&texts, "Some"));
    assert_eq!(reflections(&world, first), None);
    run(&mut inspector, &mut world, turn(&texts, "None"));
    assert_eq!(reflections(&world, first), tuned);
    run(&mut inspector, &mut world, shortcut(egui::Key::Z, false));
    assert_eq!(reflections(&world, first), None);
    inspector.pick(id(&world, second));
    let texts = run(&mut inspector, &mut world, Vec::new());
    run(&mut inspector, &mut world, turn(&texts, "None"));
    assert_eq!(reflections(&world, second), defaults);
    inspector.pick(id(&world, sprite));
    let texts = run(&mut inspector, &mut world, Vec::new());
    let none = find(&texts, "texture", "None").center();
    let texts = run(
        &mut inspector,
        &mut world,
        vec![egui::Event::PointerMoved(none)],
    );
    let why = (texts.iter()).any(|(text, _)| text.contains("a texture has no default"));
    assert!(why, "{texts:?}");
    let edits = inspector.log.edits().len();
    run(&mut inspector, &mut world, click(Primary, none));
    assert_eq!(world.get::<Sprite>(sprite).expect("a sprite").texture, None);
    assert_eq!(inspector.log.edits().len(), edits);
}

/// An option that's on, holding a struct, keeps its fields within its row: with every option
/// on, each of an environment's rows ends above the next one's start. Regression: a grid
/// centered the content in the previous frame's row height, putting its last field on the next
/// row.
#[test]
fn options_holding_structs_keep_within_their_rows() {
    let mut world = World::default();
    let entity = world.spawn(crate::Environment {
        adaptation: Some(crate::Adaptation::default()),
        fog: Some(crate::Fog::default()),
        reflections: Some(crate::Reflections::default()),
        depth_of_field: Some(crate::DepthOfField::default()),
        motion_blur: Some(crate::MotionBlur::default()),
        ..crate::Environment::default()
    });
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    inspector.pick(id(&world, entity));
    let mut texts = Vec::new();
    for time in 1..=3 {
        texts = frame(&mut inspector, &mut world, &ctx, time.into(), Vec::new()).0;
    }
    let rows = [
        "exposure",
        "adaptation",
        "tonemapper",
        "ambient_occlusion",
        "fog",
        "reflections",
        "depth_of_field",
        "motion_blur",
        "lens",
    ];
    let mut starts = Vec::new();
    for row in rows {
        let after = starts.last().copied().unwrap_or(0);
        let found = texts[after..].iter().position(|(text, _)| text == row);
        starts.push(after + found.unwrap_or_else(|| panic!("no {row}: {texts:?}")));
    }
    for (i, pair) in starts.windows(2).enumerate() {
        let next = rows.get(i + 2).map_or(texts.len(), |_| starts[i + 2]);
        let bottom = texts[pair[0]..pair[1]].iter().map(|(_, rect)| rect.max.y);
        let top = texts[pair[1]..next].iter().map(|(_, rect)| rect.min.y);
        let (bottom, top) = (
            bottom.fold(f32::MIN, f32::max),
            top.fold(f32::MAX, f32::min),
        );
        assert!(
            bottom <= top,
            "{}'s row ends at {bottom}, below {}'s start at {top}",
            rows[i],
            rows[i + 1]
        );
    }
}

/// A variant holding opaque state, which no value makes, shows disabled in its enum's choice,
/// saying why on hover, and can't be switched to.
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

/// An axis button resets its component alone to the default, a click an edit:
/// - a translation's x to 0 (its type's default);
/// - a scale's y to 1 (the transform's);
/// - the x of a sprite rect's bottom right to 1 (the sprite's);
/// - a rotation's angle about z to the default rotation's, keeping the others.
#[test]
fn axis_buttons_set_their_component_to_its_default() {
    let mut world = World::default();
    let transform = Transform {
        translation: DVec3::new(1.0, 2.0, 3.0),
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
    assert_eq!(set.translation, DVec3::new(0.0, 2.0, 3.0));
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

/// Axis button labels draw in [`crate::BOLD`]'s family where the context binds it (the
/// engine's does), else in egui's proportional one (egui's own fonts, or a game's); never in an
/// unbound family.
#[test]
fn axis_buttons_are_bold_where_the_family_is_bound() {
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
            assert_eq!(drawn_in, [&family; 3], "{axis}");
        }
    }
}

/// An axis drag sets that component alone, at its own path, so a system moving another between
/// frames keeps its move. The drag is one edit; undo takes back that component alone.
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
    assert_eq!(translation(&world), DVec3::new(0.0, 5.0, 0.0));
}

/// Sets of a vector and its components waiting for a tick draw it as they'll make it, in order;
/// the tick then makes it so.
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
    let whole = |x, y, z| Value::DVec3(DVec3::new(x, y, z));
    let mut time = 0.0;
    for (sets, made) in [
        (
            vec![
                set("translation", whole(1.5, 2.5, 3.5)),
                set("translation.x", Value::F64(7.5)),
            ],
            Vec3::new(7.5, 2.5, 3.5),
        ),
        (
            vec![
                set("translation.y", Value::F64(8.5)),
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
        let shown = ["X", "Y", "Z"].map(|axis| {
            let (text, _) = next_to(&texts, find_after(&texts, &["translation", axis]));
            text.trim_start_matches('\u{2007}')
                .parse::<f32>()
                .unwrap_or_else(|_| panic!("a number: {text}"))
        });
        assert_eq!(Vec3::from(shown), made, "{texts:?}");
        Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
        let held = world.get::<Transform>(entity).expect("a transform");
        assert_eq!(held.translation, made.as_dvec3());
    }
}

/// A vector's numbers keep their places as they change. Each axis field takes an equal share of
/// the row; its centered number keeps its width as digits change and the sign comes and goes
/// (a minus sign or a figure space). A number showing as zero shows no minus.
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
        // Same last digit as the first two: where their ink ends compares widths.
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
        // Where ink ends: a start would include a figure space's blank.
        numbers.push(at.max.x);
        ys.push(find_after(&texts, &["translation", "Y"]).min.x);
    }
    assert!(
        numbers[..3].iter().all(|&end| end == numbers[0]),
        "{numbers:?}"
    );
    assert!(ys.iter().all(|&y| y == ys[0]), "{ys:?}");
}

/// In a panel as narrow as the editor's first Inspector, cut off in its scroll area as the dock
/// draws it, a vector's axes share what shows of their row: the last ends within the panel,
/// short of the scroll bar, and stays put whatever the first shows.
#[test]
fn a_vectors_axes_fit_a_panel_as_narrow_as_the_editors() {
    let mut world = World::default();
    let turned = Transform {
        translation: DVec3::new(-1.875, 0.0, -1.25),
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

/// A link gesture is one edit, whatever the panels' input says meanwhile (a drag in the game's
/// view doesn't pass through it). Across passes whose cues let go, press and release, its sets
/// queue as `Set`s, its seal as a `Seal`, and one undo reverses it. Its first set ends an
/// ongoing panel edit; a panel set ends the gesture.
#[test]
fn a_gesture_of_the_links_is_one_edit_whatever_the_panels_cues() {
    let mut world = World::default();
    let entity = world.spawn((Transform::IDENTITY, everything()));
    let picked = id(&world, entity);
    let mut inspector = Inspector::default();
    inspector.pick(picked);
    let link = inspector.link();
    let at = |x: f64| Value::DVec3(DVec3::new(x, 0.0, 0.0));
    let count = |n| Edit {
        component: "Everything".to_owned(),
        path: "count".to_owned(),
        value: Value::UInt(n),
    };
    let held = || Cues {
        held: true,
        ..Cues::default()
    };
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
    assert_eq!(translation(&world), Some(DVec3::new(3.0, 0.0, 0.0)));
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
    assert_eq!(translation(&world), Some(DVec3::ZERO));
    let typed = world.get::<Everything>(entity).map(|all| all.count);
    assert_eq!(typed, Some(7));
    link.set(picked, "Transform", "translation", at(4.0));
    inspector.queue(Pass::default());
    inspector.queue(Pass {
        sets: vec![count(9)],
        cues: held(),
        ..Pass::default()
    });
    assert_eq!(queued_names(&inspector), ["Set", "Seal", "Set"]);
}

/// A link set belongs to its entity, whatever's picked as it's queued: here, one picked through
/// the link in the same pass.
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

/// Link sets wait from their frame until a tick makes them. A drag's sets and the undo after
/// them in one queue leave the world as it was, nothing waiting. A refused set is dropped.
#[test]
fn the_links_sets_wait_until_a_tick_makes_them() {
    let mut world = World::default();
    let entity = world.spawn(Transform::IDENTITY);
    let picked = id(&world, entity);
    let mut inspector = Inspector::default();
    let link = inspector.link();
    let at = |x: f64| Value::DVec3(DVec3::new(x, 0.0, 0.0));
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
    // A non-unit rotation, which the world refuses.
    let skewed = Value::Quat(Quat::from_xyzw(0.0, 0.0, 2.0, 1.0));
    link.set(picked, "Transform", "rotation", skewed);
    make(&mut inspector, &mut world, Pass::default());
    assert!(!link.waiting());
    assert_eq!(world.get::<Transform>(entity), Some(&Transform::IDENTITY));
}

/// The panel draws a field as the link's waiting set will make it, from the set's frame, after
/// earlier queued sets.
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

/// The link picks a living entity as the next pass starts, opening the rows above it. It
/// unpicks for a despawned entity's ID, an ID no entity had, or none. With no pick asked for,
/// the pick stays.
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

/// A restored world, being another, drops the link's sets queued for the old world (those no
/// pass has taken) and the pick it asked for.
#[test]
fn another_world_drops_what_the_link_asked_of_the_one_before() {
    let mut world = World::default();
    let entity = world.spawn(Transform::IDENTITY);
    let picked = id(&world, entity);
    let snapshot = world.clone();
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let link = inspector.link();
    Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
    let at = |x: f64| Value::DVec3(DVec3::new(x, 0.0, 0.0));
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

/// Between passes the link keeps only the latest value of each field of its gesture; a seal
/// keeps earlier ones apart. A second seal ends nothing more.
#[test]
fn the_link_keeps_the_latest_value_of_each_field_between_passes() {
    let link = Inspector::default().link();
    let [a, b]: [EntityId; 2] =
        ["0123456789abcdef", "fedcba9876543210"].map(|id| id.parse().expect("an ID"));
    let at = |x: f64| Value::DVec3(DVec3::new(x, 0.0, 0.0));
    for x in [1.0, 2.0] {
        link.set(a, "Transform", "translation", at(x));
        link.set(b, "Transform", "translation", at(x));
    }
    link.set(
        a,
        "Transform",
        "scale",
        Value::Vec3(Vec3::new(5.0, 0.0, 0.0)),
    );
    link.seal();
    link.seal();
    link.set(a, "Transform", "translation", at(3.0));
    let set = |id, path: &str, value| Some((id, "Transform".to_owned(), path.to_owned(), value));
    assert_eq!(
        link.pending(),
        [
            set(a, "translation", at(2.0)),
            set(b, "translation", at(2.0)),
            set(a, "scale", Value::Vec3(Vec3::new(5.0, 0.0, 0.0))),
            None,
            set(a, "translation", at(3.0))
        ]
    );
}

/// A debug build refuses a link set of a whole component, or inside a non-vector field: the
/// panel couldn't draw it as it'll be.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "a link sets a field of a component's own")]
fn a_link_refuses_a_set_inside_another_kind_of_field() {
    let link = Inspector::default().link();
    let id: EntityId = "0123456789abcdef".parse().expect("an ID");
    link.set(id, "Everything", "maybe.level", Value::UInt(1));
}

/// A debug build refuses, as the next pass takes it, a link set of a field whose insides the
/// panels set at their own paths (a struct's): the panel couldn't draw it beside theirs.
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

/// A field's latest link set goes after sets made since its last, so a vector set whole and
/// then one component still apply in that order.
#[test]
fn the_links_latest_set_of_a_field_comes_after_those_made_since() {
    let link = Inspector::default().link();
    let id: EntityId = "0123456789abcdef".parse().expect("an ID");
    let whole = |x: f64| Value::DVec3(DVec3::splat(x));
    link.set(id, "Transform", "translation", whole(1.0));
    link.set(id, "Transform", "translation.x", Value::F64(5.0));
    link.set(id, "Transform", "translation", whole(2.0));
    let set = |path: &str, value| Some((id, "Transform".to_owned(), path.to_owned(), value));
    let expected = [
        set("translation.x", Value::F64(5.0)),
        set("translation", whole(2.0)),
    ];
    assert_eq!(link.pending(), expected);
}

/// A link create queues after earlier link sets, ending their gesture, and makes a root at the
/// next tick, which the Inspector picks; one undo takes it back alone. A refused create (a
/// type the world lacks) shows above the components, as the menu's does.
#[test]
fn a_create_through_the_link_makes_a_root_and_picks_it() {
    let mut world = World::default();
    world.register::<Name>().expect("registered");
    let entity = world.spawn(Transform::IDENTITY);
    let picked = id(&world, entity);
    let ctx = crate::egui_context();
    let mut inspector = Inspector::default();
    let link = inspector.link();
    let at = Value::DVec3(DVec3::new(1.0, 0.0, 0.0));
    link.set(picked, "Transform", "translation", at);
    let made = Transform {
        translation: DVec3::new(0.0, 2.0, 0.0),
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
    assert_eq!(translation, Some(DVec3::X), "the set stays");
    assert_eq!(
        world.entities(None).expect("all").len(),
        1,
        "the create undone"
    );
    // Refused at the first frame's tick, shown from the next.
    link.create(vec![("Nothing".into(), Value::Bool(true))]);
    frame(&mut inspector, &mut world, &ctx, 1.0, Vec::new());
    let (texts, _) = frame(&mut inspector, &mut world, &ctx, 2.0, Vec::new());
    let said = inspector.not_done.clone().expect("refused");
    assert!(texts.iter().any(|(text, _)| *text == said), "{texts:?}");
}

/// A link set of a component the picked entity lacks (no field drawn) is refused above its
/// components, like an operation, not beside a field.
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
    // Refused at the first frame's tick, shown from the next.
    link.set(picked, "Sprite", "color", Value::Vec4(Vec4::ONE));
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

    /// How long a test waits for what should come in a few frames, before it fails, not hangs.
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

    /// A frame of a remote Inspector's panels, drawn as [`panels`] does: its `ui` takes in the
    /// game's answers first, then sends the pass and tells its link the pick and its place.
    /// `give` adds to the pass before it's sent. Returns the texts.
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

    /// Runs frames of `game` and `inspector`, the first given `give`, until `until` holds of the
    /// texts and the game. Returns the texts.
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
        transform.expect("a transform").translation.x as f32
    }

    /// A remote Inspector, through a game's API:
    /// - lists the game's entities as they come and go;
    /// - shows the pick, its components in registration order;
    /// - sets a field in a gesture one undo reverses, its Edit menu naming the edit;
    /// - creates and picks an entity, as for one its link creates.
    ///
    /// A link set of a component the entity lacks is refused above its components.
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
        let texts = until(
            &mut inspector,
            &mut game,
            &ctx,
            &mut time,
            |_| {},
            |texts, _| has(texts, "Hero") && has(texts, "2 entities"),
        );
        assert!(!has(&texts, "Kid"), "{texts:?}");
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
        // Placed in world space by the read that shows it, the Inspector's first reach.
        let placed = inspector.link().placed().expect("placed");
        let expected = Placed {
            id: hero_id,
            local: Transform::IDENTITY,
            space: crate::ecs::Pose::IDENTITY,
            bounds: None,
            node: false,
            generation: 1,
        };
        assert_eq!(placed, expected);
        let own = game.cx.world().components_of(hero).expect("alive");
        let own: Vec<&str> = own.collect();
        let drawn: Vec<&str> = (texts.iter())
            .filter_map(|text| own.iter().find(|&&name| name == text).copied())
            .collect();
        assert_eq!(drawn, own, "in the order they registered");
        let set = |x: f32| Edit {
            component: "Transform".to_owned(),
            path: "translation.x".to_owned(),
            value: Value::F64(f64::from(x)),
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
        game.cx.world_mut().despawn(kid).expect("alive");
        until(
            &mut inspector,
            &mut game,
            &ctx,
            &mut time,
            |_| {},
            |texts, _| has(texts, "1 entity"),
        );
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
        let made = Transform {
            translation: DVec3::new(0.0, 3.0, 0.0),
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
        // A component it lacks: the sprite's color.
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
        let nameless = game.cx.world_mut().spawn(Transform::IDENTITY);
        let nameless = id(game.cx.world(), nameless);
        until(
            &mut inspector,
            &mut game,
            &ctx,
            &mut time,
            |_| {},
            |texts, _| has(texts, "Entity"),
        );
        // The hero is away from the origin, turned and scaled; the rock stays clear as its child.
        let away = Transform {
            translation: DVec3::new(10.0, 0.0, -2.0),
            rotation: Quat::from_rotation_z(0.5),
            scale: Vec3::splat(2.0),
        };
        game.cx.world_mut().insert(hero, away).expect("alive");
        // The rock renamed, made the hero's last child and a root again (kept in place in the
        // world); the nameless one named. Each as the game takes it.
        let entity = |game: &Game, id| game.cx.world().entity(id).expect("alive");
        let name = |game: &Game, id| game.cx.world().get::<Name>(entity(game, id)).cloned();
        let parent = |game: &Game, id| game.cx.world().parent(entity(game, id)).expect("alive");
        let kept = |game: &Game| {
            let pose = game.cx.world().pose(entity(game, rock)).expect("alive");
            let at = pose.matrix.translation;
            at.abs_diff_eq(made.translation, 1e-4)
        };
        let mut operate = |command: Command, done: &dyn Fn(&Game) -> bool| {
            let give = |pass: &mut Pass| pass.operations.push(command);
            until(
                &mut inspector,
                &mut game,
                &ctx,
                &mut time,
                give,
                |_, game| done(game),
            );
        };
        let stone = Command::Rename(rock, "Stone".to_owned());
        operate(stone, &|game| name(game, rock) == Some(called("Stone")));
        let under = Command::Reparent(rock, Some(hero_id));
        operate(under, &|game| {
            parent(game, rock) == Some(hero) && kept(game)
        });
        operate(Command::Reparent(rock, None), &|game| {
            parent(game, rock).is_none() && kept(game)
        });
        let found = Command::Rename(nameless, "Found".to_owned());
        operate(found, &|game| name(game, nameless) == Some(called("Found")));
    }
}
