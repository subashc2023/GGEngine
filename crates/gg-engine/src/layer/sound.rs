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
    /// Loads the audio file at `path` (WAV, Ogg Vorbis, FLAC or MP3) as a sound, decoded whole
    /// into memory. Ticks queue it with [`World::play_sound`]; layers play it with
    /// [`play_sound`](Self::play_sound).
    ///
    /// `name` names it in errors and the log. The add logs `asset.sound_loaded` at the debug
    /// level, with how long it took. A file added again, by any form of its path, gives the first
    /// sound without decoding again. In a development run that serves the tool API, the sound
    /// decodes again as its file changes and plays the new file from its next play; a change that
    /// doesn't decode warns, and the sound plays as before.
    ///
    /// Windowed and embedded runs play sound, as ARCHITECTURE.md's Audio says. The first sound
    /// loaded opens the default output device, so a game without sounds opens none. A run that
    /// can't open one plays none, warning `audio.no_output`. Headless runs, replays and tests play
    /// none. What plays follows the run: the tool API's `pause` holds it, `stop` stops it.
    ///
    /// Fails with `engine.bad_sound` if the file can't be read, isn't a regular file, is over
    /// 16 MiB, doesn't decode, or decodes to over 2²⁴ frames (some 6 minutes at 48 kHz), which
    /// it's refused as it passes. Longer audio is music: [`add_music`](Self::add_music) streams it.
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

    /// Plays `sound` from `place` now, in a run that plays sound ([`add_sound`](Self::add_sound)),
    /// panned and faded as the game's view hears it there, as [`Place`] says. For a layer's UI;
    /// ticks queue theirs with [`World::play_sound`]. Past 128 sounds at once it's left out,
    /// warning `audio.full`. While the run is paused or stopped it isn't played.
    ///
    /// # Panics
    ///
    /// If `sound` is another context's.
    pub fn play_sound(&mut self, sound: Sound, place: Place) {
        self.audio.play(sound, place, &self.world);
    }

    /// Loads the audio file at `path` (WAV, Ogg Vorbis, FLAC or MP3) as music for layers to play
    /// with [`play_music`](Self::play_music). It's read whole into memory, still compressed, and
    /// decoded as it plays on a kira thread, so a long piece takes little memory.
    ///
    /// `name` names it in errors and the log. The add logs `asset.music_loaded`, with how long it
    /// took. A file added again, by any form of its path, gives the first music without reading
    /// again. In a development run that serves the tool API, it's read again as its file changes;
    /// a play of it crossfades to the new file from where it was. A change that doesn't read
    /// warns, and it plays as before. It plays as sounds do ([`add_sound`](Self::add_sound)); the
    /// first one loaded opens the output.
    ///
    /// Fails with `engine.bad_music` if the file can't be read, isn't a regular file, is over
    /// 64 MiB, or isn't WAV, Ogg Vorbis, FLAC or MP3 of one or two channels that says its rate and
    /// length and holds a frame, its first packet decoding. Later packets decode only as it plays:
    /// a play that fails there warns `audio.music_failed` and stops.
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

    /// Plays `music` looping in place of the music playing, or `None` for none, in a run that
    /// plays sound ([`add_music`](Self::add_music)). The old music fades out over `fade` as
    /// `music` fades in from its start, on the music's bus. A zero fade takes kira's 10 ms, against
    /// a click. Playing the music already playing changes nothing, so a layer can say each frame
    /// which plays. Ticks don't play music: a layer that reads the world as it updates plays what
    /// it calls for. While the run is stopped (`--stopped`, the tool API's `stop`) nothing plays;
    /// when it plays, the music the layers last had play starts from its start, fading in over
    /// its given fade.
    ///
    /// # Panics
    ///
    /// If `music` is another context's.
    pub fn play_music(&mut self, music: Option<Music>, fade: Duration) {
        self.audio.play_music(music, fade);
    }

    /// The music layers last had play ([`play_music`](Self::play_music)), also in a run that plays
    /// no sound.
    pub fn music_playing(&self) -> Option<Music> {
        self.audio.music_playing()
    }

    /// Fades `bus`'s volume to `volume` over `fade`, as [`play_music`](Self::play_music)'s fades
    /// do: 1 is as loud as what plays through it, 0.5 half as loud (about -6 dB), 0 or less
    /// silent. Each bus starts at 1 and keeps its volume for the run, also in a run that plays no
    /// sound. Setting the volume it's going to changes nothing, so a layer can say each frame how
    /// loud each is, as a menu that ducks the music does.
    ///
    /// # Panics
    ///
    /// If `volume` isn't a finite number.
    pub fn set_volume(&mut self, bus: Bus, volume: f32, fade: Duration) {
        self.audio.set_volume(bus, volume, fade);
    }

    /// `bus`'s volume as last set with [`set_volume`](Self::set_volume).
    pub fn volume(&self, bus: Bus) -> f32 {
        self.audio.volume(bus)
    }

    /// Plays and empties the sounds the world's ticks queued ([`World::play_sound`]), once each
    /// tick has run.
    pub(crate) fn play_queued(&mut self) {
        self.audio.play_queued(&mut self.world);
    }

    /// Like [`play_queued`](Self::play_queued), but skips those in `played`, which earlier runs of
    /// a tick that rollback reruns already played, and adds the ones it plays to `played`.
    pub(crate) fn play_queued_again(&mut self, played: &mut Vec<Sound>) {
        self.audio.play_queued_again(&mut self.world, played);
    }

    /// Has what plays follow `stopped` and `paused`, as [`Playback`](audio::Playback) says.
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

    /// Does nothing: a build without `dev` neither pauses nor stops.
    #[cfg(not(feature = "dev"))]
    pub(crate) fn follow_playback(&mut self, _stopped: bool, _paused: bool) {}
}
