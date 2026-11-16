//! Exposure that adapts to what the game's view sees, as eyes adapt between sunlight and shade.
//! Compute passes meter the view's light each frame from a histogram of its luminance, after
//! antialiasing and before bloom (`shaders/exposure.slang`). The frame two later reads the result
//! back as it begins, so no frame waits. On the CPU, that frame's exposure moves from the last
//! one's toward what the meter calls for, as [`Adaptation`] says, and every reader takes it as a
//! fixed one.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::libm::{exp2f, log2f};
use gg_core::{Result, label};
use gg_gpu::{BufferId, ComputePipeline, Device, Frame, Graph, ImageId, PassId};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/exposure.spv"));

/// The histogram's bins, the bins per stop, and the first bin's log2 luminance (2^-12 cd/m²):
/// `BINS`, `PER_STOP` and `LOWEST` in `shaders/exposure.slang`. Only the tests read the last two.
const BINS: u64 = 256;
#[cfg(test)]
const PER_STOP: f32 = 8.0;
#[cfg(test)]
const LOWEST: f32 = -12.0;

/// Pixels across and down per histogram workgroup: `GROUP` in `shaders/exposure.slang`.
const GROUP: u32 = 16;

/// The names of the buffers frames read the meter back from: the game's view's, and a tool's
/// look's, which adapt apart, as [`RenderData::looked`](crate::RenderData::looked) says.
pub(crate) const METERED: [&str; 2] = ["exposure metered", "exposure metered by a look"];

/// The stops between the EV100 a reflected-light meter calls for and the log2 of the luminance it
/// meters, in cd/m²: log2(100 / 12.5), for ISO 100 and K = 12.5, after Lagarde and de Rousiers
/// (2014).
const METER: f32 = 3.0;

/// What lit shading multiplies light by at exposure value `ev100`: 1 over the largest luminance
/// that doesn't saturate a camera there, 1.2 × 2^EV100, as Frostbite and Filament do (Lagarde and
/// de Rousiers, "Moving Frostbite to Physically Based Rendering", 2014).
pub fn light_scale(ev100: f32) -> f32 {
    1.0 / (1.2 * exp2f(ev100))
}

/// The inverse of [`light_scale`].
fn exposure_value(scale: f32) -> f32 {
    -log2f(1.2 * scale)
}

/// What colors kept from a frame exposed at `then` (antialiasing's history, reflections' colors)
/// are multiplied by to match a frame exposed at `now`: `now` over `then`, or 1 where that isn't
/// positive and finite.
pub(crate) fn rescale(now: f32, then: f32) -> f32 {
    Some(now / then)
        .filter(|scale| scale.is_finite() && *scale > 0.0)
        .unwrap_or(1.0)
}

/// How the game's view's exposure adapts to what it sees, as
/// [`RenderData::adaptation`](crate::RenderData::adaptation) takes it.
///
/// Each frame meters the view's light as Unreal's histogram does: 2^-12 cd/m² (darker than
/// starlight) to 2^20, in eighths of a stop, leaving out what's darker, black included. The meter
/// takes the mean log2 luminance of the pixels between two percentiles. A reflected-light meter of
/// ISO 100 and K = 12.5 calls for an exposure value of log2(luminance × 100 / 12.5). The exposure
/// moves toward that, less the compensation and kept between `min` and `max`, at its own speeds up
/// and down. Unlit things (sprites, text, unlit materials) show their colors at any exposure, and
/// are metered as though lit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Adaptation {
    /// The lowest EV100 it adapts to; what calls for lower shows darker. 0 by default, about a
    /// street at night, so a moonlit world shows as dark as it is.
    pub min: f32,
    /// The highest; what calls for higher shows brighter. 18 by default, past snow in sunlight.
    pub max: f32,
    /// Stops brighter than the meter calls for, as a camera's exposure compensation: 1 is twice as
    /// bright, -1 half. 0 by default.
    pub compensation: f32,
    /// How fast the exposure value rises toward a brighter view's, in stops a second. 3 by default,
    /// as Unreal's: eyes adapt to light within seconds.
    pub speed_up: f32,
    /// How fast it falls toward a darker view's. 1 by default, as Unreal's: eyes adapt to dark
    /// slower.
    pub speed_down: f32,
    /// The luminance percentile, as a fraction, below which pixels aren't metered. 0.1 by default,
    /// leaving out the darkest tenth, as Unreal does.
    pub low_percentile: f32,
    /// The percentile above which they aren't. 0.9 by default.
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
    /// It as the renderer adapts by it: fields but the speeds finite (else the default's), `max` at
    /// least `min`, the speeds at least 0 (an infinite one moves at once), the percentiles within
    /// 0..1, the high at least the low.
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

    /// `ev100` kept between `min` and `max`, or `min` for NaN.
    fn clamped(&self, ev100: f32) -> f32 {
        if ev100.is_nan() {
            return self.min;
        }
        ev100.clamp(self.min, self.max)
    }

    /// The exposure value the meter calls for, before the compensation, for a metered mean log2
    /// luminance `metered`. Kept within the bounds plus the compensation, so less it, it's within
    /// them.
    fn called(&self, metered: f32) -> f32 {
        let compensation = self.compensation;
        (metered + METER).clamp(self.min + compensation, self.max + compensation)
    }

    /// What's shown of `called`: less the compensation, within the bounds.
    fn shown(&self, called: f32) -> f32 {
        self.clamped(called - self.compensation)
    }
}

/// `Push` in `shaders/exposure.slang`, field for field, for every pass.
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

/// Meters the view: a pipeline per pass.
pub(crate) struct ExposurePass {
    clear: ComputePipeline,
    histogram: ComputePipeline,
    average: ComputePipeline,
}

/// A frame's metering in its graph: the view's colors and size, the histogram and result buffers,
/// and the passes that clear the histogram, count pixels into it, average it and read that back.
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

    /// Declares in `graph` the metering of `colors`, an HDR image an earlier pass wrote, and the
    /// readback, which [`Frame::buffer_read`] gives by [`METERED`]'s name for the game's view, or a
    /// tool's look if `looked`.
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

    /// Records `plan`'s metering of a frame whose lit shading multiplied light by `scale`, between
    /// `adaptation`'s percentiles.
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

/// A readback by [`METERED`]: the metering frame's number, and its mean log2 luminance, if it
/// metered any of the view.
pub(crate) fn metered((frame, read): (u64, &[u8])) -> Option<(u64, f32)> {
    let (words, _) = read.as_chunks::<4>();
    let [mean, counted] = *words.first_chunk::<2>()?;
    let [mean, counted] = [mean, counted].map(f32::from_ne_bytes);
    (counted > 0.0).then_some((frame, mean))
}

/// Adapts an exposure from frame to frame. The bounds, compensation and speeds, read by the CPU
/// alone, aren't kept but read each frame, so changing them leaves out no metering.
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
    /// Takes frame `frame`'s mean, if adapting and the frame metered by the current percentiles.
    pub(crate) fn meter(&mut self, frame: u64, mean: f32) {
        if self.percentiles.is_some() && frame >= self.since {
            self.mean = Some(mean);
            self.fresh = true;
        }
    }

    /// What lit shading multiplies light by in frame `number`, at `time` seconds: `scale`, unless
    /// `adaptation` adapts it.
    ///
    /// Then the exposure value called for, before the compensation, moves toward the last mean's,
    /// by at most a speed's stops a second, never past it, and shows less the compensation, within
    /// the bounds. It starts at `scale`'s value. It takes the first mean metered by its percentiles
    /// at once, also after they change; until then it moves toward the last mean of those before.
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
                // No time moves nothing, where an infinite speed times 0 would be NaN.
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

    /// The exposure values `adapting` shades frames at under `adaptation`, from frame `number`, a
    /// tenth of a second apart from `time`. `metered[i]` is what frame `number + i - 2` metered, if
    /// anything.
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

    /// As [`shown`], each frame under its own adaptation: `frames[i]` is its adaptation and what
    /// frame `number + i - 2` metered, if anything.
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

    /// gg-engine's environment tests check the scale's steps.
    #[test]
    fn an_exposure_value_comes_back_from_its_scale() {
        for ev100 in [-6.0, 0.0, 7.5, 15.0, 22.0] {
            assert!((exposure_value(light_scale(ev100)) - ev100).abs() < 1e-5);
        }
    }

    /// log2(L × 100 / 12.5): 15 for a mean luminance of 4,096, a sunny day's.
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

    /// Until then, frames shade at the given exposure within the bounds. After it, down is slower
    /// than up, and never past the target.
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

    /// The bounds, compensation and speeds take effect the frame they change, even when they change
    /// every frame, as a script easing the compensation does. The compensation shifts what's shown
    /// at once; the called-for exposure moves at its speed. A `min` raised past what's shown holds
    /// it there at once.
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

    /// Earlier frames' meterings are left out; meanwhile the exposure moves on toward the last.
    /// Turned off, the exposure is the given one; turned on again, it starts afresh.
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

    /// NaN and infinite fields take the default's, a `max` below `min` is `min`, negative speeds 0,
    /// and percentiles stay within 0..1, the high at least the low. An infinite speed jumps at
    /// once.
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

    /// Nothing for a frame that metered no pixels or read none.
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
