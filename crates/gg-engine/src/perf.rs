//! The engine's own frame timing. [`PerfPanel`] shows it in development builds, and each run that
//! doesn't end in an error sums it up in its `engine.perf` event. Always on: a few clock reads a
//! frame.

use std::time::Duration;

pub(crate) use gg_core::log::millis;

use crate::Layer;
use crate::render_thread::Rendered;
#[cfg(feature = "dev")]
use crate::{Context, Ui};

/// Frames the percentiles cover: the last 16,384, 68 s at 240 fps. Means and maxima cover the
/// whole run. They take 1,280 KiB, from the first frame measured.
const KEPT: usize = 16_384;

/// The fewest frames the graph shows, at a pixel each: bars are as many whole pixels wide as still
/// leave room for that many.
#[cfg(feature = "dev")]
const GRAPHED: usize = 240;

/// One frame in nanoseconds: its main-thread time, from its start to the next frame's, in parts,
/// and the render it took back (the frame before's), which ran beside it on the render thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Sample {
    pub(crate) frame: u64,
    /// Ticks, each with the world copy drawing takes.
    pub(crate) ticks: u64,
    /// Layer updates and drawing the world, between ticks.
    pub(crate) update: u64,
    /// Layers' and egui's UI, with tessellation.
    pub(crate) ui: u64,
    /// Waiting for the render thread to finish the frame before.
    pub(crate) handoff: u64,
    /// Waiting for the next frame to be due under
    /// [`Context::set_max_fps`](crate::Context::set_max_fps).
    pub(crate) limit: u64,
    /// Rendering the frame before, with submission and present, without its waits.
    pub(crate) render: u64,
    /// The render's waits for the GPU and for the display to release an image.
    pub(crate) wait: u64,
    /// Of the render, copying the game's view to the client it's published to.
    pub(crate) publish: u64,
    /// GPU time, first pass's start to last's end, of the frame two before; 0 where none came back.
    pub(crate) gpu: u64,
    /// Instanced draws in the game's view.
    pub(crate) draws: u64,
    pub(crate) instances: u64,
    pub(crate) triangles: u64,
}

impl Sample {
    /// The frame minus its wait for the cap: what happens between frames (torn-off windows,
    /// gamepad polling, OS events).
    #[cfg(any(feature = "dev", test))]
    pub(crate) fn other(&self) -> u64 {
        let parts = self.ticks + self.update + self.ui + self.handoff + self.limit;
        self.frame.saturating_sub(parts)
    }

    /// Sets each field to `f` of its and `other`'s.
    fn merge(&mut self, other: &Sample, f: impl Fn(u64, u64) -> u64) {
        self.frame = f(self.frame, other.frame);
        self.ticks = f(self.ticks, other.ticks);
        self.update = f(self.update, other.update);
        self.ui = f(self.ui, other.ui);
        self.handoff = f(self.handoff, other.handoff);
        self.limit = f(self.limit, other.limit);
        self.render = f(self.render, other.render);
        self.wait = f(self.wait, other.wait);
        self.publish = f(self.publish, other.publish);
        self.gpu = f(self.gpu, other.gpu);
        self.draws = f(self.draws, other.draws);
        self.instances = f(self.instances, other.instances);
        self.triangles = f(self.triangles, other.triangles);
    }
}

/// A run's frames so far. The main loop reports each frame's start and parts; a frame is measured
/// once the next starts or the run ends, with the render it took back. A frame that took none back
/// (the first) isn't measured. The first that did is kept apart, since its render made the
/// pipelines and uploaded the first meshes and textures.
#[derive(Default)]
pub(crate) struct Perf {
    /// The frames measured, the oldest overwritten once there are `KEPT`.
    kept: Vec<Sample>,
    /// Where the next frame goes once `kept` is full: its oldest.
    next: usize,
    /// The frame under way.
    pending: Option<Sample>,
    /// The first frame's time.
    first: Option<u64>,
    measured: u64,
    /// Of those, the frames whose GPU time came back.
    gpu_measured: u64,
    sum: Sample,
    max: Sample,
}

impl Perf {
    /// A frame starts `dt` after the last, which is then measured. A zero `dt` (the first frame's,
    /// or the first after a pause) drops the last frame instead: its time would count the pause.
    pub(crate) fn start_frame(&mut self, dt: Duration) {
        self.end(dt);
        self.pending = Some(Sample::default());
    }

    pub(crate) fn ran(&mut self, ticks: Duration, update: Duration, ui: Duration) {
        if let Some(sample) = &mut self.pending {
            sample.ticks = nanos(ticks);
            sample.update = nanos(update);
            sample.ui = nanos(ui);
        }
    }

    /// Records tessellation, the wait `handoff` for the render thread, and that render. A frame
    /// with no render back, or one that presented nothing or failed, isn't measured.
    pub(crate) fn rendered(
        &mut self,
        tessellate: Duration,
        handoff: Duration,
        rendered: Option<&Rendered>,
    ) {
        let Some(rendered) = rendered.filter(|rendered| matches!(rendered.presented, Ok(true)))
        else {
            self.pending = None;
            return;
        };
        if let Some(sample) = &mut self.pending {
            sample.ui += nanos(tessellate);
            sample.handoff = nanos(handoff);
            sample.render = nanos(rendered.render.saturating_sub(rendered.wait));
            sample.wait = nanos(rendered.wait);
            sample.publish = nanos(rendered.publish);
            sample.gpu = rendered.gpu.map_or(0, nanos);
            sample.draws = rendered.drawn.draws as u64;
            sample.instances = rendered.drawn.instances as u64;
            sample.triangles = rendered.drawn.triangles as u64;
        }
    }

    pub(crate) fn limited(&mut self, limit: Duration) {
        if let Some(sample) = &mut self.pending {
            sample.limit = nanos(limit);
        }
    }

    /// The run ends `last` after the last frame started, which is measured.
    pub(crate) fn finish(&mut self, last: Duration) {
        self.end(last);
    }

    /// Measures the frame under way, which took `time`, unless zero.
    fn end(&mut self, time: Duration) {
        let Some(mut sample) = self.pending.take() else {
            return;
        };
        if time.is_zero() {
            return;
        }
        sample.frame = nanos(time);
        if self.first.is_none() {
            self.first = Some(sample.frame);
            return;
        }
        if self.kept.len() < KEPT {
            self.kept.reserve_exact(KEPT - self.kept.len());
            self.kept.push(sample);
        } else {
            self.kept[self.next] = sample;
            self.next = (self.next + 1) % KEPT;
        }
        self.measured += 1;
        self.gpu_measured += u64::from(sample.gpu > 0);
        self.sum.merge(&sample, |a, b| a + b);
        self.max.merge(&sample, u64::max);
    }

    /// Kept frames, newest first.
    #[cfg(any(feature = "dev", test))]
    pub(crate) fn recent(&self) -> impl Iterator<Item = &Sample> {
        let (newer, older) = self.kept.split_at(self.next);
        newer.iter().rev().chain(older.iter().rev())
    }

    /// The run summed up, in milliseconds.
    pub(crate) fn summary(&self) -> Summary {
        let stats = |field: fn(&Sample) -> u64| {
            let kept = self.kept.iter().map(field).collect();
            Stats::of(kept, field(&self.sum), self.measured, field(&self.max))
        };
        // Frames with GPU time only.
        let gpu = {
            let kept = self.kept.iter().map(|s| s.gpu).filter(|&gpu| gpu > 0);
            Stats::of(
                kept.collect(),
                self.sum.gpu,
                self.gpu_measured,
                self.max.gpu,
            )
        };
        Summary {
            measured: self.measured,
            percentile_frames: self.kept.len(),
            first_frame: self.first.map(|first| ms(first as f64)),
            frame: stats(|s| s.frame),
            ticks: stats(|s| s.ticks),
            update: stats(|s| s.update),
            ui: stats(|s| s.ui),
            handoff: stats(|s| s.handoff),
            limit: stats(|s| s.limit),
            render: stats(|s| s.render),
            wait: stats(|s| s.wait),
            publish: stats(|s| s.publish),
            gpu,
            draws_max: self.max.draws,
            instances_max: self.max.instances,
            triangles_max: self.max.triangles,
        }
    }
}

/// A run summed up in milliseconds, as its `engine.perf` event logs it; `None` where no frame was
/// measured.
#[derive(Debug, PartialEq)]
pub(crate) struct Summary {
    /// The frames measured: what means and maxima cover.
    pub(crate) measured: u64,
    /// What percentiles cover: the last of those, up to `KEPT`.
    pub(crate) percentile_frames: usize,
    pub(crate) first_frame: Option<f64>,
    pub(crate) frame: Stats,
    pub(crate) ticks: Stats,
    pub(crate) update: Stats,
    pub(crate) ui: Stats,
    pub(crate) handoff: Stats,
    pub(crate) limit: Stats,
    pub(crate) render: Stats,
    pub(crate) wait: Stats,
    pub(crate) publish: Stats,
    /// Over the frames whose GPU time came back alone.
    pub(crate) gpu: Stats,
    pub(crate) draws_max: u64,
    pub(crate) instances_max: u64,
    pub(crate) triangles_max: u64,
}

/// One part of the frames, in milliseconds.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Stats {
    pub(crate) p50: Option<f64>,
    pub(crate) p95: Option<f64>,
    pub(crate) p99: Option<f64>,
    pub(crate) mean: Option<f64>,
    pub(crate) max: Option<f64>,
}

impl Stats {
    /// Percentiles of `kept` (nanoseconds), and the mean of `count` frames totaling `sum`, longest
    /// `max`. All `None` if no frame was measured.
    fn of(mut kept: Vec<u64>, sum: u64, count: u64, max: u64) -> Self {
        if count == 0 || kept.is_empty() {
            return Self::default();
        }
        kept.sort_unstable();
        // Nearest rank: the smallest at or above `percent` of them.
        let at = |percent: usize| {
            let rank = (percent * kept.len()).div_ceil(100);
            Some(ms(kept[rank - 1] as f64))
        };
        Self {
            p50: at(50),
            p95: at(95),
            p99: at(99),
            mean: Some(ms(sum as f64 / count as f64)),
            max: Some(ms(max as f64)),
        }
    }
}

/// `nanos` in milliseconds, rounded to the microsecond.
fn ms(nanos: f64) -> f64 {
    (nanos / 1e3).round() / 1e3
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

/// The Performance panel: `.overlay(gg_engine::PerfPanel)`. Shows the last second's frame time
/// and rate, a graph of recent frames by part, and where the second went. Does nothing without
/// `dev` (no UI).
pub struct PerfPanel;

#[cfg(feature = "dev")]
impl Layer for PerfPanel {
    fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
        let perf = &cx.perf;
        ui.panel("Performance", |ui| {
            // A new panel floats smaller than its contents.
            egui::ScrollArea::vertical().show(ui, |ui| show(perf, ui));
        });
    }
}

#[cfg(not(feature = "dev"))]
impl Layer for PerfPanel {}

/// Reads a part's time from a sample.
#[cfg(feature = "dev")]
type Time = fn(&Sample) -> u64;

/// A part of a frame the panel shows: short name (times beside it must fit a narrow panel),
/// tooltip, reading, and graph color on light and dark backgrounds.
#[cfg(feature = "dev")]
struct Part {
    name: &'static str,
    tip: &'static str,
    time: Time,
    light: egui::Color32,
    dark: egui::Color32,
}

#[cfg(feature = "dev")]
impl Part {
    fn color(&self, visuals: &egui::Visuals) -> egui::Color32 {
        if visuals.dark_mode {
            self.dark
        } else {
            self.light
        }
    }
}

/// The main thread's parts, stacked from the bottom: work, then the wait in gray. Neighbors'
/// colors stay distinct under protanopia and deuteranopia on egui's light and dark backgrounds,
/// hence UI between Ticks and Update (UI's orange beside Other's amber isn't). Recheck on adding
/// a part.
#[cfg(feature = "dev")]
const PARTS: [Part; 5] = [
    Part {
        name: "Ticks",
        tip: "The simulation's ticks, on the main thread",
        time: |s| s.ticks,
        light: egui::Color32::from_rgb(0x1b, 0xaf, 0x7a),
        dark: egui::Color32::from_rgb(0x19, 0x9e, 0x70),
    },
    Part {
        name: "UI",
        tip: "The layers' UI and egui's, and tessellating it, on the main thread",
        time: |s| s.ui,
        light: egui::Color32::from_rgb(0xeb, 0x68, 0x34),
        dark: egui::Color32::from_rgb(0xd9, 0x59, 0x26),
    },
    Part {
        name: "Update",
        tip: "The layers' updates and drawing the world, on the main thread",
        time: |s| s.update,
        light: egui::Color32::from_rgb(0x2a, 0x78, 0xd6),
        dark: egui::Color32::from_rgb(0x39, 0x87, 0xe5),
    },
    Part {
        name: "Other",
        tip: "The rest of the main thread's frame: the OS's events, the gamepads, torn-off windows",
        time: Sample::other,
        light: egui::Color32::from_rgb(0xed, 0xa1, 0x00),
        dark: egui::Color32::from_rgb(0xc9, 0x85, 0x00),
    },
    Part {
        name: "Handoff",
        tip: "The main thread waiting for the render thread to finish the frame before",
        time: |s| s.handoff,
        light: egui::Color32::from_rgb(0xc3, 0xc2, 0xb7),
        dark: egui::Color32::from_rgb(0x38, 0x38, 0x35),
    },
];

/// The render thread's time on the frame before and the GPU's on the frame two before. The panel
/// lists them; the graph doesn't stack them, as they overlap the main thread's parts.
#[cfg(feature = "dev")]
const RENDERING: [(&str, &str, Time); 3] = [
    (
        "Render",
        "The render thread rendering the frame before, but its waits",
        |s| s.render,
    ),
    (
        "Render wait",
        "The render thread waiting for the GPU and the display",
        |s| s.wait,
    ),
    (
        "GPU",
        "The GPU's time on the frame's passes, two frames before",
        |s| s.gpu,
    ),
];

#[cfg(feature = "dev")]
const FRAME_TIP: &str = "The whole frame on the main thread, from its start to the next's";

#[cfg(feature = "dev")]
const LIMIT_TIP: &str = "The main thread waiting for the next frame to be due under the frame rate limit, which the \
     graph leaves empty";

#[cfg(feature = "dev")]
#[derive(Clone, Copy, Default)]
struct Total {
    sum: u64,
    max: u64,
}

#[cfg(feature = "dev")]
impl Total {
    fn add(&mut self, time: u64) {
        self.sum += time;
        self.max = self.max.max(time);
    }
}

#[cfg(feature = "dev")]
fn show(perf: &Perf, ui: &mut egui::Ui) {
    let Some(last) = perf.recent().next() else {
        ui.label("No frames measured yet");
        return;
    };
    // The text covers the last second, steadier than the last frame at high rates; the graph
    // shows each frame.
    let mut frames = 0;
    let mut frame = Total::default();
    let mut parts = [Total::default(); PARTS.len()];
    let mut limit = Total::default();
    let mut rendering = [Total::default(); RENDERING.len()];
    for sample in perf.recent() {
        if frame.sum >= 1_000_000_000 {
            break;
        }
        frames += 1;
        frame.add(sample.frame);
        for (total, part) in parts.iter_mut().zip(&PARTS) {
            total.add((part.time)(sample));
        }
        limit.add(sample.limit);
        for (total, (.., time)) in rendering.iter_mut().zip(&RENDERING) {
            total.add(time(sample));
        }
    }
    let mean_ms = frame.sum as f64 / frames as f64 / 1e6;
    ui.label(format!(
        "{mean_ms:.2} ms a frame, {:.0} fps, {}",
        1000.0 / mean_ms,
        draws_text(last)
    ));
    graph(perf, ui);
    egui::Grid::new("performance parts")
        .striped(true)
        .show(ui, |ui| {
            ui.label("");
            ui.label(format!("{frames} frames"))
                .on_hover_text("The frames of the last second");
            ui.label("Mean");
            ui.label("Max");
            ui.end_row();
            let row = |ui: &mut egui::Ui, color, (name, tip): (&str, &str), total: Total| {
                row_name(ui, color, name).on_hover_text(tip);
                ui.label(millis_text(total.sum as f64 / frames as f64));
                ui.label(millis_text(total.max as f64));
                ui.end_row();
            };
            row(ui, None, ("Frame", FRAME_TIP), frame);
            for (part, total) in PARTS.iter().zip(parts) {
                let color = part.color(ui.visuals());
                row(ui, Some(color), (part.name, part.tip), total);
            }
            // While frames are capped.
            if limit.max > 0 {
                row(ui, None, ("Limit", LIMIT_TIP), limit);
            }
            for (&(name, tip, _), total) in RENDERING.iter().zip(rendering) {
                row(ui, None, (name, tip), total);
            }
        });
}

/// Recent frames as bars, newest right, split into parts and scaled to twice their mean so one
/// long frame doesn't flatten the rest; longer ones are cut off under a text-colored line. The
/// wait for the cap is left empty above a bar. Hovering holds the graph still and shows the
/// hovered frame's parts.
#[cfg(feature = "dev")]
fn graph(perf: &Perf, ui: &mut egui::Ui) {
    let size = egui::vec2(ui.available_width(), 64.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let bars = Bars::new(rect, ui.pixels_per_point());
    // Frames measured since the pointer came over: skipped, to hold the ones shown then, until
    // the ring would overwrite them (68 s at 240 fps).
    let held = if response.hovered() {
        ui.data_mut(|data| {
            let since = data.get_temp_mut_or_insert_with(response.id, || perf.measured);
            if (perf.measured - *since) as usize + bars.count > KEPT {
                *since = perf.measured;
            }
            (perf.measured - *since) as usize
        })
    } else {
        ui.data_mut(|data| data.remove::<u64>(response.id));
        0
    };
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    painter.rect_filled(rect, 0.0, visuals.extreme_bg_color);
    let frames = || perf.recent().skip(held).take(bars.count);
    let (count, sum) = frames().fold((0.0, 0.0), |(count, sum), sample| {
        (count + 1.0, sum + sample.frame as f32)
    });
    // At least a millisecond high.
    let top = (2.0 * sum / count).max(1e6);
    // One mesh, which egui doesn't anti-alias: that would blur thin bars together.
    let mut mesh = egui::Mesh::default();
    for (i, sample) in frames().enumerate() {
        let mut bottom = rect.bottom();
        for part in &PARTS {
            let height = (part.time)(sample) as f32 / top * rect.height();
            let above = (bottom - height).max(rect.top());
            let bar = egui::Rect::from_x_y_ranges(bars.across(i), above..=bottom);
            mesh.add_colored_rect(bar, part.color(visuals));
            bottom = above;
        }
        if sample.frame as f32 > top {
            let cut = egui::Rect::from_x_y_ranges(bars.across(i), rect.top()..=rect.top() + 2.0);
            mesh.add_colored_rect(cut, visuals.strong_text_color());
        }
    }
    painter.add(egui::Shape::mesh(mesh));
    // 60 and 30 fps.
    for line in [1e9 / 60.0, 1e9 / 30.0] {
        if line < top {
            let y = rect.bottom() - line / top * rect.height();
            painter.hline(rect.x_range(), y, (1.0, visuals.weak_text_color()));
        }
    }
    let paused = if response.hovered() { ", paused" } else { "" };
    painter.text(
        rect.left_top() + egui::vec2(4.0, 2.0),
        egui::Align2::LEFT_TOP,
        format!("{:.1} ms{paused}", top / 1e6),
        egui::FontId::monospace(10.0),
        visuals.text_color(),
    );
    if let Some(i) = response.hover_pos().and_then(|pointer| bars.at(pointer.x))
        && let Some(sample) = frames().nth(i)
    {
        // Drawn on the panel's painter: the graph's clip would cut the outline.
        let hovered = egui::Rect::from_x_y_ranges(bars.across(i), rect.y_range());
        let stroke = (1.0, visuals.strong_text_color());
        ui.painter()
            .rect_stroke(hovered, 0.0, stroke, egui::StrokeKind::Outside);
        response.on_hover_ui_at_pointer(|ui| frame_tooltip(ui, sample));
    }
}

/// The graph's bars, each at least one whole pixel wide: unanti-aliased, a bar between pixels
/// would change width as it scrolls, or vanish.
#[cfg(feature = "dev")]
struct Bars {
    /// The newest bar's right edge, in points.
    right: f32,
    /// Bar width, in points.
    width: f32,
    count: usize,
}

#[cfg(feature = "dev")]
impl Bars {
    fn new(rect: egui::Rect, pixels_per_point: f32) -> Self {
        let pixels = (rect.width() * pixels_per_point).floor();
        let bar = (pixels / GRAPHED as f32).floor().max(1.0);
        Self {
            right: (rect.right() * pixels_per_point).floor() / pixels_per_point,
            width: bar / pixels_per_point,
            count: (pixels / bar) as usize,
        }
    }

    /// The `i`th newest bar's x range.
    fn across(&self, i: usize) -> std::ops::RangeInclusive<f32> {
        let right = self.right - i as f32 * self.width;
        right - self.width..=right
    }

    /// The bar at `x`, as its count of newer ones.
    fn at(&self, x: f32) -> Option<usize> {
        let i = ((self.right - x) / self.width).floor();
        (0.0..self.count as f32).contains(&i).then_some(i as usize)
    }
}

#[cfg(feature = "dev")]
fn draws_text(sample: &Sample) -> String {
    let Sample {
        draws,
        instances,
        triangles,
        ..
    } = sample;
    format!("{draws} draws of {instances} instances, {triangles} triangles")
}

#[cfg(feature = "dev")]
fn frame_tooltip(ui: &mut egui::Ui, sample: &Sample) {
    egui::Grid::new("performance frame").show(ui, |ui| {
        let row = |ui: &mut egui::Ui, color, name: &str, time: u64| {
            row_name(ui, color, name);
            ui.label(millis_text(time as f64));
            ui.end_row();
        };
        row(ui, None, "Frame", sample.frame);
        for part in &PARTS {
            let color = part.color(ui.visuals());
            row(ui, Some(color), part.name, (part.time)(sample));
        }
        if sample.limit > 0 {
            row(ui, None, "Limit", sample.limit);
        }
        for (name, _, time) in &RENDERING {
            row(ui, None, name, time(sample));
        }
    });
    ui.label(draws_text(sample));
}

/// A row's first two cells: a `color` square (none for rows the graph doesn't stack) and the name,
/// whose label it returns.
#[cfg(feature = "dev")]
fn row_name(ui: &mut egui::Ui, color: Option<egui::Color32>, name: &str) -> egui::Response {
    if let Some(color) = color {
        let (square, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
        ui.painter().rect_filled(square, 2.0, color);
    } else {
        ui.label("");
    }
    ui.label(name)
}

/// `nanos` in milliseconds to the microsecond, padded with figure spaces (digit-wide) for the
/// missing digits before the point, so a column lines up at the point and holds still.
#[cfg(feature = "dev")]
fn millis_text(nanos: f64) -> String {
    format!("{:\u{2007}>7.3} ms", nanos / 1e6)
}

#[cfg(test)]
mod tests {
    use gg_renderer::DrawCounts;

    use super::*;

    const MS: Duration = Duration::from_millis(1);

    fn rendered(render: Duration, wait: Duration, drawn: DrawCounts, presented: bool) -> Rendered {
        Rendered {
            presented: Ok(presented),
            render,
            wait,
            publish: Duration::ZERO,
            drawn,
            gpu: None,
        }
    }

    /// Runs empty presented frames of `times` milliseconds, then ends.
    fn run(times: &[u64]) -> Perf {
        let mut perf = Perf::default();
        let mut dt = Duration::ZERO;
        let presented = rendered(Duration::ZERO, Duration::ZERO, DrawCounts::default(), true);
        for &time in times {
            perf.start_frame(dt);
            perf.ran(Duration::ZERO, Duration::ZERO, Duration::ZERO);
            perf.rendered(Duration::ZERO, Duration::ZERO, Some(&presented));
            dt = Duration::from_millis(time);
        }
        perf.finish(dt);
        perf
    }

    #[test]
    fn percentiles_are_nearest_rank_and_the_first_frame_is_apart() {
        let times: Vec<u64> = iter_first(1000, 1..=100);
        let summary = run(&times).summary();
        assert_eq!(summary.first_frame, Some(1000.0));
        assert_eq!(summary.measured, 100);
        assert_eq!(summary.percentile_frames, 100);
        let frame = summary.frame;
        assert_eq!(
            (frame.p50, frame.p95, frame.p99, frame.max),
            (Some(50.0), Some(95.0), Some(99.0), Some(100.0))
        );
        assert_eq!(frame.mean, Some(50.5));
    }

    fn iter_first(first: u64, rest: impl IntoIterator<Item = u64>) -> Vec<u64> {
        std::iter::once(first).chain(rest).collect()
    }

    #[test]
    fn short_runs_have_what_they_measured() {
        let none = run(&[]).summary();
        assert_eq!(none.first_frame, None);
        assert_eq!(none.measured, 0);
        assert_eq!(none.frame, Stats::default());

        let one = run(&[5]).summary();
        assert_eq!(one.first_frame, Some(5.0));
        assert_eq!(one.measured, 0);
        assert_eq!(one.frame.p50, None);

        let two = run(&[5, 7]).summary();
        assert_eq!(two.measured, 1);
        assert_eq!((two.frame.p50, two.frame.p99), (Some(7.0), Some(7.0)));
    }

    #[test]
    fn pauses_and_frames_without_a_render_that_presented_go_unmeasured() {
        let mut perf = Perf::default();
        let present = |perf: &mut Perf, presented| {
            let drew = DrawCounts {
                draws: 1,
                instances: 1,
                triangles: 12,
            };
            let render = rendered(MS, Duration::ZERO, drew, presented);
            perf.rendered(Duration::ZERO, Duration::ZERO, Some(&render));
        };
        // The first frame has no render, so the next is the first measured.
        perf.start_frame(Duration::ZERO);
        perf.rendered(Duration::ZERO, Duration::ZERO, None);
        perf.start_frame(5 * MS);
        present(&mut perf, true);
        perf.start_frame(10 * MS);
        // A pause (window restored) drops the frame before it.
        present(&mut perf, true);
        perf.start_frame(Duration::ZERO);
        // A frame whose render presented nothing.
        present(&mut perf, false);
        perf.start_frame(30 * MS);
        present(&mut perf, true);
        perf.finish(20 * MS);
        let summary = perf.summary();
        assert_eq!(summary.first_frame, Some(10.0));
        assert_eq!(summary.measured, 1);
        assert_eq!(summary.frame.max, Some(20.0));
    }

    #[test]
    fn rendering_leaves_out_its_waits_and_the_ui_takes_tessellation() {
        let mut perf = Perf::default();
        let none = DrawCounts::default();
        let nothing = rendered(Duration::ZERO, Duration::ZERO, none, true);
        perf.start_frame(Duration::ZERO);
        perf.rendered(Duration::ZERO, Duration::ZERO, Some(&nothing));
        perf.start_frame(MS);
        perf.ran(MS, 2 * MS, 3 * MS);
        let drew = DrawCounts {
            draws: 7,
            instances: 900,
            triangles: 1800,
        };
        perf.rendered(MS, 5 * MS, Some(&rendered(10 * MS, 4 * MS, drew, true)));
        perf.start_frame(30 * MS);
        // Waits outlasting their render leave no rendering.
        let waits = rendered(MS, 4 * MS, none, true);
        perf.rendered(Duration::ZERO, Duration::ZERO, Some(&waits));
        perf.finish(5 * MS);
        let newest: Vec<Sample> = perf.recent().copied().collect();
        let ms = |ms: u64| ms * 1_000_000;
        assert_eq!(
            newest,
            [
                Sample {
                    frame: ms(5),
                    wait: ms(4),
                    ..Sample::default()
                },
                Sample {
                    frame: ms(30),
                    ticks: ms(1),
                    update: ms(2),
                    ui: ms(4),
                    handoff: ms(5),
                    limit: 0,
                    render: ms(6),
                    wait: ms(4),
                    publish: 0,
                    gpu: 0,
                    draws: 7,
                    instances: 900,
                    triangles: 1800,
                },
            ]
        );
        // Render parts overlap the main thread's, so `other` ignores them.
        assert_eq!(newest[1].other(), ms(18));
        let summary = perf.summary();
        let maxima = (summary.draws_max, summary.instances_max);
        assert_eq!((maxima, summary.triangles_max), ((7, 900), 1800));
    }

    #[test]
    fn a_frames_wait_for_the_cap_is_its_own_and_none_of_the_rest() {
        let mut perf = Perf::default();
        let presented = rendered(Duration::ZERO, Duration::ZERO, DrawCounts::default(), true);
        perf.start_frame(Duration::ZERO);
        perf.rendered(Duration::ZERO, Duration::ZERO, Some(&presented));
        perf.start_frame(MS);
        perf.ran(MS, MS, MS);
        perf.rendered(Duration::ZERO, MS, Some(&presented));
        perf.limited(12 * MS);
        perf.start_frame(17 * MS);
        perf.finish(MS);
        let newest = perf.recent().nth(1).copied().expect("the capped frame");
        let ms = |ms: u64| ms * 1_000_000;
        assert_eq!((newest.frame, newest.limit), (ms(17), ms(12)));
        assert_eq!(newest.other(), ms(1));
        assert_eq!(perf.summary().limit.max, Some(12.0));
    }

    /// GPU times cover only the frames they came back for; the first frames' don't.
    #[test]
    fn gpu_times_cover_the_frames_they_came_back_for() {
        let mut perf = Perf::default();
        let none = DrawCounts::default();
        perf.start_frame(Duration::ZERO);
        for (gpu, next) in [
            (None, MS),
            (None, MS),
            (Some(2 * MS), MS),
            (Some(4 * MS), MS),
        ] {
            let render = Rendered {
                gpu,
                ..rendered(Duration::ZERO, Duration::ZERO, none, true)
            };
            perf.rendered(Duration::ZERO, Duration::ZERO, Some(&render));
            perf.start_frame(next);
        }
        perf.finish(MS);
        let summary = perf.summary();
        assert_eq!(summary.measured, 4);
        let gpu = summary.gpu;
        assert_eq!(
            (gpu.mean, gpu.max, gpu.p50),
            (Some(3.0), Some(4.0), Some(2.0))
        );
        assert_eq!(Perf::default().summary().gpu, Stats::default());
    }

    #[test]
    fn percentiles_cover_the_last_frames_kept_and_the_rest_the_whole_run() {
        // Ten long frames, then enough short ones to push them out of the percentiles.
        let times = iter_first(
            1,
            [1000; 10].into_iter().chain(std::iter::repeat_n(1, KEPT)),
        );
        let perf = run(&times);
        let summary = perf.summary();
        assert_eq!(summary.measured, KEPT as u64 + 10);
        assert_eq!(summary.percentile_frames, KEPT);
        assert_eq!(summary.frame.p99, Some(1.0));
        assert_eq!(summary.frame.max, Some(1000.0));
        assert!(summary.frame.mean > Some(1.0));
        assert_eq!(perf.recent().count(), KEPT);
    }

    #[cfg(feature = "dev")]
    #[test]
    fn bars_are_whole_pixels_and_fill_the_graph() {
        let whole = |points: f32, ppp: f32| (points * ppp - (points * ppp).round()).abs() < 1e-3;
        // Fewer pixels than frames; 1.25 pixels a point; room for two-pixel bars.
        for (width, ppp) in [(100.0, 1.0), (300.0, 1.25), (575.0, 1.0)] {
            let rect = egui::Rect::from_min_size(egui::pos2(10.3, 0.0), egui::vec2(width, 64.0));
            let bars = Bars::new(rect, ppp);
            let pixels = (width * ppp).floor() as usize;
            let bar = (bars.width * ppp).round() as usize;
            assert!(whole(bars.width, ppp) && bar >= 1);
            assert!(whole(bars.right, ppp) && bars.right <= rect.right());
            assert!(bars.count * bar <= pixels && pixels < (bars.count + 1) * bar);
            assert!(bars.count >= GRAPHED.min(pixels));
            let oldest = bars.right - (bars.count as f32 - 0.5) * bars.width;
            assert_eq!(bars.at(bars.right - bars.width / 2.0), Some(0));
            assert_eq!(bars.at(oldest), Some(bars.count - 1));
            assert_eq!(bars.at(bars.right + 0.1), None);
            assert_eq!(bars.at(oldest - bars.width), None);
        }
    }

    #[cfg(feature = "dev")]
    #[test]
    fn hovering_over_a_bar_holds_the_graph_and_shows_its_frame() {
        // Twenty frames: the sixth newest has 42 draws of 4,200 instances, the rest one draw of
        // 100.
        let mut perf = Perf::default();
        let frame = |perf: &mut Perf, draws| {
            let drew = DrawCounts {
                draws,
                instances: draws * 100,
                triangles: draws * 200,
            };
            perf.ran(Duration::ZERO, MS, MS);
            let render = rendered(2 * MS, MS, drew, true);
            perf.rendered(Duration::ZERO, Duration::ZERO, Some(&render));
            perf.start_frame(4 * MS);
        };
        perf.start_frame(Duration::ZERO);
        for i in 0..=20 {
            frame(&mut perf, if i == 15 { 42 } else { 1 });
        }
        let ctx = crate::egui_context();
        let background = ctx.global_style().visuals.extreme_bg_color;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let mut graph: Option<egui::Rect> = None;
        let mut texts = Vec::new();
        // A layout pass, the pointer over the sixth newest bar, then passes past egui's tooltip
        // delay.
        for pass in 0..4 {
            let mut events = Vec::new();
            if let (1, Some(rect)) = (pass, graph) {
                let bars = Bars::new(rect, ctx.pixels_per_point());
                let at = egui::pos2(bars.right - 5.5 * bars.width, rect.center().y);
                events.push(egui::Event::PointerMoved(at));
            }
            if pass > 1 {
                frame(&mut perf, 1);
            }
            let input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(f64::from(pass)),
                events,
                ..egui::RawInput::default()
            };
            let mut output = ctx.run_ui(input, |ui| show(&perf, ui));
            output.textures_delta.clear();
            texts.clear();
            find(&output.shapes, background, &mut graph, &mut texts);
        }
        assert!(graph.is_some());
        let drew = "42 draws of 4200 instances, 8400 triangles";
        assert!(texts.iter().any(|text| text == drew), "{texts:?}");
    }

    #[cfg(feature = "dev")]
    #[test]
    fn the_top_line_averages_the_last_second() {
        // 2 ms frames then a 12 ms one: the last second is the newest 495 frames.
        let times = iter_first(1, std::iter::repeat_n(2, 1000).chain([12]));
        let perf = run(&times);
        let ctx = crate::egui_context();
        let background = ctx.global_style().visuals.extreme_bg_color;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            ..egui::RawInput::default()
        };
        let mut output = ctx.run_ui(input, |ui| show(&perf, ui));
        output.textures_delta.clear();
        let mut graph: Option<egui::Rect> = None;
        let mut texts = Vec::new();
        find(&output.shapes, background, &mut graph, &mut texts);
        // Not the last frame's 12.00 ms, nor the whole run's 2.01.
        let line = "2.02 ms a frame, 495 fps, 0 draws of 0 instances, 0 triangles";
        assert!(texts.iter().any(|text| text == line), "{texts:?}");
        // The mean and the maximum, padded with figure spaces to line up at the point.
        for time in ["\u{2007}\u{2007}2.020 ms", "\u{2007}12.000 ms"] {
            assert!(texts.iter().any(|text| text == time), "{time}: {texts:?}");
        }
    }

    /// Finds the graph (the one shape filled in `background`) and the texts in a pass's `shapes`.
    #[cfg(feature = "dev")]
    fn find(
        shapes: &[egui::epaint::ClippedShape],
        background: egui::Color32,
        graph: &mut Option<egui::Rect>,
        texts: &mut Vec<String>,
    ) {
        for shape in crate::testing::painted(shapes) {
            match shape {
                egui::Shape::Rect(rect) if rect.fill == background => *graph = Some(rect.rect),
                egui::Shape::Text(text) => texts.push(text.galley.text().to_owned()),
                _ => {}
            }
        }
    }
}
