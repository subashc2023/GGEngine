//! Sounds and music that layers load from audio files. Sounds decode whole on load; ticks queue
//! them and layers play them. Music decodes as it plays, one piece at a time, looping. What plays
//! follows the run (the tool API pauses and stops it). Both mix through volume buses in kira,
//! whose mixer runs on cpal's audio thread (ARCHITECTURE.md's Audio).
//!
//! The engine pans and fades each play itself, each frame ([`Place`]), instead of giving each
//! place a kira spatial track: a sound starts and follows its entity without a track made or
//! dropped.

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

/// Panning of a sound at a side edge of the view: halfway, so the other ear still hears it (kira
/// pans at constant power, all in one ear at a side).
const PAN: f32 = 0.5;

/// Views' widths or heights beyond the view's edges at which a sound is silent.
const FADE: f32 = 1.0;

/// Distances in world units from a camera with perspective: full volume within the first,
/// silence from the second, linear in decibels between. kira's spatial tracks' defaults
/// (`SpatialTrackDistances`).
const HEARD: [f32; 2] = [1.0, 100.0];

/// Panning of a sound straight out to a side of a camera with perspective: kira's spatial tracks'
/// default spatialization strength, so the other ear still hears it.
const SIDE_PAN: f32 = 0.75;

/// The quickest fade: kira's default tween, since a quicker volume change clicks.
const QUICKEST: Duration = Duration::from_millis(10);

/// How long music whose file changed crossfades to the file's new content, from where it was.
#[cfg(feature = "dev")]
const RELOAD_FADE: Duration = Duration::from_millis(100);

/// A sound loaded with [`Context::add_sound`], whole in memory. Ticks queue it with
/// [`World::play_sound`], layers play it with [`Context::play_sound`], many at once, each from its
/// start. A handle of the context that loaded it.
///
/// [`Context::add_sound`]: crate::Context::add_sound
/// [`Context::play_sound`]: crate::Context::play_sound
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Sound(u32);

/// Music loaded with [`Context::add_music`]: its file's bytes, kept compressed, decoded as a play
/// goes, looping. Layers play one at a time ([`Context::play_music`]). A handle of the context
/// that loaded it.
///
/// [`Context::add_music`]: crate::Context::add_music
/// [`Context::play_music`]: crate::Context::play_music
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Music(u32);

/// A bus that what plays mixes through, with a volume layers set ([`Context::set_volume`]).
///
/// [`Context::set_volume`]: crate::Context::set_volume
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bus {
    /// The music's bus and the sounds'.
    Main,
    /// The music.
    Music,
    /// The sounds ticks queue and layers play.
    Sounds,
}

/// Where a sound plays from. The game's view, the listener, pans and fades it.
///
/// - A view without perspective (a 2D game's) hears where the sound shows in it: panned toward its
///   side, halfway at the view's edge; full volume inside the view, fading to silence a view's
///   width or height past its edges.
/// - A camera with perspective hears direction and distance, like kira's spatial tracks: panned
///   toward its side, three quarters of the way straight out to the side, before or behind; full
///   volume within a unit, fading linearly in decibels to silence 100 units away.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    /// Nowhere in the world: centered, full volume wherever the view looks, like a game's UI
    /// sounds.
    Nowhere,
    /// A point in the world.
    At(DVec3),
    /// An entity: where the latest tick left it as each frame is handed off, following it as it
    /// moves. Once despawned, where it was last, until the sound ends. An entity already gone when
    /// the sound is queued or played means nowhere. That's up to a tick ahead of where the entity
    /// draws, too little to hear.
    On(Entity),
}

/// A [`Place`] as followed: an entity's with where it was last.
#[derive(Clone, Copy, Debug)]
enum Spot {
    Nowhere,
    At(DVec3),
    On(Entity, DVec3),
}

impl Spot {
    /// `place` in `world`: nowhere if its entity is gone.
    fn new(place: Place, world: &World) -> Self {
        match place {
            Place::Nowhere => Self::Nowhere,
            Place::At(at) => Self::At(at),
            Place::On(entity) => {
                position(world, entity).map_or(Self::Nowhere, |at| Self::On(entity, at))
            }
        }
    }

    /// Where it plays from now, `None` for nowhere. Follows its entity while alive, then stays.
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

/// Where `entity` is in `world`, as the latest tick left it, if alive.
fn position(world: &World, entity: Entity) -> Option<DVec3> {
    // Checked first: a pose of a gone entity makes an error, which allocates.
    let pose = world.contains(entity).then(|| world.pose(entity).ok())??;
    Some(pose.matrix.translation)
}

/// The sounds a world's ticks queued ([`World::play_sound`]). A resource, empty between ticks, so
/// no snapshot holds one; no hash, recording or scene reads it.
#[derive(Clone, Default)]
pub(crate) struct Queue(Vec<(Sound, Spot)>);

impl World {
    /// Queues `sound` to play from `place` once the tick has run, in a run that plays sound
    /// ([`Context::add_sound`](crate::Context::add_sound)). What plays never feeds the world: its
    /// hash, a recording and a replay know nothing of it, and each lockstep peer plays what its own
    /// ticks queue. Outside its ticks, a layer plays with
    /// [`Context::play_sound`](crate::Context::play_sound). Once the queue has held as many sounds,
    /// it allocates nothing.
    pub fn play_sound(&mut self, sound: Sound, place: Place) {
        let spot = Spot::new(place, self);
        match self.resource_mut::<Queue>() {
            Some(queue) => queue.0.push((sound, spot)),
            None => {
                self.insert_resource(Queue(vec![(sound, spot)]));
            }
        }
    }

    /// The sounds queued and not yet played: for tests that call a game's ticks directly.
    #[doc(hidden)]
    pub fn sounds_queued(&self) -> impl Iterator<Item = (Sound, Place)> + '_ {
        let queued = self.resource::<Queue>().map_or(&[][..], |queue| &queue.0);
        queued.iter().map(|&(sound, spot)| (sound, spot.place()))
    }
}

type MusicHandle = StreamingSoundHandle<FromFileError>;

/// How the run plays, which what plays follows.
///
/// - Playing.
/// - Paused: holds what plays where it is and starts no sound.
/// - Stopped: stops what plays and starts nothing. The music the layers last had play restarts
///   from its start when the run plays again.
///
/// Only development builds, whose tool API pauses and stops runs, pause or stop it.
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

/// The run's sounds and music, kira's output (cpal's backend, except in tests) and what plays.
pub(crate) struct Audio<B: Backend = CpalBackend> {
    output: Output<B>,
    /// By index: name and data, which each play shares.
    sounds: Vec<(Box<str>, StaticSoundData)>,
    /// By index: name and file as read, which each play decodes.
    music: Vec<(Box<str>, Encoded)>,
    /// With where from, so a stop stops them all.
    playing: Vec<(StaticSoundHandle, Spot)>,
    /// The music layers last had play, with its fade and its play, which is `None` while the
    /// output isn't open or the run is stopped.
    now_playing: Option<(Music, Duration, Option<MusicHandle>)>,
    /// As layers last set them, in [`Bus`]'s order; the output takes them as it opens.
    volumes: [f32; 3],
    /// The latest handed-off view that projects; `None` before any.
    listener: Option<Listener>,
    /// Swapped with a world's queue as it's played, so both keep their memory.
    queued: Vec<(Sound, Spot)>,
    /// Whether the mixer was full as the last sound started, so it warns once as it fills.
    full: bool,
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

/// What hears the sounds ([`Place`]), at an `f64` position that places are taken relative to, so
/// what's near is as precise far from the origin as at it. `View` holds the matrix to clip space;
/// `Ears` the matrix to the camera's own space, looking along -Z, right +X.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Listener {
    View(Mat4, DVec3),
    Ears(Mat4, DVec3),
}

impl Listener {
    /// `None` if `size` has no area or `viewpoint` can't project.
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

/// kira's output.
enum Output<B: Backend> {
    /// The run plays no sound (headless runs, replays and tests), or the output couldn't open.
    Silent,
    /// Not opened yet: the first sound or music loaded opens it.
    Unopened,
    /// Open, its mixer on the backend's audio thread.
    Open(Box<Mixer<B>>),
}

/// An open output: its manager and the music and sounds tracks, which with the main track are the
/// buses.
struct Mixer<B: Backend> {
    manager: AudioManager<B>,
    music: TrackHandle,
    sounds: TrackHandle,
}

impl<B: Backend> Mixer<B> {
    fn settings(volumes: [f32; 3]) -> AudioManagerSettings<B>
    where
        B::Settings: Default,
    {
        AudioManagerSettings {
            main_track_builder: MainTrackBuilder::new().volume(decibels(volumes[0])),
            ..AudioManagerSettings::default()
        }
    }

    /// `manager` (made with [`settings`](Self::settings)) with the music and sounds tracks added.
    /// `volumes` are in [`Bus`]'s order.
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
    /// For a windowed or embedded run, which plays aloud.
    pub(crate) fn aloud() -> Self {
        Self {
            output: Output::Unopened,
            ..Self::default()
        }
    }

    /// Opens the output ([`open`]) if it isn't yet, as the first sound or music is added.
    pub(crate) fn open(&mut self) {
        if matches!(self.output, Output::Unopened) {
            self.output = open(self.volumes);
        }
    }

    /// Pans and fades the sounds playing from a place as the frame being handed off (`viewpoint`,
    /// `size` pixels) hears them, following entities in `world`. Forgets sounds that ended.
    ///
    /// Warns `audio.output_failed` for output errors since (kira reopens the default device) and
    /// `audio.music_failed` for music decoding errors, which stop it. Allocates nothing unless
    /// something went wrong.
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
                // Over kira's default 10 ms: at once would click.
                handle.set_panning(panning, Tween::default());
                handle.set_volume(volume, Tween::default());
            }
            true
        });
    }
}

impl<B: Backend> Audio<B> {
    pub(crate) fn add(&mut self, name: &str, data: StaticSoundData) -> Sound {
        let index = u32::try_from(self.sounds.len()).expect("fewer than 2³² sounds");
        self.sounds.push((name.into(), data));
        Sound(index)
    }

    pub(crate) fn add_music(&mut self, name: &str, encoded: Encoded) -> Music {
        let index = u32::try_from(self.music.len()).expect("fewer than 2³² pieces of music");
        self.music.push((name.into(), encoded));
        Music(index)
    }

    /// Replaces what `sound` holds. Later plays take it; those playing play on unchanged.
    #[cfg(feature = "dev")]
    pub(crate) fn replace(&mut self, sound: Sound, data: StaticSoundData) {
        self.sounds[sound.0 as usize].1 = data;
    }

    /// Replaces what `music` holds. Later plays take it; one playing crossfades to it over
    /// [`RELOAD_FADE`] from where it was, or as far into its loop. While the run's stopped, none
    /// plays, and the next play starts it from its start.
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

    /// Has what plays follow `playback` ([`Playback`]), within kira's 10 ms. Playing after a stop
    /// restarts the layers' music from its start, with the fade it had.
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
        // Paused tracks hold what's on them, and what starts on them, until they resume.
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

    #[cfg(test)]
    pub(crate) fn frames(&self, sound: Sound) -> usize {
        self.sounds[sound.0 as usize].1.num_frames()
    }

    #[cfg(test)]
    pub(crate) fn music_frames(&self, music: Music) -> usize {
        self.music[music.0 as usize].1.frames
    }

    /// Plays `sound` from `place` in `world` now ([`Context::play_sound`]).
    ///
    /// # Panics
    ///
    /// If `sound` is another context's, past those loaded here.
    ///
    /// [`Context::play_sound`]: crate::Context::play_sound
    pub(crate) fn play(&mut self, sound: Sound, place: Place, world: &World) {
        self.start(sound, Spot::new(place, world), world);
    }

    /// Plays what `world`'s ticks queued, emptying its queue. In a run that plays no sound, it
    /// only empties the queue.
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

    /// Like [`play_queued`](Self::play_queued), for a tick rollback reruns: skips one play per
    /// sound in `played`, which the tick's earlier runs played, and adds what it plays. Sounds an
    /// earlier run played that this one doesn't queue have played all the same.
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

    /// Starts `sound` from `spot` if the output's open and the run is playing. Past the 128
    /// sounds kira's mixer plays at once, it's left out, warning `audio.full` as the mixer fills.
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

    /// Plays `music`, or none, in place of the music playing, fading over `fade`
    /// ([`Context::play_music`]). While the run's stopped, it starts at the next play.
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

    pub(crate) fn music_playing(&self) -> Option<Music> {
        self.now_playing.as_ref().map(|&(music, ..)| music)
    }

    /// Starts `music` looping on the music's bus from `at` seconds in (or as far into its loop),
    /// fading in over `fade`, if the output's open. Warns `audio.music_failed` if it can't start.
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
                // `at` in seconds: a reload starts from where it was.
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

    /// Sets `bus`'s volume to `volume` over `fade` ([`Context::set_volume`]).
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

    pub(crate) fn volume(&self, bus: Bus) -> f32 {
        self.volumes[bus as usize]
    }
}

/// Opens kira's output on the default device, its mixer on cpal's audio thread, each bus at its
/// volume in `volumes` ([`Bus`]'s order). Logs `audio.opened` (with its duration) at debug level.
/// If it can't open, warns `audio.no_output` and the run plays no sound.
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

/// A tween over `fade`, at least [`QUICKEST`].
fn tween(fade: Duration) -> Tween {
    Tween {
        duration: fade.max(QUICKEST),
        ..Tween::default()
    }
}

/// `amplitude` in decibels: 1 is 0 dB, 0.5 about -6 dB, floored at kira's silence, -60 dB, which 0
/// or below is.
fn decibels(amplitude: f32) -> Decibels {
    if amplitude > 0.0 {
        Decibels((20.0 * log10f(amplitude)).max(Decibels::SILENCE.0))
    } else {
        Decibels::SILENCE
    }
}

/// How `listener` hears a sound at `at`, by [`Place`]'s rules with [`PAN`], [`FADE`], [`SIDE_PAN`]
/// and [`HEARD`]. From nowhere, or without a listener: centered, full volume. At a point that
/// isn't a number: silent.
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
            // Clip space spans the view from -1 to 1.
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
