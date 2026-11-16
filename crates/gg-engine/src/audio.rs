//! Sounds and music: what layers load from audio files. Sounds decode whole as they load; ticks
//! queue them in the world and layers play them as they update, each panned and faded as the game's
//! view hears it: by where it shows in a view without perspective, and by its direction and
//! distance from a camera with perspective. Music decodes as it plays, which layers play one at a
//! time, looping. What plays follows how the run plays, as the tool API pauses and stops it.
//! Both mix through volume buses in kira, whose mixer runs on cpal's audio thread, as
//! ARCHITECTURE.md's Audio says.
//!
//! The engine pans and fades each play itself, each frame, rather than giving each place a spatial
//! track of kira's: so a sound starts and follows its entity without a track made or dropped.

mod decode;

pub(crate) use decode::{Encoded, decode, read_music};

use std::mem;
use std::time::Duration;

use gg_core::math::libm::log10f;
use gg_core::math::{DVec3, Mat4};
use gg_renderer::{Projection, Viewpoint};
use kira::backend::Backend;
use kira::backend::cpal::CpalBackend;
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::sound::streaming::{StreamingSoundData, StreamingSoundHandle};
use kira::sound::{FromFileError, PlaybackState};
use kira::track::{MainTrackBuilder, TrackBuilder, TrackHandle};
use kira::{AudioManager, AudioManagerSettings, Decibels, Panning, Tween};
use tracing::{debug, warn};

use crate::{Entity, World};

use decode::Stream;

/// How far toward a side a sound at that edge of the view pans: halfway, so that the other ear
/// still hears it, as kira pans at constant power, all of it in one ear at a side.
const PAN: f32 = 0.5;

/// How far beyond the view's edges a sound fades out to silence: a view's width or height past
/// them.
const FADE: f32 = 1.0;

/// How far from a camera with perspective a sound is as loud as it is, and how far it fades to
/// silence by, in world units, the fade linear in decibels: kira's spatial tracks' defaults
/// (`SpatialTrackDistances`, with their linear attenuation).
const HEARD: [f32; 2] = [1.0, 100.0];

/// How far toward a side a sound straight out to that side of a camera with perspective pans: as
/// far as kira's spatial tracks' default spatialization strength pans one, so that the other ear
/// still hears it.
const SIDE_PAN: f32 = 0.75;

/// The quickest a fade goes: kira's default tween, as a change of volume any quicker clicks.
const QUICKEST: Duration = Duration::from_millis(10);

/// How long a play of music whose file changed crossfades to what the file holds now, from where it
/// was.
#[cfg(feature = "dev")]
const RELOAD_FADE: Duration = Duration::from_millis(100);

/// A sound a layer loaded with [`Context::add_sound`], whole in memory, which ticks queue with
/// [`World::play_sound`] and layers play with [`Context::play_sound`]: as often as they like, many
/// at once, each from its start. A handle, of the context that loaded it.
///
/// [`Context::add_sound`]: crate::Context::add_sound
/// [`Context::play_sound`]: crate::Context::play_sound
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Sound(u32);

/// Music a layer loaded with [`Context::add_music`]: its file's bytes, in memory as compressed as
/// the file is, which a play decodes as it goes, looping. Layers play one at a time, with
/// [`Context::play_music`]. A handle, of the context that loaded it.
///
/// [`Context::add_music`]: crate::Context::add_music
/// [`Context::play_music`]: crate::Context::play_music
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Music(u32);

/// A bus that what plays mixes through, whose volume layers set with [`Context::set_volume`], as a
/// game's settings would, or a menu that ducks the music under it.
///
/// [`Context::set_volume`]: crate::Context::set_volume
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bus {
    /// All that plays: the music's bus and the sounds'.
    Main,
    /// The music.
    Music,
    /// The sounds: those ticks queue, and those layers play.
    Sounds,
}

/// Where a sound plays from, which pans and fades it as the game's view, the listener, hears it.
/// A view without perspective, as a 2D game's, hears it by where it shows in the view: toward the
/// side of the view it's on, halfway there at the view's edge, and as loud as it is while it's in
/// the view, fading out beyond the view's edges to silence a view's width or height past them. A
/// camera with perspective hears it by its direction and distance, as kira's spatial tracks do:
/// toward the side it's on, three quarters of the way there straight out to that side, before or
/// behind; and as loud as it is within a unit of the camera, fading, linearly in decibels, to
/// silence 100 units away.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    /// Nowhere in the world: in the middle, as loud as it is, wherever the view looks, as a game's
    /// UI's sounds play.
    Nowhere,
    /// At a point in the world, where it stays.
    At(DVec3),
    /// On an entity: where the latest tick left it as each frame is handed off, following it as it
    /// moves, and once it's despawned, where it was last, until the sound ends. One that's gone as
    /// the sound is queued or played plays from nowhere. That's up to a tick ahead of where the
    /// entity draws, too little to hear.
    On(Entity),
}

/// Where a sound plays from, as the engine follows it: a [`Place`], an entity's with where it was
/// last.
#[derive(Clone, Copy, Debug)]
enum Spot {
    Nowhere,
    At(DVec3),
    On(Entity, DVec3),
}

impl Spot {
    /// `place` in `world`: where its entity is now, if it's on one, or nowhere if that's gone.
    fn new(place: Place, world: &World) -> Self {
        match place {
            Place::Nowhere => Self::Nowhere,
            Place::At(at) => Self::At(at),
            Place::On(entity) => {
                position(world, entity).map_or(Self::Nowhere, |at| Self::On(entity, at))
            }
        }
    }

    /// Where it plays from now, `None` for nowhere: following its entity in `world` while it's
    /// alive, and once it's gone, where it was last, from then on.
    fn follow(&mut self, world: &World) -> Option<DVec3> {
        match *self {
            Self::Nowhere => None,
            Self::At(at) => Some(at),
            Self::On(entity, last) => match position(world, entity) {
                Some(at) => {
                    *self = Self::On(entity, at);
                    Some(at)
                }
                None => {
                    *self = Self::At(last);
                    Some(last)
                }
            },
        }
    }

    fn place(self) -> Place {
        match self {
            Self::Nowhere => Place::Nowhere,
            Self::At(at) => Place::At(at),
            Self::On(entity, _) => Place::On(entity),
        }
    }
}

/// Where `entity` is in `world`, as the latest tick left it, if it's alive.
fn position(world: &World, entity: Entity) -> Option<DVec3> {
    // Checked first, as a pose of an entity that's gone makes an error, which allocates.
    let pose = world.contains(entity).then(|| world.pose(entity).ok())??;
    Some(pose.matrix.translation)
}

/// The sounds a world's ticks queued, which the engine plays once each tick has run, as
/// [`World::play_sound`] says: a resource, which is empty between ticks, so that no snapshot holds
/// one, and which no hash, recording or scene reads.
#[derive(Clone, Default)]
pub(crate) struct Queue(Vec<(Sound, Spot)>);

impl World {
    /// Queues `sound` to play from `place` once the tick that queues it has run, in a run that
    /// plays sound, as [`Context::add_sound`](crate::Context::add_sound) says: what plays follows
    /// from the world and never feeds it, so the world's hash, a recording and a replay know
    /// nothing of it, and each lockstep peer plays the sounds its own ticks queue. One on an entity
    /// follows it from where it is as it's queued, as [`Place::On`] says. A layer plays a sound
    /// outside its ticks, as its UI's, through [`Context::play_sound`](crate::Context::play_sound).
    /// Once the world's queue has held as many sounds, it allocates nothing.
    pub fn play_sound(&mut self, sound: Sound, place: Place) {
        let spot = Spot::new(place, self);
        match self.resource_mut::<Queue>() {
            Some(queue) => queue.0.push((sound, spot)),
            None => {
                self.insert_resource(Queue(vec![(sound, spot)]));
            }
        }
    }

    /// The sounds the world's ticks queued since they last played, with where they play from, as
    /// queued: for tests of a game's ticks, which call them directly, without the engine playing
    /// what they queue.
    #[doc(hidden)]
    pub fn sounds_queued(&self) -> impl Iterator<Item = (Sound, Place)> + '_ {
        let queued = self.resource::<Queue>().map_or(&[][..], |queue| &queue.0);
        queued.iter().map(|&(sound, spot)| (sound, spot.place()))
    }
}

/// A play of music, as kira's handle to it.
type MusicHandle = StreamingSoundHandle<FromFileError>;

/// How the run plays, which what plays follows: playing; paused, which holds what plays where it
/// is and starts no sound; or stopped, which stops what plays and starts nothing, the music the
/// layers last had play starting afresh, from its start, as the run plays again. Only development
/// builds, whose tool API pauses and stops runs, pause or stop it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(
    not(feature = "dev"),
    allow(dead_code, reason = "only development builds pause and stop")
)]
pub(crate) enum Playback {
    #[default]
    Playing,
    Paused,
    Stopped,
}

/// The run's sounds and music: those the layers loaded, by their handles, kira's output, which
/// plays them in a run that plays sound, on cpal's backend but in tests, the sounds playing, and
/// where those from a place are, which each frame pans and fades as the game's view hears them,
/// the music playing, the buses' volumes, and how the run plays.
pub(crate) struct Audio<B: Backend = CpalBackend> {
    output: Output<B>,
    /// Each sound loaded, by its index: its name, and what it holds, which each play shares.
    sounds: Vec<(Box<str>, StaticSoundData)>,
    /// Each music loaded, by its index: its name, and its file as it was read, which each play
    /// decodes.
    music: Vec<(Box<str>, Encoded)>,
    /// The sounds playing, with where from, so that a stop stops them all.
    playing: Vec<(StaticSoundHandle, Spot)>,
    /// The music layers last had play, if any, with the fade it started with, and its play, while
    /// the output's open and it started, which it hasn't while the run's stopped.
    now_playing: Option<(Music, Duration, Option<MusicHandle>)>,
    /// Each bus's volume as layers last set it, in [`Bus`]'s order, which the output takes as it
    /// opens.
    volumes: [f32; 3],
    /// The view the last frame handed off with one that projects, which hears the sounds; `None`
    /// before any.
    listener: Option<Listener>,
    /// What a world's queue is swapped with as it's played, so that both keep their memory.
    queued: Vec<(Sound, Spot)>,
    /// Whether kira's mixer was full as the last sound started, so that it warns as it fills.
    full: bool,
    /// How the run plays, as [`follow`](Self::follow) was last told.
    playback: Playback,
}

impl<B: Backend> Default for Audio<B> {
    fn default() -> Self {
        Self {
            output: Output::Silent,
            sounds: Vec::new(),
            music: Vec::new(),
            playing: Vec::new(),
            now_playing: None,
            volumes: [1.0; 3],
            listener: None,
            queued: Vec::new(),
            full: false,
            playback: Playback::default(),
        }
    }
}

/// What hears the sounds, as [`Place`] says, from where it is, in `f64`, which a sound's place is
/// taken from first, so that what's near is heard as precisely far from the origin as at it: a view
/// without perspective, by its matrix from there to its clip space; or a camera with perspective,
/// by its matrix from there to its own space, which looks along its -Z, its right +X.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Listener {
    View(Mat4, DVec3),
    Ears(Mat4, DVec3),
}

impl Listener {
    /// What hears through `viewpoint`, `size` pixels across and down: `None` while it has no area,
    /// or can't project.
    fn new(viewpoint: &Viewpoint, size: [u32; 2]) -> Option<Self> {
        let [width, height] = size.map(|side| side as f32);
        if !(width > 0.0 && height > 0.0 && viewpoint.projection.projects()) {
            return None;
        }
        Some(match viewpoint.projection {
            Projection::Orthographic { .. } => {
                let near = viewpoint.relative_to(viewpoint.position);
                Self::View(near.view_projection(width / height), viewpoint.position)
            }
            Projection::Perspective { .. } => {
                let turned = Mat4::from_quat(viewpoint.rotation);
                Self::Ears(turned.inverse(), viewpoint.position)
            }
        })
    }
}

/// kira's output, which plays a run's sounds and music.
enum Output<B: Backend> {
    /// None: the run plays no sound, as headless runs, replays and tests don't, or its output
    /// couldn't be opened.
    Silent,
    /// None yet: the run plays sound from its first sound or music loaded, which opens it.
    Unopened,
    /// Open, its mixer on the backend's audio thread.
    Open(Box<Mixer<B>>),
}

/// kira's output, open: its manager, and the tracks under its main track that the music and the
/// sounds play on, the three of them the buses.
struct Mixer<B: Backend> {
    manager: AudioManager<B>,
    music: TrackHandle,
    sounds: TrackHandle,
}

impl<B: Backend> Mixer<B> {
    /// The settings of a manager whose main track, the main bus, is at its volume in `volumes`,
    /// in [`Bus`]'s order.
    fn settings(volumes: [f32; 3]) -> AudioManagerSettings<B>
    where
        B::Settings: Default,
    {
        AudioManagerSettings {
            main_track_builder: MainTrackBuilder::new().volume(decibels(volumes[0])),
            ..AudioManagerSettings::default()
        }
    }

    /// The mixer of `manager`, made with [`settings`](Self::settings), with a track under its main
    /// track for the music and one for the sounds, each bus at its volume in `volumes`.
    fn new(mut manager: AudioManager<B>, volumes: [f32; 3]) -> Self {
        let [_, music, sounds] = volumes.map(decibels);
        let mut track = |volume: Decibels| {
            let track = manager.add_sub_track(TrackBuilder::new().volume(volume));
            track.expect("two of the 128 tracks kira's mixer takes")
        };
        let (music, sounds) = (track(music), track(sounds));
        Self {
            manager,
            music,
            sounds,
        }
    }
}

impl Audio {
    /// A run's sounds, which play aloud, as a windowed or embedded run's do.
    pub(crate) fn aloud() -> Self {
        Self {
            output: Output::Unopened,
            ..Self::default()
        }
    }

    /// Opens the output of a run that plays sound, as [`open`] says, if it isn't yet: as the first
    /// sound or music is added.
    pub(crate) fn open(&mut self) {
        if matches!(self.output, Output::Unopened) {
            self.output = open(self.volumes);
        }
    }

    /// Has the sounds playing from a place pan and fade as the view of the frame being handed off
    /// hears them, seeing from `viewpoint`, `size` pixels across and down, while it has an area,
    /// following each on an entity of `world` that's alive to where the latest tick left it, as
    /// [`Place::On`] says; and forgets those that have ended. Warns `audio.output_failed` of what
    /// went wrong with the output since, which kira opens again on the system's default output
    /// device, as it does as that changes, and `audio.music_failed` of what went wrong decoding
    /// the music playing, which stops it. It allocates nothing unless something went wrong.
    pub(crate) fn hear_from(&mut self, viewpoint: &Viewpoint, size: [u32; 2], world: &World) {
        let Output::Open(mixer) = &mut self.output else {
            return;
        };
        while let Some(error) = mixer.manager.backend_mut().pop_error() {
            warn!(
                code = "audio.output_failed",
                "The sound's output failed: {error}; it opens again on the default output device"
            );
        }
        if let Some((music, _, Some(handle))) = &mut self.now_playing {
            while let Some(error) = handle.pop_error() {
                let name = &*self.music[music.0 as usize].0;
                warn!(
                    code = "audio.music_failed",
                    music = name,
                    "Music {name:?} failed as it played: {error}; it stops"
                );
            }
        }
        if let Some(listener) = Listener::new(viewpoint, size) {
            self.listener = Some(listener);
        }
        let listener = self.listener.as_ref();
        self.playing.retain_mut(|(handle, spot)| {
            if handle.state() == PlaybackState::Stopped {
                return false;
            }
            if !matches!(spot, Spot::Nowhere) {
                let (panning, volume) = heard(listener, spot.follow(world));
                // Over kira's default 10 ms, rather than at once, which would click.
                handle.set_panning(panning, Tween::default());
                handle.set_volume(volume, Tween::default());
            }
            true
        });
    }
}

impl<B: Backend> Audio<B> {
    /// Adds `data`, sound `name`'s.
    pub(crate) fn add(&mut self, name: &str, data: StaticSoundData) -> Sound {
        let index = u32::try_from(self.sounds.len()).expect("fewer than 2³² sounds");
        self.sounds.push((name.into(), data));
        Sound(index)
    }

    /// Adds `encoded`, music `name`'s.
    pub(crate) fn add_music(&mut self, name: &str, encoded: Encoded) -> Music {
        let index = u32::try_from(self.music.len()).expect("fewer than 2³² pieces of music");
        self.music.push((name.into(), encoded));
        Music(index)
    }

    /// Puts `data` in place of what `sound` holds, which its plays take from then on; those playing
    /// play on as they were.
    #[cfg(feature = "dev")]
    pub(crate) fn replace(&mut self, sound: Sound, data: StaticSoundData) {
        self.sounds[sound.0 as usize].1 = data;
    }

    /// Puts `encoded` in place of what `music` holds, which its plays take from then on: one
    /// playing crossfades to it over [`RELOAD_FADE`], from where it was, or as far into its loop;
    /// while the run's stopped, none plays, and the next play starts it from its start.
    #[cfg(feature = "dev")]
    pub(crate) fn replace_music(&mut self, music: Music, encoded: Encoded) {
        self.music[music.0 as usize].1 = encoded;
        let stopped = self.playback == Playback::Stopped;
        let playing = self.now_playing.as_mut();
        let Some((_, fade, handle)) = playing.filter(|(playing, ..)| *playing == music && !stopped)
        else {
            return;
        };
        let fade = *fade;
        let at = handle.take().map_or(0.0, |mut handle| {
            let at = handle.position();
            handle.stop(tween(RELOAD_FADE));
            at
        });
        let handle = self.start_music(music, at, RELOAD_FADE);
        self.now_playing = Some((music, fade, handle));
    }

    /// Has what plays follow how the run plays, as `playback` says it now does, as [`Playback`]
    /// says, at once but for kira's 10 ms against a click: a run paused holds the music and the
    /// sounds, a run stopped stops them, and a run played after a stop starts the music the layers
    /// last had play from its start, fading in as it did as they had it play.
    #[cfg_attr(
        not(feature = "dev"),
        allow(dead_code, reason = "only development builds pause and stop")
    )]
    pub(crate) fn follow(&mut self, playback: Playback) {
        let was = mem::replace(&mut self.playback, playback);
        if was == playback {
            return;
        }
        let Output::Open(mixer) = &mut self.output else {
            return;
        };
        let quickest = tween(Duration::ZERO);
        if playback == Playback::Stopped {
            for (mut handle, _) in self.playing.drain(..) {
                handle.stop(quickest);
            }
            if let Some((_, _, handle)) = &mut self.now_playing
                && let Some(mut handle) = handle.take()
            {
                handle.stop(quickest);
            }
        }
        // Paused, the tracks hold what's on them, and what starts on them, until they resume.
        if playback == Playback::Paused {
            mixer.music.pause(quickest);
            mixer.sounds.pause(quickest);
        } else if was == Playback::Paused {
            mixer.music.resume(quickest);
            mixer.sounds.resume(quickest);
        }
        if was == Playback::Stopped
            && let Some((music, fade, None)) = self.now_playing
        {
            let handle = self.start_music(music, 0.0, fade);
            self.now_playing = Some((music, fade, handle));
        }
    }

    /// How many frames `sound` holds, at its own rate.
    #[cfg(test)]
    pub(crate) fn frames(&self, sound: Sound) -> usize {
        self.sounds[sound.0 as usize].1.num_frames()
    }

    /// How many frames `music` holds, at its own rate.
    #[cfg(test)]
    pub(crate) fn music_frames(&self, music: Music) -> usize {
        self.music[music.0 as usize].1.frames
    }

    /// Plays `sound` from `place` in `world` now, as [`Context::play_sound`] says.
    ///
    /// # Panics
    ///
    /// If `sound` is another context's, past those loaded here.
    ///
    /// [`Context::play_sound`]: crate::Context::play_sound
    pub(crate) fn play(&mut self, sound: Sound, place: Place, world: &World) {
        self.start(sound, Spot::new(place, world), world);
    }

    /// Plays what `world`'s ticks queued since, emptying its queue, as the engine does once each
    /// tick has run: in a run that plays no sound, it empties the queue alone.
    ///
    /// # Panics
    ///
    /// If a sound queued is another context's.
    pub(crate) fn play_queued(&mut self, world: &mut World) {
        let Some(queue) = world.resource_mut::<Queue>() else {
            return;
        };
        mem::swap(&mut queue.0, &mut self.queued);
        let mut queued = mem::take(&mut self.queued);
        for (sound, spot) in queued.drain(..) {
            self.start(sound, spot, world);
        }
        self.queued = queued;
    }

    /// Plays what `world`'s ticks queued, as [`play_queued`](Self::play_queued) does, as a tick
    /// that rollback runs again has: but for those of `played`, the sounds the tick's runs before
    /// played, each once, so that a sound queued again isn't played again; and adds those it plays
    /// to `played`. Sounds a run before played that this one doesn't queue have played all the
    /// same.
    pub(crate) fn play_queued_again(&mut self, world: &mut World, played: &mut Vec<Sound>) {
        let Some(queue) = world.resource_mut::<Queue>() else {
            return;
        };
        let mut queued = mem::take(&mut queue.0);
        let mut before = played.clone();
        for &(sound, spot) in &queued {
            match before.iter().position(|&was| was == sound) {
                Some(at) => {
                    before.swap_remove(at);
                }
                None => {
                    self.start(sound, spot, world);
                    played.push(sound);
                }
            }
        }
        queued.clear();
        // Kept for its memory.
        if let Some(queue) = world.resource_mut::<Queue>() {
            queue.0 = queued;
        }
    }

    /// Starts `sound` from `spot`, following it in `world`, panned and faded as the listener hears
    /// it there, if the output's open and the run plays, rather than being paused or stopped: past
    /// the 128 sounds kira's mixer plays at once, it's left out, warning `audio.full` as the mixer
    /// fills.
    fn start(&mut self, sound: Sound, mut spot: Spot, world: &World) {
        let loaded = self.sounds.len();
        let Some((name, data)) = self.sounds.get(sound.0 as usize) else {
            panic!(
                "sound {} is another context's, of the {loaded} loaded here",
                sound.0
            );
        };
        let Output::Open(mixer) = &mut self.output else {
            return;
        };
        if self.playback != Playback::Playing {
            return;
        }
        let (panning, volume) = heard(self.listener.as_ref(), spot.follow(world));
        match mixer.sounds.play(data.panning(panning).volume(volume)) {
            Ok(handle) => {
                self.full = false;
                debug!(sound = &**name, "Sound started");
                self.playing.push((handle, spot));
            }
            // A static sound always makes one: the mixer is full.
            Err(_) => {
                if !mem::replace(&mut self.full, true) {
                    warn!(
                        code = "audio.full",
                        sound = &**name,
                        "The mixer plays 128 sounds at once, the most it takes: {name:?} is left \
                         out, and those after it until one ends"
                    );
                }
            }
        }
    }

    /// Plays `music`, or none, in place of the music playing, fading over `fade`, as
    /// [`Context::play_music`] says: while the run's stopped, it starts at the next play.
    ///
    /// # Panics
    ///
    /// If `music` is another context's, past those loaded here.
    ///
    /// [`Context::play_music`]: crate::Context::play_music
    pub(crate) fn play_music(&mut self, music: Option<Music>, fade: Duration) {
        if let Some(Music(index)) = music {
            let loaded = self.music.len();
            let here = (index as usize) < loaded;
            assert!(
                here,
                "music {index} is another context's, of the {loaded} loaded here"
            );
        }
        if self.music_playing() == music {
            return;
        }
        if let Some((_, _, Some(mut handle))) = self.now_playing.take() {
            handle.stop(tween(fade));
        }
        let stopped = self.playback == Playback::Stopped;
        let started = music.map(|music| {
            let handle = if stopped {
                None
            } else {
                self.start_music(music, 0.0, fade)
            };
            (music, fade, handle)
        });
        self.now_playing = started;
    }

    /// The music layers last had play, if any.
    pub(crate) fn music_playing(&self) -> Option<Music> {
        self.now_playing.as_ref().map(|&(music, ..)| music)
    }

    /// Starts `music` from `at` seconds into it, or as far into its loop, fading in over `fade`, on
    /// the music's bus, looping, if the output's open; or, if it can't, warns
    /// `audio.music_failed`.
    fn start_music(&mut self, music: Music, at: f64, fade: Duration) -> Option<MusicHandle> {
        let Output::Open(mixer) = &mut self.output else {
            return None;
        };
        let (name, encoded) = &self.music[music.0 as usize];
        let stream = Stream::new(encoded.bytes.clone());
        let stream = stream.expect("a music's bytes probe as they did as they were read");
        let at = at % (encoded.frames as f64 / f64::from(stream.decoding.rate));
        let data = StreamingSoundData::from_decoder(stream)
            .loop_region(..)
            .start_position(at)
            .fade_in_tween(tween(fade));
        match mixer.music.play(data) {
            Ok(handle) => {
                // Where in it, in seconds, as a reload starts it from where it was.
                debug!(music = &**name, at, "Music started");
                Some(handle)
            }
            // A seek to where it starts that fails, or a full track.
            Err(error) => {
                warn!(
                    code = "audio.music_failed",
                    music = &**name,
                    "Music {name:?} can't start: {error}"
                );
                None
            }
        }
    }

    /// Has `bus`'s volume go to `volume` over `fade`, as [`Context::set_volume`] says.
    ///
    /// # Panics
    ///
    /// If `volume` isn't a finite number.
    ///
    /// [`Context::set_volume`]: crate::Context::set_volume
    pub(crate) fn set_volume(&mut self, bus: Bus, volume: f32, fade: Duration) {
        assert!(
            volume.is_finite(),
            "a bus's volume is {volume}, not a number"
        );
        let volume = volume.max(0.0);
        if mem::replace(&mut self.volumes[bus as usize], volume) == volume {
            return;
        }
        let Output::Open(mixer) = &mut self.output else {
            return;
        };
        let (volume, fade) = (decibels(volume), tween(fade));
        match bus {
            Bus::Main => mixer.manager.main_track().set_volume(volume, fade),
            Bus::Music => mixer.music.set_volume(volume, fade),
            Bus::Sounds => mixer.sounds.set_volume(volume, fade),
        }
    }

    /// `bus`'s volume, as layers last set it.
    pub(crate) fn volume(&self, bus: Bus) -> f32 {
        self.volumes[bus as usize]
    }
}

/// Opens kira's output on the system's default output device, its mixer on cpal's audio thread,
/// with a track under its main track for the music and one for the sounds, each bus at its volume
/// in `volumes`, in [`Bus`]'s order; or, if it can't, as on a machine without one, warns
/// `audio.no_output` with why, and the run plays no sound. Opened, it logs `audio.opened`, with how
/// long that took, at the debug level.
fn open(volumes: [f32; 3]) -> Output<CpalBackend> {
    let _zone = gg_core::zone!("open the sound's output");
    let started = std::time::Instant::now();
    match AudioManager::<CpalBackend>::new(Mixer::settings(volumes)) {
        Ok(manager) => {
            let mixer = Mixer::new(manager, volumes);
            debug!(
                code = "audio.opened",
                ms = crate::perf::millis(started.elapsed()),
                "The sound's output opened"
            );
            Output::Open(Box::new(mixer))
        }
        Err(error) => {
            warn!(
                code = "audio.no_output",
                "The sound's output can't be opened: {error}; the run plays no sound"
            );
            Output::Silent
        }
    }
}

/// A tween over `fade`, or over [`QUICKEST`] if that's sooner.
fn tween(fade: Duration) -> Tween {
    Tween {
        duration: fade.max(QUICKEST),
        ..Tween::default()
    }
}

/// `amplitude` in decibels: 1 as loud as it is, 0.5 half as loud, about -6 dB, down to silence at
/// 0, or below, where kira's silence, -60 dB, is reached.
fn decibels(amplitude: f32) -> Decibels {
    if amplitude > 0.0 {
        Decibels((20.0 * log10f(amplitude)).max(Decibels::SILENCE.0))
    } else {
        Decibels::SILENCE
    }
}

/// How a sound at `at` is heard by `listener`. Through a view without perspective: panned by how
/// far across the view it shows, [`PAN`] of the way toward a side at its edge, and as loud as it is
/// inside the view, fading out beyond its edges to silence [`FADE`] views past them. By a camera
/// with perspective: panned by how far out to a side it is, [`SIDE_PAN`] of the way straight out
/// to it, and as loud as it is within [`HEARD`]'s nearer distance, fading to silence by its farther
/// one, linearly in decibels. In the middle and as loud as it is from nowhere, or without a
/// listener; silent at a point that isn't a number.
fn heard(listener: Option<&Listener>, at: Option<DVec3>) -> (Panning, Decibels) {
    let (Some(listener), Some(at)) = (listener, at) else {
        return (Panning::CENTER, Decibels::IDENTITY);
    };
    let silent = (Panning::CENTER, Decibels::SILENCE);
    match *listener {
        Listener::View(listener, from) => {
            let clip = listener * (at - from).as_vec3().extend(1.0);
            let [x, y] = [clip.x / clip.w, clip.y / clip.w];
            if !(clip.w > 0.0 && x.is_finite() && y.is_finite()) {
                return silent;
            }
            // Clip space spans the view from -1 to 1, across and down.
            let beyond = (x.abs().max(y.abs()) - 1.0).max(0.0);
            let volume = decibels(1.0 - beyond / (2.0 * FADE));
            (Panning(x.clamp(-1.0, 1.0) * PAN), volume)
        }
        Listener::Ears(listener, from) => {
            let own = listener.transform_point3((at - from).as_vec3());
            let distance = own.length();
            if !distance.is_finite() {
                return silent;
            }
            let side = if distance > 0.0 {
                own.x / distance
            } else {
                0.0
            };
            let [near, far] = HEARD;
            let fading = ((distance - near) / (far - near)).clamp(0.0, 1.0);
            (
                Panning(side * SIDE_PAN),
                Decibels(Decibels::SILENCE.0 * fading),
            )
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
