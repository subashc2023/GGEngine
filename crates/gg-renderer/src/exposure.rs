//! Exposure that adapts to what the game's view sees, as eyes adapt between sunlight and shade: the
//! view's light metered each frame, from a histogram of its luminance, in compute passes after it's
//! antialiased and before it blooms, as `shaders/exposure.slang` says; what the meter found read
//! back by the frame two later, as it begins, so that no frame waits for it; and the exposure that
//! frame shades at moved from the frame before's toward what the meter calls for, as [`Adaptation`]
//! says, on the CPU, where every reader of the exposure takes it as it would a fixed one.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::libm::{exp2f, log2f};
use gg_core::{Result, label};
use gg_gpu::{BufferId, ComputePipeline, Device, Frame, Graph, ImageId, PassId};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/exposure.spv"));

/// How many bins the histogram has, and how many a stop of luminance takes, from 2^-12 candela per
/// square meter, the log2 the first starts at: `BINS`, `PER_STOP` and `LOWEST` in
/// `shaders/exposure.slang`; the tests take the last two.
const BINS: u64 = 256;
#[cfg(test)]
const PER_STOP: f32 = 8.0;
#[cfg(test)]
const LOWEST: f32 = -12.0;

/// How many pixels across and down a workgroup of the histogram's takes: `GROUP` in
/// `shaders/exposure.slang`.
const GROUP: u32 = 16;

/// The names of the buffers the meter writes what it found into, which frames read back: of the
/// game's own view, and of a tool's look, whose exposures adapt apart, as
/// [`RenderData::looked`](crate::RenderData::looked) says.
pub(crate) const METERED: [&str; 2] = ["exposure metered", "exposure metered by a look"];

/// The stops between the exposure value, EV100, a reflected-light meter calls for and the log2 of
/// the luminance it meters, in candela per square meter: log2(100 / 12.5), of ISO 100 and a meter
/// calibrated to K = 12.5, as Lagarde and de Rousiers's (2014).
const METER: f32 = 3.0;

/// What lit shading multiplies light by at exposure value `ev100`: 1 over the largest luminance that
/// doesn't saturate a camera of that exposure value, 1.2 × 2^EV100, as Frostbite's and Filament's do
/// (Lagarde and de Rousiers, "Moving Frostbite to Physically Based Rendering", 2014).
pub fn light_scale(ev100: f32) -> f32 {
    1.0 / (1.2 * exp2f(ev100))
}

/// The exposure value at which lit shading multiplies light by `scale`, as [`light_scale`] has it.
fn exposure_value(scale: f32) -> f32 {
    -log2f(1.2 * scale)
}

/// What colors kept from a frame exposed at `then`, as antialiasing's history and the colors
/// reflections trace against are, are multiplied by to be of the light of a frame exposed at
/// `now`, as an adapting exposure changes: `now` over `then`, or 1 where that isn't positive and
/// finite, which takes no colors to any light.
pub(crate) fn rescale(now: f32, then: f32) -> f32 {
    Some(now / then)
        .filter(|scale| scale.is_finite() && *scale > 0.0)
        .unwrap_or(1.0)
}

/// How the game's view's exposure adapts to what it sees, as eyes adapt between sunlight and shade,
/// as [`RenderData::adaptation`](crate::RenderData::adaptation) takes it. Each frame meters the
/// view's light as Unreal's histogram does, from luminances of 2^-12 candela per square meter,
/// darker than starlight, to 2^20, in eighths of a stop, leaving out what's darker, black among it:
/// the mean log2 of the luminance of its pixels between two percentiles of it. A reflected-light
/// meter of ISO 100 calibrated to K = 12.5 calls for an exposure value of log2(luminance × 100 /
/// 12.5) for that mean, which less the compensation, kept between `min` and `max`, is what the
/// exposure moves toward, at speeds of its own up and down. What isn't lit, as sprites, text and
/// unlit materials, shows its colors at any exposure, as ever, and is metered as though it were lit,
/// brighter the darker the exposure.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Adaptation {
    /// The lowest exposure value, EV100, it adapts to, at which what calls for a lower one shows
    /// darker: 0 by default, about a street at night, so that a moonlit world shows as dark as it
    /// is.
    pub min: f32,
    /// The highest, at which what calls for a higher one shows brighter: 18 by default, past snow in
    /// sunlight.
    pub max: f32,
    /// How many stops brighter than the meter calls for the view is exposed, as a camera's exposure
    /// compensation: 1 brightens it twice over, -1 darkens it by half; 0 by default.
    pub compensation: f32,
    /// How fast the exposure value rises toward a brighter view's, in stops a second: 3 by default,
    /// as Unreal's, eyes adapting to light within seconds.
    pub speed_up: f32,
    /// How fast it falls toward a darker view's: 1 by default, as Unreal's, slower, as eyes adapt
    /// to the dark.
    pub speed_down: f32,
    /// The percentile of the view's pixels by their luminance, as a fraction, below which they
    /// aren't metered: 0.1 by default, leaving out the darkest tenth, as Unreal does.
    pub low_percentile: f32,
    /// The percentile above which they aren't: 0.9 by default, leaving out the brightest tenth.
    pub high_percentile: f32,
}

impl Default for Adaptation {
    fn default() -> Self {
        Self {
            min: 0.0,
            max: 18.0,
            compensation: 0.0,
            speed_up: 3.0,
            speed_down: 1.0,
            low_percentile: 0.1,
            high_percentile: 0.9,
        }
    }
}

impl Adaptation {
    /// It as the renderer adapts by it: each field but the speeds finite, or the default's, `max`
    /// no lower than `min`, the speeds no lower than 0, an infinite one moving at once, and the
    /// percentiles from 0 to 1, the high no lower than the low.
    fn bounded(self) -> Self {
        let base = Self::default();
        let finite = |value: f32, or: f32| if value.is_finite() { value } else { or };
        let min = finite(self.min, base.min);
        let low_percentile = finite(self.low_percentile, base.low_percentile).clamp(0.0, 1.0);
        let high_percentile = finite(self.high_percentile, base.high_percentile);
        Self {
            min,
            max: finite(self.max, base.max).max(min),
            compensation: finite(self.compensation, base.compensation),
            // `max` takes the number where the other is NaN.
            speed_up: self.speed_up.max(0.0),
            speed_down: self.speed_down.max(0.0),
            low_percentile,
            high_percentile: high_percentile.clamp(low_percentile, 1.0),
        }
    }

    /// `ev100` kept between `min` and `max`, or `min` for no number.
    fn clamped(&self, ev100: f32) -> f32 {
        if ev100.is_nan() {
            return self.min;
        }
        ev100.clamp(self.min, self.max)
    }

    /// The exposure value the meter calls for, before the compensation, of a view whose metered
    /// pixels' mean log2 luminance is `metered`, within the bounds plus the compensation, so that
    /// less the compensation it's within them.
    fn called(&self, metered: f32) -> f32 {
        let compensation = self.compensation;
        (metered + METER).clamp(self.min + compensation, self.max + compensation)
    }

    /// What's shown of `called`, the exposure value the meter calls for before the compensation:
    /// less the compensation, within the bounds.
    fn shown(&self, called: f32) -> f32 {
        self.clamped(called - self.compensation)
    }
}

/// What every pass pushes: `Push` in `shaders/exposure.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct ExposurePush {
    bins: u64,
    metered: u64,
    size: [u32; 2],
    colors: u32,
    unexposed: f32,
    low: f32,
    high: f32,
}

/// What meters the view: a pipeline for each of its passes.
pub(crate) struct ExposurePass {
    clear: ComputePipeline,
    histogram: ComputePipeline,
    average: ComputePipeline,
}

/// A frame's metering of the view in its graph: the view's colors and their size; the histogram
/// and what the meter found, buffers of the graph's own; and the passes that empty the histogram,
/// count the view's pixels into it, average it and read that back.
pub(crate) struct Plan {
    colors: ImageId,
    size: [u32; 2],
    bins: BufferId,
    metered: BufferId,
    clear: PassId,
    histogram: PassId,
    average: PassId,
    read: PassId,
}

impl ExposurePass {
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        let pipeline = |name, entry| ComputePipeline::new(device, name, SPIRV, entry);
        Ok(Self {
            clear: pipeline("exposure clear", c"clear_main")?,
            histogram: pipeline("exposure histogram", c"histogram_main")?,
            average: pipeline("exposure average", c"average_main")?,
        })
    }

    /// Declares in `graph` the metering of the view's `colors`, of `size` pixels, an image of
    /// colors of high dynamic range that an earlier pass wrote, and what reads back what the meter
    /// found, as [`Frame::buffer_read`] gives it by [`METERED`]'s name for the game's own view, or
    /// a tool's look, if `looked`.
    pub(crate) fn plan(graph: &mut Graph, colors: ImageId, size: [u32; 2], looked: bool) -> Plan {
        let bins = graph.buffer("exposure histogram", BINS * 4);
        let metered = graph.buffer(METERED[usize::from(looked)], 8);
        let clear = (graph.compute(label!("exposure clear")).write_buffer(bins)).add();
        let histogram = (graph.compute(label!("exposure histogram")))
            .sample(colors)
            .write_buffer(bins)
            .add();
        let average = (graph.compute(label!("exposure average")))
            .read_buffer(bins)
            .write_buffer(metered)
            .add();
        let read = graph.read_back(label!("read exposure"), metered);
        Plan {
            colors,
            size,
            bins,
            metered,
            clear,
            histogram,
            average,
            read,
        }
    }

    /// Records `plan`'s metering of a frame that lit shading multiplied light by `scale` in,
    /// between the percentiles `adaptation` gives.
    pub(crate) fn record(
        &self,
        frame: &mut Frame<'_>,
        plan: &Plan,
        scale: f32,
        adaptation: &Adaptation,
    ) -> Result<()> {
        let adaptation = adaptation.bounded();
        let push = |pass: &gg_gpu::Compute<'_, '_>, colors| ExposurePush {
            bins: pass.address(plan.bins),
            metered: 0,
            size: plan.size,
            colors,
            unexposed: 1.0 / scale,
            low: adaptation.low_percentile,
            high: adaptation.high_percentile,
        };
        let mut pass = frame.compute(plan.clear);
        pass.set_pipeline(&self.clear);
        pass.push_constants(bytemuck::bytes_of(&push(&pass, 0)));
        pass.dispatch([1, 1, 1]);
        drop(pass);
        let mut pass = frame.compute(plan.histogram);
        let colors = pass.sampled(plan.colors);
        pass.set_pipeline(&self.histogram);
        pass.push_constants(bytemuck::bytes_of(&push(&pass, colors)));
        let [width, height] = plan.size;
        pass.dispatch([width.div_ceil(GROUP), height.div_ceil(GROUP), 1]);
        drop(pass);
        let mut pass = frame.compute(plan.average);
        let averaged = ExposurePush {
            metered: pass.address(plan.metered),
            ..push(&pass, 0)
        };
        pass.set_pipeline(&self.average);
        pass.push_constants(bytemuck::bytes_of(&averaged));
        pass.dispatch([1, 1, 1]);
        drop(pass);
        frame.read_back(plan.read)
    }
}

/// What the meter found, as a frame reads back what one before it did, by [`METERED`]: that
/// frame's number, and the mean log2 luminance of what it metered, if it metered any of the view.
pub(crate) fn metered((frame, read): (u64, &[u8])) -> Option<(u64, f32)> {
    let (words, _) = read.as_chunks::<4>();
    let [mean, counted] = *words.first_chunk::<2>()?;
    let [mean, counted] = [mean, counted].map(f32::from_ne_bytes);
    (counted > 0.0).then_some((frame, mean))
}

/// What adapts an exposure from frame to frame: the percentiles its frames meter by, bounded, while
/// it adapts, and the number of the first frame that metered by them; the mean log2 luminance a
/// frame metered last, which may be of the percentiles before; whether one metered by these has
/// come back, and whether the exposure has taken one at once since they changed; the exposure value
/// the meter calls for, before the compensation, as it moves at the speeds; and the time of the
/// last frame. The fields the CPU alone reads, the bounds, the compensation and the speeds, aren't
/// kept, but read in each frame, so that a change to them leaves out no metering.
#[derive(Debug, Default)]
pub(crate) struct Adapting {
    percentiles: Option<[f32; 2]>,
    since: u64,
    mean: Option<f32>,
    fresh: bool,
    taken: bool,
    called: f32,
    time: f32,
}

impl Adapting {
    /// Takes the mean log2 luminance that frame `frame` metered, `mean`, if it adapts and the frame
    /// metered by its percentiles.
    pub(crate) fn meter(&mut self, frame: u64, mean: f32) {
        if self.percentiles.is_some() && frame >= self.since {
            self.mean = Some(mean);
            self.fresh = true;
        }
    }

    /// What lit shading multiplies light by in frame `number`, which shows `time`, in seconds:
    /// `scale`, unless `adaptation` adapts it. Then it's the exposure value the meter calls for,
    /// before the compensation, moved toward what the mean last metered calls for, within the
    /// bounds, by at most a speed's stops a second over the time since the frame before, never
    /// back; less the compensation, within the bounds. It starts at `scale`'s exposure value, and
    /// takes the first mean metered by its percentiles at once, as it does again the first after
    /// they change, the means metered by those before left out but the last, which it moves on
    /// toward meanwhile.
    pub(crate) fn step(
        &mut self,
        adaptation: Option<Adaptation>,
        scale: f32,
        number: u64,
        time: f32,
    ) -> f32 {
        // A NaN's max is the number, 0.
        let dt = (time - self.time).max(0.0);
        self.time = time;
        let Some(adaptation) = adaptation.map(Adaptation::bounded) else {
            *self = Self {
                time,
                ..Self::default()
            };
            return scale;
        };
        let compensation = adaptation.compensation;
        let percentiles = [adaptation.low_percentile, adaptation.high_percentile];
        match self.percentiles {
            None => {
                *self = Self {
                    percentiles: Some(percentiles),
                    since: number,
                    called: exposure_value(scale) + compensation,
                    time,
                    ..Self::default()
                };
            }
            Some(before) if before != percentiles => {
                self.percentiles = Some(percentiles);
                self.since = number;
                self.fresh = false;
                self.taken = false;
            }
            Some(_) => {}
        }
        if let Some(mean) = self.mean {
            let target = adaptation.called(mean);
            self.called = if self.fresh && !self.taken {
                self.taken = true;
                target
            } else {
                // None over no time, rather than an infinite speed times it, which is no number.
                let reach = |speed: f32| if dt > 0.0 { speed * dt } else { 0.0 };
                let (down, up) = (reach(adaptation.speed_down), reach(adaptation.speed_up));
                self.called + (target - self.called).clamp(-down, up)
            };
        }
        light_scale(adaptation.shown(self.called))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The meter's log2 luminance in the middle of bin `bin`, as the shader averages a bin's.
    fn middle(bin: u32) -> f32 {
        LOWEST + (bin as f32 + 0.5) / PER_STOP
    }

    /// An adaptation that moves 2 stops a second up and 1 down, between -4 and 20.
    const WIDE: Adaptation = Adaptation {
        min: -4.0,
        max: 20.0,
        compensation: 0.0,
        speed_up: 2.0,
        speed_down: 1.0,
        low_percentile: 0.1,
        high_percentile: 0.9,
    };

    /// The exposure values `adapting` shades frames at under `adaptation`, frame `number` on, a
    /// tenth of a second apart from `time`, each with what the frame two before metered, if
    /// anything: `metered[i]` for frame `number + i - 2`.
    fn shown(
        adapting: &mut Adapting,
        adaptation: Option<Adaptation>,
        number: u64,
        time: f32,
        metered: &[Option<f32>],
    ) -> Vec<f32> {
        let frames: Vec<_> = metered.iter().map(|&mean| (adaptation, mean)).collect();
        shown_each(adapting, number, time, &frames)
    }

    /// As [`shown`], each frame under an adaptation of its own: `frames[i]` its adaptation and
    /// what frame `number + i - 2` metered, if anything.
    fn shown_each(
        adapting: &mut Adapting,
        number: u64,
        time: f32,
        frames: &[(Option<Adaptation>, Option<f32>)],
    ) -> Vec<f32> {
        (frames.iter().enumerate())
            .map(|(i, &(adaptation, metered))| {
                let frame = number + i as u64;
                if let Some(mean) = metered {
                    adapting.meter(frame - 2, mean);
                }
                let at = time + 0.1 * i as f32;
                exposure_value(adapting.step(adaptation, light_scale(15.0), frame, at))
            })
            .collect()
    }

    fn assert_near(got: &[f32], want: &[f32]) {
        assert_eq!(got.len(), want.len(), "{got:?} against {want:?}");
        let near = got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-4);
        assert!(near, "{got:?} against {want:?}");
    }

    /// An exposure value comes back from its scale, as gg-engine's environments' tests check the
    /// scale's steps.
    #[test]
    fn an_exposure_value_comes_back_from_its_scale() {
        for ev100 in [-6.0, 0.0, 7.5, 15.0, 22.0] {
            assert!((exposure_value(light_scale(ev100)) - ev100).abs() < 1e-5);
        }
    }

    /// The meter calls for log2(L × 100 / 12.5): a mean luminance of 4,096, a sunny day's, for
    /// 15, less the compensation, within the bounds.
    #[test]
    fn the_meter_calls_for_its_luminance_less_the_compensation_within_bounds() {
        let settled = |adaptation: Adaptation, mean| adaptation.shown(adaptation.called(mean));
        assert_eq!(settled(WIDE, 12.0), 15.0);
        let brighter = Adaptation {
            compensation: 1.5,
            ..WIDE
        };
        assert_eq!(brighter.called(12.0), 15.0);
        assert_eq!(settled(brighter, 12.0), 13.5);
        assert_eq!(settled(WIDE, 30.0), 20.0);
        assert_eq!(settled(WIDE, -20.0), -4.0);
        assert_eq!(settled(brighter, -5.0), -3.5);
        assert_eq!(settled(brighter, -6.0), -4.0);
        assert_eq!(brighter.called(-20.0), -2.5);
    }

    /// The frames before the first metering comes back shade at the given exposure within the
    /// bounds; that metering's exposure is taken at once; and after it each frame moves toward
    /// what the meter last called for at the speeds, by the time since the frame before, down
    /// slower than up, never past it.
    #[test]
    fn exposure_jumps_to_its_first_metering_then_moves_at_its_speeds() {
        let mut adapting = Adapting::default();
        let narrow = Adaptation { max: 12.0, ..WIDE };
        // Frames 1 and 2 have nothing metered back; 3 has 1's, a luminance of 2^7, for 10.
        let got = shown(
            &mut adapting,
            Some(narrow),
            1,
            0.0,
            &[None, None, Some(7.0)],
        );
        assert_near(&got, &[12.0, 12.0, 10.0]);
        // A darker view, for 6: down a tenth of a stop a frame, 1 a second.
        let got = shown(&mut adapting, Some(narrow), 4, 0.3, &[Some(3.0); 3]);
        assert_near(&got, &[9.9, 9.8, 9.7]);
        // A brighter one, for 11: up two tenths a frame, to it and no further.
        let got = shown(&mut adapting, Some(narrow), 7, 0.6, &[Some(8.0); 9]);
        let want = [9.9, 10.1, 10.3, 10.5, 10.7, 10.9, 11.0, 11.0, 11.0];
        assert_near(&got, &want);
        // A frame that metered nothing, or whose metering didn't come back, keeps the target.
        let got = shown(
            &mut adapting,
            Some(narrow),
            16,
            1.5,
            &[Some(3.0), None, None],
        );
        assert_near(&got, &[10.9, 10.8, 10.7]);
        // Time that stands still, or goes back, moves nothing.
        let got = shown(&mut adapting, Some(narrow), 19, 0.0, &[Some(3.0)]);
        assert_near(&got, &[10.7]);
        let got = shown(&mut adapting, Some(narrow), 20, 0.0, &[Some(3.0)]);
        assert_near(&got, &[10.7]);
    }

    /// The fields the CPU alone reads, the compensation, the bounds and the speeds, take effect in
    /// the frame they change, and the exposure goes on adapting through changes in every frame, as
    /// a script easing the compensation each tick makes: the compensation shifts what's shown at
    /// once, while the exposure the meter calls for moves at its speed; a `min` raised past what's
    /// shown holds it there at once.
    #[test]
    fn fields_the_cpu_alone_reads_apply_at_once_and_keep_it_adapting() {
        let mut adapting = Adapting::default();
        let got = shown(&mut adapting, Some(WIDE), 1, 0.0, &[None, None, Some(7.0)]);
        assert_near(&got, &[15.0, 15.0, 10.0]);
        // A darker view, for 6, while the compensation rises a tenth of a stop each frame.
        let eased: Vec<_> = (1..=6)
            .map(|i| {
                let compensation = 0.1 * i as f32;
                (
                    Some(Adaptation {
                        compensation,
                        ..WIDE
                    }),
                    Some(3.0),
                )
            })
            .collect();
        let got = shown_each(&mut adapting, 4, 0.3, &eased);
        assert_near(&got, &[9.8, 9.6, 9.4, 9.2, 9.0, 8.8]);
        let raised = Adaptation {
            min: 9.5,
            compensation: 0.6,
            ..WIDE
        };
        let got = shown(&mut adapting, Some(raised), 10, 0.9, &[Some(3.0); 2]);
        assert_near(&got, &[9.5, 9.5]);
    }

    /// A change of the percentiles, which the meter's passes read, leaves out what frames before
    /// it metered, the exposure moving on toward what the meter last called for, until a frame
    /// metered by them comes back, which it takes at once. Turned off, the exposure is the given
    /// one; turned on again, it starts afresh.
    #[test]
    fn changed_percentiles_wait_for_a_metering_by_them() {
        let mut adapting = Adapting::default();
        let got = shown(&mut adapting, Some(WIDE), 1, 0.0, &[None, None, Some(7.0)]);
        assert_near(&got, &[15.0, 15.0, 10.0]);
        let got = shown(&mut adapting, Some(WIDE), 4, 0.3, &[Some(3.0); 2]);
        assert_near(&got, &[9.9, 9.8]);
        let narrower = Adaptation {
            low_percentile: 0.3,
            ..WIDE
        };
        // Frame 7 brings frame 5's metering, from before the change at 6, for 4; frame 8 brings
        // 6's, for 8.
        let metered = [Some(3.0), Some(1.0), Some(5.0), Some(5.0)];
        let got = shown(&mut adapting, Some(narrower), 6, 0.5, &metered);
        assert_near(&got, &[9.7, 9.6, 8.0, 8.0]);
        let got = shown(&mut adapting, None, 10, 0.9, &[Some(5.0)]);
        assert_near(&got, &[15.0]);
        let metered = [Some(5.0), Some(5.0), Some(5.0)];
        let got = shown(&mut adapting, Some(WIDE), 11, 1.0, &metered);
        assert_near(&got, &[15.0, 15.0, 8.0]);
    }

    /// Bounds keep an adaptation adapting: NaN and infinite fields their defaults', a `max` below
    /// `min` at `min`, negative speeds at 0, and percentiles within 0 and 1, the high no lower than
    /// the low; and an infinite speed jumps at once.
    #[test]
    fn bounds_keep_an_adaptation_adapting() {
        let wild = Adaptation {
            min: f32::NAN,
            max: -3.0,
            compensation: f32::INFINITY,
            speed_up: -1.0,
            speed_down: f32::NAN,
            low_percentile: 1.5,
            high_percentile: 0.2,
        };
        let bounded = Adaptation {
            min: 0.0,
            max: 0.0,
            compensation: 0.0,
            speed_up: 0.0,
            speed_down: 0.0,
            low_percentile: 1.0,
            high_percentile: 1.0,
        };
        assert_eq!(wild.bounded(), bounded);
        let instant = Adaptation {
            speed_up: f32::INFINITY,
            speed_down: f32::INFINITY,
            ..WIDE
        };
        let mut adapting = Adapting::default();
        let metered = [None, None, Some(7.0), Some(1.0), Some(1.0)];
        let got = shown(&mut adapting, Some(instant), 1, 0.0, &metered);
        assert_near(&got, &[15.0, 15.0, 10.0, 4.0, 4.0]);
        // At a speed of 0 nothing moves after the first.
        let still = Adaptation {
            speed_up: 0.0,
            speed_down: 0.0,
            ..WIDE
        };
        let got = shown(&mut Adapting::default(), Some(still), 1, 0.0, &metered);
        assert_near(&got, &[15.0, 15.0, 10.0, 10.0, 10.0]);
    }

    /// What's read back: nothing for a frame that metered no pixels or read none, and the mean of
    /// one that did, with its number.
    #[test]
    fn a_metering_reads_back_as_its_mean_if_it_counted_any() {
        let bytes = |mean: f32, counted: f32| [mean, counted].map(f32::to_ne_bytes).concat();
        assert_eq!(
            metered((3, &bytes(middle(120), 512.0))),
            Some((3, middle(120)))
        );
        assert_eq!(metered((3, &bytes(0.0, 0.0))), None);
        assert_eq!(metered((3, &[])), None);
        assert_eq!(middle(0), -12.0 + 1.0 / 16.0);
    }
}
