use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::dirs::tests::Scratch;
use crate::layer::Commands;
use crate::reflect::Reflect;
use crate::{Context, Layer, Tick};

/// A command of [`Walker`]'s.
#[derive(Clone, Copy, Debug, Reflect)]
pub(crate) enum Walk {
    /// Puts the viewer at this x.
    Viewer(f32),
    /// Puts cell 1's entity at this x, if it's in.
    Crate(f32),
}

/// Builds a world of cells on attach, like [`streaming`], over [`project`]'s `cells` folder, and
/// moves what its commands say each tick.
#[derive(Default)]
pub(crate) struct Walker(Vec<Walk>);

impl Layer for Walker {
    fn attach(&mut self, cx: &mut Context) {
        let world = cx.world_mut();
        world.spawn(Cells {
            folder: "cells".to_owned(),
            size: 10.0,
        });
        world.spawn((at(5.0), Viewer { radius: 10.0 }));
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        for walk in self.0.drain(..) {
            let (moved, x) = match walk {
                Walk::Viewer(x) => (world.query::<(Entity, &Viewer)>().next().map(|(e, _)| e), x),
                Walk::Crate(x) => (world.entity(id_of(1)), x),
            };
            if let Some(moved) = moved {
                let transform = world.get_mut::<Transform>(moved).expect("placed");
                transform.translation.x = f64::from(x);
            }
        }
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.0)
    }
}

/// A walk across [`project`]'s cells: out to cell 3, cell 1's entity moved, away from all and back,
/// so cells come and go and cell 1 returns from its keep.
pub(crate) const WALK: [Option<Walk>; 7] = [
    None,
    Some(Walk::Viewer(25.0)),
    Some(Walk::Crate(12.0)),
    Some(Walk::Viewer(100.0)),
    None,
    Some(Walk::Viewer(5.0)),
    None,
];

/// A cell file's scene: an entity at each x, with its ID.
pub(crate) fn scene(entities: &[(u64, f32)]) -> String {
    let mut text = "(\n    entities: {\n".to_owned();
    for (id, x) in entities {
        text += &format!(
            "        \"{id:016x}\": (\n            Transform: (\n                translation: \
             ({x:?}, 0.0, 0.0),\n            ),\n        ),\n"
        );
    }
    text + "    },\n)\n"
}

/// The ID of a cell file's entity: 0x10 for cell 0, and so on.
pub(crate) fn id_of(cell: i32) -> EntityId {
    EntityId::new(0x10 + cell as u64).expect("an ID")
}

/// An asset root in `dir` whose `cells` folder holds four 10-unit cells along x, each with an
/// entity at its middle (ID by [`id_of`]), and a file that isn't a cell's.
pub(crate) fn project(dir: &Path) -> PathBuf {
    let root = dir.join("game");
    fs::create_dir_all(root.join("cells")).expect("a directory");
    for cell in 0..4 {
        let text = scene(&[(id_of(cell).get(), cell as f32 * 10.0 + 5.0)]);
        fs::write(root.join(format!("cells/{cell}_0_0.ron")), text).expect("a cell");
    }
    fs::write(root.join("cells/notes.ron"), scene(&[(1, 0.0)])).expect("not a cell");
    root
}

/// A context over asset root `root` whose world has `Cells` of `folder`, 10 units a side, and a
/// viewer of radius 10 at x 5, which it returns.
fn streaming(root: &Path, folder: &str) -> (Context, Entity) {
    let mut cx = Context::seeded(1).root(root.to_owned());
    let world = cx.world_mut();
    world.spawn(Cells {
        folder: folder.to_owned(),
        size: 10.0,
    });
    let viewer = world.spawn((at(5.0), Viewer { radius: 10.0 }));
    (cx, viewer)
}

fn at(x: f32) -> Transform {
    Transform {
        translation: DVec3::new(f64::from(x), 0.0, 0.0),
        ..Transform::IDENTITY
    }
}

fn move_to(cx: &mut Context, entity: Entity, x: f32) {
    let transform = cx.world_mut().get_mut::<Transform>(entity);
    transform.expect("placed").translation.x = f64::from(x);
}

fn cells_in(cx: &Context) -> Vec<i32> {
    xs(cx.world())
}

/// The x of each of `world`'s cells in.
pub(crate) fn xs(world: &World) -> Vec<i32> {
    world.cells_in().map(|cell| cell.0.x).collect()
}

/// The x of entity `id` if alive, and whether it streams.
fn found(cx: &Context, id: EntityId) -> Option<(f32, bool)> {
    let world = cx.world();
    let entity = world.entity(id)?;
    let x = world
        .get::<Transform>(entity)
        .expect("placed")
        .translation
        .x as f32;
    Some((x, world.get::<Streamed>(entity).is_some()))
}

fn state(cx: &Context) -> &CellState {
    cx.world().resource::<CellState>().expect("streamed")
}

/// The cells' step across a tick boundary: the end of one tick takes cells out and notes those
/// coming; the start of the next brings them in.
fn step(cx: &mut Context) {
    cx.stream_cells();
    cx.enter_coming();
}

/// The x of each cell coming in.
fn coming(cx: &Context) -> Vec<i32> {
    state(cx).coming.iter().map(|cell| cell.0.x).collect()
}

/// The x of each cell prepared.
fn prepared(cx: &Context) -> Vec<i32> {
    let cells = cx.cells.preparing.cells.iter();
    let ready = cells.filter(|(_, prep)| matches!(prep.stage, Stage::Ready(Ok(_))));
    ready.map(|(cell, _)| cell.0.x).collect()
}

/// A cell's file is named by its grid place, each integer as Rust writes it; no other name is one.
#[test]
fn a_cell_s_file_names_it_by_its_place() {
    let cell = Cell(IVec3::new(3, 0, -2));
    assert_eq!(cell.to_string(), "3_0_-2");
    assert_eq!(Cell::of_file("3_0_-2.ron"), Some(cell));
    for name in [
        "3_0_-2",
        "3_0.ron",
        "3_0_-2_1.ron",
        "03_0_-2.ron",
        "+3_0_-2.ron",
        "a_b_c.ron",
    ] {
        assert_eq!(Cell::of_file(name), None, "{name}");
    }
}

/// A point's cell is each axis's floor over the size, below the origin too; a world without
/// `Cells`, or with a size that isn't finite and positive, has none.
#[test]
fn a_point_is_in_the_cell_of_its_floor_over_the_size() {
    let cell = Cell::at(DVec3::new(25.0, -0.5, 0.0), 10.0);
    assert_eq!(cell, Cell(IVec3::new(2, -1, 0)));
    // 1,000 km out, a millimeter either side of an edge, which `f32` would round onto it.
    let edge = 1.0e6 + 10.0;
    let sides = [edge - 0.001, edge + 0.001].map(|x| Cell::at(DVec3::new(x, 0.0, -x), 10.0));
    assert_eq!(sides[0], Cell(IVec3::new(100_000, 0, -100_001)));
    assert_eq!(sides[1], Cell(IVec3::new(100_001, 0, -100_002)));
    let mut cx = Context::seeded(1);
    assert_eq!(cx.world().cell_at(DVec3::ZERO), None);
    let world = cx.world_mut();
    let cells = world.spawn(Cells {
        folder: "cells".to_owned(),
        size: 10.0,
    });
    assert_eq!(
        world.cell_at(DVec3::new(-1.0, 0.0, 0.0)),
        Some(Cell(IVec3::NEG_X))
    );
    world.get_mut::<Cells>(cells).expect("one").size = f32::NAN;
    assert_eq!(world.cell_at(DVec3::ZERO), None);
}

/// Cells come in, with their files' streaming entities, as a viewer comes within its radius, and
/// go out past its radius plus half a cell, keeping their entities as they were: one moved within
/// its cell returns where it was left, not where the file has it; one moved into a cell that's out
/// goes out at once and returns with that cell; a cell that goes out as it came keeps nothing and
/// returns from its file; what doesn't stream, or holds a viewer, stays. A file whose name isn't a
/// cell's counts for nothing.
#[test]
fn cells_come_in_near_a_viewer_and_go_out_keeping_what_they_held() {
    let logged = crate::testing::logged();
    let dir = Scratch::new("cells-come-and-go");
    let root = project(&dir.0);
    let (mut cx, viewer) = streaming(&root, "cells");
    step(&mut cx);
    assert_eq!(cells_in(&cx), [0, 1], "within 10 of x 5");
    assert_eq!(found(&cx, id_of(0)), Some((5.0, true)));
    assert_eq!(found(&cx, id_of(1)), Some((15.0, true)));
    assert_eq!(found(&cx, id_of(2)), None);
    assert!(
        cx.world()
            .entity(EntityId::new(1).expect("an ID"))
            .is_none()
    );

    move_to(&mut cx, viewer, 25.0);
    step(&mut cx);
    assert_eq!(
        cells_in(&cx),
        [0, 1, 2, 3],
        "cell 0 is 15 away, its radius and half a cell"
    );
    let one = cx.world().entity(id_of(1)).expect("in");
    move_to(&mut cx, one, 12.0);
    // Into cell -1, which has no file, 25 away.
    let three = cx.world().entity(id_of(3)).expect("in");
    move_to(&mut cx, three, -5.0);
    let world = cx.world_mut();
    let unmarked = world.spawn(at(35.0));
    let holder = world.spawn((at(35.0), Streamed));
    let far = world.spawn((at(-135.0), Viewer { radius: 0.0 }));
    world.set_parent(far, Some(holder)).expect("no cycle");
    step(&mut cx);
    assert_eq!(found(&cx, id_of(3)), None, "gone out at once");
    let kept = &state(&cx).kept[&Cell(IVec3::NEG_X)];
    assert_eq!(kept.texts.len(), 1);
    assert!(
        !state(&cx).files.contains_key(&Cell(IVec3::NEG_X)),
        "it has no file"
    );

    move_to(&mut cx, viewer, 26.0);
    step(&mut cx);
    assert_eq!(cells_in(&cx), [1, 2, 3], "cell 0 is 16 away");
    assert_eq!(found(&cx, id_of(0)), None);

    move_to(&mut cx, viewer, 100.0);
    step(&mut cx);
    assert_eq!(cells_in(&cx), [0; 0]);
    assert_eq!(found(&cx, id_of(1)), None);
    let world = cx.world();
    assert!(world.get::<Transform>(unmarked).is_some(), "doesn't stream");
    assert!(world.get::<Transform>(holder).is_some(), "holds a viewer");
    let kept: Vec<i32> = state(&cx).kept.keys().map(|cell| cell.0.x).collect();
    assert_eq!(
        kept,
        [-1, 1],
        "cells 0 and 2 went out as they came, and cell 3 holding nothing"
    );
    let files: Vec<i32> = state(&cx).files.keys().map(|cell| cell.0.x).collect();
    assert_eq!(files, [1, 3], "so their files don't come again");

    move_to(&mut cx, viewer, 5.0);
    step(&mut cx);
    assert_eq!(cells_in(&cx), [-1, 0, 1]);
    assert_eq!(
        found(&cx, id_of(0)),
        Some((5.0, true)),
        "from its file again"
    );
    assert_eq!(
        found(&cx, id_of(1)),
        Some((12.0, true)),
        "where it was left"
    );
    assert_eq!(found(&cx, id_of(3)), Some((-5.0, true)), "with cell -1");
    let codes: Vec<String> = logged.codes.try_iter().collect();
    let cells: Vec<&str> = (codes.iter())
        .filter(|code| code.starts_with("cells."))
        .map(String::as_str)
        .collect();
    let count = |code: &str| cells.iter().filter(|logged| **logged == code).count();
    assert_eq!(count("cells.entered"), 7, "{cells:?}");
    assert_eq!(count("cells.left"), 4, "{cells:?}");
    assert_eq!(cells.len(), 11, "nothing else: {cells:?}");
}

/// A cell that goes out as it came from its file keeps nothing and its file is no longer noted as
/// come in, so the world holds only cells in and changed, not those a viewer crossed; it returns
/// from its file. One changed keeps what it holds, and keeps it again as it goes out as it came
/// back, which its file no longer stands for.
#[test]
fn a_cell_going_out_as_it_came_keeps_nothing() {
    let dir = Scratch::new("cells-as-they-came");
    let root = project(&dir.0);
    let (mut cx, viewer) = streaming(&root, "cells");
    let keys = |cells: &mut dyn Iterator<Item = &Cell>| cells.map(|cell| cell.0.x).collect();
    for x in [5.0, 15.0, 25.0, 35.0, 100.0] {
        move_to(&mut cx, viewer, x);
        step(&mut cx);
    }
    assert_eq!(
        state(&cx),
        &CellState::default(),
        "crossed, holding nothing"
    );

    move_to(&mut cx, viewer, 5.0);
    step(&mut cx);
    let came: Vec<i32> = keys(&mut state(&cx).came_as.keys());
    assert_eq!(came, [0, 1]);
    let zero = cx.world().entity(id_of(0)).expect("in from its file");
    move_to(&mut cx, zero, 6.0);
    move_to(&mut cx, viewer, 100.0);
    step(&mut cx);
    let kept: Vec<i32> = keys(&mut state(&cx).kept.keys());
    assert_eq!(kept, [0]);
    let files: Vec<i32> = keys(&mut state(&cx).files.keys());
    assert_eq!(files, [0], "so its file doesn't come again");

    move_to(&mut cx, viewer, 5.0);
    step(&mut cx);
    assert_eq!(found(&cx, id_of(0)), Some((6.0, true)));
    let came: Vec<i32> = keys(&mut state(&cx).came_as.keys());
    assert_eq!(came, [1], "cell 0 came as it kept");
    move_to(&mut cx, viewer, 100.0);
    step(&mut cx);
    let kept: Vec<i32> = keys(&mut state(&cx).kept.keys());
    assert_eq!(kept, [0], "kept again");
}

/// A cell whose file text the run takes again while in, as a file save applies, no longer goes out
/// as it came but keeps what it holds, so an edit since (even one undoing the save) stays; another
/// cell's goes out as it came.
#[test]
fn a_cell_whose_file_changed_since_it_came_keeps_what_it_holds() {
    let dir = Scratch::new("cells-file-changed");
    let root = project(&dir.0);
    let (mut cx, viewer) = streaming(&root, "cells");
    step(&mut cx);
    let path = "cells/0_0_0.ron".to_owned();
    let text = scene(&[(id_of(0).get(), 6.0)]);
    let edit = crate::edits::Edit::Scene { path, text };
    cx.apply_edit(edit).expect("kept");
    let came: Vec<i32> = state(&cx).came_as.keys().map(|cell| cell.0.x).collect();
    assert_eq!(came, [1]);
    move_to(&mut cx, viewer, 100.0);
    step(&mut cx);
    let kept: Vec<i32> = state(&cx).kept.keys().map(|cell| cell.0.x).collect();
    assert_eq!(kept, [0]);
    move_to(&mut cx, viewer, 5.0);
    step(&mut cx);
    assert_eq!(
        found(&cx, id_of(0)),
        Some((5.0, true)),
        "as it was, not as its file is now"
    );
}

/// Like [`make`], as it loads.
struct Maker;

impl Layer for Maker {
    fn load(&mut self, cx: &mut Context) -> Result<()> {
        make(cx);
        Ok(())
    }
}

/// Makes the fileless cells of `cells` from -2 to 5 along x: an entity at each middle, with ID
/// [`made_id`].
fn make(cx: &mut Context) {
    let [from, to] = [-2, 5].map(|x| Cell(IVec3::new(x, 0, 0)));
    cx.make_cells("cells", from, to, |cell| {
        let x = cell.0.x;
        scene(&[(made_id(x).get(), x as f32 * 10.0 + 5.0)])
    });
}

/// The ID of cell `x`'s made entity.
fn made_id(x: i32) -> EntityId {
    EntityId::new((0x100 + x) as u64).expect("an ID")
}

/// Made cells come in around a viewer like file cells, from the game's making; a file replaces the
/// making for its cell; none comes in outside what's made; the run keeps none of their texts; each
/// goes out as it came, keeping nothing, so a viewer crossing them leaves nothing behind.
#[test]
fn the_cells_a_game_makes_come_in_as_those_of_files_do() {
    let dir = Scratch::new("cells-made");
    let root = project(&dir.0);
    let (mut cx, viewer) = streaming(&root, "cells");
    make(&mut cx);
    // As a run's ticks do, taking what preparing finished.
    let step = |cx: &mut Context| {
        cx.stream_cells();
        assert!(cx.cells_ready());
        cx.enter_coming();
    };
    step(&mut cx);
    assert_eq!(cells_in(&cx), [-1, 0, 1]);
    assert_eq!(found(&cx, made_id(-1)), Some((-5.0, true)), "made");
    assert_eq!(found(&cx, id_of(0)), Some((5.0, true)), "its file's");
    assert_eq!(found(&cx, made_id(0)), None, "its file stands for it");

    move_to(&mut cx, viewer, 45.0);
    step(&mut cx);
    assert_eq!(cells_in(&cx), [3, 4, 5]);
    assert_eq!(found(&cx, made_id(5)), Some((55.0, true)));
    move_to(&mut cx, viewer, 80.0);
    step(&mut cx);
    assert_eq!(cells_in(&cx), [0; 0], "cells 7 and 8 aren't made");
    assert_eq!(
        state(&cx),
        &CellState::default(),
        "crossed, holding nothing"
    );
    let made = |file: &crate::scene_files::SceneFile| {
        ["cells/-1_0_0.ron", "cells/4_0_0.ron", "cells/5_0_0.ron"].contains(&&*file.author)
    };
    assert!(!cx.scenes.cached.iter().any(made), "made again, not kept");

    // One changed keeps what it holds, as a file's does.
    move_to(&mut cx, viewer, 55.0);
    step(&mut cx);
    let five = cx.world().entity(made_id(5)).expect("in");
    move_to(&mut cx, five, 56.0);
    move_to(&mut cx, viewer, 100.0);
    step(&mut cx);
    let kept: Vec<i32> = state(&cx).kept.keys().map(|cell| cell.0.x).collect();
    assert_eq!(kept, [5]);
    move_to(&mut cx, viewer, 55.0);
    step(&mut cx);
    assert_eq!(found(&cx, made_id(5)), Some((56.0, true)), "as it was left");
}

/// Cells within a viewer's radius plus a cell are prepared ahead as a tick ends and come in from
/// that; a tick whose cells coming in aren't prepared waits, runs once they are, and logs how long
/// it waited; what's prepared of cells no viewer is near any more is dropped.
#[test]
fn cells_are_prepared_ahead_and_a_tick_waits_for_its_own() {
    let logged = crate::testing::logged();
    let dir = Scratch::new("cells-prepared");
    let root = project(&dir.0);
    let (mut cx, viewer) = streaming(&root, "cells");
    cx.stream_cells();
    assert_eq!(coming(&cx), [0, 1]);
    assert_eq!(prepared(&cx), [0; 0], "on their way");
    cx.prepare_cells();
    assert_eq!(prepared(&cx), [0, 1, 2], "cell 2 is 15 away");
    let zero = Cell(IVec3::ZERO);
    // On no thread: it stays unprepared.
    cx.cells.preparing.begin(zero);
    assert!(!cx.cells_ready());
    assert!(!cx.cells_ready(), "still");
    cx.cells.preparing.cells.remove(&zero);
    assert!(cx.cells_ready(), "prepared again, then and there");
    cx.enter_coming();
    assert_eq!(cells_in(&cx), [0, 1]);
    assert_eq!(prepared(&cx), [2], "taken as they came in");
    // Cell 2's preparing stands for its file (x 25): as it comes in, it puts the entity at 99.
    let two = Cell(IVec3::new(2, 0, 0));
    let moved = scene(&[(id_of(2).get(), 99.0)]);
    let parsed = cx.world().scratch().parse_scene(&moved).expect("parsed");
    let prep = cx.cells.preparing.cells.get_mut(&two).expect("prepared");
    let Stage::Ready(Ok(prepared)) = &mut prep.stage else {
        panic!("prepared");
    };
    let digest = prepared.file.expect("its file's");
    prepared.entering.file = Some((digest, parsed));
    move_to(&mut cx, viewer, 15.0);
    cx.stream_cells();
    cx.prepare_cells();
    cx.enter_coming();
    assert_eq!(cells_in(&cx), [0, 1, 2]);
    assert_eq!(found(&cx, id_of(2)), Some((99.0, true)), "as prepared");
    // Cell 3's preparing stands for a file it no longer has, as after a save: it comes in from its
    // file.
    let three = Cell(IVec3::new(3, 0, 0));
    let moved = scene(&[(id_of(3).get(), 99.0)]);
    let parsed = cx.world().scratch().parse_scene(&moved).expect("parsed");
    let prep = cx.cells.preparing.cells.get_mut(&three).expect("prepared");
    let Stage::Ready(Ok(prepared)) = &mut prep.stage else {
        panic!("prepared");
    };
    let digest = prepared.file.expect("its file's") ^ 1;
    prepared.file = Some(digest);
    prepared.entering.file = Some((digest, parsed));
    move_to(&mut cx, viewer, 25.0);
    step(&mut cx);
    assert_eq!(cells_in(&cx), [0, 1, 2, 3]);
    assert_eq!(
        found(&cx, id_of(3)),
        Some((35.0, true)),
        "as its file has it"
    );
    // Cells 0 to 2 go out, and cell 2, 20 away, is prepared ahead again from the tick after.
    move_to(&mut cx, viewer, 50.0);
    step(&mut cx);
    assert_eq!(cells_in(&cx), [3]);
    assert!(
        !cx.cells.preparing.cells.contains_key(&two),
        "in as the plan looked"
    );
    step(&mut cx);
    assert!(cx.cells.preparing.cells.contains_key(&two));
    move_to(&mut cx, viewer, 100.0);
    step(&mut cx);
    assert!(cx.cells.preparing.cells.is_empty(), "none near");
    let codes: Vec<String> = logged.codes.try_iter().collect();
    let count = |code: &str| codes.iter().filter(|logged| *logged == code).count();
    assert_eq!(count("cells.waited"), 1, "{codes:?}");
    assert_eq!(count("cells.prepared"), 5, "{codes:?}");
}

/// Cells prepared on threads, one naming a model no layer loaded (a thread reads it), come in as if
/// prepared on the spot: a SyncTest, which reruns each tick on a world copy, preparing cells there,
/// finds every tick alike.
#[test]
fn cells_prepared_on_threads_come_in_as_prepared_on_the_spot() {
    use crate::App;

    let logged = crate::testing::logged();
    let dir = Scratch::new("cells-threads");
    let root = project(&dir.0);
    fs::create_dir_all(root.join("assets")).expect("a directory");
    fs::write(root.join("assets/m.glb"), crate::gltf::tests::model()).expect("a model");
    let model = "        \"0000000000000100\": (\n            ModelRoot: (\"assets/m.glb\"),\n        \
                 ),\n    },\n)\n";
    let one = scene(&[(id_of(1).get(), 15.0)]).replace("    },\n)\n", model);
    fs::write(root.join("cells/1_0_0.ron"), one).expect("a cell");
    let mut run = App::new("Walking").root(&root).layer(Walker::default());
    run.stack.sync_test();
    let mut cx = Context::seeded(1).root(root.clone());
    cx.cells.preparing = Preparing::threaded();
    run.stack.attach(&mut cx).expect("loaded");
    let started = std::time::Instant::now();
    let mut number = 0;
    let mut queued = None;
    while number < WALK.len() as u64 {
        if queued != Some(number) {
            queued = Some(number);
            if let Some(walk) = WALK[number as usize] {
                let queue = run.stack.commands_of(0).expect("a queue");
                queue.push(&walk.to_value()).expect("queued");
            }
        }
        if run.stack.tick(&mut cx, Tick { number }) {
            number += 1;
        } else {
            assert!(started.elapsed().as_secs() < 30, "prepared at last");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    assert!(!cx.exiting(), "every tick alike");
    let matched = run.stack.sync_test_mut().map(|sync| sync.matched());
    assert_eq!(matched, Some(WALK.len() as u64));
    assert_eq!(cells_in(&cx), [0, 1]);
    assert_eq!(found(&cx, id_of(1)), Some((12.0, true)), "as it was left");
    let world = cx.world();
    let model = world
        .entity(EntityId::new(0x100).expect("an ID"))
        .expect("in");
    let nodes = world.children(model).expect("alive").count();
    assert!(nodes > 0, "its model's nodes");
    let codes: Vec<String> = logged.codes.try_iter().collect();
    let count = |code: &str| codes.iter().filter(|logged| *logged == code).count();
    assert!(count("cells.prepared") >= 4, "{codes:?}");
    assert_eq!(count("cells.refused"), 0, "{codes:?}");
    run.stack.detach(&mut cx);
}

/// A model's add takes the files a cell's preparing read on a thread, if it did, instead of reading
/// again, and reads them itself after.
#[test]
fn a_model_s_add_takes_its_files_as_a_thread_read_them() {
    let dir = Scratch::new("cells-model-read");
    let root = project(&dir.0);
    let file = root.join("assets/m.glb");
    fs::create_dir_all(root.join("assets")).expect("a directory");
    fs::write(&file, crate::gltf::tests::model()).expect("a model");
    let mut cx = Context::seeded(1).root(root);
    let read = ModelRead {
        at: std::time::SystemTime::now(),
        read: Err(Error::new("engine.bad_model", "read on a thread")),
    };
    cx.cells.preparing.models.insert(file.clone(), read);
    let refused = cx.add_model("m", &file).expect_err("as the thread read it");
    assert_eq!(refused.to_string(), "read on a thread");
    assert!(cx.cells.preparing.models.is_empty(), "taken");
    cx.add_model("m", &file).expect("read again, here");
}

/// A server's client streams no cells itself: they come and go with its server's states.
#[test]
fn a_server_s_client_brings_no_cells_in() {
    let dir = Scratch::new("cells-client");
    let root = project(&dir.0);
    let (mut cx, _) = streaming(&root, "cells");
    cx.client = true;
    step(&mut cx);
    assert_eq!(cells_in(&cx), [0; 0]);
    assert!(cx.world().resource::<CellState>().is_none());
}

/// A cell whose file gives an ID that's alive is refused, warning once, and stays out without
/// retrying while a viewer stays near; once every viewer has gone, it retries as one returns.
#[test]
fn a_cell_refused_stays_out_until_its_viewers_go() {
    let logged = crate::testing::logged();
    let dir = Scratch::new("cells-refused");
    let root = project(&dir.0);
    let (mut cx, viewer) = streaming(&root, "cells");
    let taken = cx
        .world_mut()
        .load_scene(&scene(&[(id_of(0).get(), 500.0)]));
    let taken = taken.expect("loaded")[0];
    step(&mut cx);
    assert_eq!(cells_in(&cx), [1]);
    assert_eq!(state(&cx).refused, BTreeSet::from([Cell(IVec3::ZERO)]));
    cx.world_mut().despawn(taken).expect("alive");
    step(&mut cx);
    assert_eq!(cells_in(&cx), [1], "not again while the viewer is near");
    let codes: Vec<String> = logged.codes.try_iter().collect();
    let refused = codes.iter().filter(|code| *code == "cells.refused");
    assert_eq!(refused.count(), 1, "{codes:?}");

    move_to(&mut cx, viewer, 100.0);
    step(&mut cx);
    assert!(state(&cx).refused.is_empty(), "forgotten");
    move_to(&mut cx, viewer, 5.0);
    step(&mut cx);
    assert_eq!(cells_in(&cx), [0, 1]);
    assert_eq!(found(&cx, id_of(0)), Some((5.0, true)));
}

/// A scene change empties the world's cells' state with its entities, and a world with empty cell
/// state hashes as one that never had cells; worlds differing only in a cell's keep hash apart, by
/// layout alone.
#[test]
fn a_scene_change_empties_the_cells_and_what_cells_keep_is_hashed() {
    let dir = Scratch::new("cells-hashed");
    let root = project(&dir.0);
    fs::create_dir_all(root.join("scenes")).expect("a directory");
    fs::write(root.join("scenes/plain.ron"), scene(&[(1, 0.0)])).expect("a scene");
    let kept = |x: f32| {
        let (mut cx, viewer) = streaming(&root, "cells");
        step(&mut cx);
        let zero = cx.world().entity(id_of(0)).expect("in");
        move_to(&mut cx, zero, x);
        move_to(&mut cx, viewer, 100.0);
        step(&mut cx);
        cx
    };
    let (one, other) = (kept(7.0), kept(6.0));
    let (one, other) = (one.world().hash(), other.world().hash());
    assert_eq!(one.content, other.content, "neither holds the entity");
    assert_ne!(one.layout, other.layout, "the texts kept differ");
    // Alike but for a cell coming, a came-in file's digest, or what a cell in came as.
    let [mut coming, mut filed, mut came] = [kept(6.0), kept(6.0), kept(6.0)];
    let alike = coming.world().hash();
    let two = Cell(IVec3::new(2, 0, 0));
    coming.world_mut().note_coming(&[two]);
    fn state_mut(cx: &mut Context) -> &mut CellState {
        let state = cx.world_mut().resource_mut::<CellState>();
        state.expect("streamed")
    }
    *state_mut(&mut filed)
        .files
        .get_mut(&Cell(IVec3::ZERO))
        .expect("came") ^= 1;
    state_mut(&mut came).came_as.insert(two, 1);
    let changed = [
        (&coming, "a cell coming"),
        (&filed, "a file's digest"),
        (&came, "what a cell came as"),
    ];
    for (changed, what) in changed {
        let hash = changed.world().hash();
        assert_eq!(hash.content, alike.content, "{what}");
        assert_ne!(hash.layout, alike.layout, "{what}");
    }

    let mut cx = kept(5.0);
    cx.change_scene("scenes/plain.ron").expect("in");
    assert_eq!(state(&cx), &CellState::default());
    let emptied = cx.world().hash();
    cx.world_mut().remove_resource::<CellState>();
    assert_eq!(cx.world().hash(), emptied);
}

/// A world's text (snapshot, lockstep joiner's world, server state) holds what its cells keep; a
/// world read from it has the same, and hashes alike.
#[test]
fn a_world_s_text_holds_what_its_cells_keep() {
    let dir = Scratch::new("cells-saved");
    let root = project(&dir.0);
    let (mut cx, viewer) = streaming(&root, "cells");
    step(&mut cx);
    let zero = cx.world().entity(id_of(0)).expect("in");
    move_to(&mut cx, zero, 6.0);
    move_to(&mut cx, viewer, 25.0);
    step(&mut cx);
    move_to(&mut cx, viewer, 26.0);
    step(&mut cx);
    assert_eq!(state(&cx).kept.len(), 1);
    assert_eq!(state(&cx).came_as.len(), 3, "cells 1 to 3, in as they came");
    let mut text = String::new();
    cx.world().save_whole(&mut text).expect("saved");
    let mut read = Context::seeded(2);
    let lines = (1..).zip(text.lines());
    read.world_mut().load_whole(lines, false).expect("read");
    assert_eq!(state(&read), state(&cx));
    assert_eq!(read.world().hash(), cx.world().hash());
    // A snapshot from before came-as was saved: none came as their files have them.
    let old = r#"{"cells_in":[[1,0,0]],"coming":[],"files":[],"kept":[],"refused":[]}"#;
    let old = CellState::load(old, read.world()).expect("read");
    assert_eq!(old.came_as.len(), 0);
    assert_eq!(old.cells_in.len(), 1);
}

/// What a component that aims at an entity holds.
#[derive(Clone, Component)]
struct Aim(Entity);

/// A server's client's view of a world, written as its states are and read as its copy reads it,
/// hashes as `hash_seen` does: unseen cells' streamed roots gone with what's under them, references
/// to them dead, their slots retired so the next spawn takes the server's slot; the cells' state
/// its own: the cells it sees in, nothing kept. A world without cells is seen whole.
#[test]
fn a_client_s_view_loads_as_it_hashes() {
    let dir = Scratch::new("cells-seen");
    let root = project(&dir.0);
    let (mut cx, viewer) = streaming(&root, "cells");
    let world = cx.world_mut();
    world.insert(viewer, Owner { player: 0 }).expect("alive");
    // A viewer no player owns, keeping cells 2 and 3 in, which is no client's.
    world.spawn((at(35.0), Viewer { radius: 10.0 }));
    step(&mut cx);
    assert_eq!(cells_in(&cx), [0, 1, 2, 3]);
    let world = cx.world_mut();
    let child = world.spawn(at(0.0));
    let far = world.entity(id_of(3)).expect("in");
    world.set_parent(child, Some(far)).expect("no cycle");
    let aimed = world.entity(id_of(2)).expect("in");
    let aimer = world.spawn(Aim(aimed));
    // A freed slot, which the server's next spawn takes, so the client's must.
    let freed = world.spawn(at(0.0));
    world.despawn(freed).expect("alive");

    let world = cx.world();
    let seen = world.seen_by(0, &BTreeSet::new());
    let mine = BTreeSet::from([0, 1].map(|x| Cell(IVec3::new(x, 0, 0))));
    let cells = CellState {
        cells_in: mine,
        ..CellState::default()
    };
    assert_eq!(seen.cells, cells);
    let mut text = String::new();
    world.save_seen(&mut text, Some(&seen)).expect("saved");
    let mut client = world.clone();
    let lines = (1..).zip(text.lines());
    client.load_whole(lines, true).expect("read");
    assert_eq!(client.hash(), world.hash_seen(Some(&seen)));
    assert_ne!(client.hash(), world.hash());
    let shown = [0, 1, 2, 3].map(|cell| client.entity(id_of(cell)).is_some());
    assert_eq!(shown, [true, true, false, false]);
    let id = |entity: Entity| world.id(entity).expect("alive");
    assert_eq!(client.entity(id(child)), None, "under a root hidden");
    let aimer = client.entity(id(aimer)).expect("shown");
    let aim = client.get::<Aim>(aimer).map(|aim| aim.0);
    assert_eq!(aim, Some(Entity::DEAD));
    assert_eq!(client.resource::<CellState>(), Some(&cells));
    let mut server = world.clone();
    let spawned = [&mut client, &mut server].map(|world| world.spawn(at(0.0)).slot());
    assert_eq!(spawned, [freed.slot(); 2]);
    let spawned = [&mut client, &mut server].map(|world| world.spawn(at(0.0)).slot());
    assert_eq!(
        spawned[0], spawned[1],
        "a new slot, the hidden ones retired"
    );

    let mut plain = Context::seeded(3);
    plain.world_mut().spawn(at(1.0));
    let world = plain.world();
    let seen = world.seen_by(0, &BTreeSet::new());
    assert_eq!(seen, Seen::default());
    let (mut whole, mut viewed) = (String::new(), String::new());
    world.save_whole(&mut whole).expect("saved");
    world.save_seen(&mut viewed, Some(&seen)).expect("saved");
    assert_eq!(viewed, whole);
    assert_eq!(world.hash_seen(Some(&seen)), world.hash());
}

/// A server's client sees the cells in within its own viewers' radius (a viewer is its own if it
/// or an ancestor is its player's, per `Owner`) and keeps one it saw to the radius plus half a
/// cell, like the cells' step; it sees none through a viewer no player owns, or without a viewer;
/// and no view hides a streamed root holding the client's own viewer.
#[test]
fn a_client_sees_the_cells_near_its_own_viewers() {
    let dir = Scratch::new("cells-seen-by");
    let root = project(&dir.0);
    // A viewer no player owns, at x 5, keeping cells 0 and 1 in.
    let (mut cx, _) = streaming(&root, "cells");
    step(&mut cx);
    // What `player` sees, given `had`, and the cells in whose roots it hides.
    let seen = |cx: &Context, player: u8, had: &[i32]| {
        let world = cx.world();
        let had = had.iter().map(|&x| Cell(IVec3::new(x, 0, 0))).collect();
        let seen = world.seen_by(player, &had);
        let cells: Vec<i32> = seen.cells.cells_in.iter().map(|cell| cell.0.x).collect();
        let hidden = (0..4).filter(|&cell| {
            let entity = world.entity(id_of(cell));
            entity.is_some_and(|entity| seen.hidden.get(entity.slot()) == Some(&true))
        });
        (cells, hidden.collect::<Vec<i32>>())
    };
    assert_eq!(
        seen(&cx, 0, &[]),
        (vec![], vec![0, 1]),
        "no player's viewer"
    );

    let world = cx.world_mut();
    let body = world.spawn((at(24.0), Owner { player: 0 }));
    let eye = world.spawn((Transform::IDENTITY, Viewer { radius: 10.0 }));
    world.set_parent(eye, Some(body)).expect("no cycle");
    step(&mut cx);
    assert_eq!(cells_in(&cx), [0, 1, 2, 3]);
    assert_eq!(seen(&cx, 0, &[]), (vec![1, 2, 3], vec![0]));
    assert_eq!(seen(&cx, 0, &[0]), (vec![0, 1, 2, 3], vec![]), "kept");
    assert_eq!(seen(&cx, 1, &[0]), (vec![], vec![0, 1, 2, 3]), "no viewer");
    move_to(&mut cx, body, 26.0);
    assert_eq!(
        seen(&cx, 0, &[0]),
        (vec![1, 2, 3], vec![0]),
        "past half a cell"
    );

    // In cell -1, which isn't in.
    cx.world_mut().insert(body, Streamed).expect("alive");
    move_to(&mut cx, body, -5.0);
    let world = cx.world();
    let view = world.seen_by(0, &BTreeSet::new());
    assert_eq!(seen(&cx, 0, &[]), (vec![0], vec![1, 2, 3]));
    assert_ne!(
        view.hidden.get(body.slot()),
        Some(&true),
        "its own viewer's root"
    );
}

/// Past [`MOST_CELLS`] the nearest come in and the plan says full; with cells made everywhere, a
/// radius of thousands of cells searches a bounded box, whose nearest come in as far as a flat
/// world's reach.
#[test]
fn past_the_most_cells_the_nearest_come_in() {
    let mut cx = Context::seeded(1);
    let world = cx.world_mut();
    world.spawn((at(2500.5), Viewer { radius: 5000.0 }));
    let listed: Vec<Cell> = (0..5000).map(|x| Cell(IVec3::new(x, 0, 0))).collect();
    let mut plan = Plan::default();
    cx.world().plan_cells(1.0, &listed, None, None, &mut plan);
    assert!(plan.full);
    assert_eq!(plan.entering.len(), MOST_CELLS);
    let xs = |at: usize| plan.entering[at].0.x;
    assert_eq!((xs(0), xs(MOST_CELLS - 1)), (2500 - 2048, 2500 + 2047));

    let everywhere = (Cell(IVec3::splat(i32::MIN)), Cell(IVec3::splat(i32::MAX)));
    cx.world()
        .plan_cells(1.0, &[], Some(everywhere), None, &mut plan);
    assert!(plan.full);
    assert_eq!(plan.entering.len(), MOST_CELLS);
    let middle = IVec3::new(2500, 0, 0);
    let away = |plan: &Plan| {
        let away = plan.entering.iter();
        away.map(|cell| (cell.0 - middle).abs().max_element()).max()
    };
    assert!(away(&plan) <= Some(10), "the nearest");
    // In a one-cell-deep world, a disc: the 4,096 nearest reach some 36 cells away.
    let flat = (
        Cell(IVec3::new(i32::MIN, 0, i32::MIN)),
        Cell(IVec3::new(i32::MAX, 0, i32::MAX)),
    );
    cx.world().plan_cells(1.0, &[], Some(flat), None, &mut plan);
    assert!(plan.full);
    assert_eq!(plan.entering.len(), MOST_CELLS);
    assert!(
        (30..=37).contains(&away(&plan).expect("in")),
        "{:?}",
        away(&plan)
    );
}

/// `Cells` with a size that isn't finite and positive bring nothing in, warning once; of several,
/// the lowest ID streams, warning as the tie arises; a folder that can't be listed brings no cell
/// in from a file, warning once.
#[test]
fn cells_that_can_t_stream_warn_once() {
    let logged = crate::testing::logged();
    let dir = Scratch::new("cells-warned");
    let root = project(&dir.0);
    let (mut cx, _) = streaming(&root, "cells");
    let world = cx.world_mut();
    let first = world.query::<(Entity, &Cells)>().next().expect("one").0;
    world.get_mut::<Cells>(first).expect("one").size = 0.0;
    step(&mut cx);
    step(&mut cx);
    assert_eq!(cells_in(&cx), [0; 0]);
    cx.world_mut().get_mut::<Cells>(first).expect("one").size = 10.0;
    cx.world_mut().spawn(Cells {
        folder: "cells".to_owned(),
        size: 10.0,
    });
    step(&mut cx);
    step(&mut cx);
    assert_eq!(cells_in(&cx), [0, 1]);
    let (mut gone, _) = streaming(&root, "gone");
    step(&mut gone);
    step(&mut gone);
    assert_eq!(cells_in(&gone), [0; 0]);
    let codes: Vec<String> = logged.codes.try_iter().collect();
    let count = |code: &str| codes.iter().filter(|logged| *logged == code).count();
    assert_eq!(count("cells.bad_size"), 1, "{codes:?}");
    assert_eq!(count("cells.tie"), 1, "{codes:?}");
    assert_eq!(count("cells.listing"), 1, "{codes:?}");
}

/// A run from a game's package, as a build without `dev` runs, lists the cells' folder and reads
/// each file from it, though none is on the disk.
#[test]
fn a_run_from_a_package_lists_and_reads_its_cells_there() {
    use crate::package::{Kind, Package, Writer};

    let dir = Scratch::new("cells-package");
    let mut writer = Writer::new(".", None, None);
    for cell in [0, 1] {
        let text = scene(&[(id_of(cell).get(), cell as f32 * 10.0 + 5.0)]);
        let path = format!("cells/{cell}_0_0.ron");
        let added = writer.add(&path, None, Kind::File, text.into_bytes());
        added.expect("added");
    }
    let path = dir.0.join("game.ggpk");
    fs::create_dir_all(&dir.0).expect("a directory");
    writer.write(&path).expect("written");
    // Never made: read from the package.
    let root = dir.0.join("game");
    let (mut cx, _) = streaming(&root, "cells");
    let package = Arc::new(Package::open(&path).expect("opened"));
    cx.files = Files::package(package, root);
    step(&mut cx);
    assert_eq!(cells_in(&cx), [0, 1]);
    assert_eq!(found(&cx, id_of(1)), Some((15.0, true)));
}

/// Cells go out as each tick ends and those found coming come in as the next starts; a SyncTest
/// reruns that on a world copy alike, and a recording replays to every hash, its cells coming and
/// going at the ticks the run's did.
#[test]
fn a_streaming_run_runs_again_alike_and_replays_to_its_hashes() {
    use crate::App;
    use crate::recording::{Recorder, Replay};

    let dir = Scratch::new("cells-replay");
    let root = project(&dir.0);
    let app = || App::new("Walking").root(&root).layer(Walker::default());
    let path = dir.0.join("run.recording");
    let mut run = app();
    run.stack
        .record(Recorder::create(&path, run.name).expect("created"));
    run.stack.sync_test();
    let mut cx = Context::seeded(1).root(root.clone());
    run.stack.attach(&mut cx).expect("loaded");
    let mut seen = Vec::new();
    for (number, walk) in (0..).zip(WALK) {
        if let Some(walk) = walk {
            let queue = run.stack.commands_of(0).expect("a queue");
            queue.push(&walk.to_value()).expect("queued");
        }
        assert!(run.stack.tick(&mut cx, Tick { number }));
        let coming = state(&cx).coming.iter().map(|cell| cell.0.x).collect();
        seen.push((cells_in(&cx), coming));
    }
    let none = Vec::new();
    let expected: [(Vec<i32>, Vec<i32>); 7] = [
        (none.clone(), vec![0, 1]),
        (vec![0, 1], vec![2, 3]),
        (vec![0, 1, 2, 3], none.clone()),
        (none.clone(), none.clone()),
        (none.clone(), none.clone()),
        (none.clone(), vec![0, 1]),
        (vec![0, 1], none),
    ];
    assert_eq!(seen, expected);
    assert_eq!(found(&cx, id_of(1)), Some((12.0, true)), "as it was left");
    assert!(!cx.exiting());
    let matched = run.stack.sync_test_mut().map(|sync| sync.matched());
    assert_eq!(matched, Some(WALK.len() as u64));
    let ended = cx.world().hash();
    run.stack.detach(&mut cx);
    run.stack.recorded().expect("written");

    let text = fs::read_to_string(&path).expect("written");
    let mut replay = Replay::new(app(), &text).unwrap_or_else(|error| panic!("{error:#}"));
    let mut ran = 0;
    while let Some(tick) = replay.step().unwrap_or_else(|error| panic!("{error:#}")) {
        assert_eq!(tick.number, ran);
        ran += 1;
    }
    assert_eq!((ran, replay.world().hash()), (WALK.len() as u64, ended));
    assert_eq!(xs(replay.world()), [0, 1]);
    // Each cell's file as it came in: cell 0's again, since it went out as it came.
    let files: Vec<&str> = (text.lines())
        .filter_map(|line| Some(line.strip_prefix("cell ")?.split_once(' ')?.0))
        .collect();
    assert_eq!(files, ["0_0_0", "1_0_0", "2_0_0", "3_0_0", "0_0_0"]);

    // Cell 2's file changed since: the replay (as `--replay` runs it) stops as it comes in, naming
    // it.
    let two = root.join("cells/2_0_0.ron");
    let changed = fs::read_to_string(&two)
        .expect("a cell")
        .replace("25.0", "26.0");
    fs::write(&two, changed).expect("written");
    let mut replay = Replay::new(app(), &text).unwrap_or_else(|error| panic!("{error:#}"));
    let stopped = replay.run(None, &mut std::io::sink()).expect_err("stopped");
    assert_eq!(stopped.code(), "replay.cell_file", "{stopped:#}");
    let message = stopped.to_string();
    assert!(
        message.contains("tick 2 brought cell 2_0_0 in from the file cells/2_0_0.ron"),
        "{message}"
    );
}

/// A world whose cells the game makes needs no folder: a missing one warns of nothing.
#[test]
fn a_made_world_needs_no_folder() {
    let logged = crate::testing::logged();
    let dir = Scratch::new("cells-made-unfiled");
    let (mut cx, _) = streaming(&dir.0, "cells");
    make(&mut cx);
    step(&mut cx);
    assert_eq!(cells_in(&cx), [-1, 0, 1]);
    let codes: Vec<String> = logged.codes.try_iter().collect();
    assert!(
        !codes.iter().any(|code| code == "cells.listing"),
        "{codes:?}"
    );
}

/// A run whose game makes cells reruns alike in a SyncTest, and its recording replays to its
/// hashes, naming the cells whose files came in as they came; cells 2 and 3 go out again as they
/// came at the end of their tick, and no made cell is named.
#[test]
fn a_run_of_made_cells_replays_naming_no_made_cell() {
    const WALK: [Option<Walk>; 6] = [
        None,
        Some(Walk::Viewer(25.0)),
        Some(Walk::Viewer(100.0)),
        None,
        Some(Walk::Viewer(5.0)),
        None,
    ];
    use crate::App;
    use crate::recording::{Recorder, Replay};

    let dir = Scratch::new("cells-made-replay");
    let root = project(&dir.0);
    let app = || {
        let app = App::new("Making").root(&root);
        app.layer(Walker::default()).layer(Maker)
    };
    let path = dir.0.join("run.recording");
    let mut run = app();
    run.stack
        .record(Recorder::create(&path, run.name).expect("created"));
    run.stack.sync_test();
    let mut cx = Context::seeded(1).root(root.clone());
    run.stack.attach(&mut cx).expect("loaded");
    let mut made = BTreeSet::new();
    for (number, walk) in (0..).zip(WALK) {
        if let Some(walk) = walk {
            let queue = run.stack.commands_of(0).expect("a queue");
            queue.push(&walk.to_value()).expect("queued");
        }
        assert!(run.stack.tick(&mut cx, Tick { number }));
        made.extend(
            cx.world()
                .cells_in()
                .map(|cell| cell.0.x)
                .filter(|x| *x < 0),
        );
    }
    assert_eq!(
        made,
        BTreeSet::from([-1]),
        "made, as the walk starts and ends"
    );
    let matched = run.stack.sync_test_mut().map(|sync| sync.matched());
    assert_eq!(matched, Some(WALK.len() as u64));
    let ended = cx.world().hash();
    run.stack.detach(&mut cx);
    run.stack.recorded().expect("written");

    let text = fs::read_to_string(&path).expect("written");
    let mut replay = Replay::new(app(), &text).unwrap_or_else(|error| panic!("{error:#}"));
    while replay
        .step()
        .unwrap_or_else(|error| panic!("{error:#}"))
        .is_some()
    {}
    assert_eq!(replay.world().hash(), ended);
    let files: Vec<&str> = (text.lines())
        .filter_map(|line| Some(line.strip_prefix("cell ")?.split_once(' ')?.0))
        .collect();
    assert_eq!(
        files,
        ["0_0_0", "1_0_0", "2_0_0", "3_0_0", "0_0_0", "1_0_0"]
    );
}

/// Puts the look's viewpoint (the editor's camera) at x `x`.
#[cfg(feature = "dev")]
pub(crate) fn look_at(cx: &mut Context, x: f32) {
    cx.dev.look.viewpoint = Some(gg_renderer::Viewpoint {
        position: DVec3::new(f64::from(x), 0.0, 0.0),
        ..Default::default()
    });
}

/// While the run is stopped and a world edited, cells come in around the look's viewpoint, 512
/// across unless a viewer's radius is larger, as they're prepared; as it leaves, an edited cell
/// stays in, those as their files have them go, keeping nothing, and return from their files; a
/// root moved into a cell out brings the cell's file in with it.
#[cfg(feature = "dev")]
#[test]
fn stopped_cells_come_and_go_around_the_look_s_viewpoint() {
    let dir = Scratch::new("cells-stopped");
    let root = project(&dir.0);
    let mut cx = Context::seeded(1).root(root);
    cx.world_mut().spawn(Cells {
        folder: "cells".to_owned(),
        size: 10.0,
    });
    // One frame notes and prepares the cells coming, the next brings them in.
    let frames = |cx: &mut Context| {
        cx.stream_stopped();
        cx.stream_stopped();
    };
    frames(&mut cx);
    assert!(cells_in(&cx).is_empty(), "no viewer");
    // Cell 0 is 507 away, cell 1 517.
    look_at(&mut cx, -507.0);
    frames(&mut cx);
    assert_eq!(cells_in(&cx), [0]);
    look_at(&mut cx, 15.0);
    frames(&mut cx);
    assert_eq!(cells_in(&cx), [0, 1, 2, 3]);
    let one = cx.world().entity(id_of(1)).expect("in");
    move_to(&mut cx, one, 16.0);
    cx.scenes.changed();
    look_at(&mut cx, 2000.0);
    cx.stream_stopped();
    assert_eq!(cells_in(&cx), [1], "the edited one stays");
    assert_eq!(found(&cx, id_of(0)), None);
    assert_eq!(found(&cx, id_of(1)), Some((16.0, true)));
    let state = state(&cx);
    assert!(state.kept.is_empty(), "nothing kept");
    assert_eq!(state.files.keys().collect::<Vec<_>>(), [&Cell(IVec3::X)]);
    cx.stream_stopped();
    assert_eq!(cells_in(&cx), [1], "still");
    look_at(&mut cx, 15.0);
    frames(&mut cx);
    assert_eq!(cells_in(&cx), [0, 1, 2, 3]);
    assert_eq!(found(&cx, id_of(0)), Some((5.0, true)), "from its file");
    look_at(&mut cx, -507.0);
    cx.stream_stopped();
    assert_eq!(
        cells_in(&cx),
        [0, 1],
        "cell 1 within the radius and half a cell"
    );
    let one = cx.world().entity(id_of(1)).expect("in");
    move_to(&mut cx, one, 35.0);
    cx.scenes.changed();
    frames(&mut cx);
    assert_eq!(cells_in(&cx), [0, 1, 3], "cell 3 with the root moved in");
    assert_eq!(found(&cx, id_of(3)), Some((35.0, true)));
    assert_eq!(found(&cx, id_of(1)), Some((35.0, true)));
    cx.stream_stopped();
    assert_eq!(cells_in(&cx), [0, 1, 3], "both unlike their files");
}
