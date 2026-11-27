use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use gg_core::Error;
use gg_core::math::{Mat4, Vec2, Vec3};
#[cfg(feature = "dev")]
use gg_renderer::Grade;
use gg_renderer::{Blend, Mipmaps, Sprite, Texture};

use super::adds::add_sheet;
use super::frame::followed;
#[cfg(feature = "dev")]
use super::stack::Hot;
use super::stack::short_name;
use super::*;
#[cfg(feature = "dev")]
use crate::Name;
#[cfg(feature = "dev")]
use crate::assets;
use crate::assets::Asset;
use crate::audio::{self, Place, Sound};
use crate::camera::Camera;
#[cfg(feature = "dev")]
use crate::cells::{Cell, CellState};
#[cfg(feature = "dev")]
use crate::edits::Edit;
#[cfg(feature = "dev")]
use crate::font::{self, TextStyle};
#[cfg(feature = "dev")]
use crate::grid::Plane;
#[cfg(feature = "dev")]
use crate::hot_reload::Reloaded;
use crate::image::{self, Image};
use crate::model::Nodes;
#[cfg(feature = "dev")]
use crate::package;
#[cfg(feature = "dev")]
use crate::recording::Recorder;
use crate::render_thread::Extracted;
use crate::{Fog, Key, Projection, Transform, render_thread};
use crate::{MeshDraw, Model, ModelNode, gltf};

fn handed_off(cx: &mut Context) -> Extracted {
    cx.draw_world(1.0, 0.0, 0);
    let mut frame = Extracted::default();
    let view = Viewport {
        origin: [0, 0],
        size: [64, 32],
    };
    cx.extract(&mut frame, view);
    frame
}

fn cut(cx: &mut Context) -> bool {
    handed_off(cx).cut
}

/// Where the look's grid draws: each cell in draws its cube's twelve edges, and each cell refused;
/// without the grid, none.
#[cfg(feature = "dev")]
#[test]
fn the_cells_draw_where_the_look_s_grid_does() {
    use gg_core::math::IVec3;

    let mut cx = Context::seeded(1);
    let world = cx.world_mut();
    world.spawn(crate::Cells {
        folder: "cells".to_owned(),
        size: 10.0,
    });
    world.insert_resource(CellState {
        cells_in: [Cell(IVec3::ZERO), Cell(IVec3::X)].into(),
        refused: [Cell(IVec3::Y)].into(),
        ..CellState::default()
    });
    let lines = |cx: &mut Context| {
        let before = cx.over.len();
        cx.draw_world(1.0, 0.0, 0);
        cx.over.len() - before
    };
    assert_eq!(lines(&mut cx), 0, "no grid");
    cx.dev.look.grid = Some(Plane::XZ);
    assert_eq!(lines(&mut cx), 3 * 12);
}

/// The frame's origin stays at the world's while the view is within 1,024 of it per axis; once
/// past, the next frame's origin is the view rounded to a multiple of 1,024 per axis (the frame
/// that went past keeps the old one). A non-finite view leaves it. A development build's look
/// viewpoint moves it as a frame starts.
#[test]
fn the_origin_follows_the_view_a_step_of_1024_at_a_time() {
    let x = |x| DVec3::new(x, 0.0, 0.0);
    let near = DVec3::new(1024.0, -1024.0, 3.0);
    assert_eq!(followed(DVec3::ZERO, near), DVec3::ZERO);
    let past = DVec3::new(1024.5, 0.0, -1500.0);
    assert_eq!(
        followed(DVec3::ZERO, past),
        DVec3::new(1024.0, 0.0, -1024.0)
    );
    assert_eq!(followed(x(1024.0), x(2048.0)), x(1024.0));
    assert_eq!(followed(x(1024.0), x(2049.0)), x(2048.0));
    for broken in [DVec3::new(f64::NAN, 1.0e9, 0.0), x(f64::INFINITY)] {
        assert_eq!(followed(x(1024.0), broken), x(1024.0), "{broken}");
    }

    let mut cx = Context::seeded(1);
    let far = Viewpoint {
        position: DVec3::new(1.0e6 + 0.3, 0.0, 2.0),
        ..Viewpoint::default()
    };
    cx.set_viewpoint(far);
    assert_eq!(
        handed_off(&mut cx).origin,
        DVec3::ZERO,
        "the frame that went past"
    );
    let origin = x(1_000_448.0);
    assert_eq!(cx.origin(), origin);
    cx.set_viewpoint(far);
    let at = Mat4::from_translation(Vec3::new(1_000_448.25, 0.0, 0.0));
    cx.draw_sprite(at, Sprite::default());
    let frame = handed_off(&mut cx);
    assert_eq!(frame.origin, origin);
    let mut expected = Draws::default();
    let relative = Mat4::from_translation(Vec3::new(0.25, 0.0, 0.0));
    expected.push_sprite(relative, Sprite::default());
    expected.draw_scene();
    assert_eq!(frame.draws, expected);
    #[cfg(feature = "dev")]
    {
        let looked = DVec3::new(-5000.0, 0.0, 0.0);
        cx.dev.look.viewpoint = Some(Viewpoint {
            position: looked,
            ..far
        });
        cx.follow_look();
        assert_eq!(cx.origin(), x(-5120.0));
        assert_eq!(handed_off(&mut cx).origin, x(-5120.0));
    }
}

/// The view is antialiased as a layer sets, over its world's environment's, and as the environment
/// says once unset. World and hash stay as they were.
#[test]
fn a_layers_antialiasing_takes_the_environments_place() {
    let antialiased = |cx: &mut Context| handed_off(cx).antialiasing;
    let mut cx = Context::seeded(1);
    assert_eq!(antialiased(&mut cx), Antialiasing::None, "no environment");
    cx.set_antialiasing(Some(Antialiasing::Smaa));
    assert_eq!(antialiased(&mut cx), Antialiasing::Smaa, "in place of none");
    let environment = cx.world_mut().spawn(Environment::default());
    let hash = cx.world().hash();
    assert_eq!(antialiased(&mut cx), Antialiasing::Smaa, "in place of TAA");
    cx.set_antialiasing(Some(Antialiasing::None));
    assert_eq!(antialiased(&mut cx), Antialiasing::None);
    assert_eq!(cx.antialiasing(), Some(Antialiasing::None));
    let held = cx.world().get::<Environment>(environment).copied();
    assert_eq!(held, Some(Environment::default()));
    assert_eq!(cx.world().hash(), hash);
    cx.set_antialiasing(None);
    assert_eq!(antialiased(&mut cx), Antialiasing::Taa, "the environment's");
}

fn camera_at(x: f32, primary: bool) -> (Transform, Camera) {
    let at = Transform {
        translation: DVec3::new(f64::from(x), 0.0, 0.0),
        ..Transform::IDENTITY
    };
    let projection = Projection::Perspective {
        fov: 1.0,
        near: 0.1,
    };
    (
        at,
        Camera {
            projection,
            primary,
        },
    )
}

/// A projection that can't project (near plane 0, non-number field of view) would panic the render
/// thread: the frame draws through the last good one, or the default, warning once
/// (`engine.bad_projection`) until one can again.
#[test]
fn a_projection_that_cant_project_is_replaced_by_the_last_that_could_with_a_warning() {
    let logged = crate::testing::logged();
    let mut cx = Context::seeded(1);
    let good = Projection::Perspective {
        fov: 1.0,
        near: 0.1,
    };
    let camera = cx.world_mut().spawn((
        Transform {
            translation: DVec3::X,
            ..Transform::IDENTITY
        },
        Camera {
            projection: Projection::Perspective {
                fov: f32::NAN,
                near: 0.1,
            },
            primary: true,
        },
    ));
    let seen = |cx: &mut Context| handed_off(cx).viewpoint;
    let warnings = || logged.codes.try_iter().collect::<Vec<_>>();
    let first = seen(&mut cx);
    assert_eq!(first.projection, Viewpoint::default().projection, "no last");
    assert_eq!(first.position, DVec3::X, "from where the camera is");
    assert_eq!(warnings(), ["engine.bad_projection"]);
    let set = |cx: &mut Context, projection| {
        let camera = cx.world_mut().get_mut::<Camera>(camera).expect("alive");
        camera.projection = projection;
    };
    set(&mut cx, good);
    assert_eq!(seen(&mut cx).projection, good);
    assert!(warnings().is_empty());
    let near_nothing = Projection::Perspective {
        fov: 1.0,
        near: 0.0,
    };
    set(&mut cx, near_nothing);
    assert_eq!(seen(&mut cx).projection, good, "the last that could");
    assert_eq!(seen(&mut cx).projection, good);
    assert_eq!(warnings(), ["engine.bad_projection"], "once");
    // A layer's viewpoint, set whole: its source has no good projection to fall back to, so it
    // warns anew, as the camera does when the view returns to it.
    let flat = Viewpoint::default().projection;
    cx.set_viewpoint(Viewpoint {
        projection: Projection::Orthographic {
            height: 0.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    });
    assert_eq!(seen(&mut cx).projection, flat, "not the camera's");
    assert_eq!(warnings(), ["engine.bad_projection"], "a layer's own");
    assert_eq!(seen(&mut cx).projection, flat, "the camera, with none good");
    assert_eq!(warnings(), ["engine.bad_projection"], "the camera's own");
    set(&mut cx, good);
    assert_eq!(seen(&mut cx).projection, good);
    assert!(warnings().is_empty());
}

/// Non-finite fog set from code would turn the view black: the frame draws without it, with one
/// warning (`engine.bad_fog`) for as long as it lasts.
#[test]
fn fog_that_isnt_finite_is_left_out_of_the_frame_with_a_warning() {
    let logged = crate::testing::logged();
    let mut cx = Context::seeded(1);
    let environment = |fog| Environment {
        fog: Some(fog),
        ..Environment::default()
    };
    let broken = Fog {
        falloff: f32::NAN,
        ..Fog::default()
    };
    let id = cx.world_mut().spawn(environment(broken));
    let fog = |cx: &mut Context| handed_off(cx).fog;
    let warnings = || logged.codes.try_iter().collect::<Vec<_>>();
    assert_eq!(fog(&mut cx), None);
    assert_eq!(fog(&mut cx), None);
    assert_eq!(warnings(), ["engine.bad_fog"], "once");
    *cx.world_mut().get_mut::<Environment>(id).expect("alive") = environment(Fog::default());
    assert_eq!(fog(&mut cx), Some(Fog::default()));
    assert!(warnings().is_empty());
    let infinite = Fog {
        density: f32::INFINITY,
        ..Fog::default()
    };
    *cx.world_mut().get_mut::<Environment>(id).expect("alive") = environment(infinite);
    assert_eq!(fog(&mut cx), None);
    assert_eq!(warnings(), ["engine.bad_fog"], "anew after good fog");
}

struct Sends(crate::GameUi);

impl Layer for Sends {
    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        self.0.event(cx, event);
        ControlFlow::Continue(())
    }
}

/// A game UI takes mouse presses and wheel over its buttons and lists from the layers below the
/// layer that sent them; releases go on. UI and context both say whether the cursor is over it.
#[test]
fn a_game_ui_takes_the_mouse_over_what_it_draws_from_the_layers_below() {
    use crate::MouseButton;

    let scratch = crate::dirs::tests::Scratch::new("ui-takes-the-mouse");
    std::fs::create_dir_all(&scratch.0).expect("a directory");
    let path = scratch.0.join("menu.ui");
    // A button in the view's middle, a list at its top left, a text at its top right, in canvas
    // units of a 1,920 by 1,080 view.
    let file = r#"(nodes: [
            Button(name: "play", width: Fixed(400.0), height: Fixed(80.0)),
            List(
                name: "scores",
                anchor: (Start, Start),
                width: Fixed(300.0),
                height: Fixed(200.0),
                children: [Text(name: "score", text: "120")],
            ),
            Text(name: "title", anchor: (End, Start), text: "TITLE"),
        ])"#;
    std::fs::write(&path, file).expect("written");
    let mut cx = Context::seeded(1);
    let ui = cx.add_ui("menu", &path).expect("added");
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut stack = Stack::default();
    stack.push_layer(Probe {
        name: "world",
        handles: false,
        calls: calls.clone(),
    });
    stack.push_layer(Sends(ui));
    // Drawn and handed off, so the view is known and the UI drew last frame.
    ui.draw(&mut cx);
    let mut frame = Extracted::default();
    let view = Viewport {
        origin: [0, 0],
        size: [1920, 1080],
    };
    cx.extract(&mut frame, view);
    let button = |pressed| Event::MouseButton {
        button: MouseButton::Left,
        pressed,
    };
    let wheel = Event::Scroll(crate::Scroll::Lines { x: 0.0, y: -1.0 });
    let reached = || !calls.borrow().is_empty();
    let at = |cx: &mut Context, stack: &mut Stack, x: f64, y: f64| {
        let _ = stack.event(cx, &Event::CursorMoved { x, y });
        calls.borrow_mut().clear();
    };

    at(&mut cx, &mut stack, 960.0, 540.0);
    assert!(ui.pointer_over(&mut cx), "over the button");
    assert!(cx.pointer_over_ui());
    let handled = stack.event(&mut cx, &button(true));
    assert_eq!(handled, ControlFlow::Break(()), "a press over a button");
    assert!(!reached(), "a press over a button reaches the world");
    // A release goes on, as the layers below let go of what they saw pressed.
    let _ = stack.event(&mut cx, &button(false));
    assert!(reached(), "a release goes on down");
    calls.borrow_mut().clear();

    at(&mut cx, &mut stack, 100.0, 100.0);
    assert!(ui.pointer_over(&mut cx) && cx.pointer_over_ui(), "the list");
    let _ = stack.event(&mut cx, &wheel);
    assert!(!reached(), "the wheel over a list reaches the world");

    for (x, y, what) in [(960.0, 100.0, "nothing"), (1850.0, 20.0, "a text")] {
        at(&mut cx, &mut stack, x, y);
        assert!(!ui.pointer_over(&mut cx) && !cx.pointer_over_ui(), "{what}");
        let handled = stack.event(&mut cx, &button(true));
        assert_eq!(handled, ControlFlow::Continue(()), "{what}");
        assert!(reached(), "a press over {what} doesn't reach the world");
        calls.borrow_mut().clear();
        let _ = stack.event(&mut cx, &wheel);
        assert!(reached(), "the wheel over {what} doesn't reach the world");
        calls.borrow_mut().clear();
    }

    // Not drawn for two frames: it takes nothing and is over nothing.
    at(&mut cx, &mut stack, 960.0, 540.0);
    cx.extract(&mut frame, view);
    cx.extract(&mut frame, view);
    assert!(!ui.pointer_over(&mut cx) && !cx.pointer_over_ui());
    let _ = stack.event(&mut cx, &button(true));
    assert!(reached(), "a UI that no longer draws takes no press");
    calls.borrow_mut().clear();
    ui.draw(&mut cx);
    assert!(ui.pointer_over(&mut cx), "drawn again");

    // Input off: it takes nothing and is over nothing.
    ui.set_input(&mut cx, false);
    assert!(!ui.pointer_over(&mut cx) && !cx.pointer_over_ui());
    let _ = stack.event(&mut cx, &button(true));
    assert!(reached(), "a UI without input takes no press");
}

/// The view is cut to its viewpoint when it comes from elsewhere than the last frame's (motion blur
/// treats that as standing still): the first frame, a primary camera switch or projection kind
/// change (not a move), a layer starting or stopping a viewpoint (not moving it).
#[test]
fn the_view_cuts_as_its_viewpoint_comes_from_elsewhere() {
    let mut cx = Context::seeded(1);
    let first = cx.world_mut().spawn(camera_at(0.0, true));
    let second = cx.world_mut().spawn(camera_at(9.0, false));
    assert!(cut(&mut cx), "the first frame isn't cut to");
    assert!(!cut(&mut cx), "the same camera cuts");
    cx.world_mut()
        .get_mut::<Transform>(first)
        .expect("alive")
        .translation
        .x = 5.0;
    assert!(!cut(&mut cx), "the camera moving cuts");
    cx.world_mut()
        .get_mut::<Camera>(first)
        .expect("alive")
        .primary = false;
    cx.world_mut()
        .get_mut::<Camera>(second)
        .expect("alive")
        .primary = true;
    assert!(cut(&mut cx), "the primary camera switching doesn't cut");
    assert!(!cut(&mut cx), "the camera switched to cuts again");
    let flat = Projection::Orthographic {
        height: 2.0,
        near: -1.0,
        far: 1.0,
    };
    cx.world_mut()
        .get_mut::<Camera>(second)
        .expect("alive")
        .projection = flat;
    assert!(
        cut(&mut cx),
        "the projection turning orthographic doesn't cut"
    );
    assert!(!cut(&mut cx), "the orthographic camera cuts again");
    cx.set_viewpoint(Viewpoint::default());
    assert!(cut(&mut cx), "a layer setting a viewpoint doesn't cut");
    cx.set_viewpoint(Viewpoint {
        position: DVec3::X,
        ..Viewpoint::default()
    });
    assert!(!cut(&mut cx), "the layer's viewpoint moving cuts");
    assert!(
        cut(&mut cx),
        "the layer leaving it to the camera doesn't cut"
    );
}

/// The view is cut to the tool API's look when it's set, its viewpoints change, or it's cleared,
/// and to and from a bare frame among the look's (it sees through the game's camera, as does a look
/// without a viewpoint). Each frame says whether the look shows, for the exposure.
#[cfg(feature = "dev")]
#[test]
fn the_view_cuts_to_and_from_the_tools_look() {
    let seen = |cx: &mut Context| {
        cx.draw_world(1.0, 0.0, 0);
        let mut frame = Extracted::default();
        let view = Viewport {
            origin: [0, 0],
            size: [64, 32],
        };
        cx.extract(&mut frame, view);
        (frame.cut, frame.looked)
    };
    let mut cx = Context::seeded(1);
    cx.world_mut().spawn(camera_at(0.0, true));
    seen(&mut cx);
    assert_eq!(seen(&mut cx), (false, false), "the camera");
    let looked = Viewpoint {
        position: DVec3::new(1.0, 2.0, 10.0),
        ..Viewpoint::default()
    };
    cx.dev.look.viewpoint = Some(looked);
    assert_eq!(seen(&mut cx), (true, true), "the look set");
    assert_eq!(seen(&mut cx), (false, true), "the look held");
    cx.dev.look.viewpoint = Some(Viewpoint {
        position: DVec3::ZERO,
        ..looked
    });
    assert_eq!(seen(&mut cx), (true, true), "the look changed");
    cx.dev.look.bare = true;
    assert_eq!(
        seen(&mut cx),
        (true, false),
        "a bare frame among the look's"
    );
    cx.dev.look.bare = false;
    assert_eq!(seen(&mut cx), (true, true), "the look's frame after it");
    cx.dev.look.viewpoint = None;
    assert_eq!(seen(&mut cx), (true, false), "the look cleared");
    cx.dev.look.bare = true;
    assert_eq!(seen(&mut cx), (false, false), "a bare frame without a look");
    cx.dev.look.bare = false;
    assert_eq!(seen(&mut cx), (false, false), "the frame after it");
}

/// The look's render scale replaces the game's own in the frame while set, but not in a bare frame,
/// and the game's own, which a layer reads back, is never changed.
#[cfg(feature = "dev")]
#[test]
fn the_looks_render_scale_replaces_the_games_own_in_the_frame() {
    let mut cx = Context::seeded(1);
    cx.set_render_scale(0.75);
    assert_eq!(handed_off(&mut cx).render_scale, 0.75, "the game's own");
    cx.dev.look.render_scale = Some(0.5);
    assert_eq!(handed_off(&mut cx).render_scale, 0.5, "the look's");
    cx.dev.look.bare = true;
    assert_eq!(handed_off(&mut cx).render_scale, 0.75, "a bare frame");
    cx.dev.look.bare = false;
    assert_eq!(cx.render_scale(), 0.75, "the game's own still");
    cx.dev.look.render_scale = None;
    assert_eq!(handed_off(&mut cx).render_scale, 0.75, "given back");
}

/// The look leaves out passes: the environment as if the game had none, the lights without shadows.
/// A bare frame leaves none out. The world stays as it was.
#[cfg(feature = "dev")]
#[test]
fn the_passes_the_look_leaves_out_are_left_out_of_the_frame_but_a_bare_one() {
    use gg_renderer::{Adaptation, AmbientOcclusion, DepthOfField, Lens, MotionBlur, Reflections};

    /// Each pass of a frame, in [`gg_api::Pass::ALL`]'s order; shadows by the light's.
    type Passes = (
        Option<AmbientOcclusion>,
        Option<Reflections>,
        Option<f32>,
        Option<Fog>,
        f32,
        Option<DepthOfField>,
        Option<MotionBlur>,
        Antialiasing,
        Grade,
        Lens,
        Option<Adaptation>,
    );

    fn passes(frame: &Extracted) -> Passes {
        (
            frame.ambient_occlusion,
            frame.reflections,
            frame.lights[0].shadow_distance,
            frame.fog,
            frame.bloom,
            frame.depth_of_field,
            frame.motion_blur,
            frame.antialiasing,
            frame.grade,
            frame.lens,
            frame.adaptation,
        )
    }
    let mut cx = Context::seeded(1);
    // Every pass on, none as a pass left out leaves it.
    let everything = Environment {
        adaptation: Some(crate::Adaptation::default()),
        bloom: 0.5,
        grade: Grade {
            contrast: 1.5,
            ..Grade::IDENTITY
        },
        fog: Some(Fog::default()),
        reflections: Some(Reflections::default()),
        depth_of_field: Some(crate::DepthOfField::default()),
        motion_blur: Some(crate::MotionBlur::default()),
        lens: crate::Lens {
            vignette: 0.4,
            ..crate::Lens::NONE
        },
        ..Environment::default()
    };
    let environment = cx.world_mut().spawn(everything);
    let light = (cx.world_mut()).spawn((Transform::IDENTITY, crate::Light::default()));
    let hash = cx.world().hash();
    let drawn = passes(&handed_off(&mut cx));
    let shadows = crate::Light::default().shadow_distance;
    let game = (
        everything.ambient_occlusion,
        everything.reflections,
        Some(shadows),
        everything.fog,
        everything.bloom,
        everything.depth_of_field.map(|focus| focus.rendered()),
        everything.motion_blur.map(|blur| blur.rendered()),
        Antialiasing::Taa,
        everything.grade,
        everything.lens.rendered(),
        everything
            .adaptation
            .map(|adaptation| adaptation.rendered()),
    );
    assert_eq!(drawn, game, "the game's own");
    cx.dev.look.skip = gg_api::Pass::ALL.to_vec();
    let none = (
        None,
        None,
        None,
        None,
        0.0,
        None,
        None,
        Antialiasing::None,
        Grade::IDENTITY,
        Lens::NONE,
        None,
    );
    assert_eq!(passes(&handed_off(&mut cx)), none, "every pass left out");
    cx.dev.look.skip = vec![gg_api::Pass::Reflections];
    let mut reflectionless = game;
    reflectionless.1 = None;
    assert_eq!(passes(&handed_off(&mut cx)), reflectionless, "one left out");
    cx.dev.look.skip = gg_api::Pass::ALL.to_vec();
    cx.dev.look.bare = true;
    assert_eq!(passes(&handed_off(&mut cx)), game, "a bare frame");
    let world = cx.world();
    assert_eq!(world.get::<Environment>(environment), Some(&everything));
    let lit = world.get::<crate::Light>(light).copied();
    assert_eq!(lit, Some(crate::Light::default()));
    assert_eq!(world.hash(), hash);
}

/// Each skinned part of a model's root draws its own copy of the model's mesh, skinned by its
/// joints from the bound space (at rest, as the Fox spawns: the identity for each of its 24). The
/// copy persists across frames and goes with its root.
#[test]
fn a_skinned_part_draws_a_mesh_of_its_own_moved_by_its_joints() {
    let mut cx = Context::seeded(1);
    let fox = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/models/fox");
    let model = cx
        .add_model("fox", fox.join("Fox.gltf"))
        .expect("the Fox reads");
    let root = model.spawn(cx.world_mut(), Transform::IDENTITY);
    cx.draw_world(1.0, 0.0, 0);
    let model_mesh = cx.loaded.made[model.index()].meshes[0].index();
    let skins: Vec<(u32, Vec<Mat4>)> = (cx.draws.skins())
        .map(|(mesh, joints)| (mesh, joints.to_vec()))
        .collect();
    let [(own, joints)] = &skins[..] else {
        panic!("a skin: {skins:?}");
    };
    assert_ne!(*own, model_mesh, "a copy of its own");
    assert_eq!(joints.len(), 24);
    for (i, joint) in joints.iter().enumerate() {
        assert!(
            joint.abs_diff_eq(Mat4::IDENTITY, 1e-4),
            "joint {i} at rest: {joint}"
        );
    }
    let drawn: Vec<u32> = (cx.scene.changes().flatten())
        .filter_map(|(_, set)| set)
        .map(|(_, _, draw)| draw.parts().0)
        .collect();
    assert!(
        drawn.contains(own) && !drawn.contains(&model_mesh),
        "{drawn:?}"
    );
    cx.draws.clear();
    cx.draw_world(1.0, 0.0, 0);
    assert_eq!(
        cx.draws.skins().map(|(mesh, _)| mesh).collect::<Vec<_>>(),
        [*own]
    );
    assert_eq!(cx.skins.held(), (1, 0));
    cx.world_mut().despawn(root).expect("alive");
    cx.draws.clear();
    cx.draw_world(1.0, 0.0, 0);
    assert_eq!(cx.draws.skins().count(), 0);
    assert_eq!(cx.skins.held(), (0, 1), "its copy spare");
    // Another Fox takes the spare copy, adding no mesh.
    let meshes = cx.meshes.count();
    model.spawn(cx.world_mut(), Transform::IDENTITY);
    cx.draws.clear();
    cx.draw_world(1.0, 0.0, 0);
    assert_eq!(
        cx.draws.skins().map(|(mesh, _)| mesh).collect::<Vec<_>>(),
        [*own]
    );
    assert_eq!((cx.meshes.count(), cx.skins.held()), (meshes, (1, 0)));
}

/// A joint made a root carrying nothing, which placing leaves to its transform, skins its mesh by
/// where `World::pose` puts it, bit for bit.
#[test]
fn a_joint_made_a_root_carrying_nothing_skins_by_its_transform() {
    let mut cx = Context::seeded(1);
    let fox = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/models/fox");
    let model = cx
        .add_model("fox", fox.join("Fox.gltf"))
        .expect("the Fox reads");
    let root = model.spawn(cx.world_mut(), Transform::IDENTITY);
    cx.draw_world(1.0, 0.0, 0);
    let world = cx.world();
    let nodes = world.assets().nodes(model).expect("its nodes").clone();
    let id = world.id(root).expect("alive");
    let node = |part: usize| {
        let node = crate::ecs::node_id(id, part).and_then(|node| world.entity(node));
        node.expect("spawned")
    };
    let skinned = |part: &gltf::Part| part.skin.filter(|_| part.primitive.is_some());
    let (skin, part) = (nodes.parts.iter().enumerate())
        .find_map(|(index, part)| skinned(part).map(|skin| (skin, node(index))))
        .expect("a skinned part");
    let skin = &nodes.skins[skin];
    let (joint, leaf) = (skin.joints.iter().enumerate())
        .map(|(joint, &at)| (joint, node(at as usize)))
        .find(|&(_, entity)| world.children(entity).expect("alive").next().is_none())
        .expect("a joint carrying nothing");
    assert!(world.get::<MeshDraw>(leaf).is_none(), "in no held table");
    let world = cx.world_mut();
    world.set_parent(leaf, None).expect("no cycle");
    (world.get_mut::<Transform>(leaf))
        .expect("it has one")
        .translation
        .x += 3.0;
    cx.draws.clear();
    cx.draw_world(1.0, 0.0, 0);
    let pose = |entity| cx.world().pose(entity).expect("alive").matrix;
    let into_mesh = pose(part).inverse();
    let want = Mat4::from((into_mesh * pose(leaf)).as_affine3a()) * skin.inverse_binds[joint];
    let skins: Vec<Vec<Mat4>> = (cx.draws.skins())
        .map(|(_, joints)| joints.to_vec())
        .collect();
    let [joints] = &skins[..] else {
        panic!("a skin: {skins:?}");
    };
    assert_eq!(joints[joint], want);
}

/// A Fox's context, its model added, after a Fox spawned and despawned in a frame, leaving its
/// skinned copy spare.
fn spare_fox() -> (Context, Model) {
    let mut cx = Context::seeded(1);
    let fox = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/models/fox");
    let model = cx
        .add_model("fox", fox.join("Fox.gltf"))
        .expect("the Fox reads");
    let root = model.spawn(cx.world_mut(), Transform::IDENTITY);
    cx.draw_world(1.0, 0.0, 0);
    cx.world_mut().despawn(root).expect("alive");
    cx.draws.clear();
    cx.draw_world(1.0, 0.0, 0);
    assert_eq!(cx.skins.held(), (0, 1));
    (cx, model)
}

/// A spare copy of a skinned model's mesh goes, and the renderer removes it, once its model's mesh
/// changes, as a model read again does.
#[test]
fn a_spare_skinned_copy_goes_once_its_models_mesh_is_another() {
    let (mut cx, model) = spare_fox();
    let kept = cx.meshes.kept();
    cx.loaded.made[model.index()].skinned[0] = Some(Arc::new(
        cx.loaded.made[model.index()].skinned[0]
            .as_deref()
            .expect("a skinned mesh")
            .clone(),
    ));
    cx.draw_world(1.0, 0.0, 0);
    assert_eq!((cx.skins.held(), cx.meshes.kept()), ((0, 0), kept - 1));
}

/// A spare copy no entity takes goes, and the renderer removes it, after `SPARE_FRAMES`: a gone
/// crowd's copies give back their memory.
#[test]
fn a_spare_skinned_copy_goes_once_its_time_is_up() {
    let (mut cx, _) = spare_fox();
    let kept = cx.meshes.kept();
    for _ in 1..crate::skins::SPARE_FRAMES {
        cx.draws.clear();
        cx.draw_world(1.0, 0.0, 0);
    }
    assert_eq!((cx.skins.held(), cx.meshes.kept()), ((0, 1), kept));
    cx.draw_world(1.0, 0.0, 0);
    assert_eq!((cx.skins.held(), cx.meshes.kept()), ((0, 0), kept - 1));
}

/// A copy whose entity lives but is no longer skinned goes rather than staying spare: the scene may
/// still draw it, and the next Fox gets its own.
#[test]
fn a_skinned_copy_of_an_entity_still_alive_goes_rather_than_being_spare() {
    let mut cx = Context::seeded(1);
    let fox = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/models/fox");
    let model = cx
        .add_model("fox", fox.join("Fox.gltf"))
        .expect("the Fox reads");
    model.spawn(cx.world_mut(), Transform::IDENTITY);
    cx.draw_world(1.0, 0.0, 0);
    let model_mesh = cx.loaded.made[model.index()].meshes[0].index();
    let (part, draw) = (cx.world().query::<(Entity, &MeshDraw)>())
        .find(|(_, draw)| draw.parts().0 == model_mesh)
        .map(|(part, draw)| (part, *draw))
        .expect("the skinned part");
    let (_, shader, opaque, culls, params) = draw.parts();
    let quad = MeshDraw::from_parts(0, shader, opaque, culls, params).expect("a draw");
    let [(own, _)] = cx
        .draws
        .skins()
        .map(|(mesh, _)| (mesh, ()))
        .collect::<Vec<_>>()[..]
    else {
        panic!("a skin");
    };
    let kept = cx.meshes.kept();
    cx.world_mut().insert(part, quad).expect("alive");
    cx.draws.clear();
    cx.draw_world(1.0, 0.0, 0);
    assert_eq!((cx.skins.held(), cx.meshes.kept()), ((0, 0), kept - 1));
    model.spawn(cx.world_mut(), Transform::IDENTITY);
    cx.draws.clear();
    cx.draw_world(1.0, 0.0, 0);
    let skins: Vec<u32> = cx.draws.skins().map(|(mesh, _)| mesh).collect();
    assert!(
        skins.len() == 1 && skins[0] != own,
        "{skins:?} beside {own}"
    );
    assert_eq!(cx.meshes.kept(), kept);
}

/// Records each call by layer name; handles events if `handles`.
struct Probe {
    name: &'static str,
    handles: bool,
    calls: Rc<RefCell<Vec<String>>>,
}

impl Layer for Probe {
    fn attach(&mut self, _: &mut Context) {
        self.calls
            .borrow_mut()
            .push(format!("attach {}", self.name));
    }
    fn detach(&mut self, _: &mut Context) {
        self.calls
            .borrow_mut()
            .push(format!("detach {}", self.name));
    }
    fn tick(&mut self, _: &mut World, _: Tick) {
        self.calls.borrow_mut().push(format!("tick {}", self.name));
    }
    fn update(&mut self, _: &mut Context, _: Duration) {
        self.calls
            .borrow_mut()
            .push(format!("update {}", self.name));
    }
    fn event(&mut self, _: &mut Context, _: &Event<'_>) -> ControlFlow<()> {
        self.calls.borrow_mut().push(format!("event {}", self.name));
        if self.handles {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
}

/// Spawns an entity as it attaches, at each tick and as it detaches; as it updates too, if `rogue`.
struct Spawner {
    rogue: bool,
}

impl Layer for Spawner {
    fn attach(&mut self, cx: &mut Context) {
        cx.world_mut().spawn(Transform::IDENTITY);
    }
    fn detach(&mut self, cx: &mut Context) {
        cx.world_mut().spawn(Transform::IDENTITY);
    }
    fn tick(&mut self, world: &mut World, _: Tick) {
        world.spawn(Transform::IDENTITY);
    }
    fn update(&mut self, cx: &mut Context, _: Duration) {
        if self.rogue {
            cx.world_mut().spawn(Transform::IDENTITY);
        }
    }
}

/// Notes per key event whether the input state shows that key held.
struct Poll(Rc<RefCell<Vec<bool>>>);

impl Layer for Poll {
    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        if let Event::Key { key, .. } = *event {
            self.0.borrow_mut().push(cx.input().key_down(key));
        }
        ControlFlow::Continue(())
    }
}

/// Pushes overlay `o`, layer `a`, layer `b` (handles events if `b_handles`), in this order. Returns
/// the stack, bottom to top `a b o`, with its call record.
fn probes(b_handles: bool) -> (Stack, Rc<RefCell<Vec<String>>>) {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let probe = |name, handles| Probe {
        name,
        handles,
        calls: calls.clone(),
    };
    let mut stack = Stack::default();
    stack.push_overlay(probe("o", false));
    stack.push_layer(probe("a", false));
    stack.push_layer(probe("b", b_handles));
    (stack, calls)
}

#[test]
fn lifecycle_goes_up_the_stack_and_detach_comes_back_down() {
    let (mut stack, calls) = probes(false);
    let mut cx = Context::new(None);
    stack.attach(&mut cx).expect("loaded");
    stack.tick(&mut cx, Tick { number: 0 });
    stack.update(&mut cx, Duration::ZERO);
    stack.detach(&mut cx);
    assert_eq!(
        *calls.borrow(),
        [
            "attach a", "attach b", "attach o", "tick a", "tick b", "tick o", "update a",
            "update b", "update o", "detach o", "detach b", "detach a"
        ]
    );
    assert_eq!(stack.names(), ["Probe"; 3]);
    assert_eq!(short_name::<Vec<crate::inspector::Inspector>>(), "Vec");
}

/// What the app keeps for the user reads back as saved, also in the next run's context; an unsaved
/// file reads as none. A name that isn't a file's alone is refused, saving nothing. A headless run
/// reads and keeps nothing.
#[test]
fn the_apps_state_reads_back_as_it_was_kept() {
    let scratch = crate::dirs::tests::Scratch::new("app-state");
    let mut cx = Context::new(None);
    cx.state = Some(scratch.0.clone());
    assert_eq!(cx.read_state("projects.json").expect("read"), None);
    cx.save_state("projects.json", b"first").expect("kept");
    cx.save_state("projects.json", b"second")
        .expect("kept again");
    let mut next = Context::new(None);
    next.state = Some(scratch.0.clone());
    let read = next.read_state("projects.json").expect("read");
    assert_eq!(read.as_deref(), Some(&b"second"[..]));
    for name in ["", "..", "nested/projects.json", "../projects.json"] {
        let refused = cx.save_state(name, b"x").expect_err(name);
        assert_eq!(refused.code(), "engine.state", "{name:?}");
        assert_eq!(
            cx.read_state(name).err().map(|e| e.code()),
            Some("engine.state")
        );
    }
    let headless = Context::new(None);
    headless
        .save_state("projects.json", b"x")
        .expect("nothing kept");
    assert_eq!(headless.read_state("projects.json").expect("read"), None);
    let kept = std::fs::read_dir(&scratch.0).expect("listed").count();
    assert_eq!(kept, 1, "projects.json alone");
}

struct Replacer;

impl Layer for Replacer {
    fn tick(&mut self, world: &mut World, _: Tick) {
        *world = World::default();
    }
}

/// A world a tick puts in the context's place shares the context's assets: the textures the layers
/// added stay named, and later ones are named after them.
#[test]
fn a_world_put_in_the_contexts_place_shares_its_assets() {
    let mut stack = Stack::default();
    stack.push_layer(Replacer);
    let mut cx = Context::new(None);
    stack.attach(&mut cx).expect("loaded");
    let add = |cx: &mut Context, path: &str| {
        let texture = cx.textures.add(path, [1, 1], &[0; 4], Mipmaps::None);
        let texture = texture.expect("a texture");
        cx.add_assets(|assets| assets.add_texture(texture, path.into(), path.into()));
        texture
    };
    let first = add(&mut cx, "first.png");
    stack.tick(&mut cx, Tick { number: 0 });
    let second = add(&mut cx, "second.png");
    stack.tick(&mut cx, Tick { number: 1 });
    let assets = cx.world().assets();
    assert_eq!(
        [first, second].map(|texture| assets.path(texture)),
        [Some("first.png"), Some("second.png")]
    );
}

#[test]
fn layers_change_the_world_as_they_attach_and_detach_and_at_their_ticks() {
    let mut stack = Stack::default();
    stack.push_layer(Spawner { rogue: false });
    let mut cx = Context::new(None);
    stack.attach(&mut cx).expect("loaded");
    stack.tick(&mut cx, Tick { number: 0 });
    stack.update(&mut cx, Duration::ZERO);
    stack.detach(&mut cx);
    assert_eq!(cx.world().query::<&Transform>().count(), 3);
}

struct Failer;

impl Layer for Failer {
    fn attach(&mut self, cx: &mut Context) {
        cx.fail(Error::new("test.failed", "it can't go on"));
    }
}

/// A layer that fails ends the run like an exit, logging its error by its code at once. The layers
/// still detach; then the stack says the run failed, unlike a plain exit.
#[test]
fn a_layer_that_fails_ends_the_run_with_its_error() {
    let logged = crate::testing::logged();
    let (mut stack, calls) = probes(false);
    stack.push_layer(Failer);
    let mut cx = Context::new(None);
    stack.attach(&mut cx).expect("loaded");
    assert!(cx.exiting());
    let codes = logged
        .codes
        .try_iter()
        .filter(|code| code.starts_with("test."));
    assert_eq!(codes.collect::<Vec<_>>(), ["test.failed"]);
    stack.detach(&mut cx);
    assert!(stack.failed());
    let detached = calls
        .borrow()
        .iter()
        .filter(|call| call.starts_with("detach"))
        .count();
    assert_eq!(detached, 3);

    let (mut stack, _) = probes(false);
    let mut cx = Context::new(None);
    stack.attach(&mut cx).expect("loaded");
    cx.exit();
    stack.detach(&mut cx);
    assert!(cx.exiting() && !stack.failed());
}

/// Each component type the engine registers has a doc, and each named field: the Inspector shows
/// them on hover.
#[test]
fn the_engines_components_and_their_named_fields_have_docs() {
    let cx = Context::new(None);
    let types = cx.world().types();
    let mut undocumented = Vec::new();
    for name in cx.world().component_types() {
        let info = types.info(types.find(name).expect("described"));
        if info.doc.is_empty() {
            undocumented.push(name.to_owned());
        }
        if let crate::reflect::Shape::Struct {
            fields,
            tuple: false,
        } = &info.shape
        {
            let bare = fields.iter().filter(|field| field.meta.doc.is_empty());
            undocumented.extend(bare.map(|field| format!("{name}.{}", field.name)));
        }
    }
    assert!(undocumented.is_empty(), "undocumented: {undocumented:?}");
}

#[test]
#[should_panic(expected = "only as it attaches or detaches")]
fn a_layer_changing_the_world_as_it_updates_is_a_bug() {
    let mut stack = Stack::default();
    stack.push_layer(Spawner { rogue: true });
    let mut cx = Context::new(None);
    stack.attach(&mut cx).expect("loaded");
    stack.update(&mut cx, Duration::ZERO);
}

#[test]
fn events_go_down_the_stack_until_a_layer_handles_them_but_releases_go_on() {
    let mut cx = Context::new(None);
    let (mut stack, calls) = probes(false);
    assert_eq!(
        stack.event(&mut cx, &Event::CursorEntered),
        ControlFlow::Continue(())
    );
    assert_eq!(*calls.borrow(), ["event o", "event b", "event a"]);

    // A press, and a stick or trigger away from rest, stop at the layer that handles them.
    let pad = crate::Pad(0);
    let stick = |value| Event::PadStick {
        pad,
        stick: crate::Stick::Left,
        value,
    };
    let trigger = |value| Event::PadTrigger {
        pad,
        trigger: crate::Trigger::Left,
        value,
    };
    let south = |pressed| Event::PadButton {
        pad,
        button: crate::PadButton::South,
        pressed,
    };
    for handled in [
        Event::CursorEntered,
        south(true),
        stick(Vec2::X),
        trigger(0.5),
    ] {
        let (mut stack, calls) = probes(true);
        assert_eq!(stack.event(&mut cx, &handled), ControlFlow::Break(()));
        assert_eq!(*calls.borrow(), ["event o", "event b"], "{handled:?}");
    }

    // So that the layers below let go too.
    for lets_go in [
        Event::Key {
            key: Key::Space,
            pressed: false,
            repeat: false,
        },
        Event::MouseButton {
            button: crate::MouseButton::Left,
            pressed: false,
        },
        Event::Focused(false),
        south(false),
        stick(Vec2::ZERO),
        trigger(0.0),
    ] {
        let (mut stack, calls) = probes(true);
        assert_eq!(stack.event(&mut cx, &lets_go), ControlFlow::Break(()));
        assert_eq!(
            *calls.borrow(),
            ["event o", "event b", "event a"],
            "{lets_go:?}"
        );
    }
}

#[test]
fn the_input_state_takes_each_event_before_the_layers() {
    let held = Rc::new(RefCell::new(Vec::new()));
    let mut stack = Stack::default();
    stack.push_layer(Poll(held.clone()));
    let mut cx = Context::new(None);
    for pressed in [true, false] {
        let key = Event::Key {
            key: Key::Space,
            pressed,
            repeat: false,
        };
        let _ = stack.event(&mut cx, &key);
    }
    assert_eq!(*held.borrow(), [true, false]);
}

/// Keeps [`Layer::ui`]'s default.
#[cfg(feature = "dev")]
struct Quiet;

#[cfg(feature = "dev")]
impl Layer for Quiet {}

/// Draws nothing, through a `ui` of its own.
#[cfg(feature = "dev")]
struct Blank;

#[cfg(feature = "dev")]
impl Layer for Blank {
    fn ui(&mut self, _: &mut Context, _: &mut Ui<'_>) {}
}

/// Draws what the layer it wraps does, through a `ui` of its own.
#[cfg(feature = "dev")]
struct Wrapper(Quiet);

#[cfg(feature = "dev")]
impl Layer for Wrapper {
    fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
        self.0.ui(cx, ui);
    }
}

#[cfg(feature = "dev")]
fn draws_ui(mut stack: Stack) -> bool {
    let egui = crate::egui_context();
    let mut dock = crate::dock::Dock::new(None, crate::chrome::Chrome::Os, None, false, &[]);
    let mut cx = Context::new(None);
    let mut draws = false;
    let mut output = egui.run_ui(egui::RawInput::default(), |root| {
        dock.begin(root);
        draws |= stack.ui(&mut cx, &mut Ui::new(root, &mut dock));
        dock.end(root);
    });
    // As the renderer would have.
    output.textures_delta.clear();
    draws
}

/// A layer draws UI unless its `ui` is its type's default: one with its own `ui` draws, even
/// drawing nothing, as does one whose `ui` calls another layer's default.
#[cfg(feature = "dev")]
#[test]
fn layers_draw_ui_unless_theirs_is_the_default() {
    let stack = |push: &dyn Fn(&mut Stack)| {
        let mut stack = Stack::default();
        push(&mut stack);
        stack
    };
    assert!(!draws_ui(stack(&|_| {})), "no layers");
    assert!(!draws_ui(stack(&|stack| {
        stack.push_layer(Quiet);
        stack.push_overlay(Quiet);
    })));
    assert!(draws_ui(stack(&|stack| {
        stack.push_layer(Quiet);
        stack.push_overlay(Blank);
    })));
    assert!(draws_ui(stack(&|stack| stack.push_layer(Wrapper(Quiet)))));
}

/// A run that reads a package loads from it what layers add and text names, with no files on disk:
/// textures (compressed; by path, ID, and ID in text), cooked models (levels of detail, compressed
/// images), scene text, scripts, prefabs by ID.
#[test]
fn a_run_loads_from_its_package_with_no_file_on_the_disk() {
    use crate::image::tests::ktx2;
    use crate::package::{Kind, Package, Writer};

    let dir = crate::dirs::tests::Scratch::new("package-loads");
    // Never made: what loads, loads from the package.
    let root = dir.0.join("game");
    // A block of BC7, in KTX2.
    let block = |size| ktx2(ktx2::Format::BC7_SRGB_BLOCK, size, &[vec![0; 16]], true);
    // The glTF tests' model, cooked: each primitive's triangles twice (second level), each image a
    // block.
    let glb = crate::gltf::tests::model();
    let read = |_: &Path, _: u64| Ok(glb.clone());
    let mut model = (crate::gltf::read("m", Path::new("m.glb"), read, u64::MAX))
        .unwrap_or_else(|error| panic!("{error:#}"));
    for primitive in &mut model.primitives {
        let count = primitive.indices.len() as u32;
        primitive.indices.extend_from_within(..);
        let levels = vec![
            gg_renderer::Lod {
                start: 0,
                count,
                error: 0.0,
            },
            gg_renderer::Lod {
                start: count,
                count,
                error: 0.5,
            },
        ];
        let mut mesh = crate::model::model_mesh("m", primitive).expect("a mesh");
        mesh.lods.levels = levels;
        primitive.mesh = Some(mesh);
    }
    // Each compressed as what it holds says, as the cooker does.
    let images: Vec<_> = (model.images.iter().enumerate())
        .map(|(i, image)| {
            let holds = crate::model::texels(&model.primitives, i);
            let format = crate::image::Compressed::of(holds).ktx2();
            let size = image.size.map(|side| side.min(4));
            ktx2(format, size, &[vec![0; 16]], true)
        })
        .collect();
    let still = "local Still = gg.declare(\"Still\", { rate = 1 })\nreturn Still\n";
    let mut writer = Writer::new(".", Some("scenes/main.ron"), None);
    let id = 0x5f3c_9a1e_07b2_d846;
    writer
        .add("textures/stone.png", Some(id), Kind::Texture, block([1, 1]))
        .unwrap();
    let cooked = crate::cooked::encode(&model, &images);
    writer
        .add("models/m.glb", None, Kind::Model, cooked)
        .unwrap();
    let scene = b"(\n    entities: {},\n)\n".to_vec();
    writer
        .add("scenes/main.ron", None, Kind::File, scene.clone())
        .unwrap();
    writer
        .add("scripts/still.luau", None, Kind::File, still.into())
        .unwrap();
    let lamp = b"(\n    entities: {\n        \"0000000000000001\": (\n            Sprite: (\n                \
                     texture: Some(\"5f3c9a1e07b2d846\"),\n            ),\n        ),\n    },\n)\n";
    let lamp_id = 0x6b5e_1c3f_29d0_a487;
    writer
        .add(
            "prefabs/lamp.prefab",
            Some(lamp_id),
            Kind::File,
            lamp.to_vec(),
        )
        .unwrap();
    let path = dir.0.join("game.ggpk");
    writer.write(&path).unwrap();
    let mut cx = Context::seeded(1);
    cx.root = Some(root.clone());
    cx.files = Files::package(Arc::new(Package::open(&path).unwrap()), root.clone());
    assert!(cx.reads_package());

    let stone = (cx.add_texture("stone", root.join("textures/stone.png")))
        .unwrap_or_else(|error| panic!("{error:#}"));
    assert_eq!(
        cx.assets.name(stone),
        Some("5f3c9a1e07b2d846"),
        "named by its ID"
    );
    assert_eq!(cx.assets.path(stone), Some("textures/stone.png"));
    let again = cx.add_texture("again", root.join("textures/../textures/stone.png"));
    assert_eq!(again.expect("the same"), stone);
    let gone = cx.add_texture("gone", root.join("textures/gone.png"));
    assert_eq!(
        gone.expect_err("not in the package").code(),
        "engine.bad_texture"
    );
    let (meshes, textures) = (cx.meshes.count(), cx.textures.count());
    (cx.add_model("m", root.join("models/m.glb"))).unwrap_or_else(|error| panic!("{error:#}"));
    let added = (cx.meshes.count() - meshes, cx.textures.count() - textures);
    let counts = (model.primitives.len() as u32, model.images.len() as u32);
    assert_eq!(
        added, counts,
        "a mesh for each primitive, a texture for each image"
    );
    // As text names it, by ID, which a run without `dev` finds in the package alone.
    cx.load_named([Asset::Texture("5f3c9a1e07b2d846")], false);
    assert_eq!(cx.assets.texture("5f3c9a1e07b2d846"), Some(stone));
    let unknown = cx.asset_path("0123456789abcdef", "texture", false);
    assert_eq!(unknown.expect_err("no such ID").code(), "asset.unknown_id");

    let read = crate::ecs::read_scene_from(&root.join("scenes/main.ron"), &cx.files);
    assert_eq!(read.expect("the scene's text").as_bytes(), scene);
    let mut scripts = crate::Scripts::new().expect("Luau starts");
    let declared = scripts.load(&mut cx, root.join("scripts/still.luau"));
    assert_eq!(declared.expect("the script loads"), "Still");
    // A prefab by ID, whose instance's sprite shows the texture it names.
    cx.load_named([Asset::Prefab("6b5e1c3f29d0a487")], false);
    let prefab = cx.assets.prefab("6b5e1c3f29d0a487").expect("by its ID");
    assert_eq!(cx.assets.prefab_path(prefab), "prefabs/lamp.prefab");
    let placed = prefab.spawn(cx.world_mut(), Transform::IDENTITY);
    let placed = placed.unwrap_or_else(|error| panic!("{error:#}"));
    let sprite = cx.world().get::<Sprite>(placed).expect("the prefab's");
    assert_eq!(sprite.texture, Some(stone));
    // Nor does a scan, as the tool API's `assets` asks, read the disk under the root or write an ID
    // file.
    #[cfg(feature = "dev")]
    {
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("unlisted.png"), b"").unwrap();
        cx.scan_assets(true);
        assert!(
            !root.join("unlisted.png.meta").exists(),
            "an ID file written"
        );
    }
}

#[test]
fn a_sprite_sheet_loads_only_if_its_cells_fit_its_image() {
    let (mut loader, mut textures) = (Loader::inline(), Textures::default());
    let path = Path::new("assets/tiles.png");
    let image = || Image {
        size: [64, 32],
        pixels: vec![0; 64 * 32 * 4],
        format: image::Format::Rgba8,
    };
    for cell in [[0, 16], [16, 0], [65, 16], [16, 33]] {
        let error = add_sheet(&mut loader, &mut textures, "tiles", path, image(), cell)
            .expect_err("cells that don't fit");
        assert_eq!(error.code(), "engine.bad_sprite_sheet", "{cell:?}");
        let message = error.to_string();
        assert!(
            message.contains("\"tiles\"")
                && message.contains(&path.display().to_string())
                && message.contains("64x32"),
            "{message}"
        );
    }
    let sheet = add_sheet(&mut loader, &mut textures, "tiles", path, image(), [64, 32]);
    let sheet = sheet.expect("a sheet");
    assert_eq!((sheet.size, sheet.cell), ([64, 32], [64, 32]));
    // The sheets that didn't load took no texture, so this one's comes first after the engine's
    // white one.
    let first = Textures::default().add("first", [1, 1], &[0; 4], Mipmaps::None);
    assert_eq!(sheet.texture, first.expect("a texture"));
}

#[test]
fn a_model_that_cant_be_read_fails_naming_its_file() {
    let path = Path::new("no such folder/truck.glb");
    let error = Context::seeded(0)
        .add_model("truck", path)
        .expect_err("no model");
    assert_eq!(error.code(), "engine.bad_model");
    let message = error.to_string();
    assert!(
        message.contains("\"truck\"") && message.contains(&path.display().to_string()),
        "{message}"
    );
}

/// A texture's file added again by another form of its path gives the first texture without loading
/// again, as does a sprite sheet's (cut by the image's size). The first path's name stays and the
/// other names it too; each add notes its own path, as a snapshot's asset lines count.
#[test]
fn a_texture_added_twice_loads_once_however_its_path_is_written() {
    let scratch = crate::dirs::tests::Scratch::new("texture-once");
    let root = scratch.0.clone();
    std::fs::create_dir_all(root.join("assets")).expect("a directory");
    let png = image::tests::png(
        png::ColorType::Rgba,
        png::BitDepth::Eight,
        4,
        &[9; 16],
        None,
    );
    std::fs::write(root.join("assets/tiles.png"), png).expect("a PNG");
    let plain = root.join("assets").join("tiles.png");
    // A path through `..`, which only resolving the file finds the same.
    let dotted = root.join("./assets/../assets/tiles.png");
    let mut cx = Context::seeded(0).root(root.clone());
    let texture = cx.add_texture("tiles", &plain).expect("a texture");
    let count = cx.textures.count();
    assert_eq!(cx.add_texture("again", &dotted).expect("it again"), texture);
    assert_eq!(cx.textures.count(), count, "loaded once");
    let sheet = cx
        .add_sprite_sheet("tiles", &dotted, [2, 1])
        .expect("a sheet");
    assert_eq!(
        (sheet.texture, sheet.size, sheet.cell),
        (texture, [4, 1], [2, 1])
    );
    let error = (cx.add_sprite_sheet("tiles", &plain, [8, 1])).expect_err("cells too wide");
    assert_eq!(error.code(), "engine.bad_sprite_sheet");
    assert_eq!(cx.textures.count(), count);
    let (first, other) = ("assets/tiles.png", "assets/../assets/tiles.png");
    assert_eq!(cx.assets.path(texture), Some(first));
    assert_eq!(cx.assets.texture(other), Some(texture));
    let note = |name: &str| format!("texture {} 1 {count} {name}", cx.meshes.count());
    let added: Vec<&str> = cx.assets.added().iter().map(|note| &**note).collect();
    assert_eq!(added, [note(first), note(other), note(other)]);

    let mut sheet_first = Context::seeded(0).root(root);
    let sheet = (sheet_first.add_sprite_sheet("tiles", &dotted, [2, 1])).expect("a sheet");
    assert_eq!(sheet.size, [4, 1]);
    let texture = sheet_first
        .add_texture("tiles", &plain)
        .expect("its texture");
    assert_eq!(texture, sheet.texture);
    assert_eq!(sheet_first.textures.count(), count);
}

/// A texture whose ID file holds an ID is named by it (a test's context reads it from the root,
/// writing none) and found by ID or path; without one, by path; a model's images by the model's ID
/// and place. So a world hashes alike once a texture's file is renamed with its ID file, not
/// without it.
#[cfg(feature = "dev")]
#[test]
fn a_texture_is_named_by_its_id_so_its_file_renamed_with_it_hashes_alike() {
    use gg_api::asset::Meta;

    let scratch = crate::dirs::tests::Scratch::new("named-by-id");
    let root = scratch.0.clone();
    std::fs::create_dir_all(root.join("assets")).expect("a directory");
    let png = image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
    let write = |name: &str, bytes: &[u8]| {
        std::fs::write(root.join("assets").join(name), bytes).expect("written");
    };
    let meta = |id: &str| {
        Meta {
            id: id.parse().expect("an ID"),
        }
        .text()
    };
    let (stone, model) = ("5f3c9a1e07b2d846", "1d2c3b4a59687f0e");
    write("stone.png", &png);
    write("stone.png.meta", meta(stone).as_bytes());
    write("plain.png", &png);
    write("m.glb", &gltf::tests::model());
    write("m.glb.meta", meta(model).as_bytes());
    let mut cx = Context::seeded(0).root(root.clone());
    assert!(!root.join("assets/plain.png.meta").exists(), "none written");
    let dotted = "assets/./stone.png";
    let textured = cx
        .add_texture("stone", root.join(dotted))
        .expect("a texture");
    let plain = cx.add_texture("plain", root.join("assets/plain.png"));
    let plain = plain.expect("a texture");
    cx.add_model("m", root.join("assets/m.glb"))
        .expect("a model");
    let assets = &cx.assets;
    assert_eq!(assets.name(textured), Some(stone));
    assert_eq!(assets.path(textured), Some(dotted));
    assert_eq!(
        [stone, dotted].map(|name| assets.texture(name)),
        [Some(textured); 2]
    );
    assert_eq!(assets.name(plain), Some("assets/plain.png"));
    let named: Vec<_> = assets.named_from((0, 0, 0, 0)).collect();
    let images = &named[1..named.len() - 1];
    assert_eq!(named[0], (stone, dotted));
    assert!(!images.is_empty(), "{named:?}");
    // The model itself, after the textures.
    assert_eq!(named.last(), Some(&(model, "assets/m.glb")));
    for (i, &image) in images.iter().enumerate() {
        let (id, path) = (format!("{model}#{i}"), format!("assets/m.glb#{i}"));
        assert_eq!(image, (&*id, &*path));
    }
    // A file outside the root, given by a working-directory path spelled as one in the root is, has
    // none of that one's ID.
    let stone_id = Some(stone.parse().expect("an ID"));
    assert_eq!(cx.asset_id(&root.join("assets/stone.png"), None), stone_id);
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let outside = assets::file_of(&manifest).expect("gg-engine's manifest");
    let spelled = Path::new("assets/stone.png");
    assert_eq!(cx.asset_id(spelled, Some(&outside)), None);

    let hash = |file: &str| {
        let mut cx = Context::seeded(0).root(root.clone());
        let texture = cx.add_texture("stone", root.join(file)).expect("a texture");
        let world = cx.world_mut();
        let sprite = Sprite {
            texture: Some(texture),
            ..Sprite::default()
        };
        world.spawn((Transform::IDENTITY, sprite));
        world.hash()
    };
    let before = hash("assets/stone.png");
    for name in ["stone.png", "stone.png.meta"] {
        let to = name.replacen("stone", "rock", 1);
        let assets = root.join("assets");
        std::fs::rename(assets.join(name), assets.join(to)).expect("renamed");
    }
    assert_eq!(hash("assets/rock.png"), before, "renamed with its ID file");
    std::fs::remove_file(root.join("assets/rock.png.meta")).expect("removed");
    assert_ne!(hash("assets/rock.png"), before, "by its path, without one");
}

/// Renamed with its ID file while loaded, a texture's file added again by its new path (once the
/// database has it) gives the loaded texture, loading nothing; a sheet cut from it too.
#[cfg(feature = "dev")]
#[test]
fn a_texture_renamed_while_loaded_loads_once_by_its_new_path() {
    use gg_api::asset::Meta;

    let scratch = crate::dirs::tests::Scratch::new("renamed-loaded");
    let root = scratch.0.clone();
    let assets = root.join("assets");
    std::fs::create_dir_all(&assets).expect("a directory");
    let png = image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 2, &[9; 8], None);
    std::fs::write(assets.join("stone.png"), png).expect("written");
    let meta = Meta {
        id: "5f3c9a1e07b2d846".parse().expect("an ID"),
    };
    std::fs::write(assets.join("stone.png.meta"), meta.text()).expect("written");
    let mut cx = Context::seeded(0).root(root.clone());
    let stone = cx.add_texture("stone", assets.join("stone.png"));
    let stone = stone.expect("a texture");
    for name in ["stone.png", "stone.png.meta"] {
        let to = assets.join(name.replacen("stone", "rock", 1));
        std::fs::rename(assets.join(name), to).expect("renamed");
    }
    cx.scan_assets(false);
    let count = cx.assets.counts();
    let rock = cx.add_texture("rock", assets.join("rock.png"));
    assert_eq!(rock.expect("a texture"), stone);
    let sheet = cx.add_sprite_sheet("rock", assets.join("rock.png"), [1, 1]);
    assert_eq!(sheet.expect("a sheet").texture, stone);
    assert_eq!(cx.assets.counts(), count, "nothing loaded again");
    assert_eq!(cx.assets.texture("assets/rock.png"), Some(stone));
}

/// A run serving the tool API and writing ID files caches its assets' prints through the watching
/// thread, so the next run recognizes an asset renamed while no run watched, by the ID file the
/// rename left. A run that writes none reads no cache and moves nothing.
#[cfg(feature = "dev")]
#[test]
fn a_runs_start_keeps_the_id_of_an_asset_renamed_while_none_watched() {
    let scratch = crate::dirs::tests::Scratch::new("start-renamed");
    let root = scratch.0.join("game");
    std::fs::create_dir_all(&root).expect("a directory");
    std::fs::write(root.join("a.png"), "alpha").expect("an asset");
    let cache = scratch.0.join("cache").join("prints.json");
    let started = |serves, writes| {
        let mut cx = Context::seeded(1);
        cx.root = Some(root.clone());
        cx.dev.asset_cache = Some(cache.clone());
        cx.start_assets(serves, writes);
        cx
    };
    let cx = started(true, true);
    let a = cx.catalog().id("a.png").expect("an ID written");
    cx.ask_assets().expect("a thread watches");
    // The thread keeps the prints after it answers a look; a run's end waits for it up to
    // `asset_watch::STOP`: until the cache holds a.png's.
    let kept = || std::fs::read_to_string(&cache).is_ok_and(|kept| kept.contains(&a.to_string()));
    let until = std::time::Instant::now() + Duration::from_secs(30);
    while !kept() {
        assert!(
            std::time::Instant::now() < until,
            "the thread kept the prints"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(cx);
    std::fs::rename(root.join("a.png"), root.join("b.png")).expect("renamed");
    let reads = started(false, false);
    assert_eq!(reads.catalog().id("b.png"), None, "a run that writes none");
    assert!(root.join("a.png.meta").exists());
    let writes = started(false, true);
    assert_eq!(writes.catalog().id("b.png"), Some(a));
    assert!(!writes.watches_assets(), "a run that serves no API");
    assert!(!root.join("a.png.meta").exists(), "moved beside b.png");
}

/// In a run that reloads assets, an edited texture file gives the new image as a frame starts,
/// under its handle, at another size. A texture renamed with its ID file is followed where the
/// database finds it, edits there included. A stand-in takes its file's image once the file comes,
/// followed by the ID the database then gives. A sheet's texture keeps its size, warning (its cells
/// would move). A stand-in for a file outside the asset root takes none.
#[cfg(feature = "dev")]
#[test]
fn a_texture_edited_renamed_or_come_reloads_under_its_handle() {
    use gg_api::asset::Meta;

    let scratch = crate::dirs::tests::Scratch::new("reloaded");
    let root = scratch.0.join("game");
    let assets = root.join("assets");
    std::fs::create_dir_all(&assets).expect("a directory");
    let png = |width: u32| {
        let pixels = vec![9; width as usize * 4];
        image::tests::png(
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            width,
            &pixels,
            None,
        )
    };
    // Dated an hour back, so each loads as it is.
    let written_long_ago = |name: &str, bytes: &[u8]| {
        let path = assets.join(name);
        std::fs::write(&path, bytes).expect("written");
        let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(60 * 60);
        let file = std::fs::File::options().write(true).open(path);
        (file.and_then(|file| file.set_modified(an_hour_ago))).expect("its time");
    };
    written_long_ago("stone.png", &png(1));
    let meta = Meta {
        id: "5f3c9a1e07b2d846".parse().expect("an ID"),
    };
    written_long_ago("stone.png.meta", meta.text().as_bytes());
    written_long_ago("sheet.png", &png(2));
    let logged = crate::testing::logged();
    let mut cx = Context::seeded(0).root(root.clone());
    cx.dev.hot_reload = Some(HotReload::start().expect("the thread"));
    let stone = cx.add_texture("stone", assets.join("stone.png"));
    let stone = stone.expect("a texture");
    let sheet = cx.add_sprite_sheet("sheet", assets.join("sheet.png"), [1, 1]);
    let sheet = sheet.expect("a sheet").texture;
    cx.load_named([Asset::Texture("assets/later.png")], true);
    let later = cx.assets.texture("assets/later.png").expect("a stand-in");

    let size = |cx: &Context, texture: Texture| {
        let loaded = cx.loaded.textures.values();
        loaded
            .copied()
            .find(|&(loaded, _)| loaded == texture)
            .map(|(_, size)| size)
    };
    // Frame starts until `done`.
    let until = |cx: &mut Context, done: &dyn Fn(&Context) -> bool, what: &str| {
        let until = std::time::Instant::now() + Duration::from_secs(30);
        while !done(cx) {
            assert!(std::time::Instant::now() < until, "{what}");
            std::thread::sleep(Duration::from_millis(5));
            cx.take_reloaded(false);
        }
    };
    std::fs::write(assets.join("stone.png"), png(3)).expect("an edit");
    until(
        &mut cx,
        &|cx| size(cx, stone) == Some([3, 1]),
        "the edit reloads",
    );

    // Renamed with its ID file and edited at once, before the database has it there.
    for name in ["stone.png", "stone.png.meta"] {
        let to = assets.join(name.replacen("stone", "rock", 1));
        std::fs::rename(assets.join(name), to).expect("renamed");
    }
    std::fs::write(assets.join("rock.png"), png(4)).expect("an edit");
    cx.scan_assets(false);
    until(
        &mut cx,
        &|cx| size(cx, stone) == Some([4, 1]),
        "the edit where it is reloads",
    );

    std::fs::write(assets.join("later.png"), png(5)).expect("its file comes");
    until(
        &mut cx,
        &|cx| size(cx, later) == Some([5, 1]),
        "the stand-in gives way",
    );
    let added = cx.add_texture("later", assets.join("later.png"));
    assert_eq!(added.expect("a texture"), later, "loaded once");
    // Given an ID file, which the database reads, then renamed with it and edited at once.
    let meta = Meta {
        id: "7a1b2c3d4f5a6b7c".parse().expect("an ID"),
    };
    std::fs::write(assets.join("later.png.meta"), meta.text()).expect("written");
    cx.scan_assets(false);
    for name in ["later.png", "later.png.meta"] {
        let to = assets.join(name.replacen("later", "other", 1));
        std::fs::rename(assets.join(name), to).expect("renamed");
    }
    std::fs::write(assets.join("other.png"), png(6)).expect("an edit");
    cx.scan_assets(false);
    until(
        &mut cx,
        &|cx| size(cx, later) == Some([6, 1]),
        "the edit where the stand-in's file is now reloads",
    );

    let warned = |code: &'static str| {
        let logged = &logged;
        move |_: &Context| logged.codes.try_iter().any(|logged| logged == code)
    };
    std::fs::write(assets.join("sheet.png"), png(7)).expect("an edit");
    let resized = warned("asset.sheet_resized");
    until(&mut cx, &resized, "a sheet's texture keeps its size");
    assert_eq!(size(&cx, sheet), Some([2, 1]));

    // Outside the root, through `..`: one there is refused and not watched; one that comes after is
    // refused as it comes.
    std::fs::write(scratch.0.join("there.png"), png(8)).expect("written");
    cx.load_named(["../there.png", "../after.png"].map(Asset::Texture), true);
    let there = cx.assets.texture("../there.png").expect("a stand-in");
    let after = cx.assets.texture("../after.png").expect("a stand-in");
    std::fs::write(scratch.0.join("there.png"), png(9)).expect("an edit");
    std::fs::write(scratch.0.join("after.png"), png(10)).expect("it comes");
    let outside = warned("asset.outside_root");
    until(&mut cx, &outside, "a file outside the root is refused");
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(10));
        cx.take_reloaded(false);
    }
    assert_eq!((size(&cx, there), size(&cx, after)), (None, None));
}

/// An ID no asset has names the file the project's fetch list gives it (read beside the manifest)
/// and `cargo gg fetch`. An ID the list lacks, or a context that isn't a project's game, names
/// none.
#[cfg(feature = "dev")]
#[test]
fn an_unknown_id_the_fetch_list_gives_names_what_fetches_it() {
    let scratch = crate::dirs::tests::Scratch::new("unknown-fetched");
    let dir = scratch.0.join("game");
    std::fs::create_dir_all(dir.join("content")).expect("a directory");
    let project = gg_api::Project {
        game: "tested".to_owned(),
        assets: "content".to_owned(),
        scenes: vec!["content/scenes".to_owned()],
        start: "content/scenes/main.ron".to_owned(),
        relay: None,
        icon: None,
    };
    std::fs::write(dir.join(gg_api::Project::FILE), project.text()).expect("written");
    let (id, hash) = (
        "44ae3d1eca56e109",
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    );
    let list = format!(
        "[\n    (\n        from: \"https://example.com/sponza\",\n        into: \
             \"content/models/sponza\",\n        files: {{\n            \"Sponza.gltf\": \
             \"{hash}\",\n        }},\n        ids: {{\n            \"Sponza.gltf\": \
             \"{id}\",\n        }},\n    ),\n]\n"
    );
    std::fs::write(dir.join(gg_api::Fetch::FILE), list).expect("written");

    let cx = Context::seeded(0).project(&dir).expect("the project reads");
    let error = cx.asset_path(id, "model", false).expect_err("not fetched");
    let message = format!("{error:#}");
    assert_eq!(error.code(), "asset.unknown_id", "{message}");
    assert!(
        message.contains("`content/models/sponza/Sponza.gltf`")
            && message.contains("`cargo gg fetch`"),
        "{message}"
    );
    let other = cx.asset_path("0a1b2c3d4e5f6071", "model", false);
    let message = format!("{:#}", other.expect_err("no such ID"));
    assert!(!message.contains("fetch"), "{message}");

    let rooted = Context::seeded(0).root(dir.join("content"));
    let message = format!(
        "{:#}",
        rooted.asset_path(id, "model", false).expect_err("none")
    );
    assert!(!message.contains("fetch"), "{message}");
}

/// A scene file loads the textures it names that no layer added, by ID or root-relative path, once
/// each, named as a layer's add would; a model's image loads with its model. One that can't load is
/// refused where named, saying why, spawning nothing: not an image, missing, outside the root
/// (absolute or `..`), an ID no asset has, an image the model lacks. A run following another loads
/// and stands in for those; asked by the tool API (a lockstep joiner), it loads only what it was
/// given.
#[cfg(feature = "dev")]
#[test]
fn a_scene_loads_the_textures_it_names_and_refuses_those_it_cant() {
    use gg_api::asset::Meta;

    let logged = crate::testing::logged();
    let scratch = crate::dirs::tests::Scratch::new("loaded-where-named");
    let root = scratch.0.join("game");
    for dir in ["assets", "scenes"] {
        std::fs::create_dir_all(root.join(dir)).expect("a directory");
    }
    let png = image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
    let meta = |id: &str| Meta {
        id: id.parse().expect("an ID"),
    };
    let (stone, model) = ("5f3c9a1e07b2d846", "1d2c3b4a59687f0e");
    let write = |name: &str, bytes: &[u8]| std::fs::write(root.join(name), bytes);
    write("assets/stone.png", &png).expect("written");
    write("assets/stone.png.meta", meta(stone).text().as_bytes()).expect("written");
    write("assets/plain.png", &png).expect("written");
    write("assets/bad.png", b"not a PNG").expect("written");
    write("assets/m.glb", &gltf::tests::model()).expect("written");
    write("assets/m.glb.meta", meta(model).text().as_bytes()).expect("written");
    std::fs::write(scratch.0.join("outside.png"), &png).expect("written");
    // A scene of a sprite of each texture named, IDs from `first` on.
    let scene = |first: usize, names: &[&str]| -> PathBuf {
        let mut text = "(\n    entities: {\n".to_owned();
        for (i, name) in names.iter().enumerate() {
            text += &format!(
                "        \"{:016x}\": (\n            Sprite: (\n                texture: \
                     Some(\"{name}\"),\n            ),\n        ),\n",
                first + i
            );
        }
        text += "    },\n)\n";
        let path = root.join("scenes/s.ron");
        std::fs::write(&path, text).expect("written");
        path
    };
    let shown = |cx: &Context| -> Vec<String> {
        let sprites = cx.world().query::<&Sprite>();
        let names = sprites.filter_map(|sprite| cx.assets.name(sprite.texture?));
        names.map(str::to_owned).collect()
    };

    let mut cx = Context::seeded(0).root(root.clone());
    let count = cx.textures.count();
    let image = format!("{model}#0");
    let path = scene(
        0x1000,
        &[stone, "assets/plain.png", "assets/./stone.png", &image],
    );
    cx.load_scene_file(&path).expect("loaded");
    assert_eq!(
        cx.textures.count(),
        count + 4,
        "each once, the model's two images among them"
    );
    assert!(
        cx.assets.model(model).is_some(),
        "the model, with its image"
    );
    let mut names = shown(&cx);
    names.sort_unstable();
    assert_eq!(names, [&image, stone, stone, "assets/plain.png"]);

    let absolute = root
        .join("assets/plain.png")
        .to_string_lossy()
        .replace('\\', "/");
    let image = format!("{model}#9");
    let refused = [
        ("assets/bad.png", "engine.bad_texture"),
        ("assets/none.png", "engine.bad_texture"),
        ("../outside.png", "asset.outside_root"),
        (absolute.as_str(), "asset.outside_root"),
        ("0a1b2c3d4e5f6071", "asset.unknown_id"),
        (image.as_str(), "asset.model_image"),
    ];
    let entities = cx.world().entities(None).map_or(0, |e| e.len());
    let count = cx.textures.count();
    for &(name, why) in &refused {
        let error = cx
            .load_scene_file(scene(0x2000, &["assets/plain.png", name]))
            .expect_err(name);
        let message = format!("{error:#}");
        assert_eq!(error.code(), "asset.no_texture", "{message}");
        assert!(
            message.contains(why) && message.contains(".ron:"),
            "{message}"
        );
    }
    assert_eq!(cx.world().entities(None).map_or(0, |e| e.len()), entities);
    assert_eq!(cx.textures.count(), count, "nothing loaded");

    let mut following = Context::seeded(0).root(root.clone());
    following.name_as([]);
    following.follows = true;
    let _ = logged.codes.try_iter().count();
    let names: Vec<&str> = refused.iter().map(|&(name, _)| name).collect();
    following
        .load_scene_file(scene(0x1000, &names))
        .expect("stood in");
    let mut shown = shown(&following);
    shown.sort_unstable();
    let mut expected = names.clone();
    expected.sort_unstable();
    assert_eq!(shown, expected);
    let codes = logged
        .codes
        .try_iter()
        .filter(|code| code == "asset.stand_in");
    assert_eq!(codes.count(), refused.len());

    // Asked by the tool API, it loads only what it was given.
    let given = [(Arc::from(stone), Arc::from("assets/stone.png"))];
    let mut joiner = Context::seeded(0).root(root);
    joiner.name_as(given);
    joiner.follows = true;
    joiner.load_named([stone, "assets/plain.png"].map(Asset::Texture), false);
    assert!(joiner.assets.texture(stone).is_some());
    let plain = joiner.assets.not_found("texture", "assets/plain.png");
    assert!(plain.contains("asset.not_given"), "{plain}");
}

/// A scene naming a model by ID loads it and its root spawns the model's nodes. Edited, the model's
/// nodes and materials reload into the root's nodes, unless the run is recorded (kept, warning).
/// Renamed with its ID file, it loads by ID from where it is.
#[cfg(feature = "dev")]
#[test]
fn a_scene_names_a_model_by_its_id_whose_nodes_reload_as_it_changes() {
    use gg_api::asset::Meta;

    let logged = crate::testing::logged();
    let scratch = crate::dirs::tests::Scratch::new("model-by-id");
    let root = scratch.0.join("game");
    for dir in ["assets", "scenes"] {
        std::fs::create_dir_all(root.join(dir)).expect("a directory");
    }
    let id = "3c4d5e6f708192a3";
    let gltf = |translation: [f32; 3], red: f32| {
        gltf::tests::triangle_gltf(|gltf| {
            gltf["nodes"][0]["translation"] = serde_json::json!(translation);
            let color = serde_json::json!([red, 1.0, 1.0, 1.0]);
            gltf["materials"][0]["pbrMetallicRoughness"]["baseColorFactor"] = color;
        })
    };
    // Dated an hour back, so it loads as it is.
    let written_long_ago = |name: &str, bytes: &[u8]| {
        let path = root.join("assets").join(name);
        std::fs::write(&path, bytes).expect("written");
        let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(60 * 60);
        let file = std::fs::File::options().write(true).open(path);
        (file.and_then(|file| file.set_modified(an_hour_ago))).expect("its time");
    };
    written_long_ago("tri.gltf", gltf([0.0; 3], 1.0).as_bytes());
    let meta = Meta {
        id: id.parse().expect("an ID"),
    };
    written_long_ago("tri.gltf.meta", meta.text().as_bytes());
    let scene = root.join("scenes/s.ron");
    let text = format!(
        "(\n    entities: {{\n        \"0000000000000100\": (\n            ModelRoot: \
             (\"{id}\"),\n            Transform: (),\n        ),\n    }},\n)\n"
    );
    std::fs::write(&scene, text).expect("written");

    let mut cx = Context::seeded(0).root(root.clone());
    cx.dev.hot_reload = Some(HotReload::start().expect("the thread"));
    cx.load_scene_file(&scene).expect("loaded");
    let model = cx.assets.model(id).expect("loaded by its ID");
    assert_eq!(cx.assets.model_path(model), "assets/tri.gltf");
    // Its image, named through another path to its file.
    let image = "assets/./tri.gltf#0";
    cx.load_named([Asset::Texture(image)], false);
    let found = cx.assets.texture(image);
    assert!(found.is_some() && found == cx.assets.texture(&format!("{id}#0")));
    let placed = cx
        .world()
        .entity("0000000000000100".parse().expect("an ID"));
    let placed = placed.expect("its root");
    let node = |cx: &Context| -> Entity {
        let mut children = cx.world().children(placed).expect("alive");
        children.next().expect("its node")
    };
    let at = |cx: &Context| {
        cx.world()
            .get::<Transform>(node(cx))
            .expect("a node")
            .translation
    };
    let draw = |cx: &Context| *cx.world().get::<MeshDraw>(node(cx)).expect("a draw");
    assert_eq!(cx.world().get::<ModelNode>(node(&cx)), Some(&ModelNode(0)));
    assert_eq!(at(&cx), DVec3::ZERO);
    let drew = draw(&cx);

    // Frame starts, as a run `recorded` or not, until `done`.
    let until = |cx: &mut Context, recorded, done: &dyn Fn(&Context) -> bool, what: &str| {
        let until = std::time::Instant::now() + Duration::from_secs(30);
        while !done(cx) {
            assert!(std::time::Instant::now() < until, "{what}");
            std::thread::sleep(Duration::from_millis(5));
            cx.take_reloaded(recorded);
        }
    };
    let path = root.join("assets/tri.gltf");
    std::fs::write(&path, gltf([0.0, 5.0, 0.0], 0.5)).expect("an edit");
    let moved = |cx: &Context| at(cx) == DVec3::new(0.0, 5.0, 0.0);
    until(&mut cx, false, &moved, "the node moves");
    // Its material's color changed in the materials table, where its draw reads it.
    assert_eq!(draw(&cx), drew, "the same draw, of the same material");

    let _ = logged.codes.try_iter().count();
    std::fs::write(&path, gltf([0.0, 7.0, 0.0], 0.5)).expect("an edit");
    let warned = |_: &Context| {
        logged
            .codes
            .try_iter()
            .any(|code| code == "asset.model_changed")
    };
    until(&mut cx, true, &warned, "a recorded run warns");
    assert!(moved(&cx), "and keeps its nodes");

    drop(cx);
    for name in ["tri.gltf", "tri.gltf.meta"] {
        let to = root.join("assets").join(name.replacen("tri", "moved", 1));
        std::fs::rename(root.join("assets").join(name), to).expect("renamed");
    }
    let mut cx = Context::seeded(0).root(root.clone());
    cx.load_scene_file(&scene).expect("loaded");
    let model = cx.assets.model(id).expect("loaded by its ID");
    assert_eq!(cx.assets.model_path(model), "assets/moved.gltf");
    assert!(cx.world().children(placed).expect("alive").next().is_some());
}

/// A scene naming a prefab by ID loads it and the textures its text names; its instance spawns the
/// prefab's entities. Edited, the prefab reloads and the instance takes what changed except its own
/// changes, by the time the thread's look since the save is taken, unless the run is recorded
/// (kept, warning). A run following another stands in for one it can't load: instances are their
/// roots alone.
#[cfg(feature = "dev")]
#[test]
fn a_scene_names_a_prefab_by_its_id_whose_instances_reload_as_it_changes() {
    use gg_api::asset::Meta;

    let logged = crate::testing::logged();
    let scratch = crate::dirs::tests::Scratch::new("prefab-by-id");
    let root = scratch.0.join("game");
    for dir in ["assets", "scenes"] {
        std::fs::create_dir_all(root.join(dir)).expect("a directory");
    }
    let id = "4d5e6f708192a3b4";
    let text_of = |scale: f32, name: &str| {
        format!(
            "(\n    entities: {{\n        \"0000000000000001\": (\n            Name: \
                 (\"{name}\"),\n            Sprite: (\n                color: (1.0, 1.0, 1.0, \
                 1.0),\n                texture: Some(\"assets/stone.png\"),\n            \
                 ),\n        ),\n        \"0000000000000002\": (\n            Parent: \
                 (\"0000000000000001\"),\n            Transform: (\n                scale: \
                 ({scale}, {scale}, 1.0),\n            ),\n        ),\n    }},\n)\n"
        )
    };
    // Dated an hour back, so it loads as it is.
    let written_long_ago = |name: &str, bytes: &[u8]| {
        let path = root.join("assets").join(name);
        std::fs::write(&path, bytes).expect("written");
        let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(60 * 60);
        let file = std::fs::File::options().write(true).open(path);
        (file.and_then(|file| file.set_modified(an_hour_ago))).expect("its time");
    };
    written_long_ago("lamp.prefab", text_of(2.0, "lamp").as_bytes());
    let meta = Meta {
        id: id.parse().expect("an ID"),
    };
    written_long_ago("lamp.prefab.meta", meta.text().as_bytes());
    let (color, depth) = (png::ColorType::Rgba, png::BitDepth::Eight);
    let png = crate::image::tests::png(color, depth, 1, &[255; 4], None);
    written_long_ago("stone.png", &png);
    let scene = root.join("scenes/s.ron");
    let text = format!(
        "(\n    entities: {{\n        \"0000000000000100\": (\n            Name: \
             (\"mine\"),\n            PrefabRoot: (\"{id}\"),\n        ),\n    }},\n)\n"
    );
    std::fs::write(&scene, &text).expect("written");

    let mut cx = Context::seeded(0).root(root.clone());
    cx.dev.hot_reload = Some(HotReload::start().expect("the thread"));
    cx.load_scene_file(&scene).expect("loaded");
    let prefab = cx.assets.prefab(id).expect("loaded by its ID");
    assert_eq!(cx.assets.prefab_path(prefab), "assets/lamp.prefab");
    assert!(
        cx.assets.texture("assets/stone.png").is_some(),
        "what it names"
    );
    let placed = cx
        .world()
        .entity("0000000000000100".parse().expect("an ID"));
    let placed = placed.expect("its root");
    let child = |cx: &Context| -> Entity {
        let mut children = cx.world().children(placed).expect("alive");
        children.next().expect("its child")
    };
    let scale = |cx: &Context| {
        let transform = cx.world().get::<Transform>(child(cx));
        transform.expect("the prefab's").scale
    };
    assert_eq!(scale(&cx), Vec3::new(2.0, 2.0, 1.0));
    let name = |cx: &Context| cx.world().get::<Name>(placed).cloned();
    assert_eq!(name(&cx), Some(Name("mine".to_owned())), "its own");

    // Frame starts, as a run `recorded` or not, until `done`.
    let until = |cx: &mut Context, recorded, done: &dyn Fn(&Context) -> bool, what: &str| {
        let until = std::time::Instant::now() + Duration::from_secs(30);
        while !done(cx) {
            assert!(std::time::Instant::now() < until, "{what}");
            std::thread::sleep(Duration::from_millis(5));
            cx.take_reloaded(recorded);
        }
    };
    let path = root.join("assets/lamp.prefab");
    std::fs::write(&path, text_of(3.0, "post")).expect("an edit");
    let saved = std::time::SystemTime::now();
    // Once the thread's look since the save is taken, the edit is in place: `files_looked`.
    let looked = |cx: &Context| cx.assets_looked().is_some_and(|at| at >= saved);
    until(&mut cx, false, &looked, "it looks since the save");
    let grown = |cx: &Context| scale(cx) == Vec3::new(3.0, 3.0, 1.0);
    assert!(grown(&cx), "the instance takes it");
    assert_eq!(
        name(&cx),
        Some(Name("mine".to_owned())),
        "and keeps its own"
    );

    let _ = logged.codes.try_iter().count();
    std::fs::write(&path, text_of(4.0, "post")).expect("an edit");
    let warned = |_: &Context| {
        logged
            .codes
            .try_iter()
            .any(|code| code == "asset.prefab_changed")
    };
    until(&mut cx, true, &warned, "a recorded run warns");
    assert!(grown(&cx), "and keeps its instances");

    let mut follower = Context::seeded(0);
    follower.follows = true;
    follower.load_named([Asset::Prefab("assets/none.prefab")], true);
    let stand_in = follower
        .assets
        .prefab("assets/none.prefab")
        .expect("a stand-in");
    assert_eq!(follower.assets.prefab_text(stand_in), None);
    let world = follower.world_mut();
    let root = stand_in.spawn(world, Transform::IDENTITY).expect("spawned");
    assert_eq!(
        world.children(root).expect("alive").count(),
        0,
        "its root alone"
    );
}

/// A UI added by its file's asset ID is named by it and loads the texture it names, its image fit
/// to the texture's size. Its layer sets text by node name (refused for an unknown node or one
/// saying nothing) and moves focus by key. A second add shares the file yet is a UI of its own.
/// Edited, the file reads again and each UI takes its nodes, keeping text codes set and focus by
/// name; an edit naming a texture that doesn't load warns with its code and leaves the UIs as they
/// were. Renamed with its ID file, it's found by ID at the new path, which each UI lists; a third
/// add by that path shares it. A new file at the old path is its own.
#[cfg(feature = "dev")]
#[test]
fn a_ui_added_by_its_id_reloads_as_its_file_changes() {
    use gg_api::asset::Meta;

    use crate::{Key, UiEvent};

    let logged = crate::testing::logged();
    let scratch = crate::dirs::tests::Scratch::new("ui-by-id");
    let root = scratch.0.join("game");
    std::fs::create_dir_all(root.join("ui")).expect("a directory");
    // Dated an hour back, so it loads as it is.
    let written_long_ago = |name: &str, bytes: &[u8]| {
        let path = root.join(name);
        std::fs::write(&path, bytes).expect("written");
        let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(60 * 60);
        let file = std::fs::File::options().write(true).open(path);
        (file.and_then(|file| file.set_modified(an_hour_ago))).expect("its time");
    };
    // Focused on `play` as added: a logo, a button of each name, a score.
    let menu = |texture: &str, buttons: &[&str]| {
        let buttons: String = (buttons.iter())
            .map(|name| format!("    Button(name: \"{name}\"),\n"))
            .collect();
        format!(
            "(focus: \"play\", nodes: [Column(children: [\n    Image(name: \"logo\", \
                 texture: \"{texture}\"),\n{buttons}    Text(name: \"score\", text: \"0\"),\n])])\n"
        )
    };
    let logo = "ui/logo.png";
    written_long_ago("ui/menu.ui", menu(logo, &["options", "play"]).as_bytes());
    let id = "6b7c8d9e0fa1b2c3";
    let meta = Meta {
        id: id.parse().expect("an ID"),
    };
    written_long_ago("ui/menu.ui.meta", meta.text().as_bytes());
    let (color, depth) = (png::ColorType::Rgba, png::BitDepth::Eight);
    let png = crate::image::tests::png(color, depth, 2, &[255; 8], None);
    written_long_ago(logo, &png);

    let mut cx = Context::seeded(0).root(root.clone());
    cx.dev.hot_reload = Some(HotReload::start().expect("the thread"));
    let ui = cx.add_ui("menu", id).expect("added by its ID");
    assert!(cx.assets.texture(logo).is_some(), "what it names");
    let listed = cx.ui_list();
    let (asset, path) = (&*listed.uis[0].asset, &*listed.uis[0].path);
    assert_eq!((asset, path), (id, "ui/menu.ui"));
    assert_eq!(
        listed.uis[0].nodes[1].rect[2..],
        [2.0, 1.0],
        "its texture's size"
    );
    ui.set_text_fmt(&mut cx, "score", 120).expect("a text");
    let refused = |set: Result<()>| set.expect_err("refused").code();
    assert_eq!(refused(ui.set_text(&mut cx, "scores", "1")), "ui.no_node");
    assert_eq!(refused(ui.set_text(&mut cx, "logo", "1")), "ui.wrong_kind");
    assert_eq!(ui.focused(&mut cx).as_deref(), Some("play"), "the file's");
    let key = |key| Event::Key {
        key,
        pressed: true,
        repeat: false,
    };
    let up = ui.event(&mut cx, &key(Key::ArrowUp));
    assert_eq!(up, Some(UiEvent::Focused("options".into())));
    let again = cx.add_ui("again", root.join("ui/menu.ui")).expect("added");
    assert_eq!(cx.uis.files(), 1, "the file read once");
    assert_eq!(again.focused(&mut cx).as_deref(), Some("play"), "its own");

    // Frame starts, until `done`.
    let until = |cx: &mut Context, done: &dyn Fn(&mut Context) -> bool, what: &str| {
        let until = std::time::Instant::now() + Duration::from_secs(30);
        while !done(cx) {
            assert!(std::time::Instant::now() < until, "{what}");
            std::thread::sleep(Duration::from_millis(5));
            cx.take_reloaded(false);
        }
    };
    // Whether UI `at` has a node named `name`.
    let named = |cx: &mut Context, at: usize, name: &str| {
        let listed = cx.ui_list();
        let nodes = &listed.uis[at].nodes;
        nodes.iter().any(|node| node.name.as_deref() == Some(name))
    };
    let path = root.join("ui/menu.ui");
    std::fs::write(&path, menu(logo, &["play", "options", "settings"])).expect("an edit");
    let took = |cx: &mut Context| named(cx, 0, "settings");
    until(&mut cx, &took, "the UI takes its nodes");
    assert!(
        named(&mut cx, 1, "settings"),
        "the second UI takes them too"
    );
    // `options`, though the file focuses `play` and `options` has moved.
    assert_eq!(
        ui.focused(&mut cx).as_deref(),
        Some("options"),
        "by its name"
    );
    assert_eq!(again.focused(&mut cx).as_deref(), Some("play"));
    let listed = cx.ui_list();
    let text = |at: usize, name: &str| {
        let nodes = listed.uis[at].nodes.iter();
        let found = nodes
            .clone()
            .find(|node| node.name.as_deref() == Some(name));
        found.and_then(|node| node.text.clone())
    };
    assert_eq!(text(0, "score").as_deref(), Some("120"), "what code set");
    assert_eq!(text(1, "score").as_deref(), Some("0"));

    let _ = logged.codes.try_iter().count();
    let missing = menu("ui/none.png", &["play", "quit"]);
    std::fs::write(&path, missing).expect("an edit");
    let warned = |_: &mut Context| {
        let mut codes = logged.codes.try_iter();
        codes.any(|code| code == "asset.no_texture")
    };
    until(&mut cx, &warned, "an edit naming what doesn't load warns");
    assert!(
        named(&mut cx, 0, "settings") && !named(&mut cx, 0, "quit"),
        "as it was"
    );

    // Renamed with its ID file, as `git mv` renames both.
    let renamed = root.join("ui/main.ui");
    std::fs::rename(&path, &renamed).expect("renamed");
    std::fs::rename(Meta::path(&path), Meta::path(&renamed)).expect("renamed");
    cx.scan_assets(false);
    let listed = cx.ui_list();
    let paths: Vec<&str> = listed.uis.iter().map(|ui| &*ui.path).collect();
    assert_eq!(paths, ["ui/main.ui"; 2]);
    cx.add_ui("third", &renamed).expect("added by its new path");
    cx.add_ui("fourth", id).expect("added by its ID");
    assert_eq!(cx.uis.files(), 1, "the file read once still");
    std::fs::write(&renamed, menu(logo, &["play", "quit"])).expect("an edit");
    let quit = |cx: &mut Context| named(cx, 2, "quit") && named(cx, 0, "quit");
    until(
        &mut cx,
        &quit,
        "the file renamed reads again as it's edited",
    );
    // A new file where it was, without an ID file, is a file of its own.
    std::fs::write(&path, menu(logo, &["play", "new"])).expect("a new file");
    cx.add_ui("fifth", &path).expect("added");
    assert_eq!(cx.uis.files(), 2, "a file of its own");
    assert!(named(&mut cx, 4, "new") && !named(&mut cx, 0, "new"));
}

/// A UI lays out again when a font it writes in bakes again, or a texture an image shows reloads at
/// another size (the image fits it).
#[cfg(feature = "dev")]
#[test]
fn a_ui_lays_out_again_as_its_font_or_texture_reloads() {
    let scratch = crate::dirs::tests::Scratch::new("ui-font-texture");
    let root = scratch.0.join("game");
    std::fs::create_dir_all(root.join("ui")).expect("a directory");
    let fonts = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts/Inter");
    let font_path = root.join("ui/a.ttf");
    std::fs::copy(fonts.join("Inter-Regular.ttf"), &font_path).expect("a font");
    let (color, depth) = (png::ColorType::Rgba, png::BitDepth::Eight);
    let image_of =
        |width| crate::image::tests::png(color, depth, width, &vec![255; 4 * width as usize], None);
    let logo = root.join("ui/logo.png");
    std::fs::write(&logo, image_of(2)).expect("a texture");
    let menu = "(nodes: [Column(children: [Image(name: \"logo\", texture: \"ui/logo.png\"), \
                    Text(name: \"label\", text: \"A WIDE LABEL\", font: \"ui/a.ttf\")])])";
    std::fs::write(root.join("ui/menu.ui"), menu).expect("a UI");
    let mut cx = Context::seeded(0).root(root.clone());
    cx.add_ui("menu", root.join("ui/menu.ui")).expect("added");
    let size = |cx: &mut Context, name: &str| {
        let listed = cx.ui_list();
        let found = listed.uis[0]
            .nodes
            .iter()
            .find(|node| node.name.as_deref() == Some(name));
        let rect = found.expect("a node").rect;
        [rect[2], rect[3]]
    };
    assert_eq!(size(&mut cx, "logo"), [2.0, 1.0]);
    let regular = size(&mut cx, "label");

    std::fs::write(&logo, image_of(4)).expect("an edit");
    let texture = cx.assets.texture("ui/logo.png").expect("loaded");
    let files = package::Files::default();
    let image = crate::image::load_image("logo", &logo, &files).expect("an image");
    let image = crate::image::decode_texture("logo", &logo, image).expect("decoded");
    cx.reload_texture(texture, &logo, image);
    assert_eq!(size(&mut cx, "logo"), [4.0, 1.0], "fit to its new size");

    let font = cx.assets.font("ui/a.ttf").expect("loaded");
    let bold = font::bake("a", &fonts.join("Inter-Bold.ttf")).expect("baked");
    cx.reload_font(font, &font_path, bold);
    let bolder = size(&mut cx, "label");
    assert!(
        bolder[0] > regular[0],
        "bold is wider: {bolder:?} {regular:?}"
    );
}

/// A prefab change the world as edited can't take (it hasn't registered a type the text names)
/// changes neither world: both take it or neither.
#[cfg(feature = "dev")]
#[test]
fn a_prefab_change_one_world_refuses_changes_neither() {
    #[derive(Clone, Debug, PartialEq, crate::Component)]
    struct Lit {
        bright: f32,
    }

    let mut cx = Context::seeded(0);
    let text = "(\n    entities: {\n        \"0000000000000001\": (\n            Name: \
                    (\"lamp\"),\n        ),\n    },\n)\n";
    let prefab = cx.add_assets(|assets| {
        assets.add_prefab(
            "lamp.prefab".into(),
            "lamp.prefab".into(),
            Some(text.into()),
        )
    });
    let root = prefab.spawn(cx.world_mut(), Transform::IDENTITY);
    let root = root.expect("spawned");
    // Held aside as a copy plays, which registers a type the world as edited hasn't.
    cx.dev.edited = Some(Edited {
        world: cx.world.clone(),
        ticks: 0,
        scenes: Vec::new(),
        cached: Vec::new(),
        open: scene_files::Open::default(),
        authors: Authors::default(),
        attached: false,
        played: Vec::new(),
    });
    cx.world_mut().register::<Lit>().expect("describes itself");
    let lit = text.replace(
        "Name: (\"lamp\"),",
        "Lit: (\n                bright: 2.0,\n            ),\n            Name: (\"lit\"),",
    );
    cx.reload_prefab(prefab, Path::new("lamp.prefab"), lit, false);
    let name = cx.world().get::<Name>(root).cloned();
    assert_eq!(name, Some(Name("lamp".to_owned())), "the copy as it was");
    assert_eq!(cx.world().get::<Lit>(root), None);
    let kept = cx.assets.prefab_text(prefab).map(|text| &**text);
    assert_eq!(kept, Some(text), "the prefab as it was");
}

/// A model a run following another can't load stands in as a model of no nodes, so a scene naming
/// it loads, its root spawning nothing. Its image, named, stands in as a texture.
#[test]
fn a_model_that_cant_be_loaded_stands_in_for_a_following_run() {
    let logged = crate::testing::logged();
    let mut cx = Context::seeded(0);
    cx.follows = true;
    let (model, image) = ("assets/none.glb", "assets/none.glb#0");
    cx.load_named([Asset::Model(model), Asset::Texture(image)], true);
    let stand_in = cx.assets.model(model).expect("a stand-in");
    assert_eq!(cx.assets.nodes(stand_in), Some(&Nodes::default()));
    assert!(cx.assets.texture(image).is_some(), "its image's stand-in");
    let stood_in = logged
        .codes
        .try_iter()
        .filter(|code| code == "asset.stand_in");
    assert_eq!(stood_in.count(), 2);
    let text = format!(
        "(\n    entities: {{\n        \"0000000000000100\": (\n            ModelRoot: (\"{model}\"),\n        ),\n    }},\n)\n"
    );
    let roots = cx.world_mut().load_scene(&text).expect("loaded");
    assert_eq!(cx.world().children(roots[0]).expect("alive").count(), 0);
}

/// A scene naming a text's font by ID loads and saves it by that ID; a text naming none saves none.
/// Edited, the font bakes again under its handle and text lays out by the new file.
#[cfg(feature = "dev")]
#[test]
fn a_scene_names_a_texts_font_by_its_id_which_bakes_again_as_it_changes() {
    use gg_api::asset::Meta;

    let scratch = crate::dirs::tests::Scratch::new("font-by-id");
    let root = scratch.0.join("game");
    for dir in ["fonts", "scenes"] {
        std::fs::create_dir_all(root.join(dir)).expect("a directory");
    }
    let path = root.join("fonts/title.ttf");
    std::fs::write(&path, font::INTER).expect("a font");
    // Dated an hour back, so it loads as it is.
    let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(60 * 60);
    let file = std::fs::File::options().write(true).open(&path);
    (file.and_then(|file| file.set_modified(an_hour_ago))).expect("its time");
    let id = "4d5e6f708192a3b4";
    let meta = Meta {
        id: id.parse().expect("an ID"),
    };
    std::fs::write(root.join("fonts/title.ttf.meta"), meta.text()).expect("its ID file");
    let scene = root.join("scenes/s.ron");
    let text = format!(
        "(\n    entities: {{\n        \"0000000000000100\": (\n            Text: (\n\
             \x20               text: \"W\",\n                font: Some(\"{id}\"),\n\
             \x20           ),\n            Transform: (),\n        ),\n\
             \x20       \"0000000000000200\": (\n            Text: (),\n\
             \x20           Transform: (),\n        ),\n    }},\n)\n"
    );
    std::fs::write(&scene, text).expect("written");

    let mut cx = Context::seeded(0).root(root.clone());
    cx.dev.hot_reload = Some(HotReload::start().expect("the thread"));
    let roots = cx.load_scene_file(&scene).expect("loaded");
    let title = cx.assets.font(id).expect("loaded by its ID");
    assert_eq!(cx.assets.font_path(title), "fonts/title.ttf");
    let saved = cx.world().save_scene(&roots).expect("saved");
    assert!(saved.contains(&format!("font: Some(\"{id}\")")), "{saved}");
    assert!(saved.contains("font: None"), "{saved}");

    // How wide its W is.
    let width = |cx: &mut Context| {
        let style = TextStyle {
            font: Some(title),
            size: 1.0,
            color: Vec4::ONE,
            align: [font::Align::Start; 2],
        };
        let mut glyphs = cx.fonts.lay_out(Mat4::IDENTITY, "W", &style, 0);
        glyphs.next().expect("a W").0.x_axis.x
    };
    let regular = width(&mut cx);
    let bold = include_bytes!("../../assets/fonts/Inter/Inter-Bold.ttf");
    std::fs::write(&path, bold).expect("an edit");
    let until = std::time::Instant::now() + Duration::from_secs(30);
    while width(&mut cx) == regular {
        assert!(std::time::Instant::now() < until, "the font bakes again");
        std::thread::sleep(Duration::from_millis(5));
        cx.take_reloaded(false);
    }
    assert!(width(&mut cx) > regular, "bold is wider");
    assert_eq!(cx.assets.font(id), Some(title), "under its handle");
}

/// Text naming by ID a font, texture or model a layer added already loads and notes nothing again,
/// in a run naming them by ID and in one naming them by path (a lockstep joiner, until its host's
/// names come). So both note alike what their layers added, bringing what's opaque in the host's
/// world back in the joiner's; the path-naming run then uses IDs.
#[cfg(feature = "dev")]
#[test]
fn text_naming_by_id_what_a_layer_added_notes_nothing_however_the_run_names_it() {
    use gg_api::asset::Meta;

    let scratch = crate::dirs::tests::Scratch::new("named-loaded");
    let root = scratch.0.join("game");
    for dir in ["fonts", "textures", "models"] {
        std::fs::create_dir_all(root.join(dir)).expect("a directory");
    }
    let png = image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
    let model = gltf::tests::model();
    let files: [(&str, &[u8], &str); 3] = [
        ("fonts/title.ttf", font::INTER, "4d5e6f708192a3b4"),
        ("textures/tile.png", &png, "1a2b3c4d5e6f7081"),
        ("models/m.glb", &model, "8192a3b4c5d6e7f8"),
    ];
    for (path, bytes, id) in files {
        std::fs::write(root.join(path), bytes).expect("an asset");
        let meta = Meta {
            id: id.parse().expect("an ID"),
        };
        std::fs::write(root.join(format!("{path}.meta")), meta.text()).expect("its ID file");
    }
    let [
        (font_path, _, font_id),
        (texture_path, _, texture_id),
        (model_path, _, model_id),
    ] = files;
    let mut notes = Vec::new();
    for by_paths in [false, true] {
        let mut cx = Context::seeded(0).root(root.clone());
        if by_paths {
            cx.name_as([]);
        }
        let font = cx.add_font("title", root.join(font_path)).expect("a font");
        let texture = cx.add_texture("tile", root.join(texture_path));
        let texture = texture.expect("a texture");
        let model = cx.add_model("m", root.join(model_path)).expect("a model");
        let added = cx.assets.added().to_vec();
        let named = [
            Asset::Font(font_id),
            Asset::Texture(texture_id),
            Asset::Model(model_id),
        ];
        cx.load_named(named, true);
        assert_eq!(cx.assets.added(), added, "noted again");
        assert_eq!(cx.assets.font(font_id), Some(font));
        assert_eq!(cx.assets.texture(texture_id), Some(texture));
        assert_eq!(cx.assets.model(model_id), Some(model));
        notes.push(added);
    }
    assert_eq!(notes[0], notes[1]);
}

/// A font a run following another can't load stands in, so a scene naming it loads, its text
/// drawing in the engine's own font.
#[cfg(feature = "dev")]
#[test]
fn a_font_that_cant_be_loaded_stands_in_for_a_following_run() {
    let logged = crate::testing::logged();
    let mut cx = Context::seeded(0);
    cx.follows = true;
    let name = "fonts/none.ttf";
    cx.load_named([Asset::Font(name)], true);
    let stand_in = cx.assets.font(name).expect("a stand-in");
    let stood_in = logged
        .codes
        .try_iter()
        .filter(|code| code == "asset.stand_in");
    assert_eq!(stood_in.count(), 1);
    let text = format!(
        "(\n    entities: {{\n        \"0000000000000100\": (\n            Text: (\n\
             \x20               font: Some(\"{name}\"),\n            ),\n        ),\n    }},\n)\n"
    );
    cx.world_mut().load_scene(&text).expect("loaded");
    let style = TextStyle {
        font: Some(stand_in),
        ..font::Text::default().style()
    };
    let inter = cx.fonts.inter_field();
    let mut glyphs = cx.fonts.lay_out(Mat4::IDENTITY, "Hi", &style, 0);
    assert!(glyphs.all(|(_, glyph, _)| glyph.field == inter));
}

/// A model's file added again gives the first model, adding none of its meshes or images again;
/// each add notes it.
#[test]
fn a_model_added_twice_loads_once() {
    let scratch = crate::dirs::tests::Scratch::new("model-once");
    std::fs::create_dir_all(&scratch.0).expect("a directory");
    let path = scratch.0.join("m.glb");
    std::fs::write(&path, gltf::tests::model()).expect("a model");
    let mut cx = Context::seeded(0);
    cx.add_model("m", &path).expect("a model");
    let counts = (cx.meshes.count(), cx.textures.count());
    assert!(counts.0 > 0 && counts.1 > 1, "{counts:?}");
    cx.add_model("again", &path).expect("it again");
    assert_eq!((cx.meshes.count(), cx.textures.count()), counts);
    let models = cx
        .assets
        .added()
        .iter()
        .filter(|note| note.starts_with("model "));
    assert_eq!(models.count(), 2);
}

/// Each texture, model, scene file, prefab, font, UI, sound and music loaded logs how long it took,
/// including a texture's decode, a model's images and a font's bake. A file added again, or a scene
/// that fails to load (its error says why), logs nothing.
#[test]
fn loads_log_how_long_they_took() {
    let logged = crate::testing::logged();
    let scratch = crate::dirs::tests::Scratch::new("loads-logged");
    let root = scratch.0.clone();
    for dir in ["assets", "scenes"] {
        std::fs::create_dir_all(root.join(dir)).expect("a directory");
    }
    let png = image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
    let write = |name: &str, bytes: &[u8]| std::fs::write(root.join(name), bytes);
    write("assets/stone.png", &png).expect("written");
    write("assets/plain.png", &png).expect("written");
    write("assets/m.glb", &gltf::tests::model()).expect("written");
    let inter = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts/Inter");
    let font = std::fs::read(inter.join("Inter-Regular.ttf")).expect("the engine's font");
    write("assets/inter.ttf", &font).expect("written");
    write("assets/menu.ui", b"(nodes: [Button(name: \"play\")])").expect("written");
    let wav = audio::tests::wav(&[0; 480]);
    write("assets/chime.wav", &wav).expect("written");
    write("assets/theme.wav", &wav).expect("written");
    // A scene of a sprite of the texture named by `id`, written to `file`.
    let scene = |file: &str, id: u64, name: &str| -> PathBuf {
        let sprite = format!("(Sprite: (texture: Some(\"{name}\")))");
        let text = format!("(entities: {{\"{id:016x}\": {sprite}}})");
        let path = root.join(file);
        std::fs::write(&path, text).expect("written");
        path
    };
    let mut cx = Context::seeded(0).root(root.clone());
    let _ = logged.codes.try_iter().count();
    cx.add_texture("stone", root.join("assets/stone.png"))
        .expect("a texture");
    cx.add_model("m", root.join("assets/m.glb"))
        .expect("a model");
    cx.add_texture("again", root.join("assets/stone.png"))
        .expect("it again");
    cx.add_model("again", root.join("assets/m.glb"))
        .expect("it again");
    cx.load_scene_file(scene("scenes/s.ron", 0x1000, "assets/plain.png"))
        .expect("loaded");
    let error = cx.load_scene_file(scene("scenes/s.ron", 0x2000, "assets/none.png"));
    assert_eq!(error.expect_err("no file").code(), "asset.no_texture");
    // Of a texture loaded already, which it loads nothing of.
    let prefab = scene("assets/p.prefab", 0x3000, "assets/plain.png");
    cx.add_prefab("p", &prefab).expect("a prefab");
    cx.add_prefab("again", &prefab).expect("it again");
    cx.add_font("inter", root.join("assets/inter.ttf"))
        .expect("a font");
    cx.add_ui("menu", root.join("assets/menu.ui"))
        .expect("a UI");
    cx.add_sound("chime", root.join("assets/chime.wav"))
        .expect("a sound");
    cx.add_music("theme", root.join("assets/theme.wav"))
        .expect("music");
    let codes: Vec<String> = (logged.codes.try_iter())
        .filter(|code| code.starts_with("asset.") || code.starts_with("scene."))
        .collect();
    // A test's context decodes each image as added, so each logs before its add does.
    let decoded = "asset.texture_decoded";
    assert_eq!(
        codes,
        [
            decoded,
            "asset.texture_loaded",
            decoded,
            decoded,
            "asset.model_loaded",
            decoded,
            "asset.texture_loaded",
            "scene.loaded",
            "asset.prefab_loaded",
            "asset.font_baked",
            "asset.font_loaded",
            "asset.ui_loaded",
            "asset.sound_loaded",
            "asset.music_loaded",
        ]
    );
}

fn decoded(cx: &mut Context) {
    let view = Viewport {
        origin: [0, 0],
        size: [64, 64],
    };
    let until = std::time::Instant::now() + Duration::from_secs(30);
    while cx.streaming().decoding > 0 {
        assert!(std::time::Instant::now() < until, "the images decode");
        std::thread::sleep(Duration::from_millis(1));
        cx.extract(&mut render_thread::Extracted::default(), view);
    }
}

/// A texture whose header reads but whose pixels don't decode (a PNG cut short) loads like any
/// texture and warns when its pixels are found bad: off the main thread in a run's context, at once
/// in a test's or replay's, so both add it alike, as with a model whose image is such a file.
#[test]
fn a_texture_that_reads_but_doesnt_decode_loads_and_warns() {
    let logged = crate::testing::logged();
    let scratch = crate::dirs::tests::Scratch::new("cut-texture");
    std::fs::create_dir_all(&scratch.0).expect("a directory");
    let mut png = image::tests::png(
        png::ColorType::Rgba,
        png::BitDepth::Eight,
        4,
        &[9; 16],
        None,
    );
    png.truncate(png.len() - 20);
    let path = scratch.0.join("cut.png");
    std::fs::write(&path, png).expect("a PNG, cut");
    #[cfg(feature = "dev")]
    let model = {
        let gltf = gltf::tests::triangle_gltf(|gltf| {
            gltf["images"][0] = serde_json::json!({"uri": "cut.png"});
        });
        let model = scratch.0.join("m.gltf");
        std::fs::write(&model, gltf).expect("a model");
        model
    };
    for threaded in [false, true] {
        let mut cx = Context::seeded(0).root(scratch.0.clone());
        if threaded {
            cx.loader = Loader::threaded();
        }
        let _ = logged.codes.try_iter().count();
        let cut = cx.add_texture("cut", &path);
        let cut = cut.expect("loaded, as its header reads");
        assert_eq!(cx.assets.path(cut), Some("cut.png"));
        #[cfg(feature = "dev")]
        (cx.add_model("m", &model)).expect("loaded, as its image's header reads");
        // The texture's image, and, with `dev`, the model's.
        let warned = if cfg!(feature = "dev") { 2 } else { 1 };
        assert_eq!(cx.streaming().decoding, if threaded { warned } else { 0 });
        decoded(&mut cx);
        let codes = logged.codes.try_iter();
        let bad = codes.filter(|code| code == "engine.bad_texture");
        assert_eq!(bad.count(), warned as usize, "threaded: {threaded}");
    }
}

/// A removed texture keeps its name, which finds it still, so the world's hash reads nothing of the
/// removal. Where text names it, it loads anew under its own handle. The removal is noted once, by
/// index and path; the engine's own textures aren't removed.
#[test]
fn a_removed_texture_keeps_its_name_and_loads_anew_where_named() {
    let scratch = crate::dirs::tests::Scratch::new("removed-texture");
    let root = scratch.0.clone();
    std::fs::create_dir_all(root.join("assets")).expect("a directory");
    let png = image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
    let path = root.join("assets/stone.png");
    std::fs::write(&path, png).expect("a PNG");
    let named = "assets/stone.png";
    let mut cx = Context::seeded(0).root(root);
    let stone = cx.add_texture("stone", &path).expect("a texture");
    let sprite = Sprite {
        texture: Some(stone),
        ..Sprite::default()
    };
    cx.world_mut().spawn((Transform::IDENTITY, sprite));
    let hash = cx.world().hash();
    cx.remove_texture(stone);
    cx.remove_texture(stone);
    assert_eq!(cx.world().hash(), hash, "the world reads nothing of it");
    assert_eq!(
        cx.assets.texture(named),
        Some(stone),
        "its name finds it still"
    );
    assert!(
        !cx.assets.has(&Asset::Texture(named)),
        "but nothing loaded has it"
    );
    let note = format!("removed texture {} {named}", stone.index());
    let notes = cx.assets.added().iter().filter(|added| ***added == *note);
    assert_eq!(notes.count(), 1, "noted once");

    cx.load_named([Asset::Texture(named)], false);
    let anew = cx.assets.texture(named).expect("loaded anew");
    assert!(anew.index() > stone.index(), "under a handle of its own");
    assert_eq!(cx.assets.name(anew), Some(named));
    let added = cx.add_texture("again", &path).expect("its file again");
    assert_eq!(added, anew, "loaded once, anew");
    assert_eq!(cx.world().hash(), hash);
    #[cfg(feature = "dev")]
    {
        assert!(cx.has_added(named));
        cx.remove_texture(anew);
        assert!(!cx.has_added(named));
    }
    let white = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cx.remove_texture(Texture::WHITE);
    }));
    assert!(white.is_err(), "the engine's white texture");
}

fn drawn(world: &World, entity: Entity) -> Vec<Option<MeshDraw>> {
    let mut drawn = vec![world.get::<MeshDraw>(entity).copied()];
    for child in world.children(entity).expect("alive") {
        drawn.extend(self::drawn(world, child));
    }
    drawn
}

/// A removed model takes its meshes and images with it (noted once) and keeps its nodes, so later
/// roots spawn what earlier ones did. Its file loads anew as another model; its meshes and images
/// aren't removed alone.
#[test]
fn a_removed_model_takes_its_meshes_and_images_and_spawns_alike() {
    let scratch = crate::dirs::tests::Scratch::new("removed-model");
    std::fs::create_dir_all(&scratch.0).expect("a directory");
    let path = scratch.0.join("m.glb");
    std::fs::write(&path, gltf::tests::model()).expect("a model");
    let mut cx = Context::seeded(0).root(scratch.0.clone());
    let model = cx.add_model("m", &path).expect("a model");
    let made = cx.loaded.made[model.index()].clone();
    assert!(!made.meshes.is_empty() && !made.images.is_empty());
    let before = model.spawn(cx.world_mut(), Transform::IDENTITY);
    cx.remove_model(model);
    cx.remove_model(model);
    let after = model.spawn(cx.world_mut(), Transform::IDENTITY);
    let world = cx.world();
    assert_eq!(drawn(world, after), drawn(world, before), "spawned alike");
    assert!(
        made.meshes.iter().all(|&mesh| !cx.meshes.remove(mesh)),
        "its meshes gone"
    );
    assert!(
        made.images
            .iter()
            .all(|&(image, _)| cx.assets.removed(image))
    );
    assert_eq!(
        cx.assets.model("m.glb"),
        Some(model),
        "its name finds it still"
    );
    assert!(!cx.assets.has(&Asset::Model("m.glb")));
    let note = format!("removed model {} m.glb", model.index());
    let notes = cx.assets.added().iter().filter(|added| ***added == *note);
    assert_eq!(notes.count(), 1, "noted once");

    let anew = cx.add_model("m", &path).expect("its file again");
    assert_ne!(anew, model);
    assert_eq!(cx.assets.model("m.glb"), Some(anew));
    let made = cx.loaded.made[anew.index()].clone();
    let (mesh, image) = (made.meshes[0], made.images[0].0);
    let alone = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cx.remove_mesh(mesh);
    }));
    assert!(alone.is_err(), "a model's mesh alone");
    let alone = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cx.remove_texture(image);
    }));
    assert!(alone.is_err(), "a model's image alone");
}

/// A removed texture is watched no more: an edit of its file reads nothing of it, unlike another
/// texture's.
#[cfg(feature = "dev")]
#[test]
fn a_removed_texture_is_watched_no_more() {
    let scratch = crate::dirs::tests::Scratch::new("removed-unwatched");
    std::fs::create_dir_all(&scratch.0).expect("a directory");
    let png = |width: u32| {
        let pixels = vec![9; width as usize * 4];
        image::tests::png(
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            width,
            &pixels,
            None,
        )
    };
    let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(60 * 60);
    for name in ["gone.png", "kept.png"] {
        let path = scratch.0.join(name);
        std::fs::write(&path, png(1)).expect("a PNG");
        // Dated an hour back, so that each loads as it is.
        let file = std::fs::File::options().write(true).open(path);
        (file.and_then(|file| file.set_modified(an_hour_ago))).expect("its time");
    }
    let mut cx = Context::seeded(0).root(scratch.0.clone());
    cx.dev.hot_reload = Some(HotReload::start().expect("the thread"));
    let gone = cx.add_texture("gone", scratch.0.join("gone.png"));
    let gone = gone.expect("a texture");
    let kept = cx.add_texture("kept", scratch.0.join("kept.png"));
    let kept = kept.expect("a texture");
    cx.remove_texture(gone);
    for name in ["gone.png", "kept.png"] {
        std::fs::write(scratch.0.join(name), png(2)).expect("an edit");
    }
    let mut read = Vec::new();
    let until = std::time::Instant::now() + Duration::from_secs(30);
    while !read.contains(&kept) {
        assert!(
            std::time::Instant::now() < until,
            "the kept texture reads again"
        );
        std::thread::sleep(Duration::from_millis(5));
        let hot_reload = cx.dev.hot_reload.as_ref().expect("started");
        while let Some(reloaded) = hot_reload.take() {
            if let Reloaded::Texture { texture, .. } = reloaded {
                read.push(texture);
            }
        }
    }
    assert!(!read.contains(&gone), "the removed one read again");
}

/// A sound's file added again by another form of its path gives the first sound, decoding nothing.
/// A file that isn't a sound or isn't there is refused, naming the sound and file.
#[test]
fn a_sound_decodes_once_however_its_path_is_written() {
    let scratch = crate::dirs::tests::Scratch::new("sound-once");
    let root = scratch.0.clone();
    std::fs::create_dir_all(root.join("sounds")).expect("a directory");
    std::fs::write(root.join("sounds/chime.wav"), audio::tests::wav(&[0; 480])).expect("a WAV");
    std::fs::write(root.join("sounds/noise.wav"), b"RIFF, but no more").expect("a file");
    let mut cx = Context::seeded(0).root(root.clone());
    let chime = cx.add_sound("chime", root.join("sounds/chime.wav"));
    let chime = chime.expect("a sound");
    let dotted = root.join("./sounds/../sounds/chime.wav");
    assert_eq!(cx.add_sound("again", &dotted).expect("it again"), chime);
    assert_eq!(cx.audio.frames(chime), 480);
    for (name, path) in [("noise", "sounds/noise.wav"), ("none", "sounds/none.wav")] {
        let path = root.join(path);
        let error = cx.add_sound(name, &path).expect_err("no sound");
        assert_eq!(error.code(), "engine.bad_sound");
        let message = error.to_string();
        let named = message.contains(&format!("{name:?}"));
        assert!(
            named && message.contains(&path.display().to_string()),
            "{message}"
        );
    }
}

/// Queues a sound at each tick, at its entity, as a game's tick does.
struct Chime {
    sound: Sound,
    on: Entity,
}

impl Layer for Chime {
    fn tick(&mut self, world: &mut World, _: Tick) {
        world.play_sound(self.sound, Place::On(self.on));
    }
}

/// The sounds a tick queues (every layer's) play once it has run, so the world holds none between
/// ticks, whether or not the run plays sound.
#[test]
fn the_sounds_a_tick_queues_play_once_it_has_run() {
    let scratch = crate::dirs::tests::Scratch::new("sound-tick");
    std::fs::create_dir_all(&scratch.0).expect("a directory");
    let path = scratch.0.join("chime.wav");
    std::fs::write(&path, audio::tests::wav(&[0; 480])).expect("a WAV");
    let mut cx = Context::seeded(0);
    let sound = cx.add_sound("chime", &path).expect("a sound");
    let on = cx.world_mut().spawn(Transform::IDENTITY);
    let mut stack = Stack::default();
    stack.push_layer(Chime { sound, on });
    stack.push_overlay(Chime { sound, on });
    for number in 0..2 {
        stack.tick(&mut cx, Tick { number });
        assert_eq!(cx.world().sounds_queued().count(), 0);
    }
    cx.world_mut().play_sound(sound, Place::Nowhere);
    let queued: Vec<_> = cx.world().sounds_queued().collect();
    assert_eq!(queued, [(sound, Place::Nowhere)]);
}

/// A sound's file edited decodes again under its handle; its plays take what the file holds.
#[cfg(feature = "dev")]
#[test]
fn a_sound_decodes_again_as_its_file_changes() {
    let scratch = crate::dirs::tests::Scratch::new("sound-reload");
    std::fs::create_dir_all(&scratch.0).expect("a directory");
    let path = scratch.0.join("chime.wav");
    std::fs::write(&path, audio::tests::wav(&[0; 480])).expect("a WAV");
    // Dated an hour back, so that it loads as it is.
    let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(60 * 60);
    let file = std::fs::File::options().write(true).open(&path);
    (file.and_then(|file| file.set_modified(an_hour_ago))).expect("its time");
    let mut cx = Context::seeded(0);
    cx.dev.hot_reload = Some(HotReload::start().expect("the thread"));
    let sound = cx.add_sound("chime", &path).expect("a sound");
    std::fs::write(&path, audio::tests::wav(&[0; 960])).expect("an edit");
    let until = std::time::Instant::now() + Duration::from_secs(30);
    while cx.audio.frames(sound) == 480 {
        assert!(std::time::Instant::now() < until, "the sound decodes again");
        std::thread::sleep(Duration::from_millis(5));
        cx.take_reloaded(false);
    }
    assert_eq!(cx.audio.frames(sound), 960);
}

/// Music's file added again by another form of its path gives the first music, reading nothing. A
/// file that isn't music or isn't there is refused, naming the music and file.
#[test]
fn music_reads_once_however_its_path_is_written() {
    let scratch = crate::dirs::tests::Scratch::new("music-once");
    let root = scratch.0.clone();
    std::fs::create_dir_all(root.join("music")).expect("a directory");
    std::fs::write(root.join("music/theme.wav"), audio::tests::wav(&[0; 480])).expect("a WAV");
    std::fs::write(root.join("music/noise.ogg"), b"OggS, but no more").expect("a file");
    let mut cx = Context::seeded(0).root(root.clone());
    let theme = cx.add_music("theme", root.join("music/theme.wav"));
    let theme = theme.expect("music");
    let dotted = root.join("./music/../music/theme.wav");
    assert_eq!(cx.add_music("again", &dotted).expect("it again"), theme);
    assert_eq!(cx.audio.music_frames(theme), 480);
    for (name, path) in [("noise", "music/noise.ogg"), ("none", "music/none.ogg")] {
        let path = root.join(path);
        let error = cx.add_music(name, &path).expect_err("no music");
        assert_eq!(error.code(), "engine.bad_music");
        let message = error.to_string();
        let named = message.contains(&format!("{name:?}"));
        assert!(
            named && message.contains(&path.display().to_string()),
            "{message}"
        );
    }
}

/// Music's file edited reads again, under its handle, as it plays; its plays take what the file
/// holds.
#[cfg(feature = "dev")]
#[test]
fn music_reads_again_as_its_file_changes() {
    let scratch = crate::dirs::tests::Scratch::new("music-reload");
    std::fs::create_dir_all(&scratch.0).expect("a directory");
    let path = scratch.0.join("theme.wav");
    std::fs::write(&path, audio::tests::wav(&[0; 480])).expect("a WAV");
    // Dated an hour back, so that it loads as it is.
    let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(60 * 60);
    let file = std::fs::File::options().write(true).open(&path);
    (file.and_then(|file| file.set_modified(an_hour_ago))).expect("its time");
    let mut cx = Context::seeded(0);
    cx.dev.hot_reload = Some(HotReload::start().expect("the thread"));
    let theme = cx.add_music("theme", &path).expect("music");
    cx.play_music(Some(theme), Duration::ZERO);
    std::fs::write(&path, audio::tests::wav(&[0; 960])).expect("an edit");
    let until = std::time::Instant::now() + Duration::from_secs(30);
    while cx.audio.music_frames(theme) == 480 {
        assert!(std::time::Instant::now() < until, "the music reads again");
        std::thread::sleep(Duration::from_millis(5));
        cx.take_reloaded(false);
    }
    assert_eq!(cx.audio.music_frames(theme), 960);
    assert_eq!(cx.music_playing(), Some(theme));
}

/// A shader's file added again with the same vertex and parameter types and blend gives the first
/// shader (also by another path form in the root), compiled once. Another blend, or other parameter
/// types of the same size, makes another shader.
#[test]
fn a_shader_added_twice_loads_once_for_its_types_and_blend() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = root.join("tests/shaders/flat_color.slang");
    let dotted = root.join("tests/./shaders/flat_color.slang");
    let mut cx = Context::seeded(0).root(root);
    let added = |cx: &mut Context, path: &Path, blend| {
        let shader = cx.add_shader::<[f32; 3], Vec4>("flat", path, blend);
        format!("{:?}", shader.expect("the shader compiles"))
    };
    let flat = added(&mut cx, &path, Blend::Opaque);
    let count = cx.shaders.count();
    assert_eq!(added(&mut cx, &path, Blend::Opaque), flat);
    assert_eq!(added(&mut cx, &dotted, Blend::Opaque), flat);
    assert_eq!((cx.shaders.sources().len(), cx.shaders.count()), (1, count));
    assert_ne!(added(&mut cx, &path, Blend::Alpha), flat);
    assert_eq!(cx.shaders.sources().len(), 2);
    let array = cx.add_shader::<[f32; 3], [f32; 4]>("flat", &path, Blend::Opaque);
    array.expect("the shader compiles");
    assert_eq!(cx.shaders.sources().len(), 3);
    let shaders = cx
        .assets
        .added()
        .iter()
        .filter(|note| note.starts_with("shader "));
    assert_eq!(shaders.count(), 5);
}

/// Two files a layer's paths name alike are two files, each loaded: one outside the root, by a path
/// relative to the working directory (gg-engine's, where cargo runs tests), and one inside it.
#[test]
fn files_named_alike_load_apart() {
    let scratch = crate::dirs::tests::Scratch::new("named-alike");
    let shaders = scratch.0.join("tests").join("shaders");
    std::fs::create_dir_all(&shaders).expect("a directory");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shaders/flat_color.slang");
    std::fs::copy(source, shaders.join("flat_color.slang")).expect("copied");
    let mut cx = Context::seeded(0).root(scratch.0.clone());
    let relative = Path::new("tests/shaders/flat_color.slang");
    for path in [relative, &shaders.join("flat_color.slang")] {
        let shader = cx.add_shader::<[f32; 3], Vec4>("flat", path, Blend::Opaque);
        shader.expect("the shader compiles");
    }
    assert_eq!(cx.shaders.sources().len(), 2);
}

/// What a layer outlines is the frame's it's set in alone, as `outlined` says until the frame takes
/// it; a later call replaces an earlier one's.
#[test]
fn an_outline_is_its_frames_alone() {
    let viewport = Viewport {
        origin: [0, 0],
        size: [640, 360],
    };
    let mut frame = render_thread::Extracted::default();
    let mut cx = Context::seeded(1);
    let [first, later]: [EntityId; 2] =
        ["0123456789abcdef", "fedcba9876543210"].map(|id| id.parse().expect("an ID"));
    let orange = Vec4::new(1.0, 0.35, 0.02, 1.0);
    cx.outline(first, Vec4::ONE, 2.0);
    cx.outline(later, orange, 3.0);
    assert_eq!(cx.outlined(), Some((later, orange, 3.0)));
    cx.extract(&mut frame, viewport);
    assert_eq!(frame.outline, Some((orange, 3.0)));
    assert_eq!(cx.outlined(), None);
    cx.extract(&mut frame, viewport);
    assert_eq!(frame.outline, None, "the next frame outlines nothing");
}

/// A frame hands the renderer the sorted IDs of the outlined entity and everything under it: a
/// parent's children and theirs, not siblings or its own parent; for a model's root, which draws
/// nothing, its nodes. A gone entity gives none; the next frame reuses the IDs' memory.
#[test]
fn an_outline_covers_the_entity_and_every_entity_under_it() {
    let viewport = Viewport {
        origin: [0, 0],
        size: [640, 360],
    };
    let mut frame = render_thread::Extracted::default();
    let mut cx = Context::seeded(1);
    let (model, _) = crate::model::tests::added(&mut cx, "m", crate::model::tests::imported());
    let world = cx.world_mut();
    let root = model.spawn(world, Transform::IDENTITY);
    let [above, parent, child, grandchild, sibling] =
        [(); 5].map(|()| world.spawn(Transform::IDENTITY));
    for (entity, under) in [(parent, above), (child, parent), (grandchild, child)] {
        world.set_parent(entity, Some(under)).expect("alive");
    }
    world.set_parent(sibling, Some(above)).expect("alive");
    let id = |world: &World, entity| world.id(entity).expect("alive");
    let sorted = |mut ids: Vec<u64>| {
        ids.sort_unstable();
        ids
    };
    let outline = |cx: &mut Context, frame: &mut render_thread::Extracted, id| {
        cx.outline(id, Vec4::ONE, 2.0);
        cx.extract(frame, viewport);
        frame.outline_ids.clone()
    };

    let want = [parent, child, grandchild].map(|entity| id(cx.world(), entity).get());
    let parent_id = id(cx.world(), parent);
    let got = outline(&mut cx, &mut frame, parent_id);
    assert_eq!(got, sorted(want.to_vec()));

    let world = cx.world();
    let nodes = world.query::<(Entity, &ModelNode)>();
    let nodes = nodes.filter(|&(node, _)| world.node_root(node) == root);
    let mut want: Vec<u64> = nodes.map(|(node, _)| id(world, node).get()).collect();
    assert!(want.len() > 1, "a model with nodes");
    let root_id = id(world, root);
    want.push(root_id.get());
    let got = outline(&mut cx, &mut frame, root_id);
    assert_eq!(got, sorted(want));

    // Gone: outlined, but with no IDs to match.
    let gone = id(cx.world(), grandchild);
    cx.world_mut().despawn(grandchild).expect("alive");
    assert_eq!(outline(&mut cx, &mut frame, gone), Vec::<u64>::new());
    assert!(frame.outline.is_some());
}

#[test]
fn layers_know_where_the_view_was_handed_off_while_it_had_an_area() {
    let mut cx = Context::new(None);
    assert_eq!(cx.view(), None);
    let mut frame = render_thread::Extracted::default();
    let shown = egui::Rect::from_min_max(egui::pos2(10.0, 40.0), egui::pos2(410.0, 340.0));
    cx.extract(&mut frame, Viewport::from_points(shown, 1.5));
    let view = Viewport {
        origin: [15, 60],
        size: [600, 450],
    };
    assert_eq!(cx.view(), Some(view));
    // As while a tab shows over a dockspace's view, whose dock then has it nowhere.
    cx.extract(&mut frame, Viewport::from_points(egui::Rect::NOTHING, 1.5));
    assert_eq!(cx.view(), None);
}

/// Records each call of its methods, and holds a queued command.
#[cfg(feature = "dev")]
#[derive(Default)]
struct Keeper {
    calls: Vec<&'static str>,
    queued: Vec<u32>,
}

#[cfg(feature = "dev")]
impl Layer for Keeper {
    fn load(&mut self, _: &mut Context) -> Result<()> {
        self.calls.push("load");
        Ok(())
    }
    fn attach(&mut self, _: &mut Context) {
        self.calls.push("attach");
    }
    fn restored(&mut self, _: &mut Context) {
        self.calls.push("restored");
    }
    fn swapped(&mut self, _: &mut Context) {
        self.calls.push("swapped");
    }
    fn detach(&mut self, _: &mut Context) {
        self.calls.push("detach");
    }
    fn tick(&mut self, _: &mut World, _: Tick) {
        self.calls.push("tick");
    }
    fn commands(&mut self) -> Option<&mut dyn Commands> {
        self.calls.push("commands");
        Some(&mut self.queued)
    }
    fn update(&mut self, _: &mut Context, _: Duration) {
        self.calls.push("update");
    }
    fn event(&mut self, _: &mut Context, _: &Event<'_>) -> ControlFlow<()> {
        self.calls.push("event");
        ControlFlow::Break(())
    }
}

/// A development build's stack holds each layer in a `Hot`, which calls the layer's methods and
/// returns what they return. `layers_draw_ui_unless_theirs_is_the_default` checks `ui` through it.
#[cfg(feature = "dev")]
#[test]
fn a_hot_layer_calls_the_layers_own_methods() {
    let mut hot = Hot(Keeper {
        queued: vec![7],
        ..Keeper::default()
    });
    let (mut cx, mut world) = (Context::new(None), World::default());
    hot.load(&mut cx).expect("loaded");
    hot.attach(&mut cx);
    hot.restored(&mut cx);
    hot.swapped(&mut cx);
    hot.tick(&mut world, Tick { number: 0 });
    let queued = hot.commands().map(|commands| commands.len());
    hot.update(&mut cx, Duration::ZERO);
    let handled = hot.event(&mut cx, &Event::CursorEntered);
    hot.detach(&mut cx);
    assert_eq!((queued, handled), (Some(1), ControlFlow::Break(())));
    let calls = [
        "load", "attach", "restored", "swapped", "tick", "commands", "update", "event", "detach",
    ];
    assert_eq!(hot.0.calls, calls);
}

/// Holds a queue of commands, and counts calls of its `restored`.
#[cfg(feature = "dev")]
struct Restorable {
    queued: Vec<u32>,
    restored: Rc<std::cell::Cell<u32>>,
}

#[cfg(feature = "dev")]
impl Layer for Restorable {
    fn restored(&mut self, _: &mut Context) {
        self.restored.set(self.restored.get() + 1);
    }
    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.queued)
    }
}

/// `play` sets the world as edited aside (ticks, scene files, authors' edits) while a copy plays
/// with edits of its own. `stop` restores that world with them, empties each layer's queue, has
/// each pick up from the world once, and returns its ticks. With none held, `stop` changes nothing;
/// a recorded run refuses it.
#[cfg(feature = "dev")]
#[test]
fn stop_brings_back_the_world_as_edited_that_play_held() {
    let restored = Rc::default();
    let mut stack = Stack::default();
    stack.push_layer(Restorable {
        queued: Vec::new(),
        restored: Rc::clone(&restored),
    });
    let mut cx = Context::seeded(1);
    let hero = cx.world_mut().spawn(Transform::IDENTITY);
    let file = SceneFile::new(
        "scenes/a.ron".into(),
        "scenes/a.ron".to_owned(),
        "(entities: {})\n".to_owned(),
    );
    cx.scenes.loaded(file.clone());
    stack.attach(&mut cx).expect("loaded");
    let id = cx.world().id(hero).expect("alive");
    let set = |x: f32| Edit::Apply {
        author: "test".to_owned(),
        operations: vec![crate::ecs::Operation::Set {
            entity: id,
            component: "Transform".into(),
            path: "translation.x".to_owned(),
            value: Value::F64(f64::from(x)),
        }],
    };
    let queued = |stack: &mut Stack| stack.commands_of(0).expect("a queue").len();
    cx.apply_edit(set(1.0)).expect("set");
    let queue = stack.commands_of(0).expect("a queue");
    queue.push(&Value::UInt(7)).expect("queued");
    assert_eq!(stack.stop(&mut cx), None, "nothing held");
    assert_eq!((queued(&mut stack), restored.get()), (1, 0));
    let (edited, hash) = (cx.world().serial(), cx.world().hash());
    stack.play(&mut cx, 40);
    let held = cx.dev.edited.as_ref().expect("the world as edited, held");
    assert_eq!((held.world.serial(), held.ticks), (edited, 40));
    assert_eq!(held.scenes, std::slice::from_ref(&file));
    assert_ne!(cx.world().serial(), edited, "a copy plays");
    assert_eq!(cx.authors.logs(cx.world()).count(), 0);
    cx.apply_edit(set(2.0)).expect("set in the copy");
    cx.scenes.files.clear();
    assert_eq!(stack.stop(&mut cx), Some(40));
    assert!(cx.dev.edited.is_none());
    assert_eq!((cx.world().serial(), cx.world().hash()), (edited, hash));
    assert_eq!(cx.scenes.files, [file]);
    let logs = cx.authors.logs(cx.world());
    let edits: Vec<usize> = logs.map(|(_, log)| log.edits().count()).collect();
    assert_eq!(edits, [1], "the edit made before `play` alone");
    assert_eq!((queued(&mut stack), restored.get()), (0, 1));
    assert_eq!(stack.refuses_stop(), None);
    let dir = crate::dirs::tests::Scratch::new("stop-recorded");
    std::fs::create_dir_all(&dir.0).expect("a directory");
    let recorder = Recorder::create(&dir.0.join("run.recording"), "Tested");
    stack.record(recorder.expect("created"));
    assert!(stack.refuses_stop().is_some());
}

/// Without a window, as in headless runs and tests, a file dialog shows nothing, so it picks
/// nothing and pauses no frames.
#[cfg(feature = "dev")]
#[test]
fn a_file_dialog_without_a_window_picks_nothing() {
    let mut cx = Context::seeded(0);
    let dialog = crate::dialog::FileDialog {
        title: "Save the scene as",
        picks: crate::dialog::Picks::Save,
        filter: Some(("Scenes", &["ron"])),
        directory: Some(Path::new("scenes")),
        file_name: Some("grid.ron"),
    };
    assert_eq!(cx.pick_file(&dialog), None);
    assert!(!cx.paused);
}
