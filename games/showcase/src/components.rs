//! The showcase's components, which its scenes give entities.

use gg_engine::prelude::*;
#[cfg(doc)]
use gg_engine::{AnimationStates, Joint3D, ParticleEmitter, Text};

/// A sound of the showcase's, which a [`Chime`] rings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub(crate) enum Tone {
    /// `sounds/bell.wav`, a struck bell.
    #[default]
    Bell,
    /// `sounds/pluck.wav`, a plucked string.
    Pluck,
}

/// Rings its tone from its entity every `every` seconds of the world's ticks: `sounds.ron`'s and
/// `listen.ron`'s chimes. A `Circle` on the entity flashes as it rings.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub(crate) struct Chime {
    /// What it rings.
    pub(crate) tone: Tone,
    /// The seconds between its rings.
    #[reflect(min = 0.05, default = 1.0)]
    pub(crate) every: f32,
    /// The seconds since it last rang; negative, a wait before its first.
    pub(crate) time: f32,
}

/// Bursts its entity's [`ParticleEmitter`] every `every` seconds of the world's ticks, by raising
/// the emitter's count of bursts: `particles.ron`'s sparks.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub(crate) struct Burster {
    /// The seconds between its bursts.
    #[reflect(min = 0.05, default = 1.0)]
    pub(crate) every: f32,
    /// The seconds since it last burst; negative, a wait before its first.
    pub(crate) time: f32,
}

/// Plays `music/loop.ogg` while its entity is in the world: `music.ron`'s. The music fades in,
/// fades out once no jukebox is left, and plays on through a pause.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub(crate) struct Jukebox {
    /// How loud the music's bus plays it: 1 as loud as the music is, 0 silent.
    #[reflect(min = 0.0, max = 1.0, default = 1.0)]
    pub(crate) volume: f32,
}

/// Has the showcase draw its HUD on the view's canvas while its entity is in the world: `ui.ron`'s.
/// Clicks on its button reach the world as commands.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub(crate) struct Hud {
    /// The ticks it has been in the world for.
    pub(crate) ticks: u32,
    /// The clicks on its button.
    pub(crate) clicks: u32,
}

/// Has the showcase draw `ui/menu.ui` and give it input while its entity is in the world:
/// `menu.ron`'s. Its presses reach the world as commands, which record them here.
#[derive(Clone, Debug, PartialEq, Component)]
pub(crate) struct Menu {
    /// The ticks it has been in the world for.
    pub(crate) ticks: u32,
    /// The presses of its buttons, and of back, and the name of the last pressed.
    pub(crate) presses: u32,
    pub(crate) pressed: String,
}

/// Has the showcase draw `ui/widgets.ui` and give it input while its entity is in the world:
/// `widgets.ron`'s. Their changes reach the world as commands, which record the last here.
#[derive(Clone, Debug, PartialEq, Component)]
pub(crate) struct Widgets {
    /// How many changes and presses there have been, then the last: a widget's name and value.
    pub(crate) changes: u32,
    pub(crate) changed: String,
}

/// Has the game's shader, `shaders/plasma.slang`, draw its entity as a unit square of drifting
/// waves, once the world ticks: `shader.ron`'s.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub(crate) struct Plasma {
    /// What the waves' colors are multiplied by, in linear RGB. Its alpha is unused.
    #[reflect(default = Vec4::ONE)]
    pub(crate) tint: Vec4,
    /// How many waves cross the square.
    #[reflect(min = 0.0, default = 1.0)]
    pub(crate) cells: f32,
    /// How fast the waves drift, 1 a radian a second.
    #[reflect(default = 1.0)]
    pub(crate) speed: f32,
}

/// Turns its entity, a camera, by the mouse while the cursor is grabbed: `listen.ron`'s. A click in
/// the view grabs the cursor, and Escape lets it go.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub(crate) struct Look {
    /// Its turn to its left about the world's up, in radians, from facing -Z.
    pub(crate) yaw: f32,
    /// Its turn up, in radians, no further than [`MAX_PITCH`] either way.
    #[reflect(min = -1.553_343, max = 1.553_343)]
    pub(crate) pitch: f32,
    /// How far a unit of the mouse's motion turns it, in radians.
    #[reflect(min = 0.0, default = 0.0025)]
    pub(crate) sensitivity: f32,
}

/// The furthest a [`Look`] turns up or down: 89°, short of straight, where its yaw would spin.
pub(crate) const MAX_PITCH: f32 = 1.553_343;

/// Has its entity's [`Text`] give `of`'s state machine's state and `speed`, as `Move  ·  speed 3.3`:
/// `states.ron`'s.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub(crate) struct StateLabel {
    /// The entity whose [`AnimationStates`] it tells of.
    pub(crate) of: Option<Entity>,
}

/// Stretches its entity, a model a unit tall along Y, between the anchors of the [`Joint3D`] on it:
/// `joints.ron`'s rope.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub(crate) struct Tether {
    /// How wide and deep it is.
    #[reflect(min = 0.0, default = 0.03)]
    pub(crate) thickness: f32,
}

/// Fires its entity, a body, from `from` every `every` seconds of the world's ticks: `joints.ron`'s
/// balls, fired at a thin plate.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub(crate) struct Launch {
    /// Where it's fired from.
    pub(crate) from: Vec3,
    /// How fast it's fired, in units a second.
    pub(crate) velocity: Vec3,
    /// The seconds between its firings.
    #[reflect(min = 0.05, default = 1.0)]
    pub(crate) every: f32,
    /// The seconds since it was last fired; negative, a wait before its first.
    pub(crate) time: f32,
}

/// Registers the game's own components with `world`, so that its scenes, the Inspector's Add
/// Component and the tool API know them before any entity has one.
pub(crate) fn register(world: &mut World) -> Result<(), Error> {
    world.register_all::<(Chime, Burster, Jukebox, Hud, Menu, Widgets, Plasma, Look)>()?;
    world.register_all::<(StateLabel, Tether, Launch)>()
}
