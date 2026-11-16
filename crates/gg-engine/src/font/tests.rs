use super::*;

/// Inter, baked.
fn inter() -> (Baked, Field) {
    let field = super::field(INTER).expect("Inter bakes");
    let font = FontRef::new(INTER).expect("Inter reads");
    let shaping = ShaperData::new(&font);
    let shaper = shaping.shaper(&font).build();
    let latin = Some(harfrust::script::LATIN);
    let plan = ShapePlan::new(&shaper, Direction::LeftToRight, latin, None, &features());
    let baked = Baked {
        bytes: INTER.into(),
        shaping,
        plan,
        glyphs: field.glyphs.clone(),
        units_per_em: field.units_per_em,
        lines: field.lines,
    };
    (baked, field)
}

/// What the field holds at `uv`, filtered linearly as the GPU's sampler does, clamped to its
/// edges: its red, green and blue, from 0 to 1.
fn sample(field: &Field, uv: Vec2) -> [f32; 3] {
    let [width, height] = field.size;
    let texel = |x: i64, y: i64| {
        let x = x.clamp(0, i64::from(width) - 1) as usize;
        let y = y.clamp(0, i64::from(height) - 1) as usize;
        let at = (y * width as usize + x) * 4;
        [0, 1, 2].map(|c| f32::from(field.pixels[at + c]) / 255.0)
    };
    let x = uv.x * width as f32 - 0.5;
    let y = uv.y * height as f32 - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (x0, y0) = (x0 as i64, y0 as i64);
    let corners = [texel(x0, y0), texel(x0 + 1, y0), texel(x0, y0 + 1)];
    let [a, b, c] = corners;
    let d = texel(x0 + 1, y0 + 1);
    [0, 1, 2].map(|i| {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bottom = c[i] + (d[i] - c[i]) * fx;
        top + (bottom - top) * fy
    })
}

fn median([r, g, b]: [f32; 3]) -> f32 {
    r.min(g).max(r.max(g).min(b))
}

/// Glyph `character`'s coverage of each pixel of a square `pixels` across an em, its origin at
/// the square's bottom left, as the text shader draws it from `field`, and as its outline covers
/// each, sampled 4 by 4 within it: those that differ by more than half, and the pixels covered.
fn coverage(baked: &Baked, field: &Field, character: char, pixels: u32) -> (usize, usize) {
    let font = FontRef::new(INTER).expect("Inter reads");
    let id = font.charmap().map(character).expect("Inter maps it");
    let placed = baked
        .placed(id.to_u32(), "", 0)
        .expect("its glyph is in the field");
    let mut pen = Pen::default();
    let outline = font.outline_glyphs().get(id).expect("its outline");
    let settings = DrawSettings::unhinted(Size::unscaled(), LocationRef::default());
    outline.draw(settings, &mut pen).expect("its outline draws");
    let units = f64::from(
        font.metrics(Size::unscaled(), LocationRef::default())
            .units_per_em,
    );
    let mut shape: Shape<Contour> = Shape::default();
    for contour in &pen.contours {
        let segments = contour
            .iter()
            .filter_map(|command| command.segment(|at| at));
        shape.contours.push(Contour {
            segments: segments.collect(),
        });
    }
    let prepared = shape.prepare();
    let [low, high] = placed.plane;
    // How many of the view's pixels the field's range spans, as the shader finds it.
    let range = (RANGE as f32 * pixels as f32 / EM as f32).max(1.0);
    let (mut wrong, mut covered) = (0, 0);
    let per_em = pixels as f32;
    let (from, to) = ((low * per_em).floor(), (high * per_em).ceil());
    for py in from.y as i32..to.y as i32 {
        let lines: Vec<_> = (0..4)
            .map(|sy| {
                let y = (py as f64 + (sy as f64 + 0.5) / 4.0) / f64::from(pixels) * units;
                prepared.scanline(y)
            })
            .collect();
        for px in from.x as i32..to.x as i32 {
            let at = Vec2::new(px as f32 + 0.5, py as f32 + 0.5) / per_em;
            let across = (at - low) / (high - low);
            let [top_left, bottom_right] = placed.rect;
            let uv = Vec2::new(
                top_left.x + (bottom_right.x - top_left.x) * across.x,
                bottom_right.y + (top_left.y - bottom_right.y) * across.y,
            );
            let inside = (median(sample(field, uv)) - 0.5) * range;
            let drawn = (inside + 0.5).clamp(0.0, 1.0);
            let mut hits = 0;
            for line in &lines {
                let mut cursor = line.cursor();
                for sx in 0..4 {
                    let x = (px as f64 + (sx as f64 + 0.5) / 4.0) / f64::from(pixels) * units;
                    hits += usize::from(cursor.filled(x, FillRule::Nonzero));
                }
            }
            let real = hits as f32 / 16.0;
            covered += usize::from(real > 0.0);
            wrong += usize::from((drawn - real).abs() > 0.5);
        }
    }
    (wrong, covered)
}

/// A field draws each glyph as its outline covers pixels, at sizes above and below its own: of
/// the pixels a glyph covers, at most one in a hundred is drawn more than half wrong, as where
/// a corner rounds off a little; and a glyph whose cell were placed, flipped or scaled wrongly,
/// or whose distances' signs were the wrong way, would draw most of them wrong.
#[test]
fn a_field_draws_its_glyphs_as_their_outlines_cover() {
    let (baked, field) = inter();
    for character in ['A', 'g', '&', '@', 'é', '€', 'W'] {
        for pixels in [16, 48, 160] {
            let (wrong, covered) = coverage(&baked, &field, character, pixels);
            assert!(
                wrong * 100 <= covered,
                "{character:?} at {pixels} pixels an em: {wrong} of {covered} drawn wrong"
            );
        }
    }
}

/// A baked font reads back from what a game's package holds of it, its field, glyphs, metrics
/// and file alike, its shaping made again from its file; what's cut short, has bytes past its
/// field or has fewer than 1 unit per em, which text's positions are divided by, is refused.
#[test]
fn a_baked_font_reads_back_from_its_bytes() {
    let bake = bake_inter();
    let bytes = bake.bytes();
    let read = Bake::read(&bytes).expect("it reads back");
    assert_eq!(read.size, bake.size);
    assert_eq!(read.pixels, bake.pixels);
    assert_eq!(read.baked.glyphs, bake.baked.glyphs);
    assert_eq!(read.baked.units_per_em, bake.baked.units_per_em);
    assert_eq!(read.baked.lines, bake.baked.lines);
    assert_eq!(read.baked.bytes, bake.baked.bytes);
    assert_eq!(read.bytes(), bytes);
    assert!(Bake::read(&bytes[..bytes.len() - 1]).is_err(), "cut short");
    // Its units per em, after the field's width and height.
    for units in [0.0_f32, -2048.0, 0.5] {
        let mut unitless = bytes.clone();
        unitless[8..12].copy_from_slice(&units.to_le_bytes());
        let error = Bake::read(&unitless).err();
        assert!(
            error.is_some_and(|error| error.contains("units per em")),
            "{units}"
        );
    }
    let mut longer = bytes;
    longer.push(0);
    assert!(Bake::read(&longer).is_err(), "a byte past its field");
}

/// A field holds a glyph for each character of Windows-1252 that the font maps, a space's
/// without a cell, and the `.notdef`, which draws for a character the font lacks, but not for
/// whitespace or a control character; and fits a texture every device takes.
#[test]
fn a_field_holds_windows_1252s_glyphs() {
    let (baked, field) = inter();
    let font = FontRef::new(INTER).expect("Inter reads");
    let charmap = font.charmap();
    for character in characters() {
        let id = charmap
            .map(character)
            .expect("Inter maps Windows-1252")
            .to_u32();
        let held = baked.glyphs.binary_search_by_key(&id, |&(id, _)| id);
        assert!(held.is_ok(), "{character:?}");
    }
    let space = charmap.map(' ').expect("a space").to_u32();
    assert_eq!(baked.placed(space, " ", 0), None);
    let notdef = baked.placed(0, "漢", 0).expect("the .notdef");
    assert_eq!(baked.placed(0, "\t", 0), None);
    assert_eq!(baked.placed(0, "\u{7}", 0), None);
    let a = charmap.map('a').expect("an a").to_u32();
    assert_ne!(baked.placed(a, "a", 0), Some(notdef));
    let [width, height] = field.size;
    assert!(width <= 1024 && height <= 1024, "{width}x{height}");
    assert_eq!(field.pixels.len(), width as usize * height as usize * 4);
}

/// Text lays out left to right in lines a line apart, each aligned across as its style says,
/// the box's middle or bottom at the origin as it says; and kerned, so that `AV` is narrower
/// than an `A` and a `V` apart.
#[test]
fn text_lays_out_in_aligned_lines_kerned() {
    let (baked, _) = inter();
    let mut fonts = Fonts::new(&mut Textures::default());
    let font = fonts.push(Texture::WHITE, Some(baked));
    let style = |align| TextStyle {
        font: Some(font),
        size: 2.0,
        color: Vec4::ONE,
        align,
    };
    // Each glyph's middle, across and up.
    let middles = |fonts: &mut Fonts, text: &str, align| -> Vec<Vec2> {
        let glyphs = fonts.lay_out(Mat4::IDENTITY, text, &style(align), 0);
        glyphs
            .map(|(at, ..)| at.w_axis.truncate().truncate())
            .collect()
    };
    let start = [Align::Start; 2];
    let [a, v] = middles(&mut fonts, "A V", start)[..] else {
        panic!("an A and a V, the space drawing nothing")
    };
    let [a_kerned, v_kerned] = middles(&mut fonts, "AV", start)[..] else {
        panic!("an A and a V")
    };
    assert_eq!(a, a_kerned);
    assert!(v_kerned.x < v.x, "{v_kerned} is kerned toward the A");
    // Its first line's ascent at the top, and the next line's baseline a line below.
    let lines = fonts.fonts[0].baked.as_ref().expect("baked").lines;
    assert!(a.y < 0.0 && a.y > -lines.ascent * 2.0, "{a}");
    let [first, second] = middles(&mut fonts, "A\nA", start)[..] else {
        panic!("two As")
    };
    assert_eq!(first, a);
    let below = first.y - lines.advance() * 2.0;
    assert!((second.y - below).abs() < 1e-5, "{second} on the next line");
    // Its middle, then its bottom, at the origin: the box is the same, a line tall.
    let middle = middles(&mut fonts, "A", [Align::Center; 2])[0];
    let end = middles(&mut fonts, "A", [Align::End; 2])[0];
    let tall = (lines.ascent - lines.descent) * 2.0;
    assert!((middle.y - (a.y + tall / 2.0)).abs() < 1e-5, "{middle}");
    assert!((end.y - (a.y + tall)).abs() < 1e-5, "{end}");
    // Across, half its width left of where it starts, then all of it.
    assert!(end.x < middle.x && middle.x < a.x, "{a} {middle} {end}");
    assert!(((a.x - middle.x) * 2.0 - (a.x - end.x)).abs() < 1e-5);
    // Each line aligned on its own: the shorter line's middle is the longer's.
    let [_, _, b] = middles(&mut fonts, "AA\nB", [Align::Center, Align::Start])[..] else {
        panic!("three letters")
    };
    let centered = middles(&mut fonts, "B", [Align::Center, Align::Start])[0];
    assert!((b.x - centered.x).abs() < 1e-5, "{b} {centered}");
}

/// Text measures as it lays out: as wide as its widest line's advance, kerned, which aligns it,
/// and as tall as its box, from its first line's ascent to its last line's descent, a line
/// more for each line more; text of nothing is a line tall and nothing across.
#[test]
fn text_measures_as_it_lays_out() {
    let (baked, _) = inter();
    let mut fonts = Fonts::new(&mut Textures::default());
    let font = fonts.push(Texture::WHITE, Some(baked));
    let style = TextStyle {
        font: Some(font),
        size: 2.0,
        color: Vec4::ONE,
        align: [Align::Start; 2],
    };
    let lines = fonts.fonts[0].baked.as_ref().expect("baked").lines;
    let [a, v, av] = ["A", "V", "AV"].map(|text| fonts.measure(text, &style));
    assert!(av.x < a.x + v.x, "kerned: {av} against {a} and {v}");
    let tall = (lines.ascent - lines.descent) * 2.0;
    assert!((a.y - tall).abs() < 1e-5, "{a}");
    let two = fonts.measure("AV\nA", &style);
    assert_eq!(two.x, av.x, "its widest line");
    assert!((two.y - tall - lines.advance() * 2.0).abs() < 1e-5, "{two}");
    assert_eq!(fonts.measure("", &style), Vec2::new(0.0, a.y));
    // Laid out from its right, it starts as far left of the origin as it's wide.
    let first = |fonts: &mut Fonts, align| {
        let style = TextStyle { align, ..style };
        let mut glyphs = fonts.lay_out(Mat4::IDENTITY, "AV", &style, 0);
        let (at, ..) = glyphs.next().expect("an A");
        at.w_axis.x
    };
    let start = first(&mut fonts, [Align::Start; 2]);
    let end = first(&mut fonts, [Align::End, Align::Start]);
    assert!((start - end - av.x).abs() < 1e-4, "{start} {end} {av}");
}

/// Inter with `units` font units to its em, as its `head` table says, its outlines as they are,
/// so that its glyphs are `2048 / units` times as large as its own.
fn inter_of(units: u16) -> Vec<u8> {
    let mut bytes = INTER.to_vec();
    let tables = usize::from(u16::from_be_bytes([bytes[4], bytes[5]]));
    // Each table's record, past the font's 12 bytes of header: its tag, its checksum, where it
    // starts and its length.
    let record = (0..tables)
        .map(|table| 12 + 16 * table)
        .find(|&at| &bytes[at..at + 4] == b"head")
        .expect("a head table");
    let start = bytes[record + 8..record + 12].try_into().expect("4 bytes");
    let head = u32::from_be_bytes(start) as usize;
    // Past its version, its font's revision, its checksum's adjustment, its magic number and
    // its flags.
    bytes[head + 18..head + 20].copy_from_slice(&units.to_be_bytes());
    bytes
}

/// A font whose glyph's cell would be larger than a field can be, or whose cells would come to
/// more than a field holds, is refused before any cell is made, saying which, with the font's
/// error: Inter at a unit to the em, whose cells would take tens of gigabytes each, and at 64,
/// megabytes each and gigabytes together, which would otherwise be made, or fail to be and
/// abort. An outline with a point that isn't a number is refused too.
#[test]
fn a_font_too_large_for_a_field_is_refused_before_its_cells_are_made() {
    let path = Path::new("large.ttf");
    let refused = bake_bytes("large", path, inter_of(1).into());
    let error = refused.err().expect("refused");
    assert_eq!(error.code(), "engine.bad_font");
    let message = error.to_string();
    let alone = "too large for a field of 8192 texels square: its cell, at 48 texels an em";
    assert!(message.contains(alone), "{message}");
    let error = super::field(&inter_of(64)).err().expect("refused");
    assert!(
        error.contains("has glyphs too large together for a field of 8192 texels square"),
        "{error}"
    );
    let nowhere = Point::new(f64::NAN, 1.0);
    let contour = vec![Command::Line(Point::new(0.0, 0.0), nowhere)];
    let error = outline(1, &[contour], 2048.0).err().expect("refused");
    assert!(error.contains("isn't a finite number"), "{error}");
}

/// A process bakes a font's bytes once: baked again, they take the field the first bake made;
/// other bytes bake apart. Inter at 8,192 and 16,384 units to the em, a quarter and an eighth of
/// its size, quick to bake, which no other test bakes, so that each first bake here is the
/// process's first.
#[test]
fn a_process_bakes_a_fonts_bytes_once() {
    let bytes = inter_of(8192);
    let (first, cached) = field_of(&bytes).expect("it bakes");
    assert!(!cached, "the process's first bake of these bytes");
    let (again, cached) = field_of(&bytes).expect("it bakes again");
    assert!(cached, "the field the first bake made");
    assert_eq!(again.size, first.size);
    assert_eq!(again.pixels, first.pixels);
    assert_eq!(again.glyphs, first.glyphs);
    assert_eq!(again.units_per_em, first.units_per_em);
    assert_eq!(again.lines, first.lines);
    let (other, cached) = field_of(&inter_of(16384)).expect("it bakes");
    assert!(!cached, "other bytes bake apart");
    assert_ne!(other.units_per_em, first.units_per_em);
}

/// A file that isn't a font, or is one without outlines, is refused, saying why.
#[test]
fn what_isnt_a_font_of_outlines_is_refused() {
    let error = super::field(b"not a font").err().expect("refused");
    assert!(
        error.contains("isn't a TrueType or OpenType font"),
        "{error}"
    );
    let path = Path::new("no such folder/title.ttf");
    let error = bake("title", path).err().expect("refused");
    assert_eq!(error.code(), "engine.bad_font");
    assert!(error.to_string().contains("\"title\""), "{error}");
}
