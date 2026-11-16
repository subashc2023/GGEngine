//! Flipbooks: sprites played through a sheet's cells.

use gg_core::math::Vec2;

use super::Playback;
use crate::{Component, Sprite, Tick, World};

/// Plays the cells of the sheet its entity's [`Sprite`] shows: `frames` cells from `first`, `rate`
/// a second, as `playback` says, setting the sprite's `rect` to the current cell.
///
/// The sheet is the sprite's texture cut evenly into `columns` by `rows` cells, edge to edge, as
/// [`SpriteSheet`](crate::SpriteSheet) cuts one. Cells are fractions of the texture: the renderer
/// knows its size, the world doesn't. A flipped `rect` (left past right, or top below bottom) stays
/// flipped. An entity without a `Sprite` doesn't play.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct Flipbook {
    /// Cells across the sheet.
    #[reflect(min = 1, default = 1)]
    pub columns: u32,
    /// Cells down the sheet.
    #[reflect(min = 1, default = 1)]
    pub rows: u32,
    /// The first cell, counted row by row from the top left: 0 is the top left, `columns` the
    /// first of the second row.
    pub first: u32,
    /// Cells played, from `first` on, row by row. Past the sheet's last, it shows the last.
    #[reflect(min = 1, default = 1)]
    pub frames: u32,
    /// Cells a second. At 0 it stays on its cell.
    #[reflect(min = 0.0, default = 12.0)]
    pub rate: f32,
    /// Plays once and stays on the last cell, loops, or goes back and forth.
    pub playback: Playback,
    /// Seconds played. Each tick advances it; looping wraps it within a round, and playing once
    /// stops it at the end. Negative, it waits that long on the first cell.
    pub time: f32,
}

impl Default for Flipbook {
    fn default() -> Self {
        Self {
            columns: 1,
            rows: 1,
            first: 0,
            frames: 1,
            rate: 12.0,
            playback: Playback::Once,
            time: 0.0,
        }
    }
}

impl Flipbook {
    /// The cell shown at its time, counted as `first` is.
    pub fn cell(&self) -> u32 {
        let frames = u64::from(self.frames.max(1));
        // A saturating cast: NaN becomes 0.
        let played = if self.time > 0.0 && self.rate > 0.0 {
            (self.time * self.rate) as u64
        } else {
            0
        };
        let frame = match self.playback {
            Playback::Once => played.min(frames - 1),
            Playback::Loop => played % frames,
            Playback::PingPong => {
                let round = 2 * (frames - 1);
                match round {
                    0 => 0,
                    round => {
                        let at = played % round;
                        if at < frames { at } else { round - at }
                    }
                }
            }
        };
        let cells = u64::from(self.columns.max(1)) * u64::from(self.rows.max(1));
        let cell = (u64::from(self.first) + frame).min(cells - 1);
        u32::try_from(cell).expect("within the sheet's cells, of u32s")
    }

    /// The shown cell's part of the sheet, top left then bottom right, as a [`Sprite`]'s `rect`.
    pub fn rect(&self) -> [Vec2; 2] {
        let (columns, rows) = (self.columns.max(1), self.rows.max(1));
        let cell = self.cell();
        let (column, row) = (cell % columns, cell / columns);
        // In f64, where these are exact, rounded once to the nearest f32.
        let at = |at: u32, of: u32| (f64::from(at) / f64::from(of)) as f32;
        [
            Vec2::new(at(column, columns), at(row, rows)),
            Vec2::new(at(column + 1, columns), at(row + 1, rows)),
        ]
    }

    /// Advances [`time`](Self::time) by `dt` seconds.
    fn advance(&mut self, dt: f32) {
        if !self.rate.is_finite() || self.rate <= 0.0 {
            return;
        }
        self.time += dt;
        let frames = self.frames.max(1) as f32;
        match self.playback {
            Playback::Once => {
                let end = frames / self.rate;
                if self.time > end {
                    self.time = end;
                }
            }
            Playback::Loop | Playback::PingPong => {
                let cells = match self.playback {
                    Playback::PingPong => (2.0 * (frames - 1.0)).max(1.0),
                    _ => frames,
                };
                let round = cells / self.rate;
                if self.time >= round {
                    self.time %= round;
                }
            }
        }
    }
}

/// Advances each flipbook a tick and sets its sprite's `rect` to its cell.
pub(super) fn step(world: &mut World) {
    let dt = Tick::DT.as_secs_f32();
    for (flipbook, sprite) in world.query_mut::<(&mut Flipbook, &mut Sprite)>() {
        flipbook.advance(dt);
        let [mut min, mut max] = flipbook.rect();
        let [was_min, was_max] = sprite.rect;
        if was_min.x > was_max.x {
            (min.x, max.x) = (max.x, min.x);
        }
        if was_min.y > was_max.y {
            (min.y, max.y) = (max.y, min.y);
        }
        sprite.rect = [min, max];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Transform;

    /// A world with a sprite playing `flipbook`.
    fn playing(flipbook: Flipbook) -> (World, crate::Entity) {
        let mut world = World::default();
        let sprite = Sprite::default();
        let entity = world.spawn((Transform::IDENTITY, sprite, flipbook));
        (world, entity)
    }

    /// The sheet index of the cell shown after each of `ticks` steps.
    fn cells(world: &mut World, entity: crate::Entity, ticks: usize) -> Vec<u32> {
        let mut shown = Vec::new();
        for _ in 0..ticks {
            step(world);
            let flipbook = *world.get::<Flipbook>(entity).expect("a flipbook");
            let [a, b] = world.get::<Sprite>(entity).expect("a sprite").rect;
            let column = (a.x.min(b.x) * flipbook.columns as f32).round() as u32;
            let row = (a.y.min(b.y) * flipbook.rows as f32).round() as u32;
            shown.push(row * flipbook.columns + column);
        }
        shown
    }

    /// At 16 cells a second a cell lasts 4 of 64 ticks a second. Frames play row by row from
    /// `first`: loops go round, ping-pongs back and forth, once stays on the last.
    #[test]
    fn a_flipbook_plays_its_cells_as_its_playback_says() {
        let flipbook = Flipbook {
            columns: 4,
            rows: 2,
            first: 2,
            frames: 4,
            rate: 16.0,
            ..Flipbook::default()
        };
        // The first step's cell of each cell's four.
        let every_fourth = |shown: Vec<u32>| shown.into_iter().step_by(4).collect::<Vec<_>>();
        for (playback, want) in [
            (Playback::Loop, [2, 3, 4, 5, 2, 3, 4, 5, 2, 3]),
            (Playback::PingPong, [2, 3, 4, 5, 4, 3, 2, 3, 4, 5]),
            (Playback::Once, [2, 3, 4, 5, 5, 5, 5, 5, 5, 5]),
        ] {
            let (mut world, entity) = playing(Flipbook {
                playback,
                ..flipbook
            });
            let shown = cells(&mut world, entity, 40);
            assert_eq!(every_fourth(shown), want, "{playback:?}");
        }
    }

    /// A loop's time stays within a round, so a long run plays as its start did; played once, it
    /// stays at its end; negative, it shows the first cell until its time comes.
    #[test]
    fn a_flipbook_s_time_stays_within_its_round_and_waits_while_negative() {
        let looped = Flipbook {
            columns: 8,
            frames: 8,
            rate: 8.0,
            playback: Playback::Loop,
            ..Flipbook::default()
        };
        let (mut world, entity) = playing(looped);
        let time = |world: &World| world.get::<Flipbook>(entity).expect("a flipbook").time;
        for _ in 0..64 * 10 {
            step(&mut world);
            assert!((0.0..1.0).contains(&time(&world)), "{}", time(&world));
        }
        let once = Flipbook {
            playback: Playback::Once,
            ..looped
        };
        let (mut world, entity) = playing(once);
        let time = |world: &World| world.get::<Flipbook>(entity).expect("a flipbook").time;
        for _ in 0..200 {
            step(&mut world);
        }
        assert_eq!(time(&world), 1.0);
        let waiting = Flipbook {
            time: -0.5,
            first: 3,
            ..looped
        };
        let (mut world, entity) = playing(waiting);
        assert_eq!(cells(&mut world, entity, 32), vec![3; 32]);
        assert_eq!(cells(&mut world, entity, 8)[7], 4);
    }

    /// A flipped sprite stays flipped, and cells past the sheet's last show the last.
    #[test]
    fn a_flipped_sprite_stays_flipped_and_cells_past_the_sheet_show_its_last() {
        let flipbook = Flipbook {
            columns: 2,
            rows: 2,
            first: 2,
            frames: 4,
            rate: 64.0,
            playback: Playback::Loop,
            ..Flipbook::default()
        };
        let (mut world, entity) = playing(flipbook);
        let flipped = [Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0)];
        world.get_mut::<Sprite>(entity).expect("a sprite").rect = flipped;
        step(&mut world);
        let rect = world.get::<Sprite>(entity).expect("a sprite").rect;
        // Cell 3, bottom right, flipped across.
        assert_eq!(rect, [Vec2::new(1.0, 0.5), Vec2::new(0.5, 1.0)]);
        step(&mut world);
        let rect = world.get::<Sprite>(entity).expect("a sprite").rect;
        // Cell 4 is past the sheet: shows cell 3.
        assert_eq!(rect, [Vec2::new(1.0, 0.5), Vec2::new(0.5, 1.0)]);
    }
}
