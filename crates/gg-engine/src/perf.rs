//! How long frames take: the engine's own measure of every frame, which [`PerfPanel`] shows and
//! each run that doesn't end in an error sums up as it ends, in its `engine.perf` event. It's
//! always on, and costs a few reads of the clock a frame.

use std::time::Duration;

use crate::render_thread::Rendered;
use crate::{Context, Layer, Ui};

/// How many frames the percentiles cover: the last 16,384, 68 s at 240 fps, while means and
/// maxima cover the whole run. They take 1,152 KiB, from the first frame measured.
const KEPT: usize = 16_384;

/// How many frames the Performance panel's graph shows at the fewest, given a pixel for each: its
/// bars are as many whole pixels wide as leave room for that many, and more frames fill the rest.
const GRAPHED: usize = 240;

/// One frame, in nanoseconds: its time on the main thread, from its start to the next frame's, and
/// the parts of it; and the render it took back, the frame before's, which ran beside it on the
/// render thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Sample {
    pub(crate) frame: u64,
    /// The simulation's ticks.
    pub(crate) ticks: u64,
    /// The layers' updates, and drawing the world.
    pub(crate) update: u64,
    /// The UI: the layers' and egui's, and tessellating it.
    pub(crate) ui: u64,
    /// Waiting for the render thread to finish the frame before.
    pub(crate) handoff: u64,
    /// Rendering the frame before, with its submission and present, but not its waits.
    pub(crate) render: u64,
    /// Its waits: for the GPU to finish an earlier frame, and for the display to let go of an
    /// image.
    pub(crate) wait: u64,
    /// Instanced draws in the game's view.
    pub(crate) draws: u64,
    /// The meshes they drew.
    pub(crate) instances: u64,
}

impl Sample {
    /// The rest of the frame: what happens between frames, such as the windows of their own
    /// opening and drawing their UI, and the OS's events.
    pub(crate) fn other(&self) -> u64 {
        (self.frame).saturating_sub(self.ticks + self.update + self.ui + self.handoff)
    }

    /// Sets each field to `f` of it and `other`'s.
    fn merge(&mut self, other: &Sample, f: impl Fn(u64, u64) -> u64) {
        self.frame = f(self.frame, other.frame);
        self.ticks = f(self.ticks, other.ticks);
        self.update = f(self.update, other.update);
        self.ui = f(self.ui, other.ui);
        self.handoff = f(self.handoff, other.handoff);
        self.render = f(self.render, other.render);
        self.wait = f(self.wait, other.wait);
        self.draws = f(self.draws, other.draws);
        self.instances = f(self.instances, other.instances);
    }
}

/// The frames of a run so far. The main loop tells it when each frame starts and how its parts
/// went; a frame is measured once the next one starts, or the run ends, with the render it took
/// back. A frame that took none back, as the first doesn't, isn't measured; and the first that did
/// isn't measured with the rest, since that render made the pipelines and copied the first meshes
/// and textures to the GPU, which it waited for.
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
    sum: Sample,
    max: Sample,
}

impl Perf {
    /// A frame starts, `dt` after the last one did, which is then measured. A zero `dt`, the
    /// first frame's or the first's after a pause, drops the last frame instead, since its time
    /// would count the pause.
    pub(crate) fn start_frame(&mut self, dt: Duration) {
        self.end(dt);
        self.pending = Some(Sample::default());
    }

    /// The frame's ticks took `ticks`, its updates and drawing its world `update`, and its UI `ui`.
    pub(crate) fn ran(&mut self, ticks: Duration, update: Duration, ui: Duration) {
        if let Some(sample) = &mut self.pending {
            sample.ticks = nanos(ticks);
            sample.update = nanos(update);
            sample.ui = nanos(ui);
        }
    }

    /// The frame tessellated its UI in `tessellate`, then waited `handoff` for the render thread to
    /// finish the frame before, which went as `rendered` says. A frame that took no render back,
    /// or whose render presented nothing or failed, isn't measured.
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
            sample.draws = rendered.drawn.draws as u64;
            sample.instances = rendered.drawn.instances as u64;
        }
    }

    /// The run ends, `last` after the last frame started, which is then measured.
    pub(crate) fn finish(&mut self, last: Duration) {
        self.end(last);
    }

    /// Measures the frame under way, which took `time`, unless that's zero.
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
        self.sum.merge(&sample, |a, b| a + b);
        self.max.merge(&sample, u64::max);
    }

    /// The frames kept, newest first.
    pub(crate) fn recent(&self) -> impl Iterator<Item = &Sample> {
        let (newer, older) = self.kept.split_at(self.next);
        newer.iter().rev().chain(older.iter().rev())
    }

    /// The run's frames summed up, in milliseconds.
    pub(crate) fn summary(&self) -> Summary {
        let stats = |field: fn(&Sample) -> u64| {
            if self.measured == 0 {
                return Stats::default();
            }
            let mut sorted: Vec<u64> = self.kept.iter().map(field).collect();
            sorted.sort_unstable();
            // Nearest rank: the smallest at or above `percent` of them.
            let at = |percent: usize| {
                let rank = (percent * sorted.len()).div_ceil(100);
                Some(ms(sorted[rank - 1] as f64))
            };
            Stats {
                p50: at(50),
                p95: at(95),
                p99: at(99),
                mean: Some(ms(field(&self.sum) as f64 / self.measured as f64)),
                max: Some(ms(field(&self.max) as f64)),
            }
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
            render: stats(|s| s.render),
            wait: stats(|s| s.wait),
            draws_max: self.max.draws,
            instances_max: self.max.instances,
        }
    }
}

/// A run's frames summed up, in milliseconds, which its `engine.perf` event logs; `None` where no
/// frame was measured.
#[derive(Debug, PartialEq)]
pub(crate) struct Summary {
    /// The frames measured, which the means and maxima cover.
    pub(crate) measured: u64,
    /// The frames the percentiles cover: the last of those measured, up to `KEPT`.
    pub(crate) percentile_frames: usize,
    pub(crate) first_frame: Option<f64>,
    pub(crate) frame: Stats,
    pub(crate) ticks: Stats,
    pub(crate) update: Stats,
    pub(crate) ui: Stats,
    pub(crate) handoff: Stats,
    pub(crate) render: Stats,
    pub(crate) wait: Stats,
    pub(crate) draws_max: u64,
    pub(crate) instances_max: u64,
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

/// `duration` in milliseconds, rounded to the microsecond.
pub(crate) fn millis(duration: Duration) -> f64 {
    ms(duration.as_nanos() as f64)
}

/// `nanos` in milliseconds, rounded to the microsecond.
fn ms(nanos: f64) -> f64 {
    (nanos / 1e3).round() / 1e3
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

/// The Performance panel, which an app adds as an overlay, `.overlay(gg_engine::PerfPanel)`: the
/// last second's frame time and rate, a graph of the last frames split into their parts, and
/// where the last second's time went, as the engine measures them.
pub struct PerfPanel;

impl Layer for PerfPanel {
    fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
        let perf = &cx.perf;
        ui.panel("Performance", |ui| {
            // A new panel floats smaller than its contents.
            egui::ScrollArea::vertical().show(ui, |ui| show(perf, ui));
        });
    }
}

/// How a part of a frame's time is read from its sample.
type Time = fn(&Sample) -> u64;

/// A part of a frame the panel shows: its name, how it's read from a sample, and its color in the
/// graph, on a light background and on a dark one.
struct Part {
    name: &'static str,
    time: Time,
    light: egui::Color32,
    dark: egui::Color32,
}

impl Part {
    fn color(&self, visuals: &egui::Visuals) -> egui::Color32 {
        if visuals.dark_mode {
            self.dark
        } else {
            self.light
        }
    }
}

/// The parts of a frame on the main thread, as the graph stacks them from the bottom: the work,
/// then the wait, in gray, while the main thread idles. Each color stays apart from its neighbors'
/// for the common color blindnesses, protanopia and deuteranopia, on egui's light and dark
/// backgrounds, which puts the UI between the ticks and the updates: UI's orange beside Other's
/// amber isn't. A new one needs checking for that again.
const PARTS: [Part; 5] = [
    Part {
        name: "Ticks",
        time: |s| s.ticks,
        light: egui::Color32::from_rgb(0x1b, 0xaf, 0x7a),
        dark: egui::Color32::from_rgb(0x19, 0x9e, 0x70),
    },
    Part {
        name: "UI",
        time: |s| s.ui,
        light: egui::Color32::from_rgb(0xeb, 0x68, 0x34),
        dark: egui::Color32::from_rgb(0xd9, 0x59, 0x26),
    },
    Part {
        name: "Update",
        time: |s| s.update,
        light: egui::Color32::from_rgb(0x2a, 0x78, 0xd6),
        dark: egui::Color32::from_rgb(0x39, 0x87, 0xe5),
    },
    Part {
        name: "Other",
        time: Sample::other,
        light: egui::Color32::from_rgb(0xed, 0xa1, 0x00),
        dark: egui::Color32::from_rgb(0xc9, 0x85, 0x00),
    },
    Part {
        name: "Waiting for rendering",
        time: |s| s.handoff,
        light: egui::Color32::from_rgb(0xc3, 0xc2, 0xb7),
        dark: egui::Color32::from_rgb(0x38, 0x38, 0x35),
    },
];

/// What the render thread spent on the frame before each frame, beside it: the panel lists it, but
/// the graph doesn't stack it on the main thread's parts, which it overlaps.
const RENDERING: [(&str, Time); 2] = [
    ("Render thread: rendering", |s| s.render),
    ("Render thread: waiting for the GPU and display", |s| s.wait),
];

/// The sum and the maximum of some times.
#[derive(Clone, Copy, Default)]
struct Total {
    sum: u64,
    max: u64,
}

impl Total {
    fn add(&mut self, time: u64) {
        self.sum += time;
        self.max = self.max.max(time);
    }
}

fn show(perf: &Perf, ui: &mut egui::Ui) {
    let Some(last) = perf.recent().next() else {
        ui.label("No frames measured yet");
        return;
    };
    // The text covers the last second, which reads steadier than the last frame at high frame
    // rates; the graph shows each frame.
    let mut frames = 0;
    let mut frame = Total::default();
    let mut parts = [Total::default(); PARTS.len()];
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
        for (total, (_, time)) in rendering.iter_mut().zip(&RENDERING) {
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
            ui.label(format!("Last {frames} frames"));
            ui.label("Mean");
            ui.label("Max");
            ui.end_row();
            let row = |ui: &mut egui::Ui, color, name: &str, total: Total| {
                row_name(ui, color, name);
                ui.label(millis_text(total.sum as f64 / frames as f64));
                ui.label(millis_text(total.max as f64));
                ui.end_row();
            };
            row(ui, None, "Frame", frame);
            for (part, total) in PARTS.iter().zip(parts) {
                let color = part.color(ui.visuals());
                row(ui, Some(color), part.name, total);
            }
            for ((name, _), total) in RENDERING.iter().zip(rendering) {
                row(ui, None, name, total);
            }
        });
}

/// The last frames' times as bars, newest at the right, each split into its parts and scaled to
/// twice their mean, so that one long frame doesn't flatten the rest; longer frames are cut off,
/// under a line in the text's color. While the pointer is over the graph, the graph holds still,
/// and shows the parts of the frame under the pointer.
fn graph(perf: &Perf, ui: &mut egui::Ui) {
    let size = egui::vec2(ui.available_width(), 64.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let bars = Bars::new(rect, ui.pixels_per_point());
    // The frames measured since the pointer came over the graph, which it skips to hold the ones it
    // showed then, until the ring would overwrite them: 68 s at 240 fps.
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
    // One mesh, which egui doesn't anti-alias: that would blur bars this thin into one another.
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
        // Around the bar, so on the panel's painter, which the graph's edges don't clip.
        let hovered = egui::Rect::from_x_y_ranges(bars.across(i), rect.y_range());
        let stroke = (1.0, visuals.strong_text_color());
        ui.painter()
            .rect_stroke(hovered, 0.0, stroke, egui::StrokeKind::Outside);
        response.on_hover_ui_at_pointer(|ui| frame_tooltip(ui, sample));
    }
}

/// Where the graph's bars go: each a whole number of pixels wide, and at least one, since without
/// anti-aliasing, a bar between pixels would change width as it scrolls, or not show at all.
struct Bars {
    /// The newest bar's right edge, in points.
    right: f32,
    /// Each bar's width, in points.
    width: f32,
    /// How many fit.
    count: usize,
}

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

    /// Where the `i`th newest frame's bar is across.
    fn across(&self, i: usize) -> std::ops::RangeInclusive<f32> {
        let right = self.right - i as f32 * self.width;
        right - self.width..=right
    }

    /// Which bar is at `x` across, as how many newer ones there are, if any.
    fn at(&self, x: f32) -> Option<usize> {
        let i = ((self.right - x) / self.width).floor();
        (0.0..self.count as f32).contains(&i).then_some(i as usize)
    }
}

/// A frame's instanced draws and the meshes they drew, as the panel shows them.
fn draws_text(sample: &Sample) -> String {
    format!("{} draws of {} instances", sample.draws, sample.instances)
}

/// One frame's time, its parts', the render thread's and its draws, for the graph's tooltip.
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
        for (name, time) in &RENDERING {
            row(ui, None, name, time(sample));
        }
    });
    ui.label(draws_text(sample));
}

/// A row's first two cells: a square of `color`, its part's in the graph, and its name; a row the
/// graph doesn't stack, such as the frame's, the parts summed up, has no square.
fn row_name(ui: &mut egui::Ui, color: Option<egui::Color32>, name: &str) {
    if let Some(color) = color {
        let (square, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
        ui.painter().rect_filled(square, 2.0, color);
    } else {
        ui.label("");
    }
    ui.label(name);
}

/// `nanos` in milliseconds, to the microsecond, as the panel shows it.
fn millis_text(nanos: f64) -> String {
    format!("{:.3} ms", nanos / 1e6)
}

#[cfg(test)]
mod tests {
    use gg_renderer::DrawCounts;

    use super::*;

    const MS: Duration = Duration::from_millis(1);

    /// A render of the frame before, which took `render`, `wait` of it waiting, drew `drawn`, and
    /// `presented` or not.
    fn rendered(render: Duration, wait: Duration, drawn: DrawCounts, presented: bool) -> Rendered {
        Rendered {
            presented: Ok(presented),
            render,
            wait,
            drawn,
        }
    }

    /// Runs frames of `times` milliseconds each, with nothing in them, each taking back a render
    /// that presented, then ends the run.
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

    /// `first`, then `rest`.
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
            };
            let render = rendered(MS, Duration::ZERO, drew, presented);
            perf.rendered(Duration::ZERO, Duration::ZERO, Some(&render));
        };
        // The first frame takes no render back, so the next is the first measured.
        perf.start_frame(Duration::ZERO);
        perf.rendered(Duration::ZERO, Duration::ZERO, None);
        perf.start_frame(5 * MS);
        present(&mut perf, true);
        perf.start_frame(10 * MS);
        // Then a pause: the window is restored, and the frame before it is dropped.
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
        };
        perf.rendered(MS, 5 * MS, Some(&rendered(10 * MS, 4 * MS, drew, true)));
        perf.start_frame(30 * MS);
        // Waits outlasting the render they're in round down to no rendering.
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
                    render: ms(6),
                    wait: ms(4),
                    draws: 7,
                    instances: 900,
                },
            ]
        );
        // The render thread's parts ran beside the main thread's.
        assert_eq!(newest[1].other(), ms(18));
        let summary = perf.summary();
        assert_eq!((summary.draws_max, summary.instances_max), (7, 900));
    }

    #[test]
    fn percentiles_cover_the_last_frames_kept_and_the_rest_the_whole_run() {
        // Ten long frames, then enough short ones to push them out.
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

    #[test]
    fn bars_are_whole_pixels_and_fill_the_graph() {
        let whole = |points: f32, ppp: f32| (points * ppp - (points * ppp).round()).abs() < 1e-3;
        // Fewer pixels than frames, at 1.25 pixels a point, and room for bars two pixels wide.
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

    #[test]
    fn hovering_over_a_bar_holds_the_graph_and_shows_its_frame() {
        // Twenty frames measured, the sixth newest with 42 draws of 4,200 instances and the rest
        // with one of 100.
        let mut perf = Perf::default();
        let frame = |perf: &mut Perf, draws| {
            let drew = DrawCounts {
                draws,
                instances: draws * 100,
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
        // A pass to lay the panel out, then the pointer over the sixth newest bar, and passes past
        // egui's tooltip delay, each a frame later.
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
            for clipped in &output.shapes {
                find(&clipped.shape, background, &mut graph, &mut texts);
            }
        }
        assert!(graph.is_some());
        let drew = "42 draws of 4200 instances";
        assert!(texts.iter().any(|text| text == drew), "{texts:?}");
    }

    #[test]
    fn the_top_line_averages_the_last_second() {
        // 2 ms frames, then a 12 ms one: the last second is the newest 495 frames, 1,000 ms.
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
        for clipped in &output.shapes {
            find(&clipped.shape, background, &mut graph, &mut texts);
        }
        // Not the last frame's 12.00 ms, nor the whole run's 2.01.
        let line = "2.02 ms a frame, 495 fps, 0 draws of 0 instances";
        assert!(texts.iter().any(|text| text == line), "{texts:?}");
    }

    /// Finds in `shape` the graph, the one shape filled in `background`, and the texts.
    fn find(
        shape: &egui::Shape,
        background: egui::Color32,
        graph: &mut Option<egui::Rect>,
        texts: &mut Vec<String>,
    ) {
        match shape {
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    find(shape, background, graph, texts);
                }
            }
            egui::Shape::Rect(rect) if rect.fill == background => *graph = Some(rect.rect),
            egui::Shape::Text(text) => texts.push(text.galley.text().to_owned()),
            _ => {}
        }
    }
}
