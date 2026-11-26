//! Which debug overlays are on, as the bar, `--overlays`, the tool API and the kept file set them.

use gg_api::{OverlayGroup, Overlays, Pass, Shown};
use gg_renderer::Channel;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::Context;

/// The file the overlays on and the Render menu's choices are kept in between runs, beside the
/// dock's layout.
const KEPT: &str = "overlays.ron";

/// One of the engine's overlays, in the bar's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Overlay {
    Fps,
    Frame,
    Memory,
    Rendering,
    Simulation,
    Input,
    Network,
    Log,
    Performance,
    Entities,
    Inspector,
    Colliders,
    Contacts,
    Velocities,
    Navigation,
    Cells,
    Bounds,
    Axes,
    Names,
    Skeletons,
    Cameras,
    Lights,
    Sounds,
}

impl Overlay {
    pub(crate) const ALL: [Self; 23] = [
        Self::Fps,
        Self::Frame,
        Self::Memory,
        Self::Rendering,
        Self::Simulation,
        Self::Input,
        Self::Network,
        Self::Log,
        Self::Performance,
        Self::Entities,
        Self::Inspector,
        Self::Colliders,
        Self::Contacts,
        Self::Velocities,
        Self::Navigation,
        Self::Cells,
        Self::Bounds,
        Self::Axes,
        Self::Names,
        Self::Skeletons,
        Self::Cameras,
        Self::Lights,
        Self::Sounds,
    ];

    /// As the bar and the tool API name it.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Fps => "FPS",
            Self::Frame => "Frame",
            Self::Memory => "Memory",
            Self::Rendering => "Rendering",
            Self::Simulation => "Simulation",
            Self::Input => "Input",
            Self::Network => "Network",
            Self::Log => "Log",
            Self::Performance => "Performance",
            Self::Entities => "Entities",
            Self::Inspector => "Inspector",
            Self::Colliders => "Colliders",
            Self::Contacts => "Contacts",
            Self::Velocities => "Velocities",
            Self::Navigation => "Navigation",
            Self::Cells => "Cells",
            Self::Bounds => "Bounds",
            Self::Axes => "Axes",
            Self::Names => "Names",
            Self::Skeletons => "Skeletons",
            Self::Cameras => "Cameras",
            Self::Lights => "Lights",
            Self::Sounds => "Sounds",
        }
    }

    pub(crate) const fn group(self) -> OverlayGroup {
        match self {
            Self::Fps
            | Self::Frame
            | Self::Memory
            | Self::Rendering
            | Self::Simulation
            | Self::Input
            | Self::Network
            | Self::Log => OverlayGroup::Stats,
            Self::Performance | Self::Entities | Self::Inspector => OverlayGroup::Inspect,
            Self::Colliders
            | Self::Contacts
            | Self::Velocities
            | Self::Navigation
            | Self::Cells
            | Self::Bounds
            | Self::Axes
            | Self::Names
            | Self::Skeletons
            | Self::Cameras
            | Self::Lights
            | Self::Sounds => OverlayGroup::World,
        }
    }

    /// What the bar's tooltip says it shows.
    pub(crate) const fn tip(self) -> &'static str {
        match self {
            Self::Fps => "Frames a second and the mean frame time, over the last second",
            Self::Frame => "Where the last second's frames went, by part, CPU and GPU",
            Self::Memory => "The process's resident memory, now and at its peak",
            Self::Rendering => "Draws, instances and triangles in the last frame",
            Self::Simulation => "The tick, whether the simulation runs, and the entities",
            Self::Input => "The cursor, the keys and buttons held, and the gamepads",
            Self::Network => "A session's round trip, input delay, rollbacks and traffic",
            Self::Log => "The last warnings and errors, fading",
            Self::Performance => "The Performance panel: a graph of recent frames by part",
            Self::Entities => "The Entities panel. With the bar open, a click in the view picks",
            Self::Inspector => "The Inspector panel: the picked entity's components",
            Self::Colliders => "Colliders' outlines and joints",
            Self::Contacts => "Where colliders and characters touch, and which way they push",
            Self::Velocities => "How far bodies and characters go in a quarter second",
            Self::Navigation => "The navmesh and agents' paths",
            Self::Cells => "The world's cells, those in and those refused",
            Self::Bounds => "The box round what each root entity draws",
            Self::Axes => "Each entity's X (red), Y (green) and Z (blue)",
            Self::Names => "Each root entity's name, else its ID",
            Self::Skeletons => "Skinned models' joints, each joined to its parent",
            Self::Cameras => "Cameras' frusta, but the one the view sees from",
            Self::Lights => "Lights' directions, ranges and cones",
            Self::Sounds => "Where sounds play, and how far they're heard",
        }
    }

    /// The overlay named `name`, as [`same`] matches names.
    pub(crate) fn named(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|overlay| same(overlay.name(), name))
    }
}

/// The parts of shading the Render menu offers, in its order, with their names there.
pub(crate) const CHANNELS: [(Channel, &str); 15] = [
    (Channel::BaseColor, "Base color"),
    (Channel::Normal, "Normal"),
    (Channel::SurfaceNormal, "Surface normal"),
    (Channel::Metallic, "Metallic"),
    (Channel::Roughness, "Roughness"),
    (Channel::Occlusion, "Occlusion"),
    (Channel::Emission, "Emission"),
    (Channel::SpecularColor, "Specular color"),
    (Channel::DirectLight, "Direct light"),
    (Channel::SkyLight, "Sky light"),
    (Channel::Reflection, "Reflection"),
    (Channel::DiffuseLight, "Diffuse light"),
    (Channel::SpecularLight, "Specular light"),
    (Channel::TracedReflection, "Traced reflection"),
    (Channel::ReflectionRays, "Reflection rays"),
];

/// The passes' names in the Render menu, in [`Pass::ALL`]'s order.
pub(crate) const PASSES: [&str; 11] = [
    "Ambient occlusion",
    "Reflections",
    "Shadows",
    "Fog",
    "Bloom",
    "Depth of field",
    "Motion blur",
    "Antialiasing",
    "Grade",
    "Lens",
    "Adaptation",
];

/// Which overlays are on, the Render menu's choices and whether the bar shows: a development
/// build's context's, which frames, the bar, the tool API and the game read.
#[derive(Debug, Default)]
pub(crate) struct State {
    /// Whether the app has the overlays' layer, which shows the engine's overlays: it lists them.
    pub(crate) engine: bool,
    /// By [`Overlay`] as a number.
    on: [bool; Overlay::ALL.len()],
    /// The game's own overlays, in the order it first asked for each, and whether each is on.
    game: Vec<(String, bool)>,
    /// Names turned on that no overlay has yet, as given: a game's not asked for yet.
    waiting: Vec<String>,
    /// What the view shows in place of its light, unless the look's `show` says.
    pub(crate) channel: Option<Channel>,
    /// The passes left out, in [`Pass::ALL`]'s order, each once, with the look's.
    pub(crate) skip: Vec<Pass>,
    pub(crate) bar: bool,
    /// What each World overlay drew in the last frame, lines and labels, which `overlays`
    /// reports: 0 for one on that found nothing to show.
    pub(crate) drew: super::world::Drew,
    /// Whether what's kept changed since it was kept.
    changed: bool,
}

impl State {
    pub(crate) fn on(&self, overlay: Overlay) -> bool {
        self.on[overlay as usize]
    }

    pub(crate) fn set(&mut self, overlay: Overlay, on: bool) {
        let was = &mut self.on[overlay as usize];
        self.changed |= *was != on;
        *was = on;
    }

    /// Whether any of the World menu's overlays is on.
    pub(crate) fn world(&self) -> bool {
        Overlay::ALL
            .into_iter()
            .any(|overlay| overlay.group() == OverlayGroup::World && self.on(overlay))
    }

    /// The game's overlays and whether each is on, in the order it first asked.
    pub(crate) fn game(&self) -> &[(String, bool)] {
        &self.game
    }

    /// Turns the game's `index`th overlay on or off.
    pub(crate) fn set_game(&mut self, index: usize, on: bool) {
        let was = &mut self.game[index].1;
        self.changed |= *was != on;
        *was = on;
    }

    /// Whether the overlay named `name` is on: the engine's of that name, else the game's, which
    /// the first ask lists.
    pub(crate) fn asked(&mut self, name: &str) -> bool {
        if let Some(overlay) = Overlay::named(name) {
            return self.on(overlay);
        }
        if let Some((_, on)) = self.game.iter().find(|(own, _)| same(own, name)) {
            return *on;
        }
        let waited = self.waiting.iter().position(|waiting| same(waiting, name));
        let on = waited.map(|at| self.waiting.remove(at)).is_some();
        self.game.push((name.to_owned(), on));
        on
    }

    /// Turns the overlay named `name` on or off: the engine's, a game's, or, on, one that waits.
    pub(crate) fn turn(&mut self, name: &str, on: bool) {
        if let Some(overlay) = Overlay::named(name) {
            return self.set(overlay, on);
        }
        if let Some(index) = self.game.iter().position(|(own, _)| same(own, name)) {
            return self.set_game(index, on);
        }
        let waited = self.waiting.iter().position(|waiting| same(waiting, name));
        match (waited, on) {
            (None, true) => self.waiting.push(name.to_owned()),
            (Some(at), false) => drop(self.waiting.remove(at)),
            _ => return,
        }
        self.changed = true;
    }

    /// Turns every overlay off, and the view's light and passes back on.
    pub(crate) fn all_off(&mut self) {
        let any = self.on.contains(&true)
            || self.game.iter().any(|(_, on)| *on)
            || !self.waiting.is_empty()
            || self.channel.is_some()
            || !self.skip.is_empty();
        self.on = [false; Overlay::ALL.len()];
        self.game.iter_mut().for_each(|(_, on)| *on = false);
        self.waiting.clear();
        self.channel = None;
        self.skip.clear();
        self.changed |= any;
    }

    pub(crate) fn set_channel(&mut self, channel: Option<Channel>) {
        self.changed |= self.channel != channel;
        self.channel = channel;
    }

    /// Leaves `pass` out, or not.
    pub(crate) fn set_skipped(&mut self, pass: Pass, skipped: bool) {
        if self.skip.contains(&pass) == skipped {
            return;
        }
        let skip = (Pass::ALL.into_iter()).filter(|&each| {
            if each == pass {
                skipped
            } else {
                self.skip.contains(&each)
            }
        });
        self.skip = skip.collect();
        self.changed = true;
    }

    /// Applies what the tool API's `overlays` asked, as checked ([`check`]).
    pub(crate) fn apply(&mut self, asked: &Overlays) {
        if asked.only {
            let (channel, skip) = (self.channel, std::mem::take(&mut self.skip));
            self.all_off();
            (self.channel, self.skip) = (channel, skip);
        }
        for name in &asked.on {
            self.turn(name, true);
        }
        for name in &asked.off {
            self.turn(name, false);
        }
        if let Some(channel) = &asked.channel {
            self.set_channel(channel.as_ref().and_then(channel_of));
        }
        if let Some(skip) = &asked.skip {
            for pass in Pass::ALL {
                self.set_skipped(pass, skip.contains(&pass));
            }
        }
        if let Some(bar) = asked.bar {
            self.bar = bar;
        }
    }

    /// What the tool API's `overlays` answers.
    pub(crate) fn report(&self) -> gg_api::Overlaid {
        let engine =
            (Overlay::ALL.into_iter().filter(|_| self.engine)).map(|overlay| gg_api::Overlay {
                name: overlay.name().to_owned(),
                group: overlay.group(),
                on: self.on(overlay),
                drew: (overlay.group() == OverlayGroup::World && self.on(overlay))
                    .then(|| self.drew[overlay as usize]),
            });
        let game = self.game.iter().map(|(name, on)| gg_api::Overlay {
            name: name.clone(),
            group: OverlayGroup::Game,
            on: *on,
            drew: None,
        });
        gg_api::Overlaid {
            overlays: engine.chain(game).collect(),
            waiting: self.waiting.clone(),
            channel: self.channel.map(shown_of),
            skip: self.skip.clone(),
            bar: self.bar,
        }
    }

    /// Takes whether what's kept changed since this last said so.
    pub(crate) fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// What's kept between runs, as the kept file holds it.
    fn kept(&self) -> Kept {
        let engine = Overlay::ALL.into_iter().filter(|&overlay| self.on(overlay));
        let engine = engine.map(|overlay| overlay.name().to_owned());
        let game = (self.game.iter()).filter(|(_, on)| *on);
        let game = game.map(|(name, _)| name.clone());
        Kept {
            on: engine
                .chain(game)
                .chain(self.waiting.iter().cloned())
                .collect(),
            channel: self.channel.map(shown_of),
            skip: self.skip.clone(),
        }
    }
}

/// What the kept file holds: the overlays on, by name, and the Render menu's choices.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Kept {
    on: Vec<String>,
    channel: Option<Shown>,
    skip: Vec<Pass>,
}

/// Turns on, as a run starts, the overlays `--overlays` names, `none` for none; else those kept
/// from the last run, with its Render menu's choices. A kept file that can't be read warns
/// `engine.state` and turns on none. The flag's overlays aren't kept until the bar or the tool
/// API changes them.
pub(crate) fn start(cx: &mut Context, flag: Option<&[String]>) {
    let kept = match flag {
        Some(names) => Kept {
            on: (names.iter())
                .filter(|name| !same(name, "none"))
                .cloned()
                .collect(),
            ..Kept::default()
        },
        None => read_kept(cx),
    };
    let state = &mut cx.dev.overlays;
    for name in &kept.on {
        state.turn(name, true);
    }
    state.set_channel(kept.channel.as_ref().and_then(channel_of));
    for pass in kept.skip {
        state.set_skipped(pass, true);
    }
    state.changed = false;
}

/// Keeps what changed since it was last kept, for the next runs; a headless run keeps nothing.
/// Warns `engine.state` if it can't.
pub(crate) fn keep(cx: &mut Context) {
    if !cx.dev.overlays.take_changed() {
        return;
    }
    let pretty = ron::ser::PrettyConfig::default();
    let text = ron::ser::to_string_pretty(&cx.dev.overlays.kept(), pretty).expect("RON");
    if let Err(error) = cx.save_state(KEPT, text.as_bytes()) {
        warn!(code = error.code(), "{error:#}");
    }
}

fn read_kept(cx: &Context) -> Kept {
    let read = cx.read_state(KEPT).and_then(|bytes| {
        let Some(bytes) = bytes else {
            return Ok(Kept::default());
        };
        let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string());
        let kept = text.and_then(|text| ron::from_str(text).map_err(|e| e.to_string()));
        kept.map_err(|problem| {
            let message = format!("can't read the debug overlays kept in {KEPT}: {problem}");
            gg_core::Error::new("engine.state", message)
        })
    });
    read.unwrap_or_else(|error| {
        warn!(code = error.code(), "{error:#}; no overlay is on");
        Kept::default()
    })
}

/// Whether two overlays' names are the same: in any case, `_` and `-` matching a space.
pub(crate) fn same(a: &str, b: &str) -> bool {
    let fold = |c: char| match c {
        '_' | '-' => ' ',
        c => c.to_ascii_lowercase(),
    };
    a.chars().map(fold).eq(b.chars().map(fold))
}

/// The part of shading `shown` names; `None` for an image or path tracing.
pub(crate) fn channel_of(shown: &Shown) -> Option<Channel> {
    match crate::api::Shown::of(shown) {
        crate::api::Shown::Shading(channel) => Some(channel),
        _ => None,
    }
}

/// As the tool API names `channel`.
pub(crate) fn shown_of(channel: Channel) -> Shown {
    match channel {
        Channel::BaseColor => Shown::BaseColor,
        Channel::Normal => Shown::Normal,
        Channel::SurfaceNormal => Shown::SurfaceNormal,
        Channel::Metallic => Shown::Metallic,
        Channel::Roughness => Shown::Roughness,
        Channel::Occlusion => Shown::Occlusion,
        Channel::Emission => Shown::Emission,
        Channel::SpecularColor => Shown::SpecularColor,
        Channel::DirectLight => Shown::DirectLight,
        Channel::SkyLight => Shown::SkyLight,
        Channel::Reflection => Shown::Reflection,
        Channel::DiffuseLight => Shown::DiffuseLight,
        Channel::SpecularLight => Shown::SpecularLight,
        Channel::TracedReflection => Shown::TracedReflection,
        Channel::ReflectionRays => Shown::ReflectionRays,
    }
}

/// Checks what the tool API's `overlays` asked: `None` if it's fine, else why not, as what the
/// method takes.
pub(crate) fn check(asked: &Overlays) -> Option<String> {
    let names = asked.on.iter().chain(&asked.off);
    if names.clone().any(|name| name.trim().is_empty()) {
        return Some("takes overlays' names, which aren't empty".to_owned());
    }
    if let Some(both) = (asked.on.iter()).find(|on| asked.off.iter().any(|off| same(on, off))) {
        return Some(format!("takes `{both}` in `on` or `off`, not both"));
    }
    if let Some(Some(shown)) = &asked.channel
        && channel_of(shown).is_none()
    {
        return Some(
            "takes a part of shading as `channel`, such as `normal`; the look's `show` takes \
             images and path tracing"
                .to_owned(),
        );
    }
    None
}
