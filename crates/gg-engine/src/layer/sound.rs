//! Sounds and music: adding them, playing them, and the buses' volumes.

use std::path::Path;
#[cfg(feature = "dev")]
use std::time::SystemTime;
use std::time::{Duration, Instant};

use gg_core::Result;
use tracing::{debug, info};

use super::Context;
#[cfg(doc)]
use crate::World;
use crate::assets;
use crate::audio::{self, Bus, Music, Place, Sound};
use crate::perf;

impl Context {
    /// Loads the audio file at `path`, WAV, Ogg Vorbis, FLAC or MP3, as a sound for ticks to queue
    /// with [`World::play_sound`], and layers to play with [`play_sound`](Self::play_sound), from
    /// then on, through the handle, decoding it whole into memory, as a game's short sounds are.
    /// `name` names it in errors and the log, and its add logs `asset.sound_loaded`, with how long
    /// it took, at the debug level. A file added again, by whatever form of its path, gives the
    /// sound it gave first, without decoding it again. In a development run that serves
    /// the tool API, the sound decodes again as its file changes, and plays as its file holds from
    /// its next play on; a change that doesn't decode warns, and the sound plays as it did.
    ///
    /// Windowed and embedded runs play sound, as ARCHITECTURE.md's Audio says: the first sound
    /// loaded opens the system's default output device, so that a game without sounds opens none,
    /// and a run that can't open one plays none, warning `audio.no_output`. Headless runs, replays
    /// and tests play none. What plays follows how the run plays, as the tool API's `pause` and
    /// `stop` leave it: paused, it holds where it is, and stopped, it stops.
    ///
    /// Fails with `engine.bad_sound` if the file can't be read, isn't a regular file, is more than
    /// 16 MiB, doesn't decode, or decodes to more than 2²⁴ frames, some 6 minutes at 48 kHz, which
    /// it's refused as it passes: longer audio is music, which [`add_music`](Self::add_music)
    /// streams.
    pub fn add_sound(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Sound> {
        let path = path.as_ref();
        let file = self.files.file_of(path);
        if let Some(&sound) = file.as_ref().and_then(|file| self.loaded.sounds.get(file)) {
            return Ok(sound);
        }
        let started = Instant::now();
        #[cfg(feature = "dev")]
        let read_at = SystemTime::now();
        let data = audio::decode(name, path, &self.files)?;
        // Before the output opens for the first sound, logging `audio.opened`.
        let ms = perf::millis(started.elapsed());
        self.audio.open();
        let sound = self.audio.add(name, data);
        #[cfg(feature = "dev")]
        self.watch_sound(sound, name, path, read_at);
        if let Some(file) = file {
            self.loaded.sounds.insert(file, sound);
        }
        // At the debug level, as a game may add many.
        debug!(
            code = "asset.sound_loaded",
            sound = &*assets::name_of(self.root.as_deref(), path),
            ms,
            "Sound loaded"
        );
        Ok(sound)
    }

    /// Plays `sound` from `place` now, in a run that plays sound, as
    /// [`add_sound`](Self::add_sound) says: as a layer's UI plays its sounds as it updates, panned
    /// and faded as the game's view hears it there, as [`Place`] says, following an entity of the
    /// world it's on. Ticks queue theirs in the world, with [`World::play_sound`]. Past the 128
    /// sounds that play at once, it's left out, warning `audio.full`; and while the run is paused
    /// or stopped, as the tool API's `pause` and `stop` leave it, it isn't played.
    ///
    /// # Panics
    ///
    /// If `sound` is another context's.
    pub fn play_sound(&mut self, sound: Sound, place: Place) {
        self.audio.play(sound, place, &self.world);
    }

    /// Loads the audio file at `path`, WAV, Ogg Vorbis, FLAC or MP3, as music for layers to play
    /// with [`play_music`](Self::play_music) from then on, through the handle: read whole into
    /// memory, as compressed as the file is, and decoded as it plays, on a thread of kira's, so
    /// that a long piece takes little memory. `name` names it in errors and the log, and its add
    /// logs `asset.music_loaded`, with how long it took. A file added again, by whatever form of
    /// its path, gives the music it gave first, without reading it again. In a development run
    /// that serves the tool API, it's read again as its file changes, a play of it crossfading to
    /// what the file holds from where it was; a change that doesn't read warns, and it plays as it
    /// did. It plays as sounds do, as [`add_sound`](Self::add_sound) says, its first loaded opening
    /// the output as a sound would.
    ///
    /// Fails with `engine.bad_music` if the file can't be read, isn't a regular file, is more than
    /// 64 MiB, or isn't WAV, Ogg Vorbis, FLAC or MP3 of one or two channels that says its rate and
    /// length and holds a frame, its first packet decoding. What's past its first packet decodes
    /// only as it plays: a play that fails there warns `audio.music_failed`, and stops.
    pub fn add_music(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Music> {
        let path = path.as_ref();
        let file = self.files.file_of(path);
        if let Some(&music) = file.as_ref().and_then(|file| self.loaded.music.get(file)) {
            return Ok(music);
        }
        let started = Instant::now();
        #[cfg(feature = "dev")]
        let read_at = SystemTime::now();
        let encoded = audio::read_music(name, path, &self.files)?;
        // Before the output opens, as for a sound.
        let ms = perf::millis(started.elapsed());
        self.audio.open();
        let music = self.audio.add_music(name, encoded);
        #[cfg(feature = "dev")]
        self.watch_music(music, name, path, read_at);
        if let Some(file) = file {
            self.loaded.music.insert(file, music);
        }
        info!(
            code = "asset.music_loaded",
            music = &*assets::name_of(self.root.as_deref(), path),
            ms,
            "Music loaded"
        );
        Ok(music)
    }

    /// Plays `music`, looping, in place of the music playing, or `None` for none, in a run that
    /// plays sound, as [`add_music`](Self::add_music) says: the music playing fades out over
    /// `fade` as `music` fades in from its start, on the music's bus, or at once for a fade of
    /// none, which takes kira's 10 ms against a click. Playing the music playing goes on as it
    /// was, so that a layer can say each frame which plays. Ticks don't play music: a layer that
    /// reads the world as it updates plays what the world calls for. While the run is stopped, as
    /// `--stopped` and the tool API's `stop` leave it, it doesn't play; as the run plays, the
    /// music the layers last had play starts from its start, fading in over the fade it was given.
    ///
    /// # Panics
    ///
    /// If `music` is another context's.
    pub fn play_music(&mut self, music: Option<Music>, fade: Duration) {
        self.audio.play_music(music, fade);
    }

    /// The music layers last had play, if any, as [`play_music`](Self::play_music) says: in a run
    /// that plays no sound too, though none's heard.
    pub fn music_playing(&self) -> Option<Music> {
        self.audio.music_playing()
    }

    /// Has `bus`'s volume go to `volume` over `fade`, or at once for a fade of none, as
    /// [`play_music`](Self::play_music)'s fades do: 1 as loud as what plays through it is, 0.5
    /// half as loud, about -6 dB, and 0 or less silent. Each bus starts at 1, and keeps its volume
    /// for the run, in a run that plays no sound too, though none's heard; one set to the volume
    /// it's going to already goes on as it was, so that a layer can say each frame how loud each
    /// is, as a menu that ducks the music under it does.
    ///
    /// # Panics
    ///
    /// If `volume` isn't a finite number.
    pub fn set_volume(&mut self, bus: Bus, volume: f32, fade: Duration) {
        self.audio.set_volume(bus, volume, fade);
    }

    /// `bus`'s volume, as layers last set it with [`set_volume`](Self::set_volume).
    pub fn volume(&self, bus: Bus) -> f32 {
        self.audio.volume(bus)
    }

    /// Plays the sounds the world's ticks queued, emptying its queue, as [`World::play_sound`]
    /// says: once each tick has run.
    pub(crate) fn play_queued(&mut self) {
        self.audio.play_queued(&mut self.world);
    }

    /// Plays the sounds the world's ticks queued, as [`play_queued`](Self::play_queued) does, but
    /// those of `played`, which the tick's runs before played, as a tick rollback runs again has,
    /// adding those it plays to `played`.
    pub(crate) fn play_queued_again(&mut self, played: &mut Vec<Sound>) {
        self.audio.play_queued_again(&mut self.world, played);
    }

    /// Has what plays follow how the run plays, as `stopped` and `paused` say it now does, as
    /// [`Playback`](audio::Playback) says.
    #[cfg(feature = "dev")]
    pub(crate) fn follow_playback(&mut self, stopped: bool, paused: bool) {
        let playback = if stopped {
            audio::Playback::Stopped
        } else if paused {
            audio::Playback::Paused
        } else {
            audio::Playback::Playing
        };
        self.audio.follow(playback);
    }

    /// Nothing: a build without `dev` neither pauses nor stops.
    #[cfg(not(feature = "dev"))]
    pub(crate) fn follow_playback(&mut self, _stopped: bool, _paused: bool) {}
}
