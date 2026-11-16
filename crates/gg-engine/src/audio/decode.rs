//! Reading sound and music files: sounds decoded whole, music as it plays.

use std::io::{self, Cursor};
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::Arc;

use gg_core::{Error, Result};
use kira::Frame;
use kira::sound::FromFileError;
use kira::sound::static_sound::{StaticSoundData, StaticSoundSettings};
use kira::sound::streaming::Decoder;
use symphonia::core::audio::conv::{FromSample, IntoSample};
use symphonia::core::audio::sample::Sample;
use symphonia::core::audio::{Audio as _, AudioBuffer, GenericAudioBufferRef};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::AudioDecoder;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::units::Timestamp;

use crate::package::Files;

/// The most bytes a sound's file may hold, as it's decoded whole as it loads: some 90 seconds of
/// CD-quality WAV, more than a game's sounds need; longer audio is music, which streams.
const MAX_SOUND: u64 = 16 << 20;

/// The most frames a sound may decode to, held whole in memory, two f32s each: one for each byte
/// its file may hold, so that every WAV that loads decodes, as none the engine decodes stores a
/// frame in less than a byte, 8-bit mono's; some 6 minutes at 48 kHz, in 128 MiB. A compressed
/// file can hold far more, as a FLAC of silence does, which decodes to hundreds of gigabytes: it's
/// refused as it passes this, before what's past it is held.
pub(super) const MAX_SOUND_FRAMES: usize = MAX_SOUND as usize;

/// The most bytes a music's file may hold, as it's read whole as it loads, as compressed as the
/// file is, and decoded as it plays: over an hour of Ogg Vorbis at 128 kb/s, some 12 minutes of
/// CD-quality FLAC.
const MAX_MUSIC: u64 = 64 << 20;

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
pub(super) fn decode_bytes(
    name: &str,
    path: &Path,
    bytes: Vec<u8>,
    most: usize,
) -> Result<StaticSoundData> {
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
    pub(super) bytes: Arc<[u8]>,
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
pub(super) struct Decoding {
    pub(super) reader: Box<dyn FormatReader>,
    pub(super) codec: Box<dyn AudioDecoder>,
    pub(super) track: u32,
    pub(super) rate: u32,
    /// How many frames the track holds, if its file says.
    pub(super) frames: Option<u64>,
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
pub(super) struct Stream {
    pub(super) decoding: Decoding,
    /// How many frames the track holds, as its file says.
    pub(super) frames: usize,
    /// Whether the decoder has panicked, after which the stream decodes no more.
    pub(super) panicked: bool,
}

impl Stream {
    /// A stream of `bytes`, as [`Decoding::new`] makes it, of a track whose file says how many
    /// frames it holds, as kira's stream needs.
    pub(super) fn new(bytes: Arc<[u8]>) -> Result<Self, FromFileError> {
        let decoding = Decoding::new(bytes)?;
        let frames = decoding.frames.ok_or(FromFileError::UnknownDuration)?;
        let frames = usize::try_from(frames).map_err(|_| FromFileError::UnknownDuration)?;
        Ok(Self {
            decoding,
            frames,
            panicked: false,
        })
    }

    /// Does `decode` to it, on kira's thread, which catches no panic: one of symphonia's on what's
    /// past the file's first packet, which [`read_music`] checked, is the file's doing, as a sound's
    /// is, so it fails as what doesn't decode does, which stops the play and `audio.music_failed`
    /// tells of, rather than ending the thread, which would leave the play silent for good. Once
    /// one has panicked, it fails at once. gg-core's panic hook still logs the panic.
    fn caught<T>(
        &mut self,
        decode: impl FnOnce(&mut Self) -> Result<T, FromFileError>,
    ) -> Result<T, FromFileError> {
        let panicked = || {
            let message = "its decoder panicked on it";
            FromFileError::IoError(io::Error::new(io::ErrorKind::InvalidData, message))
        };
        if self.panicked {
            return Err(panicked());
        }
        panic::catch_unwind(AssertUnwindSafe(|| decode(self))).unwrap_or_else(|_| {
            self.panicked = true;
            Err(panicked())
        })
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
        self.caught(|stream| match stream.decoding.decode()? {
            Some(buffer) => frames(&buffer),
            None => Ok(Vec::new()),
        })
    }

    fn seek(&mut self, index: usize) -> Result<usize, FromFileError> {
        self.caught(|stream| {
            let to = SeekTo::Timestamp {
                ts: Timestamp::new(index as i64),
                track_id: stream.decoding.track,
            };
            let seeked = stream.decoding.reader.seek(SeekMode::Accurate, to)?;
            // As the codec would otherwise run on from what it decoded before the seek.
            stream.decoding.codec.reset();
            let at = usize::try_from(seeked.actual_ts.get()).unwrap_or(0);
            Ok(at.min(stream.frames))
        })
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
