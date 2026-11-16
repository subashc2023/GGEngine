//! Fonts: TrueType and OpenType files that layers load or text names, and the engine's own, Inter.
//! Each bakes on load into a field: a data texture of multi-channel signed distance fields, one
//! per glyph of Windows-1252's printable characters, so glyphs draw crisp at any size. Text is
//! shaped with kerning, laid out in lines and drawn as those glyphs by [`Context::draw_text`] and,
//! for a [`Text`], the world.
//!
//! [`Context::draw_text`]: crate::Context::draw_text

use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;
use std::vec::Drain;

use fdsm::bezier::scanline::FillRule;
use fdsm::bezier::{Point, Segment};
use fdsm::correct_error::{ErrorCorrectionConfig, correct_error_mtsdf};
use fdsm::generate::generate_mtsdf;
use fdsm::render::correct_sign_mtsdf;
use fdsm::shape::{Contour, Shape};
use gg_core::math::{Mat4, Vec2, Vec4};
use gg_core::{Error, Result};
use gg_renderer::{Glyph, MAX_TEXTURE_SIZE, Texels, Texture, Textures};
use harfrust::{Direction, Feature, ShapeOptions, ShapePlan, ShaperData, Tag, UnicodeBuffer};
use image::{ImageBuffer, Rgba};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use tracing::debug;

use crate::package::{self, Files};
use crate::{Component, Reflect, dirs, jobs};

/// The engine's font, for text naming none: Inter regular, digits tabular, as the UI draws.
pub(crate) static INTER: &[u8] = include_bytes!("../assets/fonts/Inter/Inter-Regular.ttf");

/// A font [`Context::add_font`](crate::Context::add_font) or named text loaded: its index in its
/// context, held by [`TextStyle`] and [`Text`]. Scenes, recordings, snapshots and the tool API name
/// it like a texture, by asset ID, else path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Font(u32);

impl Font {
    pub(crate) fn new(index: usize) -> Self {
        Self(u32::try_from(index).expect("fewer than 2^32 fonts"))
    }

    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

/// How text is drawn: font, size, color, and which point of it is its origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    /// `None` is the engine's own, Inter.
    pub font: Option<Font>,
    /// The font's em, in the drawn space's units after its transform. Baselines are about 1.2 of
    /// it apart.
    pub size: f32,
    /// In linear RGBA, alpha not multiplied in.
    pub color: Vec4,
    /// The point of the text's box at its origin, across then down. Across: left, middle or right,
    /// for every line alike. Down: top, middle or bottom of the box, which runs from the first
    /// line's ascent to the last line's descent.
    pub align: [Align; 2],
}

/// Where along a text box's axis its origin is: [`TextStyle::align`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum Align {
    /// Its left, or its top.
    #[default]
    Start,
    /// Its middle.
    Center,
    /// Its right, or its bottom.
    End,
}

/// Text the world draws at its entity, like [`Context::draw_text`](crate::Context::draw_text) with
/// a [`TextStyle`] of its fields. Glyphs lie in the transform's XY plane facing +Z, like a sprite;
/// picking them finds the entity. Default: "Text" in the engine's font, an em a unit tall, white,
/// centered.
#[derive(Clone, Debug, PartialEq, Component)]
pub struct Text {
    /// What it says; `\n` ends a line.
    #[reflect(default = String::from("Text"))]
    pub text: String,
    /// The font, by ID or path; none is the engine's own, Inter.
    pub font: Option<Font>,
    /// How tall the font's em is, in the transform's units.
    #[reflect(min = 0.0, default = 1.0)]
    pub size: f32,
    /// In linear RGBA, alpha not multiplied in.
    #[reflect(color, default = Vec4::ONE)]
    pub color: Vec4,
    /// Which point of the box is at the entity's origin, across then down: start, middle or end.
    #[reflect(default = CENTERED)]
    pub align: [Align; 2],
}

/// Where a text's origin is by default: the middle of its box, both ways.
const CENTERED: [Align; 2] = [Align::Center; 2];

impl Default for Text {
    fn default() -> Self {
        Self {
            text: "Text".to_owned(),
            font: None,
            size: 1.0,
            color: Vec4::ONE,
            align: CENTERED,
        }
    }
}

impl Text {
    /// How it's drawn.
    pub fn style(&self) -> TextStyle {
        TextStyle {
            font: self.font,
            size: self.size,
            color: self.color,
            align: self.align,
        }
    }
}

/// Texels an em spans in a field: text above about this many pixels keeps corners and curves, and
/// Windows-1252's glyphs fit a field about 1,024 texels square.
const EM: f64 = 48.0;

/// How far a field's distances reach, in texels: `-RANGE / 2` outside a glyph's edge to `RANGE / 2`
/// inside, held as 0 to 1. Cells reach as far past the outline, so the field fades out before the
/// edge. Text down to an em of `EM / RANGE` pixels keeps a pixel to smooth edges over.
const RANGE: f64 = 8.0;

/// Texels between cells, holding the farthest outside a glyph: a linear sampler at one cell's edge
/// takes nothing of another's.
const GAP: u32 = 1;

/// The most bytes a font's file may hold: above the largest (CJK) fonts' some 20 MiB.
const MAX_FILE: u64 = 1 << 28;

/// Windows-1252's characters that aren't Latin-1's; a field also holds printable ASCII and Latin-1.
const WINDOWS_1252: [char; 27] = [
    '€', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', 'Ž', '‘', '’', '“', '”', '•', '–',
    '—', '˜', '™', 'š', '›', 'œ', 'ž', 'Ÿ',
];

/// The characters a field holds: those Windows-1252 prints, but not the soft hyphen, which draws
/// only at a line break and which fonts such as Inter leave out.
fn characters() -> impl Iterator<Item = char> {
    let latin = (' '..='~')
        .chain('\u{a0}'..='\u{ac}')
        .chain('\u{ae}'..='\u{ff}');
    latin.chain(WINDOWS_1252)
}

/// Features text is shaped without: glyph substitutions the field may lack (ligatures, contextual
/// alternates). Kerning and mark positioning stay.
fn features() -> [Feature; 5] {
    let off = |tag: &[u8; 4]| Feature::new(Tag::new(tag), 0, ..);
    [
        off(b"liga"),
        off(b"clig"),
        off(b"dlig"),
        off(b"calt"),
        off(b"rlig"),
    ]
}

/// A baked font: its file's bytes and shaping, its glyphs' places in the field, its line metrics.
pub(crate) struct Baked {
    bytes: Box<[u8]>,
    shaping: ShaperData,
    /// For left-to-right Latin text without [`features`].
    plan: ShapePlan,
    /// The glyphs of [`characters`] and the font's `.notdef`.
    glyphs: Glyphs,
    /// Font units in an em.
    units_per_em: f32,
    lines: Lines,
}

/// Glyphs by ID, in order, each placed in the field, or `None` with no outline (a space).
type Glyphs = Vec<(u32, Option<Placed>)>;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Placed {
    /// The cell in the field: top left, then bottom right, in texture coordinates.
    rect: [Vec2; 2],
    /// The cell about the origin on the baseline: bottom left, then top right, in ems, +Y up.
    plane: [Vec2; 2],
}

/// Line metrics, in ems: ascent above the baseline, descent below it (negative), gap between lines.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Lines {
    ascent: f32,
    descent: f32,
    gap: f32,
}

impl Lines {
    /// The distance between baselines.
    fn advance(&self) -> f32 {
        self.ascent - self.descent + self.gap
    }
}

impl Baked {
    /// Where glyph `id`, shaped from the character of `line` at byte `cluster`, is in the field.
    /// `None` if it has no outline. A glyph or character the font lacks gets the `.notdef`'s,
    /// except whitespace and control characters, which draw nothing.
    fn placed(&self, id: u32, line: &str, cluster: u32) -> Option<Placed> {
        let found = self.glyphs.binary_search_by_key(&id, |&(id, _)| id);
        if id != 0
            && let Ok(at) = found
        {
            return self.glyphs[at].1;
        }
        let character = line.get(cluster as usize..)?.chars().next()?;
        if character.is_whitespace() || character.is_control() {
            return None;
        }
        let notdef = self.glyphs.binary_search_by_key(&0, |&(id, _)| id);
        notdef.ok().and_then(|at| self.glyphs[at].1)
    }
}

/// A font's field texture and, once baked, the font. `baked` is `None` for the engine's own until
/// text is drawn in it, and for a stand-in, which draws in Inter until its file comes.
struct Slot {
    field: Texture,
    baked: Option<Baked>,
}

/// A context's fonts, by handle index, and the engine's own; and what drawing text shapes into,
/// kept between texts so drawing allocates nothing once a text as long has been drawn.
pub(crate) struct Fonts {
    /// Inter. Its field texture exists from the start, so no texture's index depends on when text
    /// is first drawn in it, which bakes it.
    inter: Slot,
    /// Inter's baked field size and texels, until [`place`](Self::place) puts them in its texture.
    unplaced: Option<([u32; 2], Vec<u8>)>,
    fonts: Vec<Slot>,
    buffer: Option<UnicodeBuffer>,
    glyphs: Vec<(Mat4, Glyph, u64)>,
}

impl Fonts {
    /// No fonts but the engine's, whose field is a one-texel texture in `textures` until it bakes.
    pub(crate) fn new(textures: &mut Textures) -> Self {
        let field = textures.add("Inter field", [1, 1], [0; 4].as_slice(), Texels::Data);
        let field = field.expect("a texel takes a texture");
        Self {
            inter: Slot { field, baked: None },
            unplaced: None,
            fonts: Vec::new(),
            buffer: None,
            glyphs: Vec::new(),
        }
    }

    /// Puts Inter's field in its texture if it baked since the last call; called as a frame is
    /// handed off.
    pub(crate) fn place(&mut self, textures: &mut Textures) {
        if let Some((size, pixels)) = self.unplaced.take() {
            let field = self.inter.field;
            let replaced = textures.replace("Inter field", field, size, pixels, Texels::Data);
            replaced.expect("its field fits a texture");
        }
    }

    pub(crate) fn inter_field(&self) -> Texture {
        self.inter.field
    }

    /// Takes a font and hands out its handle; `baked` is `None` for a stand-in.
    pub(crate) fn push(&mut self, field: Texture, baked: Option<Baked>) -> Font {
        let font = Font::new(self.fonts.len());
        self.fonts.push(Slot { field, baked });
        font
    }

    #[cfg(feature = "dev")]
    pub(crate) fn field(&self, font: Font) -> Texture {
        self.fonts[font.index()].field
    }

    /// Has `font` draw as `baked` from now on; its field texture is already replaced.
    #[cfg(feature = "dev")]
    pub(crate) fn set(&mut self, font: Font, baked: Baked) {
        self.fonts[font.index()].baked = Some(baked);
    }

    /// The glyphs drawing `text` in `style` with its origin at `transform`: each a transform and
    /// glyph, for entity `id` (0 for none), as [`Context::draw_text`] says. Inter draws if the
    /// style names no font, or for a stand-in; it bakes the first time, unless
    /// [`bake_inter_ahead`] has, about 50 ms in release and 140 ms in debug, its field waiting for
    /// [`place`](Self::place).
    ///
    /// # Panics
    ///
    /// If `style`'s font is another context's.
    ///
    /// [`Context::draw_text`]: crate::Context::draw_text
    pub(crate) fn lay_out(
        &mut self,
        transform: Mat4,
        text: &str,
        style: &TextStyle,
        id: u64,
    ) -> Drain<'_, (Mat4, Glyph, u64)> {
        let Self {
            inter,
            unplaced,
            fonts,
            buffer,
            glyphs,
        } = self;
        let (field, baked) = baked(inter, unplaced, fonts, style.font);
        let font = FontRef::new(&baked.bytes).expect("it read as it baked");
        let shaper = baked.shaping.shaper(&font).build();
        let features = features();
        let lines = baked.lines;
        let count = text.split('\n').count();
        let height = lines.ascent - lines.descent + (count - 1) as f32 * lines.advance();
        let top = match style.align[1] {
            Align::Start => 0.0,
            Align::Center => height / 2.0,
            Align::End => height,
        };
        let mut baseline = top - lines.ascent;
        let units = baked.units_per_em;
        let mut unshaped = buffer.take().unwrap_or_default();
        for line in text.split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            unshaped.push_str(line);
            // As the plan was made for, which shaping checks.
            unshaped.set_direction(Direction::LeftToRight);
            unshaped.set_script(harfrust::script::LATIN);
            let options = ShapeOptions::new().plan(Some(&baked.plan));
            let shaped = shaper.shape(unshaped, options.features(&features));
            let positions = shaped.glyph_positions();
            let width = positions.iter().map(|at| at.x_advance as f32).sum::<f32>() / units;
            let mut pen = match style.align[0] {
                Align::Start => 0.0,
                Align::Center => -width / 2.0,
                Align::End => -width,
            };
            for (info, at) in shaped.glyph_infos().iter().zip(positions) {
                let origin = Vec2::new(
                    pen + at.x_offset as f32 / units,
                    baseline + at.y_offset as f32 / units,
                );
                pen += at.x_advance as f32 / units;
                let Some(placed) = baked.placed(info.glyph_id, line, info.cluster) else {
                    continue;
                };
                let [low, high] = placed.plane;
                let middle = (origin + (low + high) / 2.0) * style.size;
                let size = (high - low) * style.size;
                let quad =
                    Mat4::from_translation(middle.extend(0.0)) * Mat4::from_scale(size.extend(1.0));
                let glyph = Glyph {
                    color: style.color,
                    field,
                    rect: placed.rect,
                    range: RANGE as f32,
                };
                glyphs.push((transform * quad, glyph, id));
            }
            unshaped = shaped.clear();
            baseline -= lines.advance();
        }
        *buffer = Some(unshaped);
        glyphs.drain(..)
    }

    /// The size of `text` in `style`, across then down, in `style.size` units: the widest line's
    /// advance and the box's height, as [`lay_out`](Self::lay_out) aligns by. A game's UI fits
    /// nodes to text with it. Shares `lay_out`'s memory and baking.
    ///
    /// # Panics
    ///
    /// If `style`'s font is another context's.
    pub(crate) fn measure(&mut self, text: &str, style: &TextStyle) -> Vec2 {
        let Self {
            inter,
            unplaced,
            fonts,
            buffer,
            ..
        } = self;
        let (_, baked) = baked(inter, unplaced, fonts, style.font);
        let font = FontRef::new(&baked.bytes).expect("it read as it baked");
        let shaper = baked.shaping.shaper(&font).build();
        let features = features();
        let (mut widest, mut lines) = (0.0_f32, 0);
        let mut unshaped = buffer.take().unwrap_or_default();
        for line in text.split('\n') {
            lines += 1;
            unshaped.push_str(line.strip_suffix('\r').unwrap_or(line));
            unshaped.set_direction(Direction::LeftToRight);
            unshaped.set_script(harfrust::script::LATIN);
            let options = ShapeOptions::new().plan(Some(&baked.plan));
            let shaped = shaper.shape(unshaped, options.features(&features));
            let advance: i32 = shaped.glyph_positions().iter().map(|at| at.x_advance).sum();
            widest = widest.max(advance as f32 / baked.units_per_em);
            unshaped = shaped.clear();
        }
        *buffer = Some(unshaped);
        let metrics = baked.lines;
        let height = metrics.ascent - metrics.descent + (lines - 1) as f32 * metrics.advance();
        Vec2::new(widest, height) * style.size
    }
}

/// The field and baked font of `font` in `fonts`, or `inter`'s for none or a stand-in, baking it
/// the first time, its field waiting in `unplaced` for [`Fonts::place`].
///
/// # Panics
///
/// If `font` is another context's.
fn baked<'f>(
    inter: &'f mut Slot,
    unplaced: &mut Option<([u32; 2], Vec<u8>)>,
    fonts: &'f [Slot],
    font: Option<Font>,
) -> (Texture, &'f Baked) {
    let slot = font.map(|font| match fonts.get(font.index()) {
        Some(slot) => slot,
        None => panic!("{font:?} is a font of another context"),
    });
    if let Some(found) = slot.and_then(|slot| Some((slot.field, slot.baked.as_ref()?))) {
        return found;
    }
    let baked = inter.baked.get_or_insert_with(|| {
        let Bake {
            size,
            pixels,
            baked,
        } = bake_inter();
        *unplaced = Some((size, pixels));
        baked
    });
    (inter.field, &*baked)
}

/// What a font bakes into: the field's size and RGBA8 rows ([`Field`]), for a data texture, and the
/// font drawing through it.
pub(crate) struct Bake {
    pub(crate) size: [u32; 2],
    pub(crate) pixels: Vec<u8>,
    pub(crate) baked: Baked,
}

/// Reads and bakes font `name`'s file at `path`, as [`Context::add_font`] says.
///
/// Fails with `engine.bad_font` if the file can't be read, isn't a regular file, is over 256 MiB,
/// isn't a TrueType or OpenType font with outlines, or has glyphs that don't fit a field of
/// [`MAX_TEXTURE_SIZE`] texels square (see [`field`]).
///
/// [`Context::add_font`]: crate::Context::add_font
pub(crate) fn bake(name: &str, path: &Path) -> Result<Bake> {
    let zone = gg_core::zone!();
    zone.text(path.display());
    let bad = |problem: String| {
        let message = format!("font {name:?} at {} {problem}", path.display());
        Error::new("engine.bad_font", message)
    };
    // One byte past the most, so a longer file is refused.
    let bytes = dirs::read_file(path, MAX_FILE + 1)
        .map_err(|error| bad("can't be read".to_owned()).with_source(error))?;
    if bytes.len() as u64 > MAX_FILE {
        return Err(bad(format!("is more than {} MiB", MAX_FILE >> 20)));
    }
    bake_bytes(name, path, bytes.into_boxed_slice())
}

/// Like [`bake`], from `files`. In a game's package it reads the font `tools/cook` baked, in
/// [`Bake::bytes`]' format, with no baking.
///
/// Fails like `bake`, and with `engine.bad_font` for a baked font that can't be read from the
/// package or doesn't hold together.
pub(crate) fn load(name: &str, path: &Path, files: &Files) -> Result<Bake> {
    if files.packaged().is_none() {
        return bake(name, path);
    }
    let bad = |problem: String| {
        let message = format!("font {name:?} at {} {problem}", path.display());
        Error::new("engine.bad_font", message)
    };
    let bytes = (files.read_as(path, package::Kind::Font, u64::MAX)).map_err(|error| {
        bad("can't be read from the game's package".to_owned()).with_source(error)
    })?;
    Bake::read(&bytes).map_err(|problem| bad(format!("is baked wrong in the package: {problem}")))
}

impl Bake {
    /// What `tools/cook` puts in a game's package; [`read`](Self::read) reads it back. Every number
    /// is little-endian: the field's width and height, units per em, ascent, descent, gap and glyph
    /// count; then each glyph's ID, whether it's placed, and if so its field rectangle and plane
    /// (four floats each); then the font's file after its length; then the field's texels. A change
    /// needs the package's version moved on.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        let baked = &self.baked;
        let mut bytes = Vec::with_capacity(self.pixels.len() + baked.bytes.len() + 64);
        let mut put = |data: &[u8]| bytes.extend_from_slice(data);
        put(&self.size[0].to_le_bytes());
        put(&self.size[1].to_le_bytes());
        let Lines {
            ascent,
            descent,
            gap,
        } = baked.lines;
        for number in [baked.units_per_em, ascent, descent, gap] {
            put(&number.to_le_bytes());
        }
        put(&(baked.glyphs.len() as u32).to_le_bytes());
        for &(id, placed) in &baked.glyphs {
            put(&id.to_le_bytes());
            put(&[u8::from(placed.is_some())]);
            if let Some(Placed { rect, plane }) = placed {
                for corner in rect.into_iter().chain(plane) {
                    put(&corner.x.to_le_bytes());
                    put(&corner.y.to_le_bytes());
                }
            }
        }
        put(&(baked.bytes.len() as u64).to_le_bytes());
        put(&baked.bytes);
        put(&self.pixels);
        bytes
    }

    /// The font [`bytes`](Self::bytes) wrote, its shaping made again from its file. The error says
    /// what doesn't hold together: a field of no texels or over [`MAX_TEXTURE_SIZE`] across, under
    /// 1 unit per em, texels not matching the size, glyphs out of order, a non-finite number, bytes
    /// left over, or a file that doesn't shape.
    fn read(bytes: &[u8]) -> std::result::Result<Self, String> {
        let mut read = Reading { bytes, at: 0 };
        let size = [read.u32()?, read.u32()?];
        if size
            .iter()
            .any(|&side| side == 0 || side > MAX_TEXTURE_SIZE)
        {
            return Err(format!("its field is {} by {} texels", size[0], size[1]));
        }
        let units_per_em = read.float()?;
        // A whole number from 1 in a font's file; shaped positions divide by it.
        if units_per_em < 1.0 {
            return Err(format!("it has {units_per_em} units per em"));
        }
        let lines = Lines {
            ascent: read.float()?,
            descent: read.float()?,
            gap: read.float()?,
        };
        let count = read.u32()?;
        let mut glyphs: Glyphs = Vec::new();
        for _ in 0..count {
            let id = read.u32()?;
            if glyphs.last().is_some_and(|&(last, _)| last >= id) {
                return Err(format!("glyph {id} is out of order"));
            }
            let placed = match read.take(1)? {
                [0] => None,
                [1] => {
                    let mut corners = [Vec2::ZERO; 4];
                    for corner in &mut corners {
                        *corner = Vec2::new(read.float()?, read.float()?);
                    }
                    Some(Placed {
                        rect: [corners[0], corners[1]],
                        plane: [corners[2], corners[3]],
                    })
                }
                _ => return Err(format!("glyph {id} is neither placed nor not")),
            };
            glyphs.push((id, placed));
        }
        let file_len = usize::try_from(read.u64()?).map_err(|_| "it ends too soon")?;
        let file: Box<[u8]> = read.take(file_len)?.into();
        let pixels = read.take(size[0] as usize * size[1] as usize * 4)?.to_vec();
        if read.at != bytes.len() {
            return Err("it has bytes past its field".to_owned());
        }
        let (shaping, plan) = shaping(&file)?;
        Ok(Self {
            size,
            pixels,
            baked: Baked {
                bytes: file,
                shaping,
                plan,
                glyphs,
                units_per_em,
                lines,
            },
        })
    }
}

/// [`Bake::read`]'s position in its bytes.
struct Reading<'b> {
    bytes: &'b [u8],
    at: usize,
}

impl<'b> Reading<'b> {
    fn take(&mut self, len: usize) -> std::result::Result<&'b [u8], String> {
        let end = self.at.checked_add(len).ok_or("it ends too soon")?;
        let taken = self.bytes.get(self.at..end).ok_or("it ends too soon")?;
        self.at = end;
        Ok(taken)
    }

    fn u32(&mut self) -> std::result::Result<u32, String> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn u64(&mut self) -> std::result::Result<u64, String> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }

    /// The next float, which must be finite.
    fn float(&mut self) -> std::result::Result<f32, String> {
        let float = f32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes"));
        float
            .is_finite()
            .then_some(float)
            .ok_or_else(|| "a number isn't finite".to_owned())
    }
}

/// What shapes left-to-right Latin text in the font in `bytes`, without [`features`]. The error
/// says what's wrong with the font, a panic in reading it included.
fn shaping(bytes: &[u8]) -> std::result::Result<(ShaperData, ShapePlan), String> {
    // harfrust can panic on a malformed font: the file's doing.
    let shaping = panic::catch_unwind(AssertUnwindSafe(|| {
        let font = FontRef::new(bytes).map_err(|error| error.to_string())?;
        let shaping = ShaperData::new(&font);
        let shaper = shaping.shaper(&font).build();
        let direction = Direction::LeftToRight;
        let latin = Some(harfrust::script::LATIN);
        let plan = ShapePlan::new(&shaper, direction, latin, None, &features());
        Ok((shaping, plan))
    }));
    shaping.unwrap_or_else(|_| Err("its reading panicked on it".to_owned()))
}

/// Starts baking the engine's font's field on the job system's other threads, where [`field_of`]
/// keeps it, so that the first text drawn in it doesn't bake it on the main thread. Outside a pool,
/// it bakes at that first draw.
pub(crate) fn bake_inter_ahead() {
    jobs::spawn(|| {
        field_of(INTER).expect("the engine's font bakes");
    });
}

/// Bakes the engine's font like [`bake`].
fn bake_inter() -> Bake {
    let path = Path::new("gg-engine/assets/fonts/Inter/Inter-Regular.ttf");
    bake_bytes("Inter", path, INTER.into()).expect("the engine's font bakes")
}

/// Bakes font `name`'s `bytes`, read from `path`, like [`bake`].
fn bake_bytes(name: &str, path: &Path, bytes: Box<[u8]>) -> Result<Bake> {
    let start = Instant::now();
    let bad = |problem: String| {
        let message = format!("font {name:?} at {} {problem}", path.display());
        Error::new("engine.bad_font", message)
    };
    let (field, cached) = field_of(&bytes).map_err(bad)?;
    let (shaping, plan) = shaping(&bytes).map_err(bad)?;
    let [width, height] = field.size;
    debug!(
        code = "asset.font_baked",
        font = name,
        path = %path.display(),
        glyphs = field.glyphs.len(),
        width,
        height,
        cached,
        ms = crate::perf::millis(start.elapsed()),
        "Font baked"
    );
    let baked = Baked {
        bytes,
        shaping,
        plan,
        glyphs: field.glyphs,
        units_per_em: field.units_per_em,
        lines: field.lines,
    };
    Ok(Bake {
        size: field.size,
        pixels: field.pixels,
        baked,
    })
}

/// The [`field`] of the font in `bytes`, and whether this process had already baked it (then it
/// takes that). The error says what's wrong with the font.
///
/// A process keeps each baked field, keyed by the font's bytes, which always bake the same. Tests
/// build contexts again and again, each adding its fonts and the engine's, and would bake each
/// anew, about a second a font in debug. Each kept font is some 2 MB of field and file, for the
/// process's life; each save a development run reloads keeps another, the old staying: if runs
/// save fonts often, keep only the latest few.
fn field_of(bytes: &[u8]) -> Result<(Field, bool), String> {
    static FIELDS: Mutex<Vec<(Box<[u8]>, Field)>> = Mutex::new(Vec::new());
    let find = |fields: &[(Box<[u8]>, Field)]| {
        let found = fields.iter().find(|(of, _)| **of == *bytes);
        found.map(|(_, field)| field.clone())
    };
    let lock = || FIELDS.lock().expect("no thread panics holding it");
    if let Some(field) = find(&lock()) {
        return Ok((field, true));
    }
    // fdsm can panic on a malformed font: the file's doing, so it fails like an unreadable one.
    // gg-core's panic hook still logs it.
    let field = panic::catch_unwind(AssertUnwindSafe(|| field(bytes)));
    let field = field.unwrap_or_else(|_| Err("its reading panicked on it".to_owned()))?;
    // Two threads may bake the same font at once: the first to finish keeps its field.
    let mut fields = lock();
    if find(&fields).is_none() {
        fields.push((bytes.into(), field.clone()));
    }
    Ok((field, false))
}

/// What a font bakes into before its field is a texture: the size and tightly packed RGBA8 rows,
/// top row first, each texel holding distances to a glyph's nearest edges in red, green and blue
/// (multi-channel) and to its outline in alpha; the glyphs' places; the line metrics.
#[derive(Clone)]
struct Field {
    size: [u32; 2],
    pixels: Vec<u8>,
    glyphs: Glyphs,
    units_per_em: f32,
    lines: Lines,
}

/// A glyph's outline in cell texels, so every cell's size is checked before any field is made.
struct Outline {
    id: u32,
    /// Contours, +Y up from the cell's bottom row, `RANGE / 2` texels in from its edges.
    shape: Shape<Contour>,
    /// Where the cell's texels start about the origin on the baseline, in ems.
    low: Vec2,
    /// The cell's width and height, in texels.
    size: [u32; 2],
}

/// A glyph's generated field, rows from the bottom up.
struct Cell {
    id: u32,
    /// Where the texels start about the origin on the baseline, in ems.
    low: Vec2,
    image: ImageBuffer<Rgba<f32>, Vec<f32>>,
}

/// Bakes the font in `bytes` into a [`Field`] of the glyphs it maps [`characters`] to and its
/// `.notdef`, glyph 0. A glyph whose outline doesn't draw goes without and draws as the `.notdef`.
/// The error says what's wrong with the font.
///
/// Fails, before any field is generated, if an outline has a non-finite point, a cell would be
/// over [`MAX_TEXTURE_SIZE`] across, or the cells would total more texels than a field that size
/// holds (a crafted font's can, by gigabytes); and after, if they don't pack into one.
fn field(bytes: &[u8]) -> Result<Field, String> {
    let font = FontRef::new(bytes)
        .map_err(|error| format!("isn't a TrueType or OpenType font: {error}"))?;
    let metrics = font.metrics(Size::unscaled(), LocationRef::default());
    if metrics.units_per_em == 0 {
        return Err("has no units per em".to_owned());
    }
    let units = f64::from(metrics.units_per_em);
    let charmap = font.charmap();
    let mut ids: Vec<u32> = (characters().filter_map(|character| charmap.map(character)))
        .map(GlyphId::to_u32)
        .collect();
    ids.push(0);
    ids.sort_unstable();
    ids.dedup();
    let mut glyphs = Vec::with_capacity(ids.len());
    let mut outlines = Vec::new();
    let drawn = font.outline_glyphs();
    for id in ids {
        let Some(glyph) = drawn.get(GlyphId::new(id)) else {
            continue;
        };
        let mut pen = Pen::default();
        let settings = DrawSettings::unhinted(Size::unscaled(), LocationRef::default());
        if glyph.draw(settings, &mut pen).is_err() {
            continue;
        }
        match outline(id, &pen.contours, units)? {
            Some(outline) => outlines.push(outline),
            None => glyphs.push((id, None)),
        }
    }
    if outlines.is_empty() {
        return Err("has no outlines of the glyphs it maps, as a font of bitmaps alone".to_owned());
    }
    // Cells totaling more texels than the largest texture couldn't pack: refuse them before making
    // any. A texel takes 16 bytes until packed, so a crafted font's glyphs, each field-sized, could
    // take hundreds of gigabytes together. Cells made at once take 1 GiB at most.
    let most = MAX_TEXTURE_SIZE;
    let texels: u64 = (outlines.iter())
        .map(|&Outline { size: [w, h], .. }| u64::from(w) * u64::from(h))
        .sum();
    if texels > u64::from(most) * u64::from(most) {
        return Err(format!(
            "has glyphs too large together for a field of {most} texels square: their cells, at \
             {EM} texels an em, take {texels} texels"
        ));
    }
    let cells = bake_glyphs(outlines);
    let (size, pixels, placed) = pack(&cells)?;
    glyphs.extend(placed);
    glyphs.sort_unstable_by_key(|&(id, _)| id);
    let em = |distance: f32| distance / metrics.units_per_em as f32;
    Ok(Field {
        size,
        pixels,
        glyphs,
        units_per_em: metrics.units_per_em as f32,
        lines: Lines {
            ascent: em(metrics.ascent),
            descent: em(metrics.descent),
            gap: em(metrics.leading),
        },
    })
}

/// The [`cell`]s of `outlines`, in order, made on the job system's threads: each is independent.
fn bake_glyphs(mut outlines: Vec<Outline>) -> Vec<Cell> {
    // A few glyphs a job, about as long as handing one out takes.
    if outlines.len() > 4 {
        let rest = outlines.split_off(outlines.len() / 2);
        let first = || bake_glyphs(outlines);
        let (mut baked, rest) = jobs::join(first, || bake_glyphs(rest));
        baked.extend(rest);
        return baked;
    }
    outlines.into_iter().map(cell).collect()
}

/// Glyph `id`'s outline in cell texels, from `contours` in font units, `units` to an em; `None` if
/// it has none, like a space.
///
/// Fails if a point isn't finite, or the cell would be over [`MAX_TEXTURE_SIZE`] across (a crafted
/// font's can, by far).
fn outline(id: u32, contours: &[Vec<Command>], units: f64) -> Result<Option<Outline>, String> {
    let points = || contours.iter().flatten().flat_map(Command::points);
    if !points().all(|point| point.x.is_finite() && point.y.is_finite()) {
        return Err(format!(
            "has glyph {id}, whose outline has a point that isn't a finite number"
        ));
    }
    let bounds = points().fold(None, |bounds: Option<(Point, Point)>, point| {
        Some(match bounds {
            None => (point, point),
            Some((low, high)) => (low.inf(&point), high.sup(&point)),
        })
    });
    let Some((low, high)) = bounds else {
        return Ok(None);
    };
    // Font units to cell texels, +Y up from the bottom row, `RANGE / 2` in from the edges.
    let scale = EM / units;
    let pad = RANGE / 2.0;
    let to_texels = |point: Point| {
        Point::new(
            (point.x - low.x) * scale + pad,
            (point.y - low.y) * scale + pad,
        )
    };
    let mut shape = Shape::default();
    for contour in contours {
        let segments = contour
            .iter()
            .filter_map(|command| command.segment(to_texels));
        let segments: Vec<Segment> = segments.collect();
        if !segments.is_empty() {
            shape.contours.push(Contour { segments });
        }
    }
    if shape.contours.is_empty() {
        return Ok(None);
    }
    // Finite, as the points are; a NaN side would pass the check below.
    let width = ((high.x - low.x) * scale + 2.0 * pad).ceil();
    let height = ((high.y - low.y) * scale + 2.0 * pad).ceil();
    let most = MAX_TEXTURE_SIZE;
    if width > f64::from(most) || height > f64::from(most) {
        return Err(format!(
            "has glyph {id} too large for a field of {most} texels square: its cell, at {EM} \
             texels an em, would be {width}x{height}"
        ));
    }
    let em = |units_from: f64, texels_in: f64| (units_from / units - texels_in / EM) as f32;
    Ok(Some(Outline {
        id,
        shape,
        low: Vec2::new(em(low.x, pad), em(low.y, pad)),
        size: [width as u32, height as u32],
    }))
}

/// The field of `outline`'s glyph; [`field`] has checked the cell's size.
fn cell(outline: Outline) -> Cell {
    let Outline {
        id,
        shape,
        low,
        size: [width, height],
    } = outline;
    // A fixed seed: a font bakes the same field every time.
    let colored = Shape::edge_coloring_simple(shape, 0.03, 0);
    let prepared = colored.prepare();
    let mut image = ImageBuffer::new(width, height);
    generate_mtsdf(&prepared, RANGE, &mut image);
    // Before the signs, as fdsm requires.
    let config = ErrorCorrectionConfig::default();
    correct_error_mtsdf(&mut image, &colored, &prepared, RANGE, &config);
    correct_sign_mtsdf(&mut image, &prepared, FillRule::Nonzero);
    Cell { id, low, image }
}

/// Packs `cells` into a field on shelves, tallest first, [`GAP`] apart: size, pixels, places.
fn pack(cells: &[Cell]) -> Result<([u32; 2], Vec<u8>, Glyphs), String> {
    let mut order: Vec<&Cell> = cells.iter().collect();
    order.sort_by_key(|cell| (std::cmp::Reverse(cell.image.height()), cell.id));
    let area: u64 = (order.iter())
        .map(|cell| u64::from(cell.image.width() + GAP) * u64::from(cell.image.height() + GAP))
        .sum();
    let widest = order
        .iter()
        .map(|cell| cell.image.width())
        .max()
        .unwrap_or(1);
    // About square, a power of two across; shelf packing fills some way short of it.
    let side = (area as f64 * 1.25).sqrt().ceil() as u32;
    let width = side.next_power_of_two().max(widest).min(MAX_TEXTURE_SIZE);
    let mut at = Vec::with_capacity(order.len());
    let (mut x, mut y, mut shelf) = (0, 0, 0);
    for cell in &order {
        let (w, h) = cell.image.dimensions();
        if x + w > width {
            (x, y, shelf) = (0, y + shelf + GAP, 0);
        }
        at.push([x, y]);
        x += w + GAP;
        shelf = shelf.max(h);
    }
    let height = y + shelf;
    if width < widest || height > MAX_TEXTURE_SIZE {
        let most = MAX_TEXTURE_SIZE;
        return Err(format!(
            "has more glyphs than fit a field of {most} texels square"
        ));
    }
    let mut pixels = vec![0; width as usize * height as usize * 4];
    let size = Vec2::new(width as f32, height as f32);
    let mut placed = Vec::with_capacity(order.len());
    for (cell, [x, y]) in order.into_iter().zip(at) {
        let (w, h) = cell.image.dimensions();
        for (row, texels) in cell.image.rows().enumerate() {
            // Cell rows go bottom up; the field's, top down.
            let top = (y + h - 1 - row as u32) as usize;
            let start = (top * width as usize + x as usize) * 4;
            let to = &mut pixels[start..start + w as usize * 4];
            for (to, texel) in to.as_chunks_mut::<4>().0.iter_mut().zip(texels) {
                *to = texel
                    .0
                    .map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
        let corner = Vec2::new(x as f32, y as f32);
        let across = Vec2::new(w as f32, h as f32);
        let ems = across / EM as f32;
        placed.push((
            cell.id,
            Some(Placed {
                rect: [corner / size, (corner + across) / size],
                plane: [cell.low, cell.low + ems],
            }),
        ));
    }
    Ok(([width, height], pixels, placed))
}

/// What a glyph's outline draws, in font units, a contour at a time.
#[derive(Default)]
struct Pen {
    contours: Vec<Vec<Command>>,
    /// Where the current contour started, and where it's got to.
    start: Option<Point>,
    last: Option<Point>,
}

/// A part of a contour, from where the previous one ended.
#[derive(Clone, Copy, Debug)]
enum Command {
    Line(Point, Point),
    Quad(Point, Point, Point),
    Cubic(Point, Point, Point, Point),
}

impl Command {
    fn points(&self) -> impl Iterator<Item = Point> {
        let (points, count) = match *self {
            Self::Line(a, b) => ([a, b, b, b], 2),
            Self::Quad(a, b, c) => ([a, b, c, c], 3),
            Self::Cubic(a, b, c, d) => ([a, b, c, d], 4),
        };
        points.into_iter().take(count)
    }

    /// The segment, points mapped by `to`; `None` for a zero-length line, which has no direction.
    fn segment(&self, to: impl Fn(Point) -> Point) -> Option<Segment> {
        Some(match *self {
            Self::Line(a, b) if a == b => return None,
            Self::Line(a, b) => Segment::line(to(a), to(b)),
            Self::Quad(a, b, c) => Segment::quad(to(a), to(b), to(c)),
            Self::Cubic(a, b, c, d) => Segment::cubic(to(a), to(b), to(c), to(d)),
        })
    }
}

impl Pen {
    /// Adds `command` to the current contour, ending at `to`; ignored before any contour starts (a
    /// malformed outline's may).
    fn draw(&mut self, command: impl FnOnce(Point) -> Command, to: Point) {
        let (Some(from), Some(contour)) = (self.last, self.contours.last_mut()) else {
            return;
        };
        contour.push(command(from));
        self.last = Some(to);
    }
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.close();
        let at = Point::new(x.into(), y.into());
        (self.start, self.last) = (Some(at), Some(at));
        self.contours.push(Vec::new());
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let to = Point::new(x.into(), y.into());
        self.draw(|from| Command::Line(from, to), to);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let control = Point::new(cx0.into(), cy0.into());
        let to = Point::new(x.into(), y.into());
        self.draw(|from| Command::Quad(from, control, to), to);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let first = Point::new(cx0.into(), cy0.into());
        let second = Point::new(cx1.into(), cy1.into());
        let to = Point::new(x.into(), y.into());
        self.draw(|from| Command::Cubic(from, first, second, to), to);
    }

    /// Closes the current contour with a line back to its start, if it ends elsewhere.
    fn close(&mut self) {
        if let (Some(start), Some(last)) = (self.start, self.last)
            && start != last
        {
            self.draw(|from| Command::Line(from, start), start);
        }
        (self.start, self.last) = (None, None);
    }
}

#[cfg(test)]
mod tests;
