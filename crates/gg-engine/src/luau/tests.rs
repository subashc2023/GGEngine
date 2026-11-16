use std::fs;

use gg_core::math::DVec3;
use gg_core::math::{Quat, Vec3, libm};
use mlua::Value as Lua_;

use super::api::{PARAMS, PRINTED, printed};
use super::compile::check;
use super::fields::{handle, set};
#[cfg(feature = "dev")]
use super::host::lent;
use super::*;
use crate::Component;
use crate::dirs::tests::Scratch;
use crate::edits::{Done, Edit};

/// A Rust component that scripts reach, with a field of each kind they do.
#[derive(Clone, Component)]
struct Mover {
    angle: f32,
    speed: f64,
    steps: u32,
    on: bool,
    at: Vec3,
    turn: Quat,
    target: Option<Entity>,
    /// Of a kind scripts don't reach, which has no handle.
    label: String,
}

impl Mover {
    fn new() -> Self {
        Self {
            angle: 0.25,
            speed: 1.0,
            steps: 0,
            on: false,
            at: Vec3::ZERO,
            turn: Quat::IDENTITY,
            target: None,
            label: String::new(),
        }
    }
}

/// Spins a mover, and counts its ticks in its own fields.
const SPIN: &str = r#"
local Mover = gg.component("Mover")
local Spin = gg.declare("Spin", {
    rate = 2,
    count = 0,
    on = true,
    toward = vector.create(1, 2, 3),
    other = gg.entity,
})

function Spin.tick(entity, dt)
    entity[Mover.angle] = entity[Mover.angle] + entity[Spin.rate] * dt
    entity[Mover.steps] = entity[Mover.steps] + 1
    entity[Mover.on] = entity[Spin.on]
    entity[Mover.at] = entity[Mover.at] + entity[Spin.toward]
    entity[Mover.turn] = gg.quat.from_rotation_z(entity[Mover.angle])
    entity[Mover.target] = entity
    entity[Spin.count] = entity[Spin.count] + 1
    entity[Spin.other] = entity[Mover.target]
end

return Spin
"#;

/// A context whose project's root is `dir`, with `Mover` registered, and a host that has
/// loaded `scripts`, each a path in `dir` and a text, which it writes there.
fn loaded(dir: &Scratch, scripts: &[(&str, &str)]) -> (Context, Scripts) {
    fs::create_dir_all(dir.0.join("scripts")).expect("a directory");
    let mut cx = Context::seeded(1).root(&dir.0);
    cx.world_mut().register::<Mover>().expect("described");
    let mut host = Scripts::new().expect("Luau starts");
    for (name, text) in scripts {
        let path = dir.0.join(name);
        fs::write(&path, text).expect("written");
        host.load(&mut cx, &path).expect("the script loads");
    }
    (cx, host)
}

/// What loading `text` as the script `scripts/refused.luau` fails with.
fn refused(name: &str, text: &str) -> Error {
    let dir = Scratch::new(&format!("luau-refused-{name}"));
    fs::create_dir_all(dir.0.join("scripts")).expect("a directory");
    let mut cx = Context::seeded(1).root(&dir.0);
    cx.world_mut().register::<Mover>().expect("described");
    let path = dir.0.join("scripts/refused.luau");
    fs::write(&path, text).expect("written");
    let mut host = Scripts::new().expect("Luau starts");
    host.load(&mut cx, &path).expect_err("refused")
}

/// The default of field `name` of component type `component`.
fn default(world: &World, component: &str, name: &str) -> Option<Value> {
    match world.default_of(component).expect("a default") {
        Value::Struct(fields) => fields.into_iter().find(|(field, _)| field == name),
        _ => None,
    }
    .map(|(_, value)| value)
}

#[test]
fn a_script_runs_for_each_entity_with_its_component_reaching_rust_s_fields_and_its_own() {
    let dir = Scratch::new("luau-runs");
    let (mut cx, mut host) = loaded(&dir, &[("scripts/spin.luau", SPIN)]);
    let world = cx.world_mut();
    let spun = world.spawn(Mover::new());
    let still = world.spawn(Mover::new());
    world.add_default(spun, "Spin").expect("added");
    let names: Vec<String> = match world.default_of("Spin").expect("a default") {
        Value::Struct(fields) => fields.into_iter().map(|(name, _)| name.into()).collect(),
        _ => Vec::new(),
    };
    assert_eq!(names, ["count", "on", "other", "rate", "toward"]);
    for _ in 0..4 {
        host.run(cx.world_mut());
    }
    let world = cx.world();
    let mover = world.get::<Mover>(spun).expect("a mover");
    let mut angle = 0.25f32;
    for _ in 0..4 {
        angle = (f64::from(angle) + 2.0 * Tick::DT.as_secs_f64()) as f32;
    }
    assert_eq!(mover.angle.to_bits(), angle.to_bits());
    assert_eq!(mover.steps, 4);
    assert!(mover.on);
    assert_eq!(mover.at, Vec3::new(4.0, 8.0, 12.0));
    assert_eq!(mover.turn, Quat::from_rotation_z(angle));
    assert_eq!(mover.target, Some(spun));
    let count = world.get_field(spun, "Spin", "count").expect("a count");
    assert_eq!(count, Value::F64(4.0));
    let other = world.get_field(spun, "Spin", "other").expect("an entity");
    assert_eq!(other, Value::Option(Some(Box::new(Value::Entity(spun)))));
    let untouched = world.get::<Mover>(still).expect("a mover");
    assert_eq!((untouched.angle, untouched.steps), (0.25, 0));
}

#[test]
fn a_script_that_fails_for_one_entity_runs_on_for_the_others() {
    let dir = Scratch::new("luau-fails");
    let script = "local Mover = gg.component(\"Mover\")\n\
            local Step = gg.declare(\"Step\", {})\n\
            function Step.tick(entity, dt)\n\
                entity[Mover.steps] = entity[Mover.steps] + 1\n\
            end\n\
            return Step\n";
    let (mut cx, mut host) = loaded(&dir, &[("scripts/step.luau", script)]);
    let world = cx.world_mut();
    let bare = world.spawn(());
    let mover = world.spawn(Mover::new());
    world.add_default(bare, "Step").expect("added");
    world.add_default(mover, "Step").expect("added");
    host.run(cx.world_mut());
    host.run(cx.world_mut());
    assert_eq!(cx.world().get::<Mover>(mover).expect("a mover").steps, 2);
}

#[test]
fn scripts_go_without_what_would_run_otherwise_on_another_platform_or_in_another_run() {
    let dir = Scratch::new("luau-sandbox");
    let script = "return gg.declare(\"Gone\", {\n\
            removed = os == nil and debug == nil and collectgarbage == nil and gcinfo == nil\n\
                and getfenv == nil and setfenv == nil and loadstring == nil and require == nil\n\
                and newproxy == nil,\n\
            random = math.random == nil and math.randomseed == nil and vector.angle == nil,\n\
            sin = math.sin(1.2345),\n\
            pow = math.pow(2, 0.5),\n\
            log = math.log(10, 3),\n\
        })\n";
    let (cx, _host) = loaded(&dir, &[("scripts/gone.luau", script)]);
    let world = cx.world();
    assert_eq!(default(world, "Gone", "removed"), Some(Value::Bool(true)));
    assert_eq!(default(world, "Gone", "random"), Some(Value::Bool(true)));
    let sin = default(world, "Gone", "sin");
    assert_eq!(sin, Some(Value::F64(libm::sin(1.2345))));
    let pow = default(world, "Gone", "pow");
    assert_eq!(pow, Some(Value::F64(libm::pow(2.0, 0.5))));
    let log = default(world, "Gone", "log");
    assert_eq!(log, Some(Value::F64(libm::log(10.0) / libm::log(3.0))));
}

/// The library functions that make an integer of a number fail at one past the integer's range,
/// or that isn't finite, which platforms cast otherwise, called as functions or as methods, and
/// `string.format` at one its conversion would cast so or write as a float that isn't finite;
/// within the range they work as Luau's own, `table.insert`'s value however large.
#[test]
fn library_functions_refuse_numbers_that_platforms_make_other_integers_of() {
    let dir = Scratch::new("luau-integers");
    let script = "return gg.declare(\"Checked\", {\n\
            sub = string.sub(\"abcd\", 2, -2) == \"bc\" and (\"abcd\"):sub(3) == \"cd\",\n\
            band = bit32.band(1099511627781, 7),\n\
            format = string.format(\"%d %x %5.1f %s\", -3, 255, 2.3, 1e300)\n\
                == \"-3 ff   2.3 \" .. tostring(1e300),\n\
            insert = (function() local t = {} table.insert(t, 1e300) return t[1] end)(),\n\
            unpack = select(2, unpack({ 7, 8, 9 }, 1, 3)),\n\
            buffer = buffer.readu8(buffer.fromstring(\"\\5\"), 0),\n\
        })\n";
    let (cx, _host) = loaded(&dir, &[("scripts/checked.luau", script)]);
    let world = cx.world();
    let checked = |name| default(world, "Checked", name);
    assert_eq!(checked("sub"), Some(Value::Bool(true)));
    assert_eq!(checked("band"), Some(Value::F64(5.0)));
    assert_eq!(checked("format"), Some(Value::Bool(true)));
    assert_eq!(checked("insert"), Some(Value::F64(1e300)));
    assert_eq!(checked("unpack"), Some(Value::F64(8.0)));
    assert_eq!(checked("buffer"), Some(Value::F64(5.0)));
    for (name, call) in [
        ("sub", "string.sub(\"abc\", 1e10)"),
        ("method", "(\"abc\"):sub(0/0)"),
        ("char", "string.char(65, 1/0)"),
        ("band", "bit32.band(1, 1e19)"),
        ("insert", "table.insert({}, 1e10, 1)"),
        ("select", "select(-1e10, 1, 2)"),
        ("buffer", "buffer.readu8(buffer.create(1), 0/0)"),
        ("write", "buffer.writeu32(buffer.create(4), 0, 1e19)"),
        ("ldexp", "math.ldexp(1, 1e10)"),
        ("format-d", "string.format(\"%d\", 1e19)"),
        ("format-c", "string.format(\"%%%c\", -1e10)"),
        ("format-f", "string.format(\"%5.1f\", 0/0)"),
    ] {
        let text = format!("local _ = {call}\nreturn gg.declare(\"Refused\", {{}})");
        let message = refused(&format!("integer-{name}"), &text).to_string();
        let why = ["every platform casts it to", "C libraries write"];
        assert!(
            why.iter().any(|why| message.contains(why)),
            "{name}: {message}"
        );
    }
}

#[test]
fn a_script_that_raises_to_a_power_or_sets_how_it_compiles_is_refused() {
    let pow = refused("pow", "local x = 2\nlocal y = x ^ 3");
    assert_eq!(pow.code(), "script.pow");
    assert!(
        pow.to_string().starts_with("scripts/refused.luau:2: "),
        "{pow}"
    );
    assert_eq!(
        refused("compound", "local x = 2\nx ^= 2").code(),
        "script.pow"
    );
    assert_eq!(
        refused("interpolated", "local s = `{2 ^ 3}`").code(),
        "script.pow"
    );
    assert_eq!(
        refused("optimize", "--!optimize 2\nreturn 1").code(),
        "script.header"
    );
    assert_eq!(
        refused("native", "--!native\nreturn 1").code(),
        "script.header"
    );
    let quoted = "--!strict\n-- a ^ in a comment\n--[==[ and ^ ]==]\n\
            local s = \"^\" .. '\\'^' .. [[^]] .. `^{\"}\"}^{ {1} }^`\n";
    check(quoted).expect("no `^` outside strings and comments");
}

#[test]
fn a_script_that_doesn_t_declare_its_component_as_a_script_does_is_refused() {
    assert_eq!(refused("syntax", "local = 1").code(), "script.bad");
    assert_eq!(refused("none", "return {}").code(), "script.bad");
    let other = "local One = gg.declare(\"One\", {})\nreturn {}";
    assert_eq!(refused("other", other).code(), "script.bad");
    let twice = "gg.declare(\"One\", {})\nreturn gg.declare(\"Two\", {})";
    assert_eq!(refused("twice", twice).code(), "script.bad");
    let rust = "return gg.declare(\"Mover\", {})";
    assert_eq!(refused("rust", rust).code(), "reflect.name_taken");
    let tick = "return gg.declare(\"Ticked\", { tick = 1 })";
    assert_eq!(refused("tick", tick).code(), "script.bad");
    let unknown = "local Nope = gg.component(\"Nope\")\nreturn gg.declare(\"Fine\", {})";
    assert_eq!(refused("unknown", unknown).code(), "script.bad");
}

/// Steps a mover by its own field `by`, of `fields`, and by `step`.
fn stepper(step: u32, fields: &str) -> String {
    format!(
        "local Mover = gg.component(\"Mover\")\n\
             local Step = gg.declare(\"Step\", {{ {fields} }})\n\
             function Step.tick(entity, dt)\n\
                 entity[Mover.steps] = entity[Mover.steps] + entity[Step.by] * {step}\n\
             end\n\
             return Step\n"
    )
}

/// Copies a stepper's `by` into its mover's speed, through a handle of another script's field.
const COPY: &str = "local Mover = gg.component(\"Mover\")\n\
        local Step = gg.component(\"Step\")\n\
        local Copy = gg.declare(\"Copy\", {})\n\
        function Copy.tick(entity, dt)\n\
            entity[Mover.speed] = entity[Step.by]\n\
        end\n\
        return Copy\n";

/// Reloads the script `scripts/step.luau` as an edit does, its text now `text`.
fn reload(cx: &mut Context, text: impl Into<String>) -> Result<Done> {
    let edit = Edit::Script {
        path: "scripts/step.luau".to_owned(),
        text: text.into(),
    };
    cx.apply_edit(edit)
}

#[test]
fn a_reload_runs_from_the_next_tick_its_components_keeping_the_fields_it_keeps() {
    let dir = Scratch::new("luau-reload");
    let first = stepper(1, "by = 1, on = true, gone = 3");
    let scripts = [("scripts/step.luau", &*first), ("scripts/copy.luau", COPY)];
    let (mut cx, mut host) = loaded(&dir, &scripts);
    let world = cx.world_mut();
    let mover = world.spawn(Mover::new());
    world.add_default(mover, "Step").expect("added");
    world.add_default(mover, "Copy").expect("added");
    let by = Value::F64(5.0);
    world.set_field(mover, "Step", "by", &by).expect("set");
    host.run(cx.world_mut());
    reload(&mut cx, stepper(10, "by = 1, on = true, gone = 3")).expect("reloaded");
    host.run(cx.world_mut());
    let steps = |cx: &Context| cx.world().get::<Mover>(mover).expect("a mover").steps;
    assert_eq!(steps(&cx), 55);
    // `gone` dropped, `on` of another kind, `added` added, and `by`'s default changed, which
    // now sorts second: the other script's handle of it finds it there.
    let fields = "by = 2, on = 7, added = vector.create(1, 2, 3)";
    reload(&mut cx, stepper(100, fields)).expect("reloaded");
    let world = cx.world();
    let spun = world.get_field(mover, "Step", "").expect("a stepper");
    let expected = Value::Struct(vec![
        ("added".into(), Value::Vec3(Vec3::new(1.0, 2.0, 3.0))),
        ("by".into(), Value::F64(5.0)),
        ("on".into(), Value::F64(7.0)),
    ]);
    assert_eq!(spun, expected);
    assert_eq!(default(world, "Step", "by"), Some(Value::F64(2.0)));
    host.run(cx.world_mut());
    assert_eq!(steps(&cx), 555);
    let speed = cx.world().get::<Mover>(mover).expect("a mover").speed;
    assert_eq!(speed, 5.0);
    // Refused: one that doesn't compile, one that renames its component, and one of a script
    // no layer loaded; the script runs as it did.
    let refused = |result: Result<Done>| result.expect_err("refused").code().to_owned();
    assert_eq!(refused(reload(&mut cx, "local = ")), "script.bad");
    let renamed = stepper(1, "by = 1").replace("\"Step\"", "\"Stride\"");
    assert_eq!(refused(reload(&mut cx, renamed)), "script.renamed");
    let unknown = Edit::Script {
        path: "scripts/none.luau".to_owned(),
        text: COPY.to_owned(),
    };
    assert_eq!(refused(cx.apply_edit(unknown)), "script.unknown");
    host.run(cx.world_mut());
    assert_eq!(steps(&cx), 1055);
}

/// A reload that moves its type's fields reaches the handles of them that every other VM holds,
/// another layer's or a mod's, as it reaches its own: each finds its field where it is now, and
/// one of a field gone fails as an error does.
#[test]
fn a_reload_reaches_the_handles_other_vms_hold_of_its_fields() {
    let dir = Scratch::new("luau-reload-elsewhere");
    let first = stepper(1, "by = 1");
    let (mut cx, mut steps) = loaded(&dir, &[("scripts/step.luau", &first)]);
    let path = dir.0.join("scripts/copy.luau");
    fs::write(&path, COPY).expect("written");
    let mut copies = Scripts::new().expect("Luau starts");
    copies.load(&mut cx, &path).expect("the script loads");
    let world = cx.world_mut();
    let mover = world.spawn(Mover::new());
    world.add_default(mover, "Step").expect("added");
    world.add_default(mover, "Copy").expect("added");
    let by = Value::F64(5.0);
    world.set_field(mover, "Step", "by", &by).expect("set");
    let speed = |cx: &Context| cx.world().get::<Mover>(mover).expect("a mover").speed;
    // `after` sorts first, where `by` was, which holds an entity now.
    reload(&mut cx, stepper(1, "after = gg.entity, by = 2")).expect("reloaded");
    steps.run(cx.world_mut());
    copies.run(cx.world_mut());
    assert_eq!(speed(&cx), 5.0);
    reload(&mut cx, stepper(1, "after = gg.entity")).expect("reloaded");
    cx.world_mut()
        .get_mut::<Mover>(mover)
        .expect("a mover")
        .speed = 1.0;
    copies.run(cx.world_mut());
    assert_eq!(speed(&cx), 1.0, "its field gone");
}

/// A world from before a reload that changed the fields, as `stop` brings back, holds them as
/// they are now once it's back; and so do the components an undo puts back.
#[cfg(feature = "dev")]
#[test]
fn what_comes_back_from_before_a_reload_holds_its_fields_as_they_are_now() {
    let dir = Scratch::new("luau-stop");
    let first = stepper(1, "by = 1, gone = true");
    let (mut cx, mut host) = loaded(&dir, &[("scripts/step.luau", &first)]);
    let world = cx.world_mut();
    let kept = world.spawn(Mover::new());
    let deleted = world.spawn(Mover::new());
    for entity in [kept, deleted] {
        world.add_default(entity, "Step").expect("added");
        let by = Value::F64(4.0);
        world.set_field(entity, "Step", "by", &by).expect("set");
    }
    let deleted = world.id(deleted).expect("alive");
    let mut log = crate::Log::default();
    let delete = crate::Operation::Delete { entity: deleted };
    log.apply_all(world, vec![delete]).expect("deleted");
    let mut stack = crate::layer::Stack::default();
    stack.play(&mut cx, 0);
    reload(&mut cx, stepper(1, "by = 1, added = true")).expect("reloaded");
    assert_eq!(stack.stop(&mut cx), Some(0));
    let expected = Value::Struct(vec![
        ("added".into(), Value::Bool(true)),
        ("by".into(), Value::F64(4.0)),
    ]);
    let step = |world: &World, entity| world.get_field(entity, "Step", "").expect("a stepper");
    assert_eq!(step(cx.world(), kept), expected);
    log.undo(cx.world_mut()).expect("undone");
    let world = cx.world();
    let back = world.entity(deleted).expect("back");
    assert_eq!(step(world, back), expected);
    host.run(cx.world_mut());
    let steps = cx.world().get::<Mover>(kept).expect("a mover").steps;
    assert_eq!(steps, 4);
}

/// Counts a mover's steps, through a local, after a comment, so that a breakpoint asked at the
/// comment's line takes the function's.
const COUNTED: &str = "local Mover = gg.component(\"Mover\")\n\
        local Step = gg.declare(\"Step\", { by = 1, forever = false })\n\
        \n\
        -- Counts.\n\
        function Step.tick(entity, dt)\n\
            local steps = entity[Mover.steps]\n\
            steps = steps + entity[Step.by]\n\
            entity[Mover.steps] = steps\n\
            while entity[Step.forever] do end\n\
        end\n\
        \n\
        return Step\n";

/// A context whose project's root is `dir` with two movers stepped by [`COUNTED`], whose
/// scripts `debugger` debugs, and its host.
#[cfg(feature = "dev")]
fn debugged(dir: &Scratch, debugger: &Arc<Debugger>) -> (Context, Scripts, [Entity; 2]) {
    let (mut cx, host) = loaded(dir, &[("scripts/step.luau", COUNTED)]);
    cx.hosts.debug(debugger.clone());
    let world = cx.world_mut();
    let movers = [(); 2].map(|()| world.spawn(Mover::new()));
    for mover in movers {
        world.add_default(mover, "Step").expect("added");
    }
    (cx, host, movers)
}

/// What a stop's frame holds as `name`, a local's.
#[cfg(feature = "dev")]
fn local(stop: &gg_api::Stop, name: &str) -> serde_json::Value {
    let locals = &stop.frames[0].locals;
    let found = locals.iter().find(|local| local.name == name);
    found.map_or(serde_json::Value::Null, |local| local.value.clone())
}

/// A script stops at a breakpoint inside its tick, which waits there, with its stack and locals,
/// until it's told to go on: a step over goes to its next line, a step out from the tick to the
/// next entity's, and a continue to the next stop; the ticks then end as they would have.
#[cfg(feature = "dev")]
#[test]
fn a_script_stops_at_a_breakpoint_and_steps_over_and_out_until_it_goes_on() {
    use gg_api::{DebugAction, StopReason};
    let dir = Scratch::new("luau-breakpoint");
    let debugger = Arc::new(Debugger::default());
    let (mut cx, mut host, movers) = debugged(&dir, &debugger);
    let ids = movers.map(|mover| cx.world().id(mover).expect("alive").to_string());
    debugger.set("scripts/step.luau".to_owned(), vec![7]);
    cx.hosts.place();
    let client = debugger.clone();
    let client = std::thread::spawn(move || {
        let mut stops = Vec::new();
        let wait = std::time::Duration::from_secs(20);
        let actions = [
            DebugAction::StepOver,
            DebugAction::StepOut,
            DebugAction::Continue,
        ];
        for action in actions {
            stops.push(client.answer(wait).stopped.expect("a stop"));
            client.act(action).expect("it goes on");
        }
        stops.push(client.answer(wait).stopped.expect("a stop"));
        client.set("scripts/step.luau".to_owned(), Vec::new());
        client.act(DebugAction::Continue).expect("it goes on");
        stops
    });
    host.run(cx.world_mut());
    let stops = client.join().expect("the client didn't panic");
    let at = |stop: &gg_api::Stop| (stop.number, stop.why, stop.frames[0].line);
    let numbers: Vec<_> = stops.iter().map(at).collect();
    assert_eq!(
        numbers,
        [
            (1, StopReason::Breakpoint, 7),
            (2, StopReason::Step, 8),
            (3, StopReason::Step, 6),
            (4, StopReason::Breakpoint, 7),
        ]
    );
    let frame = &stops[0].frames[0];
    assert_eq!(
        (&*frame.script, &*frame.function),
        ("scripts/step.luau", "tick")
    );
    assert_eq!(local(&stops[0], "entity"), serde_json::json!(ids[0]));
    assert_eq!(
        local(&stops[0], "dt"),
        serde_json::json!(Tick::DT.as_secs_f64())
    );
    assert_eq!(local(&stops[0], "steps"), serde_json::json!(0.0));
    assert_eq!(local(&stops[1], "steps"), serde_json::json!(1.0));
    assert_eq!(local(&stops[2], "entity"), serde_json::json!(ids[1]));
    let upvalues = &stops[0].frames[0].upvalues;
    let names: Vec<&str> = upvalues.iter().map(|upvalue| &*upvalue.name).collect();
    assert_eq!(names, ["Mover", "Step"]);
    assert_eq!(upvalues[0].value, serde_json::json!("(table)"));
    let world = cx.world();
    for mover in movers {
        assert_eq!(world.get::<Mover>(mover).expect("a mover").steps, 1);
    }
    let after = debugger.answer(std::time::Duration::ZERO);
    assert_eq!((after.stopped, after.asked, after.stops), (None, None, 4));
    assert!(after.breakpoints.is_empty());
    assert!(cx.hosts.took_stop(), "the main loop learns of the stop");
    let refused = debugger
        .act(DebugAction::Continue)
        .expect_err("nothing stopped");
    assert_eq!(refused.code(), "script.not_stopped");
}

/// A pause asked stops the next line a script runs, its tick's first; and one asked while a call
/// runs, as an interrupt in it finds.
#[cfg(feature = "dev")]
#[test]
fn a_pause_stops_the_next_line_a_script_runs() {
    use gg_api::{DebugAction, StopReason};
    let dir = Scratch::new("luau-pause");
    let debugger = Arc::new(Debugger::default());
    let (mut cx, mut host, movers) = debugged(&dir, &debugger);
    debugger.act(DebugAction::Pause).expect("asked");
    assert_eq!(
        debugger.answer(std::time::Duration::ZERO).asked,
        Some(DebugAction::Pause)
    );
    let client = debugger.clone();
    let client = std::thread::spawn(move || {
        let stop = client.answer(std::time::Duration::from_secs(20)).stopped;
        client.act(DebugAction::Continue).expect("it goes on");
        stop.expect("a stop")
    });
    host.run(cx.world_mut());
    let stop = client.join().expect("the client didn't panic");
    assert_eq!((stop.why, stop.frames[0].line), (StopReason::Pause, 6));
    let id = cx.world().id(movers[0]).expect("alive").to_string();
    assert_eq!(local(&stop, "entity"), serde_json::json!(id));
    assert_eq!(debugger.answer(std::time::Duration::ZERO).stops, 1);
    // Asked again, an interrupt in a call that runs finds it.
    debugger.act(DebugAction::Pause).expect("asked");
    let shared = &host.host.borrow().shared;
    let interrupt = lent(shared, cx.world_mut(), interrupted);
    assert!(matches!(interrupt, Interrupt::Pause));
}

/// A breakpoint takes the next line with code from the line asked, kept for a script that hasn't
/// loaded until it does, and is placed again in its new functions as it reloads.
#[cfg(feature = "dev")]
#[test]
fn a_breakpoint_takes_the_next_line_with_code_and_follows_its_script_as_it_reloads() {
    let dir = Scratch::new("luau-placed");
    fs::create_dir_all(dir.0.join("scripts")).expect("a directory");
    let mut cx = Context::seeded(1).root(&dir.0);
    cx.world_mut().register::<Mover>().expect("described");
    let debugger = Arc::new(Debugger::default());
    cx.hosts.debug(debugger.clone());
    debugger.set("scripts/step.luau".to_owned(), vec![12, 3, 9, 13]);
    let placed = |debugger: &Debugger| -> Vec<(u32, Option<u32>)> {
        let answer = debugger.answer(std::time::Duration::ZERO);
        let breakpoints = answer.breakpoints.iter();
        breakpoints.map(|at| (at.line, at.placed)).collect()
    };
    assert_eq!(
        placed(&debugger),
        [(3, None), (9, None), (12, None), (13, None)]
    );
    let path = dir.0.join("scripts/step.luau");
    fs::write(&path, COUNTED).expect("written");
    let mut host = Scripts::new().expect("Luau starts");
    host.load(&mut cx, &path).expect("the script loads");
    let scripts = debugger.answer(std::time::Duration::ZERO).scripts;
    assert_eq!(scripts, ["scripts/step.luau"]);
    // The comment's line takes the function's, which the chunk sets.
    assert_eq!(
        placed(&debugger),
        [(3, Some(5)), (9, Some(9)), (12, Some(12)), (13, None)]
    );
    reload(&mut cx, format!("\n{COUNTED}")).expect("reloaded");
    assert_eq!(
        placed(&debugger),
        [(3, Some(3)), (9, Some(9)), (12, Some(13)), (13, Some(13))]
    );
}

/// A call into a script that never returns fails once it's past its budget, saying where it
/// was, and the game goes on: a tick, the others running as they do, and a chunk as it loads.
#[test]
fn a_call_into_a_script_that_never_returns_fails_past_its_budget() {
    let dir = Scratch::new("luau-budget");
    let (mut cx, mut host) = loaded(&dir, &[("scripts/step.luau", COUNTED)]);
    let world = cx.world_mut();
    let [stuck, free] = [(); 2].map(|()| world.spawn(Mover::new()));
    for mover in [stuck, free] {
        world.add_default(mover, "Step").expect("added");
    }
    let forever = Value::Bool(true);
    world
        .set_field(stuck, "Step", "forever", &forever)
        .expect("set");
    host.run(cx.world_mut());
    host.run(cx.world_mut());
    let steps = |mover| cx.world().get::<Mover>(mover).expect("a mover").steps;
    assert_eq!((steps(stuck), steps(free)), (2, 2));
    let never = refused(
        "never",
        "while true do end\nreturn gg.declare(\"Never\", {})",
    );
    assert_eq!(never.code(), "script.bad");
    let message = never.to_string();
    assert!(
        message.contains("scripts/refused.luau:1: the call into the script ran past its budget"),
        "{message}"
    );
    // A loop that ends, of many turns, runs.
    let looped = "local n = 0\nfor i = 1, 100000 do n += 1 end\n\
            return gg.declare(\"Looped\", { n = n })";
    let (cx, _host) = loaded(
        &Scratch::new("luau-budget-ends"),
        &[("scripts/l.luau", looped)],
    );
    assert_eq!(
        default(cx.world(), "Looped", "n"),
        Some(Value::F64(100_000.0))
    );
}

/// No script code runs outside a call into a script, where alone its budget holds: a chunk is
/// told from what it returns by identity, its tables' `__eq`, which never returns, uncalled, and
/// a script's function called outside any call into a script fails at its first interrupt.
#[test]
fn no_script_code_runs_outside_a_call_into_a_script() {
    let eq = "local mt = { __eq = function() while true do end end }\n\
            local X = setmetatable(gg.declare(\"X\", {}), mt)\n\
            return setmetatable({}, mt)\n";
    let eq = refused("eq", eq);
    assert_eq!(eq.code(), "script.bad");
    assert!(eq.to_string().contains("returns other than `X`"), "{eq}");
    let turns = "local Turns = gg.declare(\"Turns\", {})\n\
            function Turns.tick(entity, dt)\n\
                for i = 1, 2 do end\n\
            end\n\
            return Turns\n";
    let dir = Scratch::new("luau-outside");
    let (_cx, host) = loaded(&dir, &[("scripts/turns.luau", turns)]);
    let tick = host.host.borrow().scripts[0].tick.clone().expect("a tick");
    let outside = tick.call::<()>(()).expect_err("it has no budget");
    let message = outside.to_string();
    assert!(
        message.contains("scripts/turns.luau:3: script code ran outside a call into a script"),
        "{message}"
    );
}

/// A script's write is refused as `World::set_field` refuses one, with its code, the field left
/// as it was: of a number outside its field's bounds, a collider's density below 0, or that
/// isn't finite there; and of `Parent`, which the world keeps, and of whose field, a tuple
/// struct's, no script finds a handle anyway.
#[test]
fn a_script_s_write_is_refused_as_a_field_set_by_name_is() {
    let dir = Scratch::new("luau-checked");
    let script = "local Box = gg.component(\"BoxCollider2D\")\n\
            local Dense = gg.declare(\"Dense\", {\n\
                to = 1,\n\
                parentless = next(gg.component(\"Parent\")) == nil,\n\
            })\n\
            function Dense.tick(entity, dt)\n\
                entity[Box.density] = entity[Dense.to]\n\
            end\n\
            return Dense\n";
    let (mut cx, mut host) = loaded(&dir, &[("scripts/dense.luau", script)]);
    let world = cx.world_mut();
    let parentless = default(world, "Dense", "parentless");
    assert_eq!(parentless, Some(Value::Bool(true)));
    let dense = world.spawn(crate::BoxCollider2D::default());
    world.add_default(dense, "Dense").expect("added");
    let collider = world.component_named("BoxCollider2D").expect("registered");
    let mut fields = world.fields_of(collider).iter();
    let density = fields.position(|field| field.name == "density");
    let density = u32::try_from(density.expect("a density")).expect("few fields");
    for to in [-1.0, f64::NAN] {
        let world = cx.world_mut();
        world
            .set_field(dense, "Dense", "to", &Value::F64(to))
            .expect("set");
        host.run(world);
        let kept = world
            .get::<crate::BoxCollider2D>(dense)
            .expect("a collider");
        assert_eq!(kept.density, 1.0);
        let by_name = world.set_field(dense, "BoxCollider2D", "density", &Value::F32(to as f32));
        let scripted = set(
            world,
            (dense, collider, density),
            Field::F32,
            &Lua_::Number(to),
        );
        let codes = [scripted, by_name.map(drop)].map(|refused| refused.expect_err("refused"));
        assert_eq!(codes[0].code(), "reflect.out_of_range", "{:?}", codes[0]);
        assert_eq!(codes[0].code(), codes[1].code());
    }
    let world = cx.world_mut();
    let [parent, child] = [(); 2].map(|()| world.spawn(()));
    world.set_parent(child, Some(parent)).expect("parented");
    let id = world.component_named("Parent").expect("registered");
    let by_name = world.set_field(child, "Parent", "0", &Value::Entity(dense));
    let other = Lua_::LightUserData(handle(dense.to_bits()));
    let scripted = set(world, (child, id, 0), Field::Entity, &other);
    let codes = [scripted, by_name.map(drop)].map(|refused| refused.expect_err("refused"));
    assert_eq!(codes[0].code(), "reflect.read_only", "{:?}", codes[0]);
    assert_eq!(codes[0].code(), codes[1].code());
    assert_eq!(world.parent(child).expect("alive"), Some(parent));
}

#[test]
fn an_edit_reloading_a_script_reads_as_it_writes() {
    let edit = Edit::Script {
        path: "scripts/spin.luau".to_owned(),
        text: "return 1\n".to_owned(),
    };
    assert_eq!(Edit::from_value(&edit.to_value()).expect("an edit"), edit);
}

#[test]
fn a_declared_component_copies_hashes_and_saves_as_a_rust_one_does() {
    let dir = Scratch::new("luau-saved");
    let (mut cx, mut host) = loaded(&dir, &[("scripts/spin.luau", SPIN)]);
    let world = cx.world_mut();
    let spun = world.spawn(Mover::new());
    world.add_default(spun, "Spin").expect("added");
    host.run(cx.world_mut());
    let world = cx.world();
    let copy = world.clone();
    assert_eq!(copy.hash(), world.hash());
    let whole = |world: &World| world.get_field(spun, "Spin", "").expect("a spin");
    assert_eq!(whole(&copy), whole(world));
    let mut changed = world.clone();
    let count = Value::F64(9.0);
    changed
        .set_field(spun, "Spin", "count", &count)
        .expect("set");
    assert_ne!(changed.hash(), world.hash());
    let mut text = String::new();
    world.save_whole(&mut text).expect("saved");
    changed
        .load_whole(text.lines().enumerate(), true)
        .expect("loads");
    assert_eq!(changed.hash(), world.hash());
    assert_eq!(whole(&changed), whole(world));
}

/// Declares `name`, after `chunk`, whose tick runs `tick` then sets its mover's steps to 7.
fn keeper(name: &str, chunk: &str, tick: &str) -> String {
    format!(
        "local Mover = gg.component(\"Mover\")\n{chunk}\n\
             local Kept = gg.declare(\"{name}\", {{}})\n\
             function Kept.tick(entity, dt)\n{tick}\nentity[Mover.steps] = 7\nend\n\
             return Kept\n"
    )
}

/// A script's chunk keeps nothing that changes: a script whose functions assign a local of its
/// chunk, directly or through a function between, or whose chunk keeps a coroutine or a
/// buffer, is refused, `script.state`, saying where; once its chunk has run, its tables and its
/// globals are frozen, which a tick that writes them fails at; and the metatable entities share
/// is frozen as the chunk runs. A function may assign its own locals, and its enclosing
/// function's.
#[test]
fn a_script_keeps_its_state_in_its_components() {
    let between = "local count = 0\n\
            local function add() return function() count += 1 end end";
    let refusals = [
        (
            "assigned",
            keeper("K", "local count = 0", "count = count + 1"),
        ),
        ("between", keeper("K", between, "add()()")),
        (
            "coroutine",
            keeper(
                "K",
                "local co = coroutine.create(function() end)",
                "local _ = co",
            ),
        ),
        (
            "wrapped",
            keeper(
                "K",
                "local step = coroutine.wrap(function() end)",
                "local _ = step",
            ),
        ),
        ("buffer", keeper("K", "Bytes = buffer.create(4)", "")),
    ];
    for (name, text) in refusals {
        let refused = refused(name, &text);
        assert_eq!(refused.code(), "script.state", "{name}: {refused}");
    }
    let assigned = refused("line", &keeper("K", "local count = 0", "count = count + 1"));
    let message = assigned.to_string();
    assert!(
        message.starts_with("scripts/refused.luau:5: a function assigns `count`"),
        "{message}"
    );
    let hijack = "getmetatable(gg.entity).__tostring = nil\nreturn gg.declare(\"H\", {})";
    let hijack = refused("hijack", hijack);
    assert!(hijack.to_string().contains("readonly"), "{hijack}");
    let dir = Scratch::new("luau-state");
    let own = keeper(
        "Own",
        "local cache = { n = 0 }",
        "local n = 0\nlocal function more() n += 1 end\nmore()\nentity[Mover.angle] = n",
    );
    let table = keeper("Table", "local cache = { n = 0 }", "cache.n = 1");
    let global = keeper("Global", "Total = 0", "Total = 1");
    let scripts = [
        ("scripts/own.luau", &*own),
        ("scripts/table.luau", &*table),
        ("scripts/global.luau", &*global),
    ];
    let (mut cx, mut host) = loaded(&dir, &scripts);
    let world = cx.world_mut();
    let movers = ["Own", "Table", "Global"].map(|component| {
        let mover = world.spawn(Mover::new());
        world.add_default(mover, component).expect("added");
        mover
    });
    host.run(cx.world_mut());
    let steps = movers.map(|mover| cx.world().get::<Mover>(mover).expect("a mover").steps);
    assert_eq!(steps, [7, 0, 0]);
    let angle = cx.world().get::<Mover>(movers[0]).expect("a mover").angle;
    assert_eq!(angle, 1.0);
}

/// A component that rides on another is given to each entity with that one, at its defaults,
/// as the scripts run, before its tick.
#[test]
fn a_component_riding_on_another_is_given_to_each_entity_with_it() {
    let dir = Scratch::new("luau-on");
    let rider = "local Mover = gg.component(\"Mover\")\n\
            local Rider = gg.declare(\"Rider\", { by = 3 }, { on = \"Mover\" })\n\
            function Rider.tick(entity, dt)\n\
                entity[Mover.steps] = entity[Mover.steps] + entity[Rider.by]\n\
            end\n\
            return Rider\n";
    let (mut cx, mut host) = loaded(&dir, &[("scripts/rider.luau", rider)]);
    let world = cx.world_mut();
    let [first, bare] = [world.spawn(Mover::new()), world.spawn(())];
    host.run(cx.world_mut());
    let later = cx.world_mut().spawn(Mover::new());
    host.run(cx.world_mut());
    let world = cx.world();
    let steps = |mover| world.get::<Mover>(mover).expect("a mover").steps;
    assert_eq!((steps(first), steps(later)), (6, 3));
    let by = world.get_field(later, "Rider", "by").expect("its own");
    assert_eq!(by, Value::F64(3.0));
    assert!(world.get_field(bare, "Rider", "by").is_err());
    for (name, options) in [
        ("on-unknown", "{ on = \"Nope\" }"),
        ("on-itself", "{ on = \"Riding\" }"),
        ("on-other", "{ under = \"Mover\" }"),
    ] {
        let text = format!("return gg.declare(\"Riding\", {{}}, {options})");
        assert_eq!(refused(name, &text).code(), "script.bad", "{name}");
    }
}

/// A string pattern that backtracks without end fails past its call's budget, as Luau's matcher
/// comes to the interrupt.
#[test]
fn a_pattern_that_backtracks_fails_past_its_budget() {
    let text = "local a = string.rep(\"a\", 40)\n\
            local found = string.find(a, \"a*a*a*a*a*a*a*a*b\")\n\
            return gg.declare(\"Matched\", {})";
    let error = refused("pattern", text);
    assert!(error.to_string().contains("ran past its budget"), "{error}");
}

/// Casts a ray, a sphere and a box down, and a ray past what the first hit, keeping what each
/// found in its own fields.
const LOOK: &str = r#"
local Look = gg.declare("Look", {
    hit = gg.entity,
    point = vector.create(0, 0, 0),
    normal = vector.create(0, 0, 0),
    distance = -1,
    through = gg.entity,
    ball = -1,
    boxed = -1,
    missed = true,
})

local down = vector.create(0, -1, 0)

function Look.tick(entity, dt)
    local from = vector.create(0.25, 5, -0.25)
    local hit, point, normal, distance = gg.cast_ray(from, down)
    entity[Look.hit] = hit
    entity[Look.point] = point
    entity[Look.normal] = normal
    entity[Look.distance] = distance
    entity[Look.through] = gg.cast_ray(from, down, 10, hit)
    local _, _, _, ball = gg.cast_sphere(0.5, from, down)
    entity[Look.ball] = ball
    local _, _, _, boxed = gg.cast_box(vector.create(1, 1, 1), gg.quat.identity, from, down, nil, gg.entity)
    entity[Look.boxed] = boxed
    entity[Look.missed] = gg.cast_capsule(2, 0.5, gg.quat.identity, from, down, 2) == nil
end

return Look
"#;

/// A script casts through the world's 3D colliders, as code does: what a ray hits first, its
/// entity, where, the surface's normal and how far; past an entity it names; and a sphere, a
/// box and a capsule swept down, nil where it hits nothing within its distance.
#[test]
fn a_script_casts_through_the_world_s_colliders() {
    use crate::{BodyType, BoxCollider3D, RigidBody3D, Transform, step_physics};

    let dir = Scratch::new("luau-casts");
    let (mut cx, mut host) = loaded(&dir, &[("scripts/look.luau", LOOK)]);
    let world = cx.world_mut();
    let fixed = RigidBody3D {
        body_type: BodyType::Static,
        ..RigidBody3D::default()
    };
    let at = |translation, scale| Transform {
        translation,
        scale,
        ..Transform::IDENTITY
    };
    let floor_at = at(DVec3::ZERO, Vec3::new(10.0, 1.0, 10.0));
    let floor = world.spawn((floor_at, fixed, BoxCollider3D::default()));
    let boxed_at = at(DVec3::new(0.0, 1.0, 0.0), Vec3::ONE);
    let boxed = world.spawn((boxed_at, fixed, BoxCollider3D::default()));
    step_physics(world);
    let looker = world.spawn(());
    world.add_default(looker, "Look").expect("added");
    host.run(cx.world_mut());
    let world = cx.world();
    let field = |name| world.get_field(looker, "Look", name).expect("a field");
    let some = |entity| Value::Option(Some(Box::new(Value::Entity(entity))));
    assert_eq!(field("hit"), some(boxed));
    assert_eq!(field("point"), Value::Vec3(Vec3::new(0.25, 1.5, -0.25)));
    assert_eq!(field("normal"), Value::Vec3(Vec3::Y));
    assert_eq!(field("distance"), Value::F64(3.5));
    assert_eq!(field("through"), some(floor));
    let near = |name, want: f64| match field(name) {
        Value::F64(got) => assert!((got - want).abs() < 1e-3, "{name}: {got}, not {want}"),
        other => panic!("{name}: {other:?}"),
    };
    near("ball", 3.0);
    near("boxed", 3.0);
    assert_eq!(field("missed"), Value::Bool(true));
}

/// Drives the motor of the joint on its entity at its own speed.
const DRIVE: &str = r#"
local Joint3D = gg.component("Joint3D")
local Drive = gg.declare("Drive", { speed = 0 })

function Drive.tick(entity, dt)
    entity[Joint3D.target_velocity] = entity[Drive.speed]
    entity[Joint3D.damping] = 20
end

return Drive
"#;

/// A script sets the target of a joint's motor through the joint's fields, as it sets any
/// component's, and the steps after drive the joint by it: a door hinged about Y turns
/// counterclockwise, as seen from above, at the script's speed once its motor has it going.
#[test]
fn a_script_drives_a_joints_motor_through_its_fields() {
    use crate::{
        BodyType, BoxCollider3D, Joint3D, JointKind3D, RigidBody3D, Transform, step_physics,
    };

    let dir = Scratch::new("luau-joints");
    let (mut cx, mut host) = loaded(&dir, &[("scripts/drive.luau", DRIVE)]);
    let world = cx.world_mut();
    let fixed = RigidBody3D {
        body_type: BodyType::Static,
        ..RigidBody3D::default()
    };
    let at = |x| Transform {
        translation: DVec3::new(x, 1.0, 0.0),
        ..Transform::IDENTITY
    };
    let post = world.spawn((at(0.0), fixed, BoxCollider3D::default()));
    let door = (at(1.0), RigidBody3D::default(), BoxCollider3D::default());
    let door = world.spawn(door);
    let hinge = Joint3D {
        body_a: Some(post),
        body_b: Some(door),
        kind: JointKind3D::Revolute,
        anchor_a: Vec3::new(0.5, 0.0, 0.0),
        anchor_b: Vec3::new(-0.5, 0.0, 0.0),
        axis: Vec3::Y,
        ..Joint3D::default()
    };
    let joint = world.spawn(hinge);
    world.add_default(joint, "Drive").expect("added");
    world
        .set_field(joint, "Drive", "speed", &Value::F64(1.0))
        .expect("set");
    for _ in 0..64 {
        host.run(cx.world_mut());
        step_physics(cx.world_mut());
    }
    let world = cx.world();
    let driven = world.get::<Joint3D>(joint).expect("a joint");
    assert_eq!((driven.target_velocity, driven.damping), (1.0, 20.0));
    let turning = world
        .get::<RigidBody3D>(door)
        .expect("a body")
        .angular_velocity;
    assert!(
        turning.abs_diff_eq(Vec3::Y, 0.02),
        "a radian a second about Y: {turning}"
    );
}

/// Sends its agent across the floor, and finds a path there, keeping its corners' count and its
/// end's X, whether its agent had a destination, and whether a path off the floor is found; the
/// next tick, its agent's destination there, stops it.
const STEER: &str = r#"
local NavAgent = gg.component("NavAgent")
local Steer = gg.declare("Steer", { corners = -1, last = 0, had = true, missed = false })

function Steer.tick(entity, dt)
    entity[Steer.had] = entity[NavAgent.destination] ~= nil
    if entity[Steer.had] then
        entity[NavAgent.destination] = nil
        return
    end
    entity[NavAgent.destination] = vector.create(3, 0, 0)
    local path = gg.find_path(vector.create(-3, 1, 0), vector.create(3, 1, 0))
    entity[Steer.corners] = #path
    entity[Steer.last] = path[#path].x
    entity[Steer.missed] = gg.find_path(vector.create(-3, 1, 0), vector.create(30, 1, 0)) == nil
end

return Steer
"#;

/// A script steers an agent by its destination, a vector or nil, and finds paths over the
/// world's navmesh, as code does: an array of corners, the last the end, or nil off it.
#[test]
fn a_script_steers_an_agent_and_finds_paths() {
    use crate::{
        BodyType, BoxCollider3D, NavAgent, NavMeshVolume, RigidBody3D, Transform, step_navigation,
    };

    let dir = Scratch::new("luau-paths");
    let (mut cx, mut host) = loaded(&dir, &[("scripts/steer.luau", STEER)]);
    let world = cx.world_mut();
    let fixed = RigidBody3D {
        body_type: BodyType::Static,
        ..RigidBody3D::default()
    };
    let floor = Transform {
        translation: DVec3::new(0.0, -0.5, 0.0),
        scale: Vec3::new(10.0, 1.0, 10.0),
        ..Transform::IDENTITY
    };
    world.spawn((floor, fixed, BoxCollider3D::default()));
    let volume = NavMeshVolume {
        size: Vec3::new(12.0, 4.0, 12.0),
        ..NavMeshVolume::default()
    };
    world.spawn((Transform::IDENTITY, volume));
    step_navigation(world);
    let agent = world.spawn((Transform::IDENTITY, NavAgent::default()));
    world.add_default(agent, "Steer").expect("added");
    host.run(cx.world_mut());
    let world = cx.world();
    let field = |name| world.get_field(agent, "Steer", name).expect("a field");
    assert_eq!(field("had"), Value::Bool(false));
    assert_eq!(field("corners"), Value::F64(1.0));
    assert_eq!(field("last"), Value::F64(3.0));
    assert_eq!(field("missed"), Value::Bool(true));
    let destination = |world: &World| world.get::<NavAgent>(agent).expect("an agent").destination;
    assert_eq!(destination(world), Some(Vec3::new(3.0, 0.0, 0.0)));
    host.run(cx.world_mut());
    assert_eq!(
        cx.world().get_field(agent, "Steer", "had").ok(),
        Some(Value::Bool(true))
    );
    assert_eq!(destination(cx.world()), None);
}

/// Starts its entity's tweens at its first tick: its scale, eased in; its own size, looping
/// after a wait; and its rotation; and counts the tweens refused, of a scale to a number, a
/// curve and an option there are none of, and seconds less than none.
const PULSE: &str = r#"
local Transform = gg.component("Transform")
local Pulse = gg.declare("Pulse", { started = false, size = 1, refused = 0 })

function Pulse.tick(entity, dt)
    if entity[Pulse.started] then
        return
    end
    entity[Pulse.started] = true
    gg.tween(entity, Transform.scale, vector.create(2, 2, 2), 0.5, { curve = "Quad", ease = "In" })
    gg.tween(entity, Pulse.size, 3, 1, { playback = "Loop", delay = 0.25 })
    gg.tween(entity, Transform.rotation, gg.quat.from_rotation_y(math.pi / 2), 1)
    local one = vector.create(1, 1, 1)
    local refusals = {
        function() gg.tween(entity, Transform.scale, 2, 1) end,
        function() gg.tween(entity, Transform.scale, one, 1, { curve = "Wobbly" }) end,
        function() gg.tween(entity, Transform.scale, one, 1, { speed = 2 }) end,
        function() gg.tween(entity, Transform.scale, one, -1) end,
    }
    for _, refusal in refusals do
        if not pcall(refusal) then
            entity[Pulse.refused] += 1
        end
    end
end

return Pulse
"#;

/// A script tweens its entity's fields, Rust's and its own, from what they hold, with the
/// curve, ease, playback and wait it names, as code does; and a tween it can't start is refused
/// where it asks, starting nothing.
#[test]
fn a_script_tweens_its_entity_s_fields() {
    use crate::{Transform, Tweens, step_animation};

    let dir = Scratch::new("luau-tweens");
    let (mut cx, mut host) = loaded(&dir, &[("scripts/pulse.luau", PULSE)]);
    let world = cx.world_mut();
    let pulsing = world.spawn(Transform::IDENTITY);
    world.add_default(pulsing, "Pulse").expect("added");
    let mut run = |world: &mut World, ticks: usize| {
        for _ in 0..ticks {
            host.run(world);
            step_animation(world);
        }
    };
    run(cx.world_mut(), 16);
    let world = cx.world();
    let field = |name| world.get_field(pulsing, "Pulse", name).expect("a field");
    assert_eq!(field("refused"), Value::F64(4.0));
    let tweens = &world.get::<Tweens>(pulsing).expect("tweens").0;
    assert_eq!(tweens.len(), 3, "{tweens:?}");
    let transform = *world.get::<Transform>(pulsing).expect("placed");
    // A quarter second, half of the scale's, eased in: a quarter of the way.
    assert_eq!(transform.scale, Vec3::splat(1.25));
    assert_eq!(field("size"), Value::F64(1.0), "waiting");
    run(cx.world_mut(), 16);
    let world = cx.world();
    let field = |name| world.get_field(pulsing, "Pulse", name).expect("a field");
    let transform = *world.get::<Transform>(pulsing).expect("placed");
    assert_eq!(transform.scale, Vec3::splat(2.0));
    assert_eq!(field("size"), Value::F64(1.5));
    let eighth = Quat::from_rotation_y(std::f32::consts::FRAC_PI_4);
    assert!(transform.rotation.angle_between(eighth) < 1e-3);
    let tweens = &world.get::<Tweens>(pulsing).expect("tweens").0;
    assert_eq!(tweens.len(), 2, "the scale's ended");
}

/// Sets its entity's state machine's speed to its own at each tick, and at its first, pulls its
/// trigger and sets its bool, counting what's refused: a parameter there's none of, a number
/// for a bool, and a string.
const PACE: &str = r#"
local Pace = gg.declare("Pace", { speed = 2.5, started = false, refused = 0 })

function Pace.tick(entity, dt)
    gg.set_param(entity, "speed", entity[Pace.speed])
    if entity[Pace.started] then
        return
    end
    entity[Pace.started] = true
    gg.set_param(entity, "pounce", true)
    gg.set_param(entity, "grounded", false)
    local refusals = {
        function() gg.set_param(entity, "height", 1) end,
        function() gg.set_param(entity, "grounded", 1) end,
        function() gg.set_param(entity, "jump", "high") end,
    }
    for _, refusal in refusals do
        if not pcall(refusal) then
            entity[Pace.refused] += 1
        end
    end
end

return Pace
"#;

/// A script sets its entity's state machine's parameters, a float by a number and a trigger and
/// a bool by a boolean, as code does; and one it can't set is refused where it asks.
#[test]
fn a_script_sets_its_entity_s_state_machine_s_parameters() {
    use crate::{AnimationParam, AnimationStates, ParamValue};

    let dir = Scratch::new("luau-params");
    let (mut cx, mut host) = loaded(&dir, &[("scripts/pace.luau", PACE)]);
    let world = cx.world_mut();
    let param = |name: &str, value| AnimationParam {
        name: name.to_owned(),
        value,
    };
    let machine = AnimationStates {
        params: vec![
            param("speed", ParamValue::Float(0.0)),
            param("pounce", ParamValue::Trigger(false)),
            param("grounded", ParamValue::Bool(true)),
        ],
        ..AnimationStates::default()
    };
    let pacing = world.spawn(machine);
    world.add_default(pacing, "Pace").expect("added");
    host.run(world);
    let machine = world.get::<AnimationStates>(pacing).expect("its machine");
    assert_eq!(machine.param("speed"), Some(ParamValue::Float(2.5)));
    assert_eq!(machine.param("pounce"), Some(ParamValue::Trigger(true)));
    assert_eq!(machine.param("grounded"), Some(ParamValue::Bool(false)));
    let refused = world.get_field(pacing, "Pace", "refused");
    assert_eq!(refused.expect("a field"), Value::F64(3.0));
    // A call refused for its value keeps no name; those past the checks are kept, the missing
    // parameter's among them.
    let host = host.host.borrow();
    let params = host.shared.params.borrow();
    let kept: Vec<&str> = params.iter().map(|(_, name)| name.as_str()).collect();
    assert_eq!(kept, ["speed", "pounce", "grounded", "height"]);
}

/// Sets 70 parameters of its entity's state machine at its first tick, each by a name of its
/// own, more than a VM keeps.
const MANY: &str = r#"
local Many = gg.declare("Many", { done = false })

function Many.tick(entity, dt)
    if entity[Many.done] then
        return
    end
    entity[Many.done] = true
    for i = 1, 70 do
        gg.set_param(entity, "p" .. i, i)
    end
end

return Many
"#;

/// A script sets parameters by names past the most its VM keeps, as it sets those it keeps.
#[test]
fn names_past_those_a_vm_keeps_set_their_parameters_too() {
    use crate::{AnimationParam, AnimationStates, ParamValue};

    let dir = Scratch::new("luau-many-params");
    let (mut cx, mut host) = loaded(&dir, &[("scripts/many.luau", MANY)]);
    let world = cx.world_mut();
    let params = (1..=70).map(|i| AnimationParam {
        name: format!("p{i}"),
        value: ParamValue::Float(0.0),
    });
    let machine = AnimationStates {
        params: params.collect(),
        ..AnimationStates::default()
    };
    let many = world.spawn(machine);
    world.add_default(many, "Many").expect("added");
    host.run(world);
    let machine = world.get::<AnimationStates>(many).expect("its machine");
    for i in 1..=70 {
        let set = machine.param(&format!("p{i}"));
        assert_eq!(set, Some(ParamValue::Float(i as f32)), "p{i}");
    }
    assert_eq!(host.host.borrow().shared.params.borrow().len(), PARAMS);
}

/// Writes what `tostring` and `string.format`'s `%*` write of a table, a function, a thread
/// and a buffer, and of a table with a `__tostring` metamethod, as defaults of its own.
const SHOWN: &str = r#"
local mine = setmetatable({}, { __tostring = function() return "mine" end })
return gg.declare("Shown", {
    named = tostring({}) == "table" and tostring(print) == "function"
        and tostring(coroutine.create(function() end)) == "thread"
        and tostring(buffer.create(1)) == "buffer",
    formatted = string.format("%* %* %*", {}, print, 1) == "table function 1"
        and ("%*"):format(buffer.create(1)) == "buffer",
    metamethod = tostring(mine) .. string.format("%*", mine) == "minemine",
    entity = tostring(gg.entity) == "gg.entity",
})
"#;

/// A script's `tostring` and `string.format`'s `%*` write a table, a function, a thread or a
/// buffer by its type's name, the same in every run and on every peer, where Luau's write its
/// address; and a table with a `__tostring` metamethod, and an entity, as Luau's do.
#[test]
fn tostring_writes_what_holds_an_address_by_its_type_alone() {
    let dir = Scratch::new("luau-tostring");
    let (cx, _host) = loaded(&dir, &[("scripts/shown.luau", SHOWN)]);
    let world = cx.world();
    assert_eq!(default(world, "Shown", "named"), Some(Value::Bool(true)));
    assert_eq!(
        default(world, "Shown", "formatted"),
        Some(Value::Bool(true))
    );
    assert_eq!(
        default(world, "Shown", "metamethod"),
        Some(Value::Bool(true))
    );
    assert_eq!(default(world, "Shown", "entity"), Some(Value::Bool(true)));
}

/// Userdata of a type that names itself, by `__type`.
struct Typed;

impl mlua::UserData for Typed {
    fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
        fields.add_meta_field("__type", "Typed");
    }
}

/// Userdata of a type that writes itself, by `__tostring`.
struct Shown;

impl mlua::UserData for Shown {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(mlua::MetaMethod::ToString, |_, _, ()| Ok("shown"));
    }
}

/// Userdata is written as Luau's `tostring` writes it, by its `__tostring` metamethod, if it
/// has one, or else by its `__type`, by `tostring` and by `string.format`'s `%*` alike.
#[test]
fn userdata_is_written_by_its_tostring_or_its_type() {
    let lua = Host::vm().expect("Luau starts");
    let tostring: Function = lua.globals().get("tostring").expect("tostring");
    let string: Table = lua.globals().get("string").expect("string");
    let format: Function = string.get("format").expect("format");
    let [typed, shown] = [
        lua.create_userdata(Typed).expect("userdata"),
        lua.create_userdata(Shown).expect("userdata"),
    ];
    for (data, written) in [(typed, "Typed"), (shown, "shown")] {
        let text: String = tostring.call(&data).expect("written");
        assert_eq!(text, written);
        let text: String = format.call(("%*", &data)).expect("formatted");
        assert_eq!(text, written);
    }
}

/// A printed line past [`PRINTED`] bytes is cut there, at a character's start, saying so and
/// how long it was, however long the strings it's given; one within them is printed whole.
#[test]
fn a_printed_line_is_cut_at_a_few_kilobytes() {
    let lua = Host::vm().expect("Luau starts");
    let tostring: Function = lua.globals().get("tostring").expect("tostring");
    let printed = |values: &[Lua_]| printed(&lua, &tostring, values).expect("printed");
    let long = lua.create_string("é".repeat(PRINTED)).expect("a string");
    let values = [Lua_::Number(1.5), Lua_::String(long)];
    let line = printed(&values);
    let whole = 4 + PRINTED * 2;
    let cut = format!(" [cut at {PRINTED} bytes of {whole}]");
    assert!(line.ends_with(&cut), "{}", &line[line.len() - 64..]);
    assert!(line.starts_with("1.5\té"));
    assert!(line.len() <= PRINTED + cut.len());
    let table = Lua_::Table(lua.create_table().expect("a table"));
    let short = [Lua_::Number(0.25), table, Lua_::LightUserData(handle(0))];
    assert_eq!(printed(&short), "0.25\ttable\tgg.entity");
}

/// Declares a component of two fields, whose handles a VM makes as it loads.
const PAIR: &str = "return gg.declare(\"Pair\", { a = 0, b = 0 })\n";

/// Iterates a table keyed by the handles of `Mover`'s fields, and keeps the order it found them
/// in as a default of its own.
const ORDER: &str = r#"
local Mover = gg.component("Mover")
local keys = {
    [Mover.angle] = 1, [Mover.speed] = 2, [Mover.steps] = 3, [Mover.on] = 4, [Mover.at] = 5,
    [Mover.turn] = 6, [Mover.target] = 7,
}
local order = 0
for _, n in pairs(keys) do
    order = order * 10 + n
end
return gg.declare("Order", { order = order })
"#;

/// The handles of a component's fields are the same in every VM, whatever handles it made
/// before, as a lockstep host's VM, which has loaded and reloaded scripts as it ran, and its
/// joiner's, which starts afresh, make others first: so that a table they key iterates alike.
#[test]
fn the_handles_of_fields_are_the_same_whatever_a_vm_made_before() {
    let order = |name: &str, scripts: &[(&str, &str)]| {
        let dir = Scratch::new(name);
        let (cx, _host) = loaded(&dir, scripts);
        default(cx.world(), "Order", "order")
    };
    let afresh = order("luau-handles-afresh", &[("scripts/order.luau", ORDER)]);
    let after = order(
        "luau-handles-after",
        &[("scripts/pair.luau", PAIR), ("scripts/order.luau", ORDER)],
    );
    assert!(afresh.is_some());
    assert_eq!(afresh, after);
}
