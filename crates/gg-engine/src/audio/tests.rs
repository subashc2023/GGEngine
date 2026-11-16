use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use kira::backend::mock::{MockBackend, MockBackendSettings};
use kira::sound::streaming::Decoder;
use kira::track::TrackPlaybackState;
use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::CodecInfo;
use symphonia::core::codecs::audio::AudioDecoder;
use symphonia::core::codecs::audio::{AudioCodecParameters, FinalizeResult};
use symphonia::core::packet::PacketRef;

use super::decode::{Decoding, MAX_SOUND_FRAMES, decode_bytes};
use super::*;
use crate::Transform;
use crate::package::Files;
use gg_core::math::{Quat, Vec3};

/// A WAV file of `samples`, 16-bit mono at 48 kHz.
pub(crate) fn wav(samples: &[i16]) -> Vec<u8> {
    let data = u32::try_from(samples.len() * 2).expect("a short sound");
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    // Format length; PCM, one channel; frames and bytes a second; bytes a frame, bits a sample.
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

/// How a 2D game's camera hears `at`: 10 units above the origin, seeing 10 units bottom to top of
/// a view 20 across.
fn heard_above(at: Vec3) -> (f32, f32) {
    let camera = Viewpoint {
        position: DVec3::Z * 10.0,
        projection: Projection::Orthographic {
            height: 10.0,
            near: 0.1,
            far: 100.0,
        },
        ..Viewpoint::default()
    };
    let listener = Listener::new(&camera, [200, 100]).expect("a view that projects");
    let (panning, volume) = heard(Some(&listener), Some(at.as_dvec3()));
    (panning.0, volume.0)
}

/// Asserts `(pan, volume)` is `want`'s, to within rounding.
fn near((pan, volume): (f32, f32), (want_pan, want_volume): (f32, f32)) {
    let close = (pan - want_pan).abs() < 1e-5 && (volume - want_volume).abs() < 1e-3;
    assert!(
        close,
        "{pan} and {volume} dB, not {want_pan} and {want_volume} dB"
    );
}

/// In a view without perspective, a sound pans toward its side, halfway at the edge and no
/// further beyond; it's full volume inside the view and fades to silence a view past the edges,
/// across or down. From nowhere or before any view it's centered at full volume; at a point
/// that isn't a number it's silent.
#[test]
fn the_view_pans_and_fades_what_it_hears() {
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
    assert_eq!(heard(None, Some(DVec3::X)), middle);
    let listener = Listener::View(Mat4::IDENTITY, DVec3::ZERO);
    assert_eq!(heard(Some(&listener), None), middle);
}

/// A camera with perspective hears by direction and distance: full volume within a unit, fading
/// linearly in decibels to silence at 100 units and past. Panning is three quarters of the way
/// toward the side straight out to it, less as the sound is more before or behind, equal behind
/// and before, and turns with the camera. A point that isn't a number is silent.
#[test]
fn a_camera_with_perspective_hears_by_direction_and_distance() {
    // At (0, 2, 10), turned a quarter turn left: it looks along -X, its right along -Z.
    let camera = Viewpoint {
        position: DVec3::new(0.0, 2.0, 10.0),
        rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
        projection: Projection::Perspective {
            fov: std::f32::consts::FRAC_PI_2,
            near: 0.1,
        },
    };
    let listener = Listener::new(&camera, [160, 90]).expect("a camera that projects");
    let hear = |at: Vec3| {
        let (panning, volume) = heard(Some(&listener), Some(at.as_dvec3()));
        (panning.0, volume.0)
    };
    let silence = Decibels::SILENCE.0;
    // Half a unit before it, and behind it: full volume, centered.
    near(hear(Vec3::new(-0.5, 2.0, 10.0)), (0.0, 0.0));
    near(hear(Vec3::new(1.0, 2.0, 10.0)), (0.0, 0.0));
    // 34 units right, a third of the way to silence; 100 left.
    near(hear(Vec3::new(0.0, 2.0, -24.0)), (SIDE_PAN, silence / 3.0));
    near(hear(Vec3::new(0.0, 2.0, 110.0)), (-SIDE_PAN, silence));
    near(hear(Vec3::new(0.0, 2.0, 500.0)), (-SIDE_PAN, silence));
    // Half right, before and behind alike: 1/√2 of the way.
    let half = SIDE_PAN * std::f32::consts::FRAC_1_SQRT_2;
    let fading = |distance: f32| silence * (distance - 1.0) / 99.0;
    let before = hear(Vec3::new(-4.0, 2.0, 6.0));
    near(before, (half, fading(32f32.sqrt())));
    near(hear(Vec3::new(4.0, 2.0, 6.0)), before);
    // Straight above it: centered.
    near(hear(Vec3::new(0.0, 12.0, 10.0)), (0.0, fading(10.0)));
    let nan = heard(
        Some(&listener),
        Some(DVec3::new(f64::from(f32::NAN), 0.0, 0.0)),
    );
    assert_eq!(nan, (Panning::CENTER, Decibels::SILENCE));
}

/// 1,000 km from the origin, views with and without perspective hear what's near them as at the
/// origin, though `f32` would place each sound only to the nearest 6 cm.
#[test]
fn far_from_the_origin_what_hears_hears_as_at_it() {
    let far = DVec3::new(1.0e6 + 0.3, -2.0e6, 1.0e6 - 0.3);
    let perspective = Projection::Perspective {
        fov: std::f32::consts::FRAC_PI_2,
        near: 0.1,
    };
    let flat = Projection::Orthographic {
        height: 10.0,
        near: 0.1,
        far: 100.0,
    };
    for projection in [perspective, flat] {
        let camera = Viewpoint {
            position: DVec3::new(0.0, 2.0, 10.0),
            rotation: Quat::from_rotation_y(0.4),
            projection,
        };
        let moved = Viewpoint {
            position: camera.position + far,
            ..camera
        };
        let listener = |camera| Listener::new(&camera, [160, 90]).expect("it projects");
        let [here, there] = [camera, moved].map(listener);
        // Not whole sixteenths from the camera, so `f32`'s rounding of each far position
        // doesn't cancel in their difference.
        for at in [
            DVec3::new(-4.03, 2.0, 6.01),
            DVec3::new(0.27, 2.51, -24.11),
            DVec3::new(3.07, 1.0, 10.53),
        ] {
            let (pan, volume) = heard(Some(&there), Some(at + far));
            let (want_pan, want_volume) = heard(Some(&here), Some(at));
            near((pan.0, volume.0), (want_pan.0, want_volume.0));
        }
    }
}

/// A sound on an entity follows it, and once it's despawned plays from where it was last. One
/// queued on a gone entity plays from nowhere.
#[test]
fn a_sound_on_an_entity_follows_it_and_plays_out_where_it_was_last() {
    let mut world = World::default();
    let at = |x| Transform {
        translation: DVec3::new(x, 2.0, 0.0),
        ..Transform::IDENTITY
    };
    let entity = world.spawn(at(1.0));
    let mut spot = Spot::new(Place::On(entity), &world);
    assert_eq!(spot.follow(&world), Some(DVec3::new(1.0, 2.0, 0.0)));
    *world.get_mut::<Transform>(entity).expect("alive") = at(3.0);
    assert_eq!(spot.follow(&world), Some(DVec3::new(3.0, 2.0, 0.0)));
    world.despawn(entity).expect("alive");
    assert_eq!(spot.follow(&world), Some(DVec3::new(3.0, 2.0, 0.0)));
    assert_eq!(spot.place(), Place::At(DVec3::new(3.0, 2.0, 0.0)));
    let gone = Spot::new(Place::On(entity), &world);
    assert_eq!(gone.place(), Place::Nowhere);
}

/// Playing a world's queued sounds empties the queue, even in a run that plays no sound. The
/// queue and its swap keep their memory, so queuing as many again allocates nothing.
#[test]
fn a_worlds_queue_empties_as_its_sounds_play() {
    let mut audio: Audio = Audio::default();
    let sound = audio.add("chime", silence(480));
    assert_eq!(audio.frames(sound), 480);
    let mut world = World::default();
    audio.play_queued(&mut world);
    for _ in 0..2 {
        world.play_sound(sound, Place::Nowhere);
        world.play_sound(sound, Place::At(DVec3::X));
        let queued: Vec<_> = world.sounds_queued().collect();
        let places = [Place::Nowhere, Place::At(DVec3::X)];
        assert_eq!(queued, places.map(|place| (sound, place)));
        audio.play_queued(&mut world);
        assert_eq!(world.sounds_queued().count(), 0);
    }
    let queue = world.resource::<Queue>().expect("the queue");
    assert!(queue.0.capacity() >= 2 && audio.queued.capacity() >= 2);
}

/// A sound decodes to what kira's own decoding does: frames, rate, settings and slice alike.
/// Covers the crate's test sounds, WAV and Ogg Vorbis (whose first packet decodes to no frames).
#[test]
fn a_sound_decodes_as_kiras_decoding_of_it_does() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/audio");
    let files: Vec<_> = (std::fs::read_dir(&dir).expect("the test sounds"))
        .map(|entry| {
            let path = entry.expect("a sound's entry").path();
            let bytes = std::fs::read(&path).expect("a sound's file");
            (path, bytes)
        })
        .collect();
    assert!(files.len() >= 6, "{} test sounds", files.len());
    for (path, bytes) in files {
        let kiras = StaticSoundData::from_cursor(Cursor::new(bytes.clone()));
        let kiras = kiras.expect("kira decodes it");
        let decoded = decode_bytes("sound", &path, bytes, MAX_SOUND_FRAMES).expect("a sound");
        assert!(decoded.num_frames() > 0, "{}", path.display());
        assert!(decoded == kiras, "{} decodes otherwise", path.display());
    }
}

/// A sound decoding to more frames than a sound holds is refused, naming it; one of exactly that
/// many isn't. A WAV of 2,000 frames, which symphonia reads in packets of 1,152, so the second
/// packet passes the most, 1,999.
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

/// Ogg Vorbis seeked back to the start, as at a loop's end, decodes bit for bit as it did the
/// first time, not running on from before the seek a block out of place.
#[test]
fn music_decodes_alike_after_seeking_back_as_a_loop_does() {
    let bytes = include_bytes!("../../tests/audio/tone.ogg");
    let mut stream = Stream::new(bytes.as_slice().into()).expect("Ogg Vorbis");
    assert_eq!((stream.decoding.rate, stream.frames), (48_000, 24_000));
    let mut passes = Vec::new();
    for _ in 0..2 {
        let start = stream.seek(0).expect("a seek to the start");
        // A packet may decode to no frames (Vorbis's first), so read until the stream's frames
        // are, as kira does.
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

/// A codec that wraps another and counts packets, but panics past the first, as symphonia's can
/// on a malformed one.
struct Panicking(Box<dyn AudioDecoder>, Arc<AtomicU32>);

impl AudioDecoder for Panicking {
    fn reset(&mut self) {
        self.0.reset();
    }

    fn codec_info(&self) -> &CodecInfo {
        self.0.codec_info()
    }

    fn codec_params(&self) -> &AudioCodecParameters {
        self.0.codec_params()
    }

    fn decode_ref(
        &mut self,
        packet: &PacketRef<'_>,
    ) -> symphonia::core::errors::Result<GenericAudioBufferRef<'_>> {
        let decoded = self.1.fetch_add(1, Ordering::Relaxed);
        assert_eq!(decoded, 0, "a packet malformed past the first");
        self.0.decode_ref(packet)
    }

    fn finalize(&mut self) -> FinalizeResult {
        self.0.finalize()
    }

    fn last_decoded(&self) -> GenericAudioBufferRef<'_> {
        self.0.last_decoded()
    }
}

/// Music whose decoder panics past its first packet (which reading checked) fails as a file
/// that doesn't decode, on kira's thread: the play stops and says so, and the panic doesn't end
/// the thread. It then fails at once, without decoding again.
#[test]
fn music_whose_decoder_panics_as_it_plays_fails() {
    let bytes = include_bytes!("../../tests/audio/tone.ogg");
    let stream = Stream::new(bytes.as_slice().into()).expect("Ogg Vorbis");
    let decoded = Arc::new(AtomicU32::new(0));
    let codec = Box::new(Panicking(stream.decoding.codec, decoded.clone()));
    let decoding = Decoding {
        codec,
        ..stream.decoding
    };
    let mut stream = Stream { decoding, ..stream };
    stream.decode().expect("the first packet");
    let error = stream.decode().expect_err("the packet past it");
    assert!(error.to_string().contains("panicked"), "{error}");
    assert!(stream.decode().is_err() && stream.seek(0).is_err());
    assert_eq!(decoded.load(Ordering::Relaxed), 2, "decoded again");
}

/// The music last played plays until another or none; playing it again starts nothing, so a
/// layer can say each frame what plays. Each bus starts at full volume and keeps what a layer
/// last set (below 0 is silence), also in a run that plays no sound.
#[test]
fn music_plays_one_at_a_time_and_buses_keep_their_volumes() {
    let mut audio: Audio = Audio::default();
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

/// Audio on kira's mock output, which plays only as [`render`] renders it, at 12,800 frames a
/// second: 128 frames take 10 ms, kira's quickest fade.
fn mocked() -> Audio<MockBackend> {
    let volumes = [1.0; 3];
    let settings = AudioManagerSettings {
        backend_settings: MockBackendSettings {
            sample_rate: 12_800,
        },
        ..Mixer::settings(volumes)
    };
    let manager = AudioManager::new(settings).expect("a mock output");
    Audio {
        output: Output::Open(Box::new(Mixer::new(manager, volumes))),
        ..Audio::default()
    }
}

/// Renders `audio`'s mock output for 30 ms, as its audio thread would: long enough for commands
/// to take hold, quickest fades to end and stopped plays to go.
fn render(audio: &mut Audio<MockBackend>) {
    let Output::Open(mixer) = &mut audio.output else {
        panic!("a mock output");
    };
    for _ in 0..3 {
        let backend = mixer.manager.backend_mut();
        backend.on_start_processing();
        backend.process();
    }
}

/// Plays on the music's track and the sounds', and whether each track is paused.
fn tracks(audio: &Audio<MockBackend>) -> [(usize, bool); 2] {
    let Output::Open(mixer) = &audio.output else {
        panic!("a mock output");
    };
    [&mixer.music, &mixer.sounds].map(|track| {
        (
            track.num_sounds(),
            track.state() == TrackPlaybackState::Paused,
        )
    })
}

/// What plays follows the run. Stopped, nothing starts, though the layers' music is still theirs.
/// Playing, that music and sounds start. Paused, the tracks hold what plays and no sound starts.
/// Stopped again, what plays stops, and the next play restarts the music from its start.
#[test]
fn what_plays_follows_the_run_as_it_plays_pauses_and_stops() {
    let mut audio = mocked();
    // A second long, a hundred renders.
    let sound = audio.add("chime", silence(48_000));
    let music = audio.add_music(
        "theme",
        Encoded {
            bytes: Arc::from(wav(&vec![0; 4_800])),
            frames: 4_800,
        },
    );
    let world = World::default();
    let sounds = |audio: &mut Audio<MockBackend>| {
        audio.play(sound, Place::Nowhere, &world);
        audio.play(sound, Place::At(DVec3::X), &world);
    };
    audio.follow(Playback::Stopped);
    audio.play_music(Some(music), Duration::from_secs(1));
    sounds(&mut audio);
    render(&mut audio);
    assert_eq!(tracks(&audio), [(0, false), (0, false)], "stopped");
    assert_eq!(audio.music_playing(), Some(music));
    audio.follow(Playback::Playing);
    sounds(&mut audio);
    render(&mut audio);
    assert_eq!(tracks(&audio), [(1, false), (2, false)], "played");
    audio.follow(Playback::Paused);
    sounds(&mut audio);
    render(&mut audio);
    assert_eq!(tracks(&audio), [(1, true), (2, true)], "paused");
    audio.follow(Playback::Playing);
    render(&mut audio);
    assert_eq!(tracks(&audio), [(1, false), (2, false)], "resumed");
    audio.follow(Playback::Stopped);
    render(&mut audio);
    assert_eq!(tracks(&audio), [(0, false), (0, false)], "stopped again");
    assert_eq!(audio.music_playing(), Some(music));
    audio.follow(Playback::Playing);
    render(&mut audio);
    assert_eq!(tracks(&audio), [(1, false), (0, false)], "played again");
    let Some((_, fade, Some(handle))) = &audio.now_playing else {
        panic!("the music plays");
    };
    assert_eq!(*fade, Duration::from_secs(1), "as the layer had it play");
    assert!(
        handle.position() < 0.1,
        "from its start: {} s",
        handle.position()
    );
}

/// A tick rollback reruns plays only what its earlier runs didn't: each sound they played is
/// skipped once, and extra plays are added to them. The queue empties either way.
#[test]
fn a_tick_run_again_plays_only_the_sounds_its_runs_before_didnt() {
    let scratch = crate::dirs::tests::Scratch::new("sounds-again");
    std::fs::create_dir_all(&scratch.0).expect("a directory");
    let path = scratch.0.join("chime.wav");
    std::fs::write(&path, wav(&[0; 480])).expect("a WAV");
    let mut cx = crate::Context::seeded(0);
    let [chime, bell] = ["chime", "bell"].map(|name| cx.add_sound(name, &path).expect("a sound"));
    let mut played = vec![chime];
    for queued in [[chime, chime, bell], [bell, chime, chime]] {
        for sound in queued {
            cx.world_mut().play_sound(sound, Place::Nowhere);
        }
        cx.play_queued_again(&mut played);
        assert_eq!(played, [chime, chime, bell]);
        assert_eq!(cx.world().sounds_queued().count(), 0);
    }
}

/// A music file reads, its frames counted. One that isn't music, holds no audio or is missing is
/// refused, naming the music and its file.
#[test]
fn music_reads_whole_and_refuses_what_isnt_any() {
    let scratch = crate::dirs::tests::Scratch::new("music-read");
    std::fs::create_dir_all(&scratch.0).expect("a directory");
    let tone = scratch.0.join("tone.ogg");
    std::fs::write(&tone, include_bytes!("../../tests/audio/tone.ogg")).expect("a file");
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
