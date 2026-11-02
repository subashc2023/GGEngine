//! Sounds: what layers load from audio files, decoded whole as they load, which ticks queue in the
//! world and layers play as they update, each panned and faded by where it shows in the game's
//! view, through kira, whose mixer runs on cpal's audio thread, as ARCHITECTURE.md's Audio says.

use std::io::Cursor;
use std::mem;
use std::path::Path;

use gg_core::math::libm::log10f;
use gg_core::math::{Mat4, Vec3};
use gg_core::{Error, Result};
use gg_renderer::Viewpoint;
use kira::backend::cpal::CpalBackend;
use kira::sound::PlaybackState;
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::{AudioManager, AudioManagerSettings, Decibels, Panning, Tween};
use tracing::{debug, warn};

use crate::{Entity, World, dirs};

/// The most bytes a sound's file may hold, as it's decoded whole as it loads: some 90 seconds of
/// CD-quality WAV, more than a game's sounds need; longer audio, as music, would stream.
const MAX_FILE: u64 = 16 << 20;

/// How far toward a side a sound at that edge of the view pans: halfway, so that the other ear
/// still hears it, as kira pans at constant power, all of it in one ear at a side.
const PAN: f32 = 0.5;

/// How far beyond the view's edges a sound fades out to silence: a view's width or height past
/// them.
const FADE: f32 = 1.0;

/// A sound a layer loaded with [`Context::add_sound`], whole in memory, which ticks queue with
/// [`World::play_sound`] and layers play with [`Context::play_sound`]: as often as they like, many
/// at once, each from its start. A handle, of the context that loaded it.
///
/// [`Context::add_sound`]: crate::Context::add_sound
/// [`Context::play_sound`]: crate::Context::play_sound
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Sound(u32);

/// Where a sound plays from, which pans and fades it by where it shows in the game's view, the
/// listener: toward the side of the view it's on, halfway there at the view's edge, and as loud as
/// it is while it's in the view, fading out beyond the view's edges to silence a view's width or
/// height past them. A camera with perspective hears nothing behind it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    /// Nowhere in the world: in the middle, as loud as it is, wherever the view looks, as a game's
    /// UI's sounds play.
    Nowhere,
    /// At a point in the world, where it stays.
    At(Vec3),
    /// On an entity: where the latest tick left it as each frame is handed off, following it as it
    /// moves, and once it's despawned, where it was last, until the sound ends. One that's gone as
    /// the sound is queued or played plays from nowhere.
    On(Entity),
}

/// Where a sound plays from, as the engine follows it: a [`Place`], an entity's with where it was
/// last.
#[derive(Clone, Copy, Debug)]
enum Spot {
    Nowhere,
    At(Vec3),
    On(Entity, Vec3),
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
    fn follow(&mut self, world: &World) -> Option<Vec3> {
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
fn position(world: &World, entity: Entity) -> Option<Vec3> {
    // Checked first, as a pose of an entity that's gone makes an error, which allocates.
    let pose = world.contains(entity).then(|| world.pose(entity).ok())??;
    Some(pose.matrix.w_axis.truncate())
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

/// Reads the audio file at `path`, sound `name`'s, and decodes it whole, as
/// [`Context::add_sound`] says.
///
/// Fails with `engine.bad_sound` if the file can't be read, isn't a regular file, is more than
/// 16 MiB, or doesn't decode as WAV, Ogg Vorbis, FLAC or MP3.
///
/// [`Context::add_sound`]: crate::Context::add_sound
pub(crate) fn decode(name: &str, path: &Path) -> Result<StaticSoundData> {
    let zone = gg_core::zone!();
    zone.text(path.display());
    let bad = |problem: String| {
        let message = format!("sound {name:?} at {} {problem}", path.display());
        Error::new("engine.bad_sound", message)
    };
    // A byte past the most a sound's file holds, so that a longer one is refused.
    let bytes = dirs::read_file(path, MAX_FILE + 1)
        .map_err(|error| bad("can't be read".to_owned()).with_source(error))?;
    if bytes.len() as u64 > MAX_FILE {
        return Err(bad(format!("is more than {} MiB", MAX_FILE >> 20)));
    }
    // Its text in the message, rather than as a source: kira's error says its own source's.
    StaticSoundData::from_cursor(Cursor::new(bytes)).map_err(|error| {
        bad(format!(
            "doesn't decode as WAV, Ogg Vorbis, FLAC or MP3: {error}"
        ))
    })
}

/// The run's sounds: those the layers loaded, by their handles, kira's output, which plays them in
/// a run that plays sound, and where those playing from a place are, which each frame pans and
/// fades as the game's view hears them.
#[derive(Default)]
pub(crate) struct Audio {
    output: Output,
    /// Each sound loaded, by its index: its name, and what it holds, which each play shares.
    sounds: Vec<(Box<str>, StaticSoundData)>,
    /// The sounds playing from a place, with where.
    playing: Vec<(StaticSoundHandle, Spot)>,
    /// The matrix from the world to the clip space of the view the last frame handed off with one
    /// that projects, which hears the sounds; `None` before any.
    listener: Option<Mat4>,
    /// What a world's queue is swapped with as it's played, so that both keep their memory.
    queued: Vec<(Sound, Spot)>,
    /// Whether kira's mixer was full as the last sound started, so that it warns as it fills.
    full: bool,
}

/// kira's output, which plays a run's sounds.
#[derive(Default)]
enum Output {
    /// None: the run plays no sound, as headless runs, replays and tests don't, or its output
    /// couldn't be opened.
    #[default]
    Silent,
    /// None yet: the run plays sound from its first sound loaded, which opens it.
    Unopened,
    /// Open, its mixer on cpal's audio thread.
    Open(Box<AudioManager<CpalBackend>>),
}

impl Audio {
    /// A run's sounds, which play aloud, as a windowed or embedded run's do.
    pub(crate) fn aloud() -> Self {
        Self {
            output: Output::Unopened,
            ..Self::default()
        }
    }

    /// Adds `data`, sound `name`'s, opening the output as the first is added to a run that plays
    /// sound, as [`open`] says.
    pub(crate) fn add(&mut self, name: &str, data: StaticSoundData) -> Sound {
        if matches!(self.output, Output::Unopened) {
            self.output = open();
        }
        let index = u32::try_from(self.sounds.len()).expect("fewer than 2³² sounds");
        self.sounds.push((name.into(), data));
        Sound(index)
    }

    /// Puts `data` in place of what `sound` holds, which its plays take from then on; those playing
    /// play on as they were.
    #[cfg(feature = "dev")]
    pub(crate) fn replace(&mut self, sound: Sound, data: StaticSoundData) {
        self.sounds[sound.0 as usize].1 = data;
    }

    /// How many frames `sound` holds, at its own rate.
    #[cfg(test)]
    pub(crate) fn frames(&self, sound: Sound) -> usize {
        self.sounds[sound.0 as usize].1.num_frames()
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

    /// Starts `sound` from `spot`, following it in `world`, panned and faded as the listener hears
    /// it there, if the output's open: past the 128 sounds kira's mixer plays at once, it's left
    /// out, warning `audio.full` as the mixer fills.
    fn start(&mut self, sound: Sound, mut spot: Spot, world: &World) {
        let loaded = self.sounds.len();
        let Some((name, data)) = self.sounds.get(sound.0 as usize) else {
            panic!(
                "sound {} is another context's, of the {loaded} loaded here",
                sound.0
            );
        };
        let Output::Open(output) = &mut self.output else {
            return;
        };
        let (panning, volume) = heard(self.listener.as_ref(), spot.follow(world));
        match output.play(data.panning(panning).volume(volume)) {
            Ok(handle) => {
                self.full = false;
                debug!(sound = &**name, "Sound started");
                if !matches!(spot, Spot::Nowhere) {
                    self.playing.push((handle, spot));
                }
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

    /// Has the sounds playing from a place pan and fade as the view of the frame being handed off
    /// hears them, seeing from `viewpoint`, `size` pixels across and down, while it has an area,
    /// following each on an entity of `world` that's alive to where the latest tick left it, as
    /// [`Place::On`] says; and forgets those that have ended. Warns `audio.output_failed` of what
    /// went wrong with the output since, which kira opens again on the system's default output
    /// device, as it does as that changes. It allocates nothing unless something went wrong.
    pub(crate) fn hear_from(&mut self, viewpoint: &Viewpoint, size: [u32; 2], world: &World) {
        let Output::Open(output) = &mut self.output else {
            return;
        };
        while let Some(error) = output.backend_mut().pop_error() {
            warn!(
                code = "audio.output_failed",
                "The sound's output failed: {error}; it opens again on the default output device"
            );
        }
        let [width, height] = size.map(|side| side as f32);
        if width > 0.0 && height > 0.0 && viewpoint.projection.projects() {
            self.listener = Some(viewpoint.view_projection(width / height));
        }
        let listener = self.listener.as_ref();
        self.playing.retain_mut(|(handle, spot)| {
            if handle.state() == PlaybackState::Stopped {
                return false;
            }
            let (panning, volume) = heard(listener, spot.follow(world));
            // Over kira's default 10 ms, rather than at once, which would click.
            handle.set_panning(panning, Tween::default());
            handle.set_volume(volume, Tween::default());
            true
        });
    }
}

/// Opens kira's output on the system's default output device, its mixer on cpal's audio thread;
/// or, if it can't, as on a machine without one, warns `audio.no_output` with why, and the run
/// plays no sound.
fn open() -> Output {
    let _zone = gg_core::zone!("open the sound's output");
    match AudioManager::<CpalBackend>::new(AudioManagerSettings::default()) {
        Ok(output) => {
            debug!("The sound's output opened");
            Output::Open(Box::new(output))
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

/// How a sound at `at` is heard through the view whose matrix from the world to its clip space is
/// `listener`: panned by how far across the view it shows, [`PAN`] of the way toward a side at its
/// edge, and as loud as it is inside the view, fading out beyond its edges to silence [`FADE`]
/// views past them. In the middle and as loud as it is from nowhere, or without a listener; silent
/// behind a camera with perspective, which shows nothing there, and at a point that isn't a number.
fn heard(listener: Option<&Mat4>, at: Option<Vec3>) -> (Panning, Decibels) {
    let (Some(listener), Some(at)) = (listener, at) else {
        return (Panning::CENTER, Decibels::IDENTITY);
    };
    let clip = *listener * at.extend(1.0);
    let [x, y] = [clip.x / clip.w, clip.y / clip.w];
    if !(clip.w > 0.0 && x.is_finite() && y.is_finite()) {
        return (Panning::CENTER, Decibels::SILENCE);
    }
    // Clip space spans the view from -1 to 1, across and down.
    let beyond = (x.abs().max(y.abs()) - 1.0).max(0.0);
    let amplitude = 1.0 - beyond / (2.0 * FADE);
    let volume = if amplitude > 0.0 {
        Decibels((20.0 * log10f(amplitude)).max(Decibels::SILENCE.0))
    } else {
        Decibels::SILENCE
    };
    (Panning(x.clamp(-1.0, 1.0) * PAN), volume)
}

#[cfg(test)]
pub(crate) mod tests {
    use gg_renderer::Projection;

    use super::*;
    use crate::Transform;

    /// A WAV file of `samples`, 16-bit mono at 48 kHz.
    pub(crate) fn wav(samples: &[i16]) -> Vec<u8> {
        let data = u32::try_from(samples.len() * 2).expect("a short sound");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        // The format's length; PCM, of one channel; its frames a second, and its bytes; a frame's
        // bytes, and a sample's bits.
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&[1, 0, 1, 0]);
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&96_000u32.to_le_bytes());
        bytes.extend_from_slice(&[2, 0, 16, 0]);
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes
    }

    /// A sound of `frames` frames of silence, decoded.
    fn silence(frames: usize) -> StaticSoundData {
        let decoded = StaticSoundData::from_cursor(Cursor::new(wav(&vec![0; frames])));
        decoded.expect("a WAV decodes")
    }

    /// How a 2D game's camera hears a point at `at`: from 10 units above the origin, seeing 10
    /// units from bottom to top of a view twice as wide, 20 across.
    fn heard_above(at: Vec3) -> (f32, f32) {
        let camera = Viewpoint {
            position: Vec3::Z * 10.0,
            projection: Projection::Orthographic {
                height: 10.0,
                near: 0.1,
                far: 100.0,
            },
            ..Viewpoint::default()
        };
        let (panning, volume) = heard(Some(&camera.view_projection(2.0)), Some(at));
        (panning.0, volume.0)
    }

    /// A sound pans toward the side of the view it shows on, halfway at its edge and no further
    /// beyond, as loud as it is inside the view, and fades out beyond the view's edges, across or
    /// down, to silence a view past them; from nowhere, or before any view, it's in the middle, as
    /// loud as it is; and behind a camera with perspective, or at a point that isn't a number, it's
    /// silent.
    #[test]
    fn the_view_pans_and_fades_what_it_hears() {
        let near = |(pan, volume): (f32, f32), (want_pan, want_volume): (f32, f32)| {
            let close = (pan - want_pan).abs() < 1e-5 && (volume - want_volume).abs() < 1e-3;
            assert!(
                close,
                "{pan} and {volume} dB, not {want_pan} and {want_volume} dB"
            );
        };
        near(heard_above(Vec3::ZERO), (0.0, 0.0));
        near(heard_above(Vec3::new(10.0, 0.0, 0.0)), (PAN, 0.0));
        near(heard_above(Vec3::new(-5.0, 4.0, 0.0)), (-PAN / 2.0, 0.0));
        // Half a view's width past its right edge: half as loud, -6 dB.
        near(heard_above(Vec3::new(20.0, 0.0, 0.0)), (PAN, -6.0206));
        // A quarter of its height below its bottom edge.
        near(heard_above(Vec3::new(0.0, -7.5, 0.0)), (0.0, -2.4988));
        let silence = Decibels::SILENCE.0;
        near(heard_above(Vec3::new(-30.0, 0.0, 0.0)), (-PAN, silence));
        near(heard_above(Vec3::new(0.0, 0.0, f32::NAN)), (0.0, silence));
        let middle = (Panning::CENTER, Decibels::IDENTITY);
        assert_eq!(heard(None, Some(Vec3::X)), middle);
        assert_eq!(heard(Some(&Mat4::IDENTITY), None), middle);
        let camera = Viewpoint {
            position: Vec3::Z * 10.0,
            projection: Projection::Perspective {
                fov: std::f32::consts::FRAC_PI_2,
                near: 0.1,
            },
            ..Viewpoint::default()
        };
        let listener = camera.view_projection(1.0);
        assert_eq!(
            heard(Some(&listener), Some(Vec3::ZERO)).1,
            Decibels::IDENTITY
        );
        let behind = heard(Some(&listener), Some(Vec3::Z * 20.0));
        assert_eq!(behind, (Panning::CENTER, Decibels::SILENCE));
    }

    /// A sound on an entity follows it as it moves, and once it's despawned, plays from where it
    /// was last; one queued on an entity that's gone plays from nowhere.
    #[test]
    fn a_sound_on_an_entity_follows_it_and_plays_out_where_it_was_last() {
        let mut world = World::default();
        let at = |x| Transform {
            translation: Vec3::new(x, 2.0, 0.0),
            ..Transform::IDENTITY
        };
        let entity = world.spawn(at(1.0));
        let mut spot = Spot::new(Place::On(entity), &world);
        assert_eq!(spot.follow(&world), Some(Vec3::new(1.0, 2.0, 0.0)));
        *world.get_mut::<Transform>(entity).expect("alive") = at(3.0);
        assert_eq!(spot.follow(&world), Some(Vec3::new(3.0, 2.0, 0.0)));
        world.despawn(entity).expect("alive");
        assert_eq!(spot.follow(&world), Some(Vec3::new(3.0, 2.0, 0.0)));
        assert_eq!(spot.place(), Place::At(Vec3::new(3.0, 2.0, 0.0)));
        let gone = Spot::new(Place::On(entity), &world);
        assert_eq!(gone.place(), Place::Nowhere);
    }

    /// A world's ticks queue sounds, each with where it plays from, which playing them empties,
    /// though a run that plays no sound plays none of them; the queue and what it's swapped with
    /// keep their memory, so that queuing as many again allocates nothing.
    #[test]
    fn a_worlds_queue_empties_as_its_sounds_play() {
        let mut audio = Audio::default();
        let sound = audio.add("chime", silence(480));
        assert_eq!(audio.frames(sound), 480);
        let mut world = World::default();
        audio.play_queued(&mut world);
        for _ in 0..2 {
            world.play_sound(sound, Place::Nowhere);
            world.play_sound(sound, Place::At(Vec3::X));
            let queued: Vec<_> = world.sounds_queued().collect();
            let places = [Place::Nowhere, Place::At(Vec3::X)];
            assert_eq!(queued, places.map(|place| (sound, place)));
            audio.play_queued(&mut world);
            assert_eq!(world.sounds_queued().count(), 0);
        }
        let queue = world.resource::<Queue>().expect("the queue");
        assert!(queue.0.capacity() >= 2 && audio.queued.capacity() >= 2);
    }
}
