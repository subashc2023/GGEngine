//! SMAA 1x's lookup textures (subsample offset 0), by the algorithm of `Scripts/AreaTex.py` and
//! `Scripts/SearchTex.py` in <https://github.com/iryoku/smaa>, under the MIT license.
//!
//! - Area: from a line of edges' shape, distances to its ends and crossings there, to the shares of
//!   a pixel and its neighbor across the edge that the revectorized line cuts off.
//! - Search: from the edges a search's last bilinear fetch found, to how many of its two pixels the
//!   line goes on through.

/// The reference's first 80 rows (subsample offset 0): orthogonal patterns in the left 80 columns,
/// diagonal in the right 80.
pub(super) const AREA_SIZE: [u32; 2] = [160, 80];

/// The reference's packed size, cropped from 66 by 33.
pub(super) const SEARCH_SIZE: [u32; 2] = [64, 16];

/// An orthogonal pattern's square, in texels each way, its distances the texels' squares:
/// `SMAA_AREATEX_MAX_DISTANCE`.
const ORTHO: usize = 16;

/// A diagonal pattern's square, in texels each way, a texel a distance:
/// `SMAA_AREATEX_MAX_DISTANCE_DIAG`.
const DIAG: usize = 20;

/// The U length from which areas are the lines' alone, shorter ones smoothed: the reference's
/// `SMOOTH_MAX_DISTANCE`.
const SMOOTH_MAX_DISTANCE: f64 = 32.0;

/// The area texture's RGBA8 rows, top first: red, a pixel's share of its neighbor across its edge;
/// green, the neighbor's share of it.
pub(super) fn area() -> Vec<u8> {
    let _zone = gg_core::zone!();
    let [width, height] = AREA_SIZE.map(|side| side as usize);
    let mut texels = vec![0; width * height * 4];
    let mut put = |x: usize, y: usize, [r, g]: [f64; 2]| {
        let at = (y * width + x) * 4;
        // Within 0 to 1 by construction.
        texels[at] = (r * 255.0).round() as u8;
        texels[at + 1] = (g * 255.0).round() as u8;
    };
    // Orthogonal patterns, a square per start and end crossing. A crossing toward the pixel counts
    // 3, toward its neighbor 1, as the shader's rounded bilinear fetch weighs them.
    for start in 0..4 {
        for end in 0..4 {
            let tile = |down: usize, up: usize| 3 * down + up;
            let (start_down, start_up, end_down, end_up) =
                (start & 1, start >> 1, end & 1, end >> 1);
            let pattern = start_down | end_down << 1 | start_up << 2 | end_up << 3;
            let (x0, y0) = (
                ORTHO * tile(start_down, start_up),
                ORTHO * tile(end_down, end_up),
            );
            for x in 0..ORTHO {
                for y in 0..ORTHO {
                    let (left, right) = ((x * x) as f64, (y * y) as f64);
                    put(x0 + x, y0 + y, ortho(pattern, left, right));
                }
            }
        }
    }
    // Diagonal patterns, past the orthogonal ones' five squares, a square per merged start and end
    // crossing.
    for start in 0..4 {
        for end in 0..4 {
            for x in 0..DIAG {
                for y in 0..DIAG {
                    let areas = diag(start, end, x as f64, y as f64);
                    put(5 * ORTHO + DIAG * start + x, DIAG * end + y, areas);
                }
            }
        }
    }
    texels
}

/// The areas of orthogonal `pattern` for a pixel `left` pixels from the line's start and `right`
/// from its end. The edge runs along y = 0 from 0 to the length, the pixel (`left` to `left + 1`)
/// below and its neighbor above. The bits are crossings: at the start toward the pixel, at the end
/// so, at the start toward the neighbor, at the end so.
///
/// - L, a corner: from one end's crossing's middle to the line's middle.
/// - Z, a slanted edge's step: from a crossing's middle on one side to the other side's at the
///   other end.
/// - U, a bump: an L at each end on one side, which [`smooth`] rounds where short.
/// - No crossing, or crossed through at an end (ambiguous): unchanged.
fn ortho(pattern: usize, left: f64, right: f64) -> [f64; 2] {
    let length = left + right + 1.0;
    let middle = length / 2.0;
    // A crossing's middle, toward the neighbor and toward the pixel.
    let (up, down) = (0.5, -0.5);
    let line = |from: [f64; 2], to: [f64; 2]| ortho_area(from, to, left);
    match pattern {
        // L patterns span the line's half at their crossing's end. The reference takes an odd
        // line's middle pixel on either side.
        1 if left <= right => line([0.0, down], [middle, 0.0]),
        2 if left >= right => line([middle, 0.0], [length, down]),
        4 if left <= right => line([0.0, up], [middle, 0.0]),
        8 if left >= right => line([middle, 0.0], [length, up]),
        3 => smooth(
            length,
            line([0.0, down], [middle, 0.0]),
            line([middle, 0.0], [length, down]),
        ),
        12 => smooth(
            length,
            line([0.0, up], [middle, 0.0]),
            line([middle, 0.0], [length, up]),
        ),
        // The Z patterns, and those crossed through at one end, which take the other end's.
        6 | 7 | 14 => line([0.0, up], [length, down]),
        9 | 11 | 13 => line([0.0, down], [length, up]),
        _ => [0.0, 0.0],
    }
}

/// The areas between the line `from`–`to` and the edge, y = 0, over the pixel `x` to `x + 1`,
/// within the line's span: below (the pixel's take of its neighbor) and above (the neighbor's of
/// it).
fn ortho_area(from: [f64; 2], to: [f64; 2], x: f64) -> [f64; 2] {
    let (x1, x2) = (x.max(from[0]), (x + 1.0).min(to[0]));
    if x1 >= x2 {
        return [0.0, 0.0];
    }
    let y = |at: f64| from[1] + (to[1] - from[1]) * (at - from[0]) / (to[0] - from[0]);
    let (y1, y2) = (y(x1), y(x2));
    // Each part's area, negative below the edge, either side of a crossing within the pixel.
    let parts = if y1 * y2 < 0.0 {
        let crossing = x1 + (x2 - x1) * y1 / (y1 - y2);
        [y1 * (crossing - x1) / 2.0, y2 * (x2 - crossing) / 2.0]
    } else {
        [(y1 + y2) / 2.0 * (x2 - x1), 0.0]
    };
    let below = parts.iter().filter(|&&a| a < 0.0).map(|a| -a).sum();
    let above = parts.iter().filter(|&&a| a > 0.0).sum();
    [below, above]
}

/// A U's areas: its lines', each smoothed toward half the square root of twice itself, the more the
/// shorter the U. A short bump then blends more than its lines' small triangles would.
fn smooth(length: f64, start: [f64; 2], end: [f64; 2]) -> [f64; 2] {
    let p = (length / SMOOTH_MAX_DISTANCE).clamp(0.0, 1.0);
    let smoothed = |a: f64| {
        let b = (a * 2.0).sqrt() * 0.5;
        b + (a - b) * p
    };
    [0, 1].map(|i| smoothed(start[i]) + smoothed(end[i]))
}

/// The areas of a diagonal pattern, a line of pixels rising to the right, each with edges on its
/// left and top; the pixel is `left` from the lower end and `right` from the upper. In unit
/// squares, Y up, from (1, 0): the share of the pixel at (1 + left, left) above the revectorized
/// line, and of its upper neighbor, (1 + left, 1 + left), below it.
///
/// `start` and `end` are the shader's merged crossings. At the start, 1 is going on down (an edge
/// on its left) and 2 going on left (atop the pixel left of it); at the end, 2 is right and 1 up.
/// Each is a two-pixel step where a line off 45° meets its staircase. Down or right puts that end
/// on the outer corners' line, y = x - 1; left or up on the inner corners', y = x. An unknown end
/// (the search ran out, or both go on) averages both lines, as the reference does; two unknown ends
/// average the inner and outer corners' lines.
fn diag(start: usize, end: usize, left: f64, right: f64) -> [f64; 2] {
    let length = left + right + 1.0;
    let starts: &[[f64; 2]] = match start {
        0 => &[[0.0, 0.0], [1.0, 0.0]],
        1 => &[[1.0, 0.0]],
        2 => &[[1.0, 1.0]],
        _ => &[[1.0, 1.0], [1.0, 0.0]],
    };
    let (inner, outer) = ([length, length], [1.0 + length, length]);
    let ends: &[[f64; 2]] = match end {
        0 => &[inner, outer],
        1 => &[[1.0 + length, 1.0 + length]],
        2 => &[outer],
        _ => &[[1.0 + length, 1.0 + length], outer],
    };
    let lines = starts.len().max(ends.len());
    let mut areas = [0.0; 2];
    for i in 0..lines {
        let (from, to) = (starts[i.min(starts.len() - 1)], ends[i.min(ends.len() - 1)]);
        let below = covered(from, to, [1.0 + left, left]);
        let above = covered(from, to, [1.0 + left, 1.0 + left]);
        areas[0] += (1.0 - below) / lines as f64;
        areas[1] += above / lines as f64;
    }
    areas
}

/// How much of the unit square at lower left `corner` lies right of the line `from`–`to`, looking
/// along it: below a line rising to the right.
fn covered(from: [f64; 2], to: [f64; 2], corner: [f64; 2]) -> f64 {
    let side =
        |[x, y]: [f64; 2]| (to[1] - from[1]) * (x - from[0]) - (to[0] - from[0]) * (y - from[1]);
    let [x, y] = corner;
    let square = [[x, y], [x + 1.0, y], [x + 1.0, y + 1.0], [x, y + 1.0]];
    // The square clipped to the line's right: one corner more at most.
    let mut clipped = [[0.0; 2]; 5];
    let mut count = 0;
    for (i, &a) in square.iter().enumerate() {
        let b = square[(i + 1) % 4];
        let (sa, sb) = (side(a), side(b));
        if sa >= 0.0 {
            clipped[count] = a;
            count += 1;
        }
        if (sa >= 0.0) != (sb >= 0.0) {
            let t = sa / (sa - sb);
            clipped[count] = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            count += 1;
        }
    }
    let twice: f64 = (0..count)
        .map(|i| {
            let ([x1, y1], [x2, y2]) = (clipped[i], clipped[(i + 1) % count]);
            x1 * y2 - x2 * y1
        })
        .sum();
    twice.abs() / 2.0
}

/// The search texture's RGBA8 rows, top first, in red: per bilinear fetch of a search, how many of
/// its two pixels the line goes on through, × 127. A fetch sits a quarter pixel and an eighth off a
/// pixel's middle, toward the pixel before along the search and the row above.
///
/// Columns are the crossing (left) edges' fetch, rows the line's (top) edges', in 32nds. The left
/// half serves left searches, the right half right ones. Rows are flipped and cropped to where the
/// line goes on through the nearer pixel, as the reference packs them.
pub(super) fn search() -> Vec<u8> {
    let _zone = gg_core::zone!();
    // A fetch in 32nds, of the row above's farther and nearer pixels, then the line's row's:
    // weighed ¼ and ¾ across, ⅛ and ⅞ down.
    let fetched = |e: [bool; 4]| {
        usize::from(e[0]) + 3 * usize::from(e[1]) + 7 * usize::from(e[2]) + 21 * usize::from(e[3])
    };
    let bits = |of: usize| std::array::from_fn(|i| of >> i & 1 == 1);
    let mut full = [[0u8; 66]; 33];
    for crossing in 0..16 {
        for line in 0..16 {
            let (crossing, line): ([bool; 4], [bool; 4]) = (bits(crossing), bits(line));
            let (x, y) = (fetched(crossing), fetched(line));
            full[y][x] = 127 * search_left(crossing, line);
            full[y][33 + x] = 127 * search_right(crossing, line);
        }
    }
    let [width, height] = SEARCH_SIZE.map(|side| side as usize);
    let mut texels = vec![0; width * height * 4];
    for row in 0..height {
        for column in 0..width {
            texels[(row * width + column) * 4] = full[32 - row][column];
        }
    }
    texels
}

/// How many of a left search's last two pixels the line goes on through: the nearer if it has the
/// line's edge, then the farther if it does too and no edge crosses between. Arrays are the row
/// above's farther and nearer pixels, then the line's row's; a crossing edge is on a pixel's left.
fn search_left(crossing: [bool; 4], line: [bool; 4]) -> u8 {
    let nearer = line[3];
    let farther = nearer && line[2] && !crossing[1] && !crossing[3];
    u8::from(nearer) + u8::from(farther)
}

/// [`search_left`] to the right: the nearer pixel is still the one weighed ¾, and a crossing on a
/// pixel's left lies before it. The nearer counts if it has the line's edge and none crosses before
/// it, then the farther likewise.
fn search_right(crossing: [bool; 4], line: [bool; 4]) -> u8 {
    let nearer = line[3] && !crossing[1] && !crossing[3];
    let farther = nearer && line[2] && !crossing[0] && !crossing[2];
    u8::from(nearer) + u8::from(farther)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row's edges as the shader's searches read them: each pixel's top (the line's) and left (a
    /// crossing), in its row and the row above.
    struct Row {
        line: Vec<bool>,
        crossing: Vec<bool>,
        crossing_above: Vec<bool>,
    }

    impl Row {
        /// The bilinear fetch `x` pixels across (a middle at half past its index), ⅛ pixel into the
        /// row above, clamped to the row: the crossing edges' and the line's (none above).
        fn fetch(&self, x: f64) -> [f64; 2] {
            let last = self.line.len() as i64 - 1;
            let left = (x - 0.5).floor();
            let t = x - 0.5 - left;
            let at = |of: &Vec<bool>, i: f64| {
                f64::from(u8::from(of[(i as i64).clamp(0, last) as usize]))
            };
            let lerp = |of: &Vec<bool>| at(of, left) * (1.0 - t) + at(of, left + 1.0) * t;
            let crossing = lerp(&self.crossing) * 0.875 + lerp(&self.crossing_above) * 0.125;
            [crossing, lerp(&self.line) * 0.875]
        }

        /// The search texture's value at fetch `e`, as the shader's `search_length` reads it.
        fn length(texels: &[u8], e: [f64; 2], right: bool) -> f64 {
            let offset = if right { 0.5 } else { 0.0 };
            let column = 32.0 * e[0] + 66.0 * offset;
            let row = 32.0 - 32.0 * e[1];
            let (column, row) = (column.round() as usize, row.round() as usize);
            if column >= 64 || row >= 16 {
                return 0.0;
            }
            f64::from(texels[(row * 64 + column) * 4]) / 255.0
        }

        /// How far the line runs left or right from pixel `x`, as the shader finds it: fetches two
        /// pixels apart, from ¼ pixel off its middle, while both have the line and none crosses, at
        /// most 16. The search texture resolves the last fetch.
        fn search(&self, texels: &[u8], x: usize, right: bool) -> f64 {
            let center = x as f64 + 0.5;
            let step = if right { 2.0 } else { -2.0 };
            let mut at = if right { center + 1.25 } else { center - 0.25 };
            let end = at + step * 16.0;
            let mut e = [0.0, 1.0];
            while (if right { at < end } else { at > end }) && e[1] > 0.8281 && e[0] == 0.0 {
                e = self.fetch(at);
                at += step;
            }
            let offset = 3.25 - (255.0 / 127.0) * Self::length(texels, e, right);
            let found = if right { at - offset } else { at + offset };
            (found - center).round().abs()
        }
    }

    /// The searches find the ends of lines of every length to 30, from each pixel, ended or crossed
    /// at any pixel's left, in the line's row or the row above.
    #[test]
    fn searches_find_the_ends_of_lines() {
        let texels = search();
        for length in 1..=30 {
            for crossed in [None, Some((5, false)), Some((5, true)), Some((8, false))] {
                // The line from pixel 4, in a row with room round it.
                let size = length + 10;
                let mut row = Row {
                    line: vec![false; size],
                    crossing: vec![false; size],
                    crossing_above: vec![false; size],
                };
                row.line[4..4 + length].fill(true);
                if let Some((at, above)) = crossed.filter(|&(at, _)| at < 4 + length) {
                    let crossing = if above {
                        &mut row.crossing_above
                    } else {
                        &mut row.crossing
                    };
                    crossing[at] = true;
                }
                // Where the line's crossed, it's two lines, which end either side of the crossing.
                let cut = crossed.map(|(at, _)| at).filter(|&at| at < 4 + length);
                for x in 4..4 + length {
                    let start = cut.filter(|&at| at <= x).unwrap_or(4);
                    let end = cut.filter(|&at| at > x).map_or(4 + length - 1, |at| at - 1);
                    let what = format!("a line of {length} crossed at {crossed:?}, from {x}");
                    assert_eq!(
                        row.search(&texels, x, false),
                        (x - start) as f64,
                        "left: {what}"
                    );
                    assert_eq!(
                        row.search(&texels, x, true),
                        (end - x) as f64,
                        "right: {what}"
                    );
                }
            }
        }
    }

    /// An L's and a Z's areas are the triangles between their lines and the edge: an L's across its
    /// half at its corner, half a pixel high there; a Z's either side of its middle, the pixel's
    /// below the first half, its neighbor's above the second.
    #[test]
    fn orthogonal_areas_are_the_triangles_their_lines_cut() {
        for length in [4.0f64, 9.0, 16.0] {
            let pixels = 0..length as usize;
            let along = |pattern| {
                pixels
                    .clone()
                    .map(move |x| ortho(pattern, x as f64, length - 1.0 - x as f64))
            };
            let total =
                |pattern, i: usize| along(pattern).map(|areas: [f64; 2]| areas[i]).sum::<f64>();
            let triangle = length / 2.0 * 0.5 / 2.0;
            assert!((total(1, 0) - triangle).abs() < 1.0e-9, "an L of {length}");
            assert_eq!(total(1, 1), 0.0, "an L of {length}");
            for i in 0..2 {
                assert!((total(6, i) - triangle).abs() < 1.0e-9, "a Z of {length}");
            }
            assert_eq!(
                total(0, 0) + total(0, 1),
                0.0,
                "a line of {length} with no crossing"
            );
        }
    }

    /// Diagonal lines' blends with the neighbor above:
    ///
    /// - Ends unknown: a quarter of each pixel, the mean of the inner and outer corners' lines.
    /// - Down at the start, right at the end: the outer corners' line, halving each pixel.
    /// - Left and up: the inner corners' line, halving each neighbor.
    /// - Left at the start, right at the end: shallower, from the inner corner at its start, (1,
    ///   1), to the outer at its end.
    #[test]
    fn diagonal_areas_are_those_of_their_lines() {
        let close = |[r, g]: [f64; 2], [x, y]: [f64; 2]| {
            assert!((r - x).abs() < 1.0e-9 && (g - y).abs() < 1.0e-9, "{r}, {g}");
        };
        for (left, right) in [(0.0, 7.0), (5.0, 5.0), (9.0, 1.0)] {
            close(diag(0, 0, left, right), [0.25, 0.25]);
            close(diag(1, 2, left, right), [0.5, 0.0]);
            close(diag(2, 1, left, right), [0.0, 0.5]);
        }
        // From (1, 1) to (12, 11): over the first pixel, 1 to 2 across, it rises from 1 to 1 +
        // 10/11, above the pixel and through its neighbor's lower part.
        close(diag(2, 2, 0.0, 10.0), [0.0, 0.5 * 10.0 / 11.0]);
    }

    /// Both textures are of their sizes, in RGBA8. Every area is within 0 to 1 before rounding to 8
    /// bits, which would hide one out of range.
    #[test]
    fn the_textures_are_their_sizes_and_their_areas_within_0_to_1() {
        let pixels = |[width, height]: [u32; 2]| (width * height * 4) as usize;
        assert_eq!(area().len(), pixels(AREA_SIZE));
        assert_eq!(search().len(), pixels(SEARCH_SIZE));
        let within = |areas: [f64; 2], what: &str| {
            for area in areas {
                assert!((0.0..=1.0).contains(&area), "{what}: {area}");
            }
        };
        for left in 0..ORTHO {
            for right in 0..ORTHO {
                let (left, right) = ((left * left) as f64, (right * right) as f64);
                for pattern in 0..16 {
                    let what = format!("pattern {pattern} at {left} and {right}");
                    within(ortho(pattern, left, right), &what);
                }
            }
        }
        for left in 0..DIAG {
            for right in 0..DIAG {
                let (left, right) = (left as f64, right as f64);
                for (start, end) in (0..4).flat_map(|start| (0..4).map(move |end| (start, end))) {
                    let what = format!("diagonal {start}, {end} at {left} and {right}");
                    within(diag(start, end, left, right), &what);
                }
            }
        }
    }
}
