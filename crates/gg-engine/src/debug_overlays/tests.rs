//! The overlays' state, kept file and flag, the tool API's checks, and the layer's key and clicks.

use gg_api::{OverlayGroup, Overlays, Pass, Shown};
use gg_renderer::Channel;

use super::DebugOverlays;
use super::state::{self, Overlay, State, check, same};
use crate::layer::Stack;
use crate::{Context, Event, Key, Layer, MouseButton};

/// The names of the overlays `state` reports on.
fn on(state: &State) -> Vec<String> {
    let report = state.report();
    let on = report.overlays.into_iter().filter(|overlay| overlay.on);
    on.map(|overlay| overlay.name).collect()
}

#[test]
fn names_match_in_any_case_with_an_underscore_or_dash_for_a_space() {
    assert!(same("Enemy aim", "enemy_aim"));
    assert!(same("FPS", "fps"));
    assert!(same("Depth of-field", "depth_of field"));
    assert!(!same("fps", "fpss"));
    assert_eq!(Overlay::named("COLLIDERS"), Some(Overlay::Colliders));
    assert_eq!(Overlay::named("nothing"), None);
    // Every engine overlay's name is its own.
    for overlay in Overlay::ALL {
        assert_eq!(Overlay::named(overlay.name()), Some(overlay));
    }
}

#[test]
fn a_games_overlay_lists_once_as_first_asked_and_takes_a_name_waiting_for_it() {
    let mut state = State::default();
    state.turn("enemy_aim", true);
    assert_eq!(state.report().waiting, ["enemy_aim"]);
    assert!(state.asked("Enemy aim"));
    assert!(!state.asked("Paths"));
    assert!(state.asked("enemy aim"));
    let game = [("Enemy aim".to_owned(), true), ("Paths".to_owned(), false)];
    assert_eq!(state.game(), game);
    assert!(state.report().waiting.is_empty());
    // An engine overlay's name gives its state and lists nothing.
    state.set(Overlay::Colliders, true);
    assert!(state.asked("colliders"));
    assert_eq!(state.game().len(), 2);
    // Turned off by name, a name waiting goes.
    state.turn("Later", true);
    state.turn("later", false);
    assert!(state.report().waiting.is_empty());
}

#[test]
fn the_api_sets_overlays_and_lists_the_engines_only_with_the_layer() {
    let mut state = State::default();
    let asked = Overlays {
        on: vec!["fps".into(), "bounds".into(), "Later".into()],
        channel: Some(Some(Shown::Normal)),
        skip: Some(vec![Pass::Fog, Pass::AmbientOcclusion]),
        bar: Some(true),
        ..Overlays::default()
    };
    assert_eq!(check(&asked), None);
    state.apply(&asked);
    let report = state.report();
    assert!(report.overlays.is_empty(), "no layer, no engine overlays");
    assert_eq!(report.waiting, ["Later"]);
    assert_eq!(report.channel, Some(Shown::Normal));
    assert_eq!(report.skip, [Pass::AmbientOcclusion, Pass::Fog]);
    assert!(report.bar);
    state.engine = true;
    let report = state.report();
    assert_eq!(report.overlays.len(), Overlay::ALL.len());
    assert_eq!(report.overlays[0].group, OverlayGroup::Stats);
    assert_eq!(on(&state), ["FPS", "Bounds"]);
    // `only` turns the rest off first, the Render menu's choices kept.
    let only = Overlays {
        only: true,
        on: vec!["names".into()],
        off: vec!["skeletons".into()],
        ..Overlays::default()
    };
    state.apply(&only);
    assert_eq!(on(&state), ["Names"]);
    assert!(state.report().waiting.is_empty());
    assert_eq!(state.channel, Some(Channel::Normal));
    // `null` gives the light back, `[]` every pass.
    let back = Overlays {
        channel: Some(None),
        skip: Some(Vec::new()),
        ..Overlays::default()
    };
    state.apply(&back);
    assert_eq!(state.channel, None);
    assert!(state.skip.is_empty());
}

#[test]
fn the_api_refuses_a_name_both_on_and_off_an_empty_name_and_more_than_shading() {
    let both = Overlays {
        on: vec!["FPS".into()],
        off: vec!["fps".into()],
        ..Overlays::default()
    };
    assert!(check(&both).is_some_and(|why| why.contains("not both")));
    let empty = Overlays {
        off: vec![" ".into()],
        ..Overlays::default()
    };
    assert!(check(&empty).is_some());
    for shown in [Shown::Image("view depth".into()), Shown::PathTraced] {
        let channel = Overlays {
            channel: Some(Some(shown)),
            ..Overlays::default()
        };
        assert!(check(&channel).is_some_and(|why| why.contains("`show`")));
    }
}

#[test]
fn whats_on_is_kept_for_the_next_run_unless_the_flag_names_others() {
    let scratch = crate::dirs::tests::Scratch::new("overlays");
    let context = || {
        let mut cx = Context::new(None);
        cx.keep_state_in(scratch.0.clone());
        cx
    };
    let mut cx = context();
    state::start(&mut cx, None);
    assert!(on(&cx.dev.overlays).is_empty());
    cx.dev.overlays.engine = true;
    cx.dev.overlays.set(Overlay::Fps, true);
    cx.dev.overlays.turn("Enemy aim", true);
    cx.dev.overlays.set_channel(Some(Channel::Roughness));
    cx.dev.overlays.set_skipped(Pass::Bloom, true);
    state::keep(&mut cx);
    // The next run starts with them.
    let mut next = context();
    state::start(&mut next, None);
    let kept = &mut next.dev.overlays;
    assert!(kept.on(Overlay::Fps));
    assert!(kept.asked("enemy aim"));
    assert_eq!(kept.channel, Some(Channel::Roughness));
    assert_eq!(kept.skip, [Pass::Bloom]);
    // The flag's in their place, kept only once changed.
    let mut flagged = context();
    state::start(&mut flagged, Some(&["colliders".to_owned()]));
    let given = &flagged.dev.overlays;
    assert!(given.on(Overlay::Colliders) && !given.on(Overlay::Fps));
    assert_eq!(given.channel, None);
    state::keep(&mut flagged);
    let mut after = context();
    state::start(&mut after, None);
    assert!(after.dev.overlays.on(Overlay::Fps));
    // `none` turns none on.
    let mut none = context();
    state::start(&mut none, Some(&["none".to_owned()]));
    let none = &none.dev.overlays;
    assert!(!Overlay::ALL.into_iter().any(|overlay| none.on(overlay)));
}

#[test]
fn a_kept_file_that_doesnt_read_turns_none_on() {
    let scratch = crate::dirs::tests::Scratch::new("overlays-unread");
    std::fs::create_dir_all(&scratch.0).expect("a scratch directory");
    std::fs::write(scratch.0.join("overlays.ron"), "(on: [").expect("a kept file");
    let mut cx = Context::new(None);
    cx.keep_state_in(scratch.0.clone());
    state::start(&mut cx, None);
    assert!(
        !Overlay::ALL
            .into_iter()
            .any(|overlay| cx.dev.overlays.on(overlay))
    );
}

#[test]
fn the_key_below_esc_opens_and_closes_the_bar_and_stops_there() {
    let mut cx = Context::new(None);
    let mut overlays = DebugOverlays::default();
    let mut press = |cx: &mut Context, key, pressed, repeat| {
        let event = Event::Key {
            key,
            pressed,
            repeat,
        };
        overlays.event(cx, &event)
    };
    assert!(press(&mut cx, Key::Backquote, true, false).is_break());
    assert!(cx.dev.overlays.bar);
    // A repeat doesn't close it.
    assert!(press(&mut cx, Key::Backquote, true, true).is_break());
    assert!(cx.dev.overlays.bar);
    assert!(press(&mut cx, Key::Backquote, false, false).is_continue());
    assert!(press(&mut cx, Key::Backquote, true, false).is_break());
    assert!(!cx.dev.overlays.bar);
    assert!(press(&mut cx, Key::KeyA, true, false).is_continue());
}

#[test]
fn a_click_picks_with_the_bar_open_and_entities_or_inspector_on() {
    let mut cx = Context::new(None);
    let mut stack = Stack::default();
    stack.push_overlay(DebugOverlays::default());
    assert_eq!(stack.names(), ["DebugOverlays"]);
    let _ = stack.event(&mut cx, &Event::CursorMoved { x: 10.0, y: 20.0 });
    let click = |pressed| Event::MouseButton {
        button: MouseButton::Left,
        pressed,
    };
    assert!(stack.event(&mut cx, &click(true)).is_continue());
    cx.dev.overlays.bar = true;
    assert!(stack.event(&mut cx, &click(true)).is_continue());
    for overlay in [Overlay::Entities, Overlay::Inspector] {
        cx.dev.overlays.all_off();
        cx.dev.overlays.set(overlay, true);
        assert!(stack.event(&mut cx, &click(true)).is_break(), "{overlay:?}");
        assert!(stack.event(&mut cx, &click(false)).is_continue());
    }
    cx.dev.overlays.bar = false;
    assert!(stack.event(&mut cx, &click(true)).is_continue());
}

#[test]
fn the_layer_hands_on_the_inspectors_commands_for_recordings() {
    let mut overlays = DebugOverlays::default();
    let commands = overlays.commands().expect("the Inspector's commands");
    assert_eq!(commands.len(), 0);
}
