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

/// Whether `(pan, volume)` is `want`'s, to within rounding.
fn near((pan, volume): (f32, f32), (want_pan, want_volume): (f32, f32)) {
    let close = (pan - want_pan).abs() < 1e-5 && (volume - want_volume).abs() < 1e-3;
    assert!(
        close,
        "{pan} and {volume} dB, not {want_pan} and {want_volume} dB"
    );
}

/// A sound pans toward the side of a view without perspective it shows on, halfway at its edge
/// and no further beyond, as loud as it is inside the view, and fades out beyond the view's
/// edges, across or down, to silence a view past them; from nowhere, or before any view, it's
/// in the middle, as loud as it is; and at a point that isn't a number, it's silent.
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

/// A camera with perspective hears a sound by its direction and distance: as loud as it is
/// within a unit, before or behind, fading linearly in decibels to silence 100 units away and
/// past; panned three quarters of the way to the side it's straight out to, less as it's more
/// before or behind, and as much behind as before; turned with the camera; and silent at a
/// point that isn't a number.
#[test]
fn a_camera_with_perspective_hears_by_direction_and_distance() {
    // At (0, 2, 10), turned a quarter turn to its left, so that it looks along -X, its right
    // along -Z.
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
    // Before it, half a unit, and behind it, as loud as it is, in the middle.
    near(hear(Vec3::new(-0.5, 2.0, 10.0)), (0.0, 0.0));
    near(hear(Vec3::new(1.0, 2.0, 10.0)), (0.0, 0.0));
    // To its right, 34 units, a third of the way to silence; to its left, 100.
    near(hear(Vec3::new(0.0, 2.0, -24.0)), (SIDE_PAN, silence / 3.0));
    near(hear(Vec3::new(0.0, 2.0, 110.0)), (-SIDE_PAN, silence));
    near(hear(Vec3::new(0.0, 2.0, 500.0)), (-SIDE_PAN, silence));
    // Half right, before and behind alike: 1/√2 of the way to the side.
    let half = SIDE_PAN * std::f32::consts::FRAC_1_SQRT_2;
    let fading = |distance: f32| silence * (distance - 1.0) / 99.0;
    let before = hear(Vec3::new(-4.0, 2.0, 6.0));
    near(before, (half, fading(32f32.sqrt())));
    near(hear(Vec3::new(4.0, 2.0, 6.0)), before);
    // Straight above it: in the middle.
    near(hear(Vec3::new(0.0, 12.0, 10.0)), (0.0, fading(10.0)));
    let nan = heard(
        Some(&listener),
        Some(DVec3::new(f64::from(f32::NAN), 0.0, 0.0)),
    );
    assert_eq!(nan, (Panning::CENTER, Decibels::SILENCE));
}

/// 1,000 km from the origin, a view with perspective and one without hear what's near them as
/// they do at the origin, where `f32` would place each sound to the nearest 6 cm.
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
        // Apart from the camera by other than whole sixteenths, so that `f32`'s rounding of
        // each far position wouldn't cancel in their difference.
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

/// A sound on an entity follows it as it moves, and once it's despawned, plays from where it
/// was last; one queued on an entity that's gone plays from nowhere.
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

/// A world's ticks queue sounds, each with where it plays from, which playing them empties,
/// though a run that plays no sound plays none of them; the queue and what it's swapped with
/// keep their memory, so that queuing as many again allocates nothing.
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

/// A sound decodes to what kira's own decoding of its file does, its frames, its rate, its
/// settings and its slice alike: the crate's test sounds, of WAV, and Ogg Vorbis, whose first
/// packet decodes to no frames.
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
    let bytes = include_bytes!("../../tests/audio/tone.ogg");
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

/// A codec that decodes as the one it wraps does, counting each packet, but panics at those
/// past the first, as symphonia's can at a malformed one.
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

/// Music whose decoder panics past its first packet, which reading it checked, fails as what
/// doesn't decode does, on kira's thread, which then stops its play and tells of it, rather than
/// the panic ending the thread; and from then on fails at once, without decoding again.
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

/// The music one layer had play last plays, until another or none: playing it again starts
/// nothing, so that a layer can say each frame what plays. Each bus starts at its full volume,
/// keeps what a layer last set it to, a number below 0 silence, and a run that plays no sound
/// keeps them as well.
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

/// Audio whose output is kira's mock, which plays only as [`render`] renders it, at 12,800
/// frames a second, so that each of its renders of 128 frames takes 10 ms, kira's quickest
/// fade.
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

/// Renders `audio`'s mock output for 30 ms, as its audio thread would: long enough for what
/// it was told to take hold, its quickest fades to end and what stopped to go.
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

/// The plays on the music's track and on the sounds', and whether each track holds them
/// paused.
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

/// What plays follows how the run plays: stopped, nothing starts, though the music the layers
/// had play is still theirs; played, that music starts, and sounds do; paused, the tracks hold
/// what plays, and no sound starts; and stopped again, what plays stops, the next play starting
/// the music again from its start.
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

/// A tick that rollback runs again plays only what its runs before didn't: a sound they played,
/// once for each time, isn't played again, and those it plays besides are added to them; and the
/// world's queue is emptied either way.
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

/// A file that's music reads, its frames counted, and one that isn't, holds none, or isn't
/// there is refused, naming the music and its file.
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
