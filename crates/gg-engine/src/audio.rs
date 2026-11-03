//! Sounds and music: what layers load from audio files. Sounds decode whole as they load; ticks
//! queue them in the world and layers play them as they update, each panned and faded by where it
//! shows in the game's view. Music decodes as it plays, which layers play one at a time, looping.
//! Both mix through volume buses in kira, whose mixer runs on cpal's audio thread, as
//! ARCHITECTURE.md's Audio says.

use std::io::Cursor;
use std::mem;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gg_core::math::libm::log10f;
use gg_core::math::{Mat4, Vec3};
use gg_core::{Error, Result};
use gg_renderer::Viewpoint;
use kira::backend::cpal::CpalBackend;
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle, StaticSoundSettings};
use kira::sound::streaming::{Decoder, StreamingSoundData, StreamingSoundHandle};
use kira::sound::{FromFileError, PlaybackState};
use kira::track::{MainTrackBuilder, TrackBuilder, TrackHandle};
use kira::{AudioManager, AudioManagerSettings, Decibels, Frame, Panning, Tween};
use symphonia::core::audio::conv::{FromSample, IntoSample};
use symphonia::core::audio::sample::Sample;
use symphonia::core::audio::{Audio as _, AudioBuffer, GenericAudioBufferRef};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::AudioDecoder;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::units::Timestamp;
use tracing::{debug, warn};

use crate::package::Files;
use crate::{Entity, World};

/// The most bytes a sound's file may hold, as it's decoded whole as it loads: some 90 seconds of
/// CD-quality WAV, more than a game's sounds need; longer audio is music, which streams.
const MAX_SOUND: u64 = 16 << 20;

/// The most frames a sound may decode to, held whole in memory, two f32s each: one for each byte
/// its file may hold, so that every WAV that loads decodes, as none the engine decodes stores a
/// frame in less than a byte, 8-bit mono's; some 6 minutes at 48 kHz, in 128 MiB. A compressed
/// file can hold far more, as a FLAC of silence does, which decodes to hundreds of gigabytes: it's
/// refused as it passes this, before what's past it is held.
const MAX_SOUND_FRAMES: usize = MAX_SOUND as usize;

/// The most bytes a music's file may hold, as it's read whole as it loads, as compressed as the
/// file is, and decoded as it plays: over an hour of Ogg Vorbis at 128 kb/s, some 12 minutes of
/// CD-quality FLAC.
const MAX_MUSIC: u64 = 64 << 20;

/// How far toward a side a sound at that edge of the view pans: halfway, so that the other ear
/// still hears it, as kira pans at constant power, all of it in one ear at a side.
const PAN: f32 = 0.5;

/// How far beyond the view's edges a sound fades out to silence: a view's width or height past
/// them.
const FADE: f32 = 1.0;

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

/// Reads the audio file at `path` from `files`, sound `name`'s, and decodes it whole, as
/// [`Context::add_sound`] says.
///
/// Fails with `engine.bad_sound` if the file can't be read, isn't a regular file, is more than
/// 16 MiB, doesn't decode as WAV, Ogg Vorbis, FLAC or MP3, or decodes to more than
/// [`MAX_SOUND_FRAMES`].
///
/// [`Context::add_sound`]: crate::Context::add_sound
pub(crate) fn decode(name: &str, path: &Path, files: &Files) -> Result<StaticSoundData> {
    let zone = gg_core::zone!();
    zone.text(path.display());
    let bad = |problem: String| {
        let message = format!("sound {name:?} at {} {problem}", path.display());
        Error::new("engine.bad_sound", message)
    };
    // A byte past the most a sound's file holds, so that a longer one is refused.
    let bytes = (files.read(path, MAX_SOUND + 1))
        .map_err(|error| bad("can't be read".to_owned()).with_source(error))?;
    if bytes.len() as u64 > MAX_SOUND {
        return Err(bad(format!("is more than {} MiB", MAX_SOUND >> 20)));
    }
    decode_bytes(name, path, bytes, MAX_SOUND_FRAMES)
}

/// Decodes the audio file in `bytes`, sound `name`'s, read from `path`, whole, as [`decode`] does:
/// to the frames kira's `StaticSoundData::from_cursor` decodes it to, but refusing it as it passes
/// `most` frames, before it holds those past them, where kira's holds every frame a file decodes
/// to.
fn decode_bytes(name: &str, path: &Path, bytes: Vec<u8>, most: usize) -> Result<StaticSoundData> {
    let bad = |problem: String| {
        let message = format!("sound {name:?} at {} {problem}", path.display());
        Error::new("engine.bad_sound", message)
    };
    // Its text in the message, rather than as a source: kira's error says its own source's.
    let undecoded = |error: FromFileError| {
        bad(format!(
            "doesn't decode as WAV, Ogg Vorbis, FLAC or MP3: {error}"
        ))
    };
    let decode = || {
        let mut decoding = Decoding::new(bytes).map_err(undecoded)?;
        let mut decoded = Vec::new();
        while let Some(buffer) = decoding.decode().map_err(undecoded)? {
            if decoded.len() + buffer.frames() > most {
                return Err(bad(format!(
                    "decodes to more than {most} frames, the most a sound holds: longer audio is \
                     music, which streams"
                )));
            }
            decoded.extend(frames(&buffer).map_err(undecoded)?);
        }
        Ok(StaticSoundData {
            sample_rate: decoding.rate,
            frames: decoded.into(),
            settings: StaticSoundSettings::default(),
            slice: None,
        })
    };
    // symphonia can panic on a malformed file, as reading files is, so that's the file's doing: the
    // sound fails as one that doesn't decode does. gg-core's panic hook still logs it.
    panic::catch_unwind(AssertUnwindSafe(decode)).unwrap_or_else(|_| {
        let problem = "doesn't decode as WAV, Ogg Vorbis, FLAC or MP3: its decoder panicked on it";
        Err(bad(problem.to_owned()))
    })
}

/// A music's file as it was read: its bytes, which each play decodes, and how many frames they
/// hold, at their own rate.
pub(crate) struct Encoded {
    bytes: Arc<[u8]>,
    pub(crate) frames: usize,
}

/// Reads the audio file at `path`, music `name`'s, whole, as [`Context::add_music`] says, and checks
/// that a play can decode it: that it's WAV, Ogg Vorbis, FLAC or MP3 of a rate and a length it says,
/// at least a frame long, and that its first packet decodes, of one channel or two. What's past its
/// first packet decodes only as it plays.
///
/// Fails with `engine.bad_music` if the file can't be read, isn't a regular file, is more than
/// 64 MiB, or isn't such audio.
///
/// [`Context::add_music`]: crate::Context::add_music
pub(crate) fn read_music(name: &str, path: &Path, files: &Files) -> Result<Encoded> {
    let zone = gg_core::zone!();
    zone.text(path.display());
    let bad = |problem: String| {
        let message = format!("music {name:?} at {} {problem}", path.display());
        Error::new("engine.bad_music", message)
    };
    // A byte past the most a music's file holds, so that a longer one is refused.
    let bytes = (files.read(path, MAX_MUSIC + 1))
        .map_err(|error| bad("can't be read".to_owned()).with_source(error))?;
    if bytes.len() as u64 > MAX_MUSIC {
        return Err(bad(format!("is more than {} MiB", MAX_MUSIC >> 20)));
    }
    let bytes: Arc<[u8]> = bytes.into();
    // Its text in the message, rather than as a source, as a sound's.
    let undecoded = |error: FromFileError| {
        bad(format!(
            "doesn't decode as WAV, Ogg Vorbis, FLAC or MP3 of one or two channels: {error}"
        ))
    };
    let check = || {
        let mut stream = Stream::new(bytes.clone()).map_err(undecoded)?;
        if stream.frames == 0 {
            return Err(bad("holds no audio".to_owned()));
        }
        stream.decode().map_err(undecoded)?;
        Ok(stream.frames)
    };
    // As a sound's decoding is: a panic of symphonia's on a malformed file is the file's doing.
    let frames = panic::catch_unwind(AssertUnwindSafe(check)).unwrap_or_else(|_| {
        let problem = "doesn't decode as WAV, Ogg Vorbis, FLAC or MP3: its decoder panicked on it";
        Err(bad(problem.to_owned()))
    })?;
    Ok(Encoded { bytes, frames })
}

/// An audio file's default audio track as symphonia reads it, a packet at a time, and a codec that
/// decodes each, as kira's decoders make them: what a sound decodes whole through as it loads, in
/// place of kira's `StaticSoundData::from_cursor`, which holds every frame a file decodes to,
/// however many, and what a music's [`Stream`] decodes through as a play goes.
struct Decoding {
    reader: Box<dyn FormatReader>,
    codec: Box<dyn AudioDecoder>,
    track: u32,
    rate: u32,
    /// How many frames the track holds, if its file says.
    frames: Option<u64>,
}

impl Decoding {
    /// The file in `bytes`, probed for its format and its default audio track, and a codec for it,
    /// as kira's decoders make them.
    fn new(bytes: impl AsRef<[u8]> + Send + Sync + 'static) -> Result<Self, FromFileError> {
        let source = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
        let probe = symphonia::default::get_probe();
        let reader = probe.probe(
            &Hint::default(),
            source,
            Default::default(),
            Default::default(),
        )?;
        let track =
            (reader.default_track(TrackType::Audio)).ok_or(FromFileError::NoDefaultTrack)?;
        let Some(CodecParameters::Audio(parameters)) = &track.codec_params else {
            return Err(FromFileError::NoDefaultTrack);
        };
        let rate = (parameters.sample_rate).ok_or(FromFileError::UnknownSampleRate)?;
        let codecs = symphonia::default::get_codecs();
        let codec = codecs.make_audio_decoder(parameters, &Default::default())?;
        let (track, frames) = (track.id, track.num_frames);
        Ok(Self {
            reader,
            codec,
            track,
            rate,
            frames,
        })
    }

    /// The track's next packet, decoded: `None` past its last.
    fn decode(&mut self) -> Result<Option<GenericAudioBufferRef<'_>>, FromFileError> {
        let packet = loop {
            match self.reader.next_packet()? {
                Some(packet) if packet.track_id == self.track => break packet,
                Some(_) => {}
                None => return Ok(None),
            }
        };
        Ok(Some(self.codec.decode(&packet)?))
    }
}

/// What decodes a music's bytes as a play goes, for kira's stream of it, as kira's own decoder does
/// but that it resets the codec as it seeks: kira 0.12.6's doesn't, so the packet after a seek, as
/// at a loop's end, runs on from those decoded before it, as Vorbis's and MP3's overlap, and every
/// frame after it plays a block out of place. Once kira resets it, its own decoder, through
/// `StreamingSoundData::from_cursor`, does as this does.
struct Stream {
    decoding: Decoding,
    /// How many frames the track holds, as its file says.
    frames: usize,
}

impl Stream {
    /// A stream of `bytes`, as [`Decoding::new`] makes it, of a track whose file says how many
    /// frames it holds, as kira's stream needs.
    fn new(bytes: Arc<[u8]>) -> Result<Self, FromFileError> {
        let decoding = Decoding::new(bytes)?;
        let frames = decoding.frames.ok_or(FromFileError::UnknownDuration)?;
        let frames = usize::try_from(frames).map_err(|_| FromFileError::UnknownDuration)?;
        Ok(Self { decoding, frames })
    }
}

impl Decoder for Stream {
    type Error = FromFileError;

    fn sample_rate(&self) -> u32 {
        self.decoding.rate
    }

    fn num_frames(&self) -> usize {
        self.frames
    }

    fn decode(&mut self) -> Result<Vec<Frame>, FromFileError> {
        match self.decoding.decode()? {
            Some(buffer) => frames(&buffer),
            None => Ok(Vec::new()),
        }
    }

    fn seek(&mut self, index: usize) -> Result<usize, FromFileError> {
        let to = SeekTo::Timestamp {
            ts: Timestamp::new(index as i64),
            track_id: self.decoding.track,
        };
        let seeked = self.decoding.reader.seek(SeekMode::Accurate, to)?;
        // As the codec would otherwise run on from what it decoded before the seek.
        self.decoding.codec.reset();
        let at = usize::try_from(seeked.actual_ts.get()).unwrap_or(0);
        Ok(at.min(self.frames))
    }
}

/// What `buffer` holds as kira's frames, as kira's decoder makes them.
///
/// Fails if it holds more than two channels.
fn frames(buffer: &GenericAudioBufferRef) -> Result<Vec<Frame>, FromFileError> {
    match buffer {
        GenericAudioBufferRef::U8(buffer) => frames_of(buffer),
        GenericAudioBufferRef::U16(buffer) => frames_of(buffer),
        GenericAudioBufferRef::U24(buffer) => frames_of(buffer),
        GenericAudioBufferRef::U32(buffer) => frames_of(buffer),
        GenericAudioBufferRef::S8(buffer) => frames_of(buffer),
        GenericAudioBufferRef::S16(buffer) => frames_of(buffer),
        GenericAudioBufferRef::S24(buffer) => frames_of(buffer),
        GenericAudioBufferRef::S32(buffer) => frames_of(buffer),
        GenericAudioBufferRef::F32(buffer) => frames_of(buffer),
        GenericAudioBufferRef::F64(buffer) => frames_of(buffer),
    }
}

/// [`frames`] for a buffer of samples of type `S`: one channel's in both of a frame's sides, or two
/// channels' each in its own.
fn frames_of<S: Sample>(buffer: &AudioBuffer<S>) -> Result<Vec<Frame>, FromFileError>
where
    f32: FromSample<S>,
{
    let sample = |sample: &S| -> f32 { (*sample).into_sample() };
    match (buffer.num_planes(), buffer.plane(0), buffer.plane(1)) {
        (1, Some(mono), _) => Ok(mono.iter().map(|s| Frame::from_mono(sample(s))).collect()),
        (2, Some(left), Some(right)) => Ok((left.iter().zip(right))
            .map(|(l, r)| Frame::new(sample(l), sample(r)))
            .collect()),
        _ => Err(FromFileError::UnsupportedChannelConfiguration),
    }
}

/// A play of music, as kira's handle to it.
type MusicHandle = StreamingSoundHandle<FromFileError>;

/// The run's sounds and music: those the layers loaded, by their handles, kira's output, which
/// plays them in a run that plays sound, where the sounds playing from a place are, which each
/// frame pans and fades as the game's view hears them, the music playing, and the buses' volumes.
pub(crate) struct Audio {
    output: Output,
    /// Each sound loaded, by its index: its name, and what it holds, which each play shares.
    sounds: Vec<(Box<str>, StaticSoundData)>,
    /// Each music loaded, by its index: its name, and its file as it was read, which each play
    /// decodes.
    music: Vec<(Box<str>, Encoded)>,
    /// The sounds playing from a place, with where.
    playing: Vec<(StaticSoundHandle, Spot)>,
    /// The music layers last had play, if any, with its play, while the output's open and it
    /// started.
    now_playing: Option<(Music, Option<MusicHandle>)>,
    /// Each bus's volume as layers last set it, in [`Bus`]'s order, which the output takes as it
    /// opens.
    volumes: [f32; 3],
    /// The matrix from the world to the clip space of the view the last frame handed off with one
    /// that projects, which hears the sounds; `None` before any.
    listener: Option<Mat4>,
    /// What a world's queue is swapped with as it's played, so that both keep their memory.
    queued: Vec<(Sound, Spot)>,
    /// Whether kira's mixer was full as the last sound started, so that it warns as it fills.
    full: bool,
}

impl Default for Audio {
    fn default() -> Self {
        Self {
            output: Output::default(),
            sounds: Vec::new(),
            music: Vec::new(),
            playing: Vec::new(),
            now_playing: None,
            volumes: [1.0; 3],
            listener: None,
            queued: Vec::new(),
            full: false,
        }
    }
}

/// kira's output, which plays a run's sounds and music.
#[derive(Default)]
enum Output {
    /// None: the run plays no sound, as headless runs, replays and tests don't, or its output
    /// couldn't be opened.
    #[default]
    Silent,
    /// None yet: the run plays sound from its first sound or music loaded, which opens it.
    Unopened,
    /// Open, its mixer on cpal's audio thread.
    Open(Box<Mixer>),
}

/// kira's output, open: its manager, and the tracks under its main track that the music and the
/// sounds play on, the three of them the buses.
struct Mixer {
    manager: AudioManager<CpalBackend>,
    music: TrackHandle,
    sounds: TrackHandle,
}

impl Audio {
    /// A run's sounds, which play aloud, as a windowed or embedded run's do.
    pub(crate) fn aloud() -> Self {
        Self {
            output: Output::Unopened,
            ..Self::default()
        }
    }

    /// Opens the output, as the first sound or music is added to a run that plays sound, as
    /// [`open`] says.
    fn adding(&mut self) {
        if matches!(self.output, Output::Unopened) {
            self.output = open(self.volumes);
        }
    }

    /// Adds `data`, sound `name`'s, opening the output as the first sound or music is added to a
    /// run that plays sound, as [`open`] says.
    pub(crate) fn add(&mut self, name: &str, data: StaticSoundData) -> Sound {
        self.adding();
        let index = u32::try_from(self.sounds.len()).expect("fewer than 2³² sounds");
        self.sounds.push((name.into(), data));
        Sound(index)
    }

    /// Adds `encoded`, music `name`'s, opening the output as [`add`](Self::add) does.
    pub(crate) fn add_music(&mut self, name: &str, encoded: Encoded) -> Music {
        self.adding();
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
    /// playing crossfades to it over [`RELOAD_FADE`], from where it was, or as far into its loop.
    #[cfg(feature = "dev")]
    pub(crate) fn replace_music(&mut self, music: Music, encoded: Encoded) {
        self.music[music.0 as usize].1 = encoded;
        let playing = self.now_playing.as_mut();
        let Some((_, handle)) = playing.filter(|(playing, _)| *playing == music) else {
            return;
        };
        let at = handle.take().map_or(0.0, |mut handle| {
            let at = handle.position();
            handle.stop(tween(RELOAD_FADE));
            at
        });
        let handle = self.start_music(music, at, RELOAD_FADE);
        self.now_playing = Some((music, handle));
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
        let Output::Open(mixer) = &mut self.output else {
            return;
        };
        let (panning, volume) = heard(self.listener.as_ref(), spot.follow(world));
        match mixer.sounds.play(data.panning(panning).volume(volume)) {
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

    /// Plays `music`, or none, in place of the music playing, fading over `fade`, as
    /// [`Context::play_music`] says.
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
        if let Some((_, Some(mut handle))) = self.now_playing.take() {
            handle.stop(tween(fade));
        }
        let started = music.map(|music| (music, self.start_music(music, 0.0, fade)));
        self.now_playing = started;
    }

    /// The music layers last had play, if any.
    pub(crate) fn music_playing(&self) -> Option<Music> {
        self.now_playing.as_ref().map(|&(music, _)| music)
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
        if let Some((music, Some(handle))) = &mut self.now_playing {
            while let Some(error) = handle.pop_error() {
                let name = &*self.music[music.0 as usize].0;
                warn!(
                    code = "audio.music_failed",
                    music = name,
                    "Music {name:?} failed as it played: {error}; it stops"
                );
            }
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

/// Opens kira's output on the system's default output device, its mixer on cpal's audio thread,
/// with a track under its main track for the music and one for the sounds, each bus at its volume
/// in `volumes`, in [`Bus`]'s order; or, if it can't, as on a machine without one, warns
/// `audio.no_output` with why, and the run plays no sound.
fn open(volumes: [f32; 3]) -> Output {
    let _zone = gg_core::zone!("open the sound's output");
    let [main, music, sounds] = volumes.map(decibels);
    let settings = AudioManagerSettings {
        main_track_builder: MainTrackBuilder::new().volume(main),
        ..AudioManagerSettings::default()
    };
    match AudioManager::<CpalBackend>::new(settings) {
        Ok(mut manager) => {
            let mut track = |volume: Decibels| {
                let track = manager.add_sub_track(TrackBuilder::new().volume(volume));
                track.expect("two of the 128 tracks kira's mixer takes")
            };
            let (music, sounds) = (track(music), track(sounds));
            debug!("The sound's output opened");
            Output::Open(Box::new(Mixer {
                manager,
                music,
                sounds,
            }))
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
    let volume = decibels(1.0 - beyond / (2.0 * FADE));
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

    /// A sound decodes to what kira's own decoding of its file does, its frames, its rate, its
    /// settings and its slice alike: the demo's sounds, of WAV, and Ogg Vorbis, whose first packet
    /// decodes to no frames.
    #[test]
    fn a_sound_decodes_as_kiras_decoding_of_it_does() {
        let demo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/shards/sounds");
        let mut files: Vec<_> = (std::fs::read_dir(&demo).expect("the demo's sounds"))
            .map(|entry| {
                let path = entry.expect("a sound's entry").path();
                let bytes = std::fs::read(&path).expect("a sound's file");
                (path, bytes)
            })
            .collect();
        assert!(files.len() >= 5, "{} of the demo's sounds", files.len());
        let tone = include_bytes!("../tests/audio/tone.ogg").to_vec();
        files.push(("tone.ogg".into(), tone));
        for (path, bytes) in files {
            let kiras = StaticSoundData::from_cursor(Cursor::new(bytes.clone()));
            let kiras = kiras.expect("kira decodes it");
            let decoded = decode_bytes("sound", &path, bytes, MAX_SOUND_FRAMES).expect("a sound");
            assert!(decoded.num_frames() > 0, "{}", path.display());
            assert!(decoded == kiras, "{} decodes otherwise", path.display());
        }
    }

    /// A sound that decodes to more frames than a sound holds is refused as it passes them, naming
    /// the sound, which one of as many isn't: a WAV of 2,000 frames, which symphonia reads in
    /// packets of 1,152, so that the second packet passes the most, 1,999.
    #[test]
    fn a_sound_longer_than_a_sound_holds_is_refused() {
        let path = Path::new("long.wav");
        let long = wav(&vec![0; 2_000]);
        let decoded = decode_bytes("long", path, long.clone(), 2_000).expect("as long as it holds");
        assert_eq!(decoded.num_frames(), 2_000);
        let error = decode_bytes("long", path, long, 1_999).expect_err("refused");
        assert_eq!(error.code(), "engine.bad_sound");
        let message = error.to_string();
        let why = "decodes to more than 1999 frames, the most a sound holds";
        let named = message.contains("\"long\"");
        assert!(named && message.contains(why), "{message}");
    }

    /// Ogg Vorbis that a stream seeks back to the start of, as a loop's end does, decodes as it
    /// did from its start, bit for bit, rather than run on from what it decoded before the seek, a
    /// block out of place.
    #[test]
    fn music_decodes_alike_after_seeking_back_as_a_loop_does() {
        let bytes = include_bytes!("../tests/audio/tone.ogg");
        let mut stream = Stream::new(bytes.as_slice().into()).expect("Ogg Vorbis");
        assert_eq!((stream.decoding.rate, stream.frames), (48_000, 24_000));
        let mut passes = Vec::new();
        for _ in 0..2 {
            let start = stream.seek(0).expect("a seek to the start");
            // A packet may decode to no frames, as Vorbis's first does, so packets are read until
            // the stream's frames are, as kira reads them.
            let mut frames = Vec::new();
            for _ in 0..1_000 {
                frames.extend(stream.decode().expect("a packet"));
                if frames.len() >= stream.frames {
                    break;
                }
            }
            assert!(frames.len() >= 24_000, "{} frames", frames.len());
            frames.truncate(24_000);
            passes.push((start, frames));
        }
        assert!(passes[0] == passes[1], "the second pass decodes otherwise");
    }

    /// The music one layer had play last plays, until another or none: playing it again starts
    /// nothing, so that a layer can say each frame what plays. Each bus starts at its full volume,
    /// keeps what a layer last set it to, a number below 0 silence, and a run that plays no sound
    /// keeps them as well.
    #[test]
    fn music_plays_one_at_a_time_and_buses_keep_their_volumes() {
        let mut audio = Audio::default();
        let encoded = |frames| Encoded {
            bytes: Arc::from(wav(&vec![0; frames])),
            frames,
        };
        let [match_music, menu_music] = [encoded(480), encoded(960)].map(|music| {
            let music = audio.add_music("music", music);
            assert_eq!(audio.music_playing(), None);
            music
        });
        assert_eq!(audio.music_frames(menu_music), 960);
        let second = Duration::from_secs(1);
        audio.play_music(Some(match_music), second);
        audio.play_music(Some(match_music), Duration::ZERO);
        assert_eq!(audio.music_playing(), Some(match_music));
        audio.play_music(Some(menu_music), second);
        assert_eq!(audio.music_playing(), Some(menu_music));
        audio.play_music(None, second);
        assert_eq!(audio.music_playing(), None);
        assert_eq!(
            [Bus::Main, Bus::Music, Bus::Sounds].map(|bus| audio.volume(bus)),
            [1.0; 3]
        );
        audio.set_volume(Bus::Music, 0.25, second);
        audio.set_volume(Bus::Sounds, -1.0, second);
        assert_eq!(audio.volume(Bus::Music), 0.25);
        assert_eq!(audio.volume(Bus::Sounds), 0.0);
        assert_eq!(audio.volume(Bus::Main), 1.0);
    }

    /// A file that's music reads, its frames counted, and one that isn't, holds none, or isn't
    /// there is refused, naming the music and its file.
    #[test]
    fn music_reads_whole_and_refuses_what_isnt_any() {
        let scratch = crate::dirs::tests::Scratch::new("music-read");
        std::fs::create_dir_all(&scratch.0).expect("a directory");
        let tone = scratch.0.join("tone.ogg");
        std::fs::write(&tone, include_bytes!("../tests/audio/tone.ogg")).expect("a file");
        assert_eq!(
            read_music("tone", &tone, &Files::default())
                .expect("music")
                .frames,
            24_000
        );
        let empty = wav(&[]);
        let files = [
            ("noise.ogg", &b"OggS, but no more"[..], "doesn't decode"),
            ("empty.wav", &empty, "holds no audio"),
        ];
        for (file, bytes, why) in files {
            std::fs::write(scratch.0.join(file), bytes).expect("a file");
            let Err(error) = read_music("bad", &scratch.0.join(file), &Files::default()) else {
                panic!("{file} reads as music");
            };
            assert_eq!(error.code(), "engine.bad_music");
            let message = format!("{error:#}");
            assert!(message.contains(file) && message.contains(why), "{message}");
        }
        let Err(error) = read_music("none", &scratch.0.join("none.ogg"), &Files::default()) else {
            panic!("a file that isn't there reads as music");
        };
        assert_eq!(error.code(), "engine.bad_music");
    }
}
