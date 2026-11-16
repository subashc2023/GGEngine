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

/// The most bytes a sound's file may hold: some 90 seconds of CD-quality WAV, more than a game's
/// sounds need. Longer audio is music, which streams.
const MAX_SOUND: u64 = 16 << 20;

/// The most frames a sound may decode to, held at two f32s each: one per byte its file may hold,
/// so every WAV that loads decodes (no format stores a frame in under a byte, 8-bit mono's). Some
/// 6 minutes at 48 kHz, in 128 MiB. A compressed file can decode to far more (a FLAC of silence,
/// hundreds of gigabytes): it's refused on passing this, before the rest is held.
pub(super) const MAX_SOUND_FRAMES: usize = MAX_SOUND as usize;

/// The most bytes a music's file may hold, read whole and kept compressed: over an hour of Ogg
/// Vorbis at 128 kb/s, some 12 minutes of CD-quality FLAC.
const MAX_MUSIC: u64 = 64 << 20;

/// Reads the audio file at `path` from `files`, sound `name`'s, and decodes it whole
/// ([`Context::add_sound`]).
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
    // One byte past the most, so a longer file is refused.
    let bytes = (files.read(path, MAX_SOUND + 1))
        .map_err(|error| bad("can't be read".to_owned()).with_source(error))?;
    if bytes.len() as u64 > MAX_SOUND {
        return Err(bad(format!("is more than {} MiB", MAX_SOUND >> 20)));
    }
    decode_bytes(name, path, bytes, MAX_SOUND_FRAMES)
}

/// Like [`decode`], for the file in `bytes` read from `path`. Decodes to the frames kira's
/// `StaticSoundData::from_cursor` does, but refuses the file on passing `most` frames instead of
/// holding every frame.
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
    // Its text goes in the message, not as a source: kira's error says its own source's.
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
    // symphonia can panic on a malformed file: that's the file's doing, so the sound fails as one
    // that doesn't decode does. gg-core's panic hook still logs it.
    panic::catch_unwind(AssertUnwindSafe(decode)).unwrap_or_else(|_| {
        let problem = "doesn't decode as WAV, Ogg Vorbis, FLAC or MP3: its decoder panicked on it";
        Err(bad(problem.to_owned()))
    })
}

/// A music's file as read: its bytes, which each play decodes, and their frames at their own rate.
pub(crate) struct Encoded {
    pub(super) bytes: Arc<[u8]>,
    pub(crate) frames: usize,
}

/// Reads the audio file at `path`, music `name`'s, whole ([`Context::add_music`]), and checks that
/// a play can decode it: WAV, Ogg Vorbis, FLAC or MP3 that says its rate and length, at least a
/// frame long, of one channel or two, with a first packet that decodes. The rest decodes only as
/// it plays.
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
    // One byte past the most, so a longer file is refused.
    let bytes = (files.read(path, MAX_MUSIC + 1))
        .map_err(|error| bad("can't be read".to_owned()).with_source(error))?;
    if bytes.len() as u64 > MAX_MUSIC {
        return Err(bad(format!("is more than {} MiB", MAX_MUSIC >> 20)));
    }
    let bytes: Arc<[u8]> = bytes.into();
    // In the message, not as a source, like a sound's.
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
    // Like a sound's: a symphonia panic on a malformed file is the file's doing.
    let frames = panic::catch_unwind(AssertUnwindSafe(check)).unwrap_or_else(|_| {
        let problem = "doesn't decode as WAV, Ogg Vorbis, FLAC or MP3: its decoder panicked on it";
        Err(bad(problem.to_owned()))
    })?;
    Ok(Encoded { bytes, frames })
}

/// A file's default audio track read packet by packet, and a codec that decodes each, made as
/// kira's decoders make them. Sounds decode whole through it, and a music's [`Stream`] as a play
/// goes.
pub(super) struct Decoding {
    pub(super) reader: Box<dyn FormatReader>,
    pub(super) codec: Box<dyn AudioDecoder>,
    pub(super) track: u32,
    pub(super) rate: u32,
    /// If its file says.
    pub(super) frames: Option<u64>,
}

impl Decoding {
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

    /// `None` past the last packet.
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

/// Decodes a music's bytes for kira's stream, like kira's own decoder but resetting the codec on
/// a seek. kira 0.12.6's doesn't, so the packet after a seek (as at a loop's end) runs on from
/// those before it, as Vorbis's and MP3's overlap, and every later frame plays a block out of
/// place. Once kira resets it, `StreamingSoundData::from_cursor` can replace this.
pub(super) struct Stream {
    pub(super) decoding: Decoding,
    /// As its file says.
    pub(super) frames: usize,
    /// After a decoder panic the stream decodes no more.
    pub(super) panicked: bool,
}

impl Stream {
    /// A stream of `bytes` ([`Decoding::new`]) whose file says its frames, as kira's stream needs.
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

    /// Runs `decode` on kira's thread, which catches no panic. A symphonia panic past the first
    /// packet ([`read_music`] checked it) is the file's doing: it fails as a file that doesn't
    /// decode, stopping the play with `audio.music_failed`, instead of ending the thread and
    /// leaving the play silent for good. After one, it fails at once. gg-core's panic hook still
    /// logs it.
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
            // Else the codec runs on from what it decoded before the seek.
            stream.decoding.codec.reset();
            let at = usize::try_from(seeked.actual_ts.get()).unwrap_or(0);
            Ok(at.min(stream.frames))
        })
    }
}

/// What `buffer` holds as kira's frames.
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

/// [`frames`] for samples of type `S`: one channel fills both sides of a frame, two fill one each.
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
