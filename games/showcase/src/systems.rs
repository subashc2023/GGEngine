//! What the showcase's ticks run: its commands, and the systems of its components.

use std::fmt::Write as _;

use gg_engine::prelude::*;
use gg_engine::reflect::Value;
use gg_engine::{
    AnimationStates, Curve, Ease, Joint3D, MeshDraw, ParamValue, ParticleEmitter, RigidBody3D,
    Text, Tween,
};

use crate::components::{
    Burster, Chime, Hud, Launch, Look, MAX_PITCH, Menu, Plasma, StateLabel, Tether, Widgets,
};
use crate::{Command, Showcase, Waves};

/// The thickness a chime's ring thins back to after its flash.
pub(crate) const RING: f32 = 0.12;

/// Applies `command` to `world`. Walkers are the entities with `walk.luau`'s `Walk`.
pub(crate) fn apply(world: &mut World, command: &Command) {
    let (field, value) = match *command {
        Command::Walk { heading } => (
            "heading",
            Value::Vec3(Vec3::new(heading.x, 0.0, -heading.y)),
        ),
        Command::Jump => ("jump", Value::Bool(true)),
        Command::Click => {
            for hud in world.query_mut::<&mut Hud>() {
                hud.clicks = hud.clicks.saturating_add(1);
            }
            return;
        }
        Command::Press { ref button } => {
            for menu in world.query_mut::<&mut Menu>() {
                menu.presses = menu.presses.saturating_add(1);
                menu.pressed.clone_from(button);
            }
            return;
        }
        Command::Change {
            ref widget,
            ref value,
        } => {
            for widgets in world.query_mut::<&mut Widgets>() {
                widgets.changes = widgets.changes.saturating_add(1);
                widgets.changed.clear();
                widgets.changed.push_str(widget);
                widgets.changed.push(' ');
                widgets.changed.push_str(value);
            }
            return;
        }
        Command::Scene { ref path } => {
            world.change_scene(path);
            return;
        }
        Command::Look { by } => {
            for (look, transform) in world.query_mut::<(&mut Look, &mut Transform)>() {
                // Right turns to the right, about the world's up, and down looks down.
                look.yaw -= by.x * look.sensitivity;
                look.pitch = (look.pitch - by.y * look.sensitivity).clamp(-MAX_PITCH, MAX_PITCH);
                transform.rotation =
                    Quat::from_rotation_y(look.yaw) * Quat::from_rotation_x(look.pitch);
            }
            return;
        }
    };
    let walkers: Vec<_> = match world.entities(Some("Walk")) {
        Ok(walkers) => walkers.collect(),
        Err(_) => return,
    };
    for walker in walkers {
        // A walker whose script holds no such field, as one edited to drop it, goes without.
        let _ = world.set_field(walker, "Walk", field, &value);
    }
}

impl Showcase {
    /// Rings each of `world`'s chimes whose time has come, from its entity, flashing its circle.
    pub(crate) fn ring(&mut self, world: &mut World) {
        let dt = Tick::DT.as_secs_f32();
        for (entity, chime) in world.query_mut::<(Entity, &mut Chime)>() {
            chime.time += dt;
            if chime.time >= chime.every {
                chime.time -= chime.every;
                self.rang.push((entity, chime.tone));
            }
        }
        for (entity, tone) in self.rang.drain(..) {
            if let Some(assets) = &self.assets {
                world.play_sound(assets.tones[tone as usize], Place::On(entity));
            }
            let flash = Tween {
                curve: Curve::Quad,
                ease: Ease::Out,
                ..Tween::new("Circle", "thickness", 1.0, RING, 0.6)
            };
            // A chime without a circle rings without one.
            let _ = world.tween(entity, flash);
        }
    }

    /// Bursts each of `world`'s bursters' emitters whose time has come.
    pub(crate) fn burst(world: &mut World) {
        let dt = Tick::DT.as_secs_f32();
        for (burster, emitter) in world.query_mut::<(&mut Burster, &mut ParticleEmitter)>() {
            burster.time += dt;
            if burster.time >= burster.every {
                burster.time -= burster.every;
                emitter.bursts = emitter.bursts.wrapping_add(1);
            }
        }
    }

    /// Fires each of `world`'s launches' bodies whose time has come.
    pub(crate) fn launch(world: &mut World) {
        let dt = Tick::DT.as_secs_f32();
        let launches = world.query_mut::<(&mut Launch, &mut Transform, &mut RigidBody3D)>();
        for (launch, transform, body) in launches {
            launch.time += dt;
            if launch.time >= launch.every {
                launch.time -= launch.every;
                transform.translation = launch.from.as_dvec3();
                transform.rotation = Quat::IDENTITY;
                body.velocity = launch.velocity;
                body.angular_velocity = Vec3::ZERO;
            }
        }
    }

    /// Places each of `world`'s tethers between its joint's anchors, as its bodies are now. A
    /// tether already in place isn't written, and one whose joint lacks a body stays put.
    pub(crate) fn tether(&mut self, world: &mut World) {
        let anchored = |body: Option<Entity>, anchor: Vec3| {
            let transform = world.get::<Transform>(body?)?;
            Some(transform.matrix().transform_point3(anchor.as_dvec3()))
        };
        for (entity, tether, joint) in world.query::<(Entity, &Tether, &Joint3D)>() {
            let (Some(from), Some(to)) = (
                anchored(joint.body_a, joint.anchor_a),
                anchored(joint.body_b, joint.anchor_b),
            ) else {
                continue;
            };
            let along = (to - from).as_vec3();
            let placed = Transform {
                translation: (from + to) * 0.5,
                rotation: Quat::from_rotation_arc(Vec3::Y, along.normalize_or(Vec3::Y)),
                scale: Vec3::new(tether.thickness, along.length(), tether.thickness),
            };
            if world.get::<Transform>(entity) != Some(&placed) {
                self.tethered.push((entity, placed));
            }
        }
        for (entity, placed) in self.tethered.drain(..) {
            world
                .insert(entity, placed)
                .expect("the tether's entity, found this tick");
        }
    }

    /// Gives each of `world`'s plasmas its draw with the showcase's shader, as its fields are now.
    pub(crate) fn shade(&mut self, world: &mut World) {
        let Some(assets) = &self.assets else {
            return;
        };
        for (entity, plasma) in world.query::<(Entity, &Plasma)>() {
            let waves = Waves {
                tint: plasma.tint,
                cells: plasma.cells,
                speed: plasma.speed,
                pad: Vec2::ZERO,
            };
            let draw = MeshDraw::new(assets.square, assets.plasma, waves);
            if world.get::<MeshDraw>(entity) != Some(&draw) {
                self.drawn.push((entity, draw));
            }
        }
        for (entity, draw) in self.drawn.drain(..) {
            world
                .insert(entity, draw)
                .expect("the plasma's entity, found this tick");
        }
    }

    /// Has each of `world`'s state labels say its state machine's state and speed, where that
    /// changed.
    pub(crate) fn label(&mut self, world: &mut World) {
        let labels = world.query::<(Entity, &StateLabel)>();
        self.labeled
            .extend(labels.map(|(entity, label)| (entity, label.of)));
        for (entity, of) in self.labeled.drain(..) {
            let Some(machine) = of.and_then(|of| world.get::<AnimationStates>(of)) else {
                continue;
            };
            let speed = match machine.param("speed") {
                Some(ParamValue::Float(speed)) => speed,
                _ => 0.0,
            };
            self.label.clear();
            let state = &machine.state;
            write!(self.label, "{state}  ·  speed {speed:.1}").expect("a string takes any text");
            let changed = world.get::<Text>(entity);
            if changed.is_some_and(|text| text.text != self.label)
                && let Some(text) = world.get_mut::<Text>(entity)
            {
                text.text.clone_from(&self.label);
            }
        }
    }
}
