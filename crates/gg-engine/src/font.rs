//! Fonts: TrueType and OpenType files that layers load, or that text names, and the engine's own,
//! Inter, each baked as it loads into its field, a texture of data holding a multi-channel signed
//! distance field of each glyph of the characters Windows-1252 prints, which draws them crisp at
//! any size; and text, shaped with the font's kerning, laid out in lines and drawn as those glyphs,
//! as [`Context::draw_text`] draws it and the world draws a [`Text`].
//!
//! [`Context::draw_text`]: crate::Context::draw_text

use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
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

/// The engine's own font, Inter's regular weight, its digits tabular, as the UI draws in it: what
/// text that names no font is drawn in.
pub(crate) static INTER: &[u8] = include_bytes!("../assets/fonts/Inter/Inter-Regular.ttf");

/// A font that [`Context::add_font`](crate::Context::add_font) loaded, or that text named and the
/// engine loaded: its index among its context's, which a [`TextStyle`] and a [`Text`] hold, and
/// which scenes, recordings, snapshots and the tool API name as they name a texture, by its asset
/// ID, or else its path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Font(u32);

impl Font {
    /// The font that's `index`th among its context's.
    pub(crate) fn new(index: usize) -> Self {
        Self(u32::try_from(index).expect("fewer than 2^32 fonts"))
    }

    /// Its index among its context's fonts.
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

/// How text is drawn: in which font, how large, in what color, and which point of it its origin
/// is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    /// None for the engine's own, Inter, as the UI draws in it.
    pub font: Option<Font>,
    /// How tall the font's em is, in the units of the space the text is drawn in, as its transform
    /// takes them: a line of text is about 1.2 of it from one baseline to the next, as fonts go.
    pub size: f32,
    /// In linear RGBA, alpha not multiplied in.
    pub color: Vec4,
    /// Which point of the text's box is at its origin, across, then down: across, its left, its
    /// middle or its right, which each of its lines is aligned to alike; down, its top, its middle
    /// or its bottom, the box reaching from its first line's ascent to its last line's descent.
    pub align: [Align; 2],
}

/// Where along an axis of a text's box its origin is, as [`TextStyle::align`] says.
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

/// Text in the world, which the world draws where its entity is, as
/// [`Context::draw_text`](crate::Context::draw_text) draws text in a [`TextStyle`] of its fields,
/// its glyphs in its transform's XY plane, facing +Z, as a sprite is, and a pick of them finding
/// its entity. By default, "Text" in the engine's font, an em a unit tall, white, centered on its
/// entity.
#[derive(Clone, Debug, PartialEq, Component)]
pub struct Text {
    /// What it says: a line for each of its own, which `\n` ends.
    #[reflect(default = String::from("Text"))]
    pub text: String,
    /// The font it's in, by its ID or its path: none for the engine's own, Inter.
    pub font: Option<Font>,
    /// How tall its font's em is, in its transform's units.
    #[reflect(min = 0.0, default = 1.0)]
    pub size: f32,
    /// In linear RGBA, alpha not multiplied in.
    #[reflect(color, default = Vec4::ONE)]
    pub color: Vec4,
    /// Which point of its box is at its entity's origin, across, then down: its start, its middle
    /// or its end, as a text style's alignment says.
    #[reflect(default = CENTERED)]
    pub align: [Align; 2],
}

/// A text's box's middle, both ways, where a text's origin is by default.
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
    /// How it's drawn, as [`TextStyle`] says.
    pub fn style(&self) -> TextStyle {
        TextStyle {
            font: self.font,
            size: self.size,
            color: self.color,
            align: self.align,
        }
    }
}

/// How many of a field's texels an em spans: enough that text larger than about this many pixels
/// keeps the field's corners and curves, which the field draws sharp at any size, and that a
/// field of Windows-1252's glyphs fits about 1,024 texels square.
const EM: f64 = 48.0;

/// How far a field's distances reach, in its texels: from `-RANGE / 2` outside a glyph's edge to
/// `RANGE / 2` inside it, which the field holds from 0 to 1. Each glyph's cell reaches as far past
/// its outline, so that the field fades out before its edge; and text drawn smaller than the field
/// still has a pixel to smooth its edges over down to an em of `EM / RANGE` pixels.
const RANGE: f64 = 8.0;

/// The texels between glyphs' cells in a field, which hold the farthest outside a glyph, so that a
/// linear sampler at one cell's edge takes nothing of another's.
const GAP: u32 = 1;

/// The most bytes a font's file may hold: more than the largest single fonts, those of Chinese,
/// Japanese and Korean, of some 20 MiB.
const MAX_FILE: u64 = 1 << 28;

/// The characters of Windows-1252 that aren't Latin-1's, which, with printable ASCII and Latin-1,
/// a field holds the glyphs of.
const WINDOWS_1252: [char; 27] = [
    '€', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', 'Ž', '‘', '’', '“', '”', '•', '–',
    '—', '˜', '™', 'š', '›', 'œ', 'ž', 'Ÿ',
];

/// The characters a font's field holds the glyphs of: those Windows-1252 prints, as Western
/// European languages write, but the soft hyphen, which shaping hides, as it draws only where a
/// line breaks, and fonts such as Inter leave out.
fn characters() -> impl Iterator<Item = char> {
    let latin = (' '..='~')
        .chain('\u{a0}'..='\u{ac}')
        .chain('\u{ae}'..='\u{ff}');
    latin.chain(WINDOWS_1252)
}

/// The features text is shaped without: those that put glyphs in place of others, which the field
/// may not hold, as a ligature's or a contextual alternate's are. Kerning and marks' positions
/// stay.
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

/// A font as it baked: its file's bytes, which text is shaped by, and how; where each glyph is in
/// its field; and its lines' metrics, in ems.
pub(crate) struct Baked {
    bytes: Box<[u8]>,
    shaping: ShaperData,
    /// For text left to right, in Latin script, without [`features`].
    plan: ShapePlan,
    /// The glyphs of [`characters`] and the font's `.notdef`.
    glyphs: Glyphs,
    /// Font units in an em.
    units_per_em: f32,
    lines: Lines,
}

/// Glyphs by their IDs, in order, each where it is in its field, or `None` for one with no outline,
/// as a space's.
type Glyphs = Vec<(u32, Option<Placed>)>;

/// Where a glyph is in its field.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Placed {
    /// Its cell in the field, its top left then its bottom right, in texture coordinates.
    rect: [Vec2; 2],
    /// Where its cell is about its origin on the baseline: its bottom left, then its top right, in
    /// ems, +Y being up.
    plane: [Vec2; 2],
}

/// A font's lines' metrics, in ems: how far above the baseline its ascent is, how far below it its
/// descent is, a negative number, and the gap it leaves between lines.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Lines {
    ascent: f32,
    descent: f32,
    gap: f32,
}

impl Lines {
    /// How far apart its lines' baselines are.
    fn advance(&self) -> f32 {
        self.ascent - self.descent + self.gap
    }
}

impl Baked {
    /// Where the glyph `id` is in the field, which shaping gave the character of `line` at byte
    /// `cluster`: `None` if it has no outline, as a space's; the `.notdef`'s for a glyph the field
    /// doesn't hold, or for a character the font lacks, but none for whitespace or a control
    /// character, which draw nothing.
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

/// A font's field's texture, and what the font baked, once it has: `None` for the engine's own
/// until text is drawn in it, and for a stand-in, which draws in the engine's until its file comes.
struct Slot {
    field: Texture,
    baked: Option<Baked>,
}

/// The fonts a context's layers loaded, or that text named, by their handles' indices, and the
/// engine's own; and what drawing text shapes into, kept from one text to the next, so that drawing
/// text allocates nothing once a text as long has been drawn.
pub(crate) struct Fonts {
    /// Inter, whose field's texture every context keeps from the start, so that no texture's index
    /// depends on when text is first drawn in it, which bakes it.
    inter: Slot,
    /// Inter's field's size and texels, from when it bakes until [`place`](Self::place) puts them
    /// in its texture.
    unplaced: Option<([u32; 2], Vec<u8>)>,
    fonts: Vec<Slot>,
    buffer: Option<UnicodeBuffer>,
    glyphs: Vec<(Mat4, Glyph, u64)>,
}

impl Fonts {
    /// No fonts but the engine's, which keeps a texture of `textures` for its field, a texel of
    /// nothing until it bakes.
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

    /// Puts the engine's font's field in its texture of `textures`, if it's baked since this was
    /// last called: as a frame is handed off, so that the frame that draws text in it has it.
    pub(crate) fn place(&mut self, textures: &mut Textures) {
        if let Some((size, pixels)) = self.unplaced.take() {
            let field = self.inter.field;
            let replaced = textures.replace("Inter field", field, size, pixels, Texels::Data);
            replaced.expect("its field fits a texture");
        }
    }

    /// The texture of the engine's font's field.
    pub(crate) fn inter_field(&self) -> Texture {
        self.inter.field
    }

    /// Takes the font whose field is `field`, and which baked `baked`, or a stand-in's, which bakes
    /// nothing until its file comes, and hands out its handle.
    pub(crate) fn push(&mut self, field: Texture, baked: Option<Baked>) -> Font {
        let font = Font::new(self.fonts.len());
        self.fonts.push(Slot { field, baked });
        font
    }

    /// The texture of `font`'s field.
    #[cfg(feature = "dev")]
    pub(crate) fn field(&self, font: Font) -> Texture {
        self.fonts[font.index()].field
    }

    /// Has `font` draw as `baked` from now on, as its file read again baked, its field's texture
    /// replaced already.
    #[cfg(feature = "dev")]
    pub(crate) fn set(&mut self, font: Font, baked: Baked) {
        self.fonts[font.index()].baked = Some(baked);
    }

    /// The glyphs that draw `text` in `style`, where `transform` places its origin in the world,
    /// each a transform and a glyph, each the draw of the entity of ID `id`, or of none for 0, as
    /// [`Context::draw_text`] says: in the engine's font if the style names none, or a stand-in,
    /// which bakes the first time, about 50 ms in release and 140 ms in a debug build, its field
    /// waiting for [`place`](Self::place).
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

    /// How large `text` is in `style`, across then down, in the units its em is `style.size` of:
    /// its widest line's advance, as [`lay_out`](Self::lay_out) aligns its lines by, and its box's
    /// height, from its first line's ascent to its last line's descent, as `lay_out` aligns it
    /// down by; for laying text out, as a game's UI fits a node to its text. It shapes the text as
    /// `lay_out` does, in the memory `lay_out` keeps, and bakes the engine's font as `lay_out`
    /// does where the style names none.
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

/// The field and what baked of the font `font` names among `fonts`, or of the engine's own,
/// `inter`, for none or for a stand-in, which bakes the first time, its field waiting in `unplaced`
/// for [`Fonts::place`].
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

/// What a font bakes into: its field's size and its rows of RGBA8, as [`Field`] has them, which a
/// texture of data takes, and the font, which draws through that texture.
pub(crate) struct Bake {
    pub(crate) size: [u32; 2],
    pub(crate) pixels: Vec<u8>,
    pub(crate) baked: Baked,
}

/// Reads the font file at `path`, font `name`'s, and bakes it, as [`Context::add_font`] says.
///
/// Fails with `engine.bad_font` if the file can't be read, isn't a regular file, is more than
/// 256 MiB, isn't a TrueType or OpenType font with outlines, or has glyphs that don't fit a field
/// of [`MAX_TEXTURE_SIZE`] texels square, as [`field`] says.
///
/// [`Context::add_font`]: crate::Context::add_font
pub(crate) fn bake(name: &str, path: &Path) -> Result<Bake> {
    let zone = gg_core::zone!();
    zone.text(path.display());
    let bad = |problem: String| {
        let message = format!("font {name:?} at {} {problem}", path.display());
        Error::new("engine.bad_font", message)
    };
    // A byte past the most a font's file holds, so that a longer one is refused.
    let bytes = dirs::read_file(path, MAX_FILE + 1)
        .map_err(|error| bad("can't be read".to_owned()).with_source(error))?;
    if bytes.len() as u64 > MAX_FILE {
        return Err(bad(format!("is more than {} MiB", MAX_FILE >> 20)));
    }
    bake_bytes(name, path, bytes.into_boxed_slice())
}

/// Reads font `name`'s file at `path` from `files` and bakes it, as [`bake`] does; or, from a game's
/// package, reads it as `tools/cook` baked it, as [`Bake::bytes`] wrote it, which needs no baking.
///
/// Fails as `bake` does, and with `engine.bad_font` for a baked font that can't be read from the
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
    /// What `tools/cook` puts in a game's package for the font, which [`read`](Self::read) reads
    /// back: every number little-endian, the field's width and height, the font's units in an em,
    /// its ascent, descent and gap, and its number of glyphs, then each glyph, its ID and whether
    /// it's placed, and if it is its cell's rectangle in the field and its plane, each four floats;
    /// then the font's file, after its length, then the field's texels. A change to it needs the
    /// package's version moved on.
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

    /// The font [`bytes`](Self::bytes) wrote into `bytes`, its shaping made again from its file.
    /// The error says what doesn't hold together: a field of no texels or wider or taller than
    /// [`MAX_TEXTURE_SIZE`], fewer than 1 unit per em, texels other than its size's, glyphs out of
    /// order, a number that isn't finite, bytes left over, or a file that doesn't shape.
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
        // As a font's file gives them, a whole number from 1, which shaped text's positions are
        // divided by.
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

/// Where [`Bake::read`] has got to in what it reads.
struct Reading<'b> {
    bytes: &'b [u8],
    at: usize,
}

impl<'b> Reading<'b> {
    /// The next `len` bytes.
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

/// What shapes text in the font in `bytes`, left to right, in Latin script, without
/// [`features`]. The error says what's wrong with the font, as a panic of its reading does.
fn shaping(bytes: &[u8]) -> std::result::Result<(ShaperData, ShapePlan), String> {
    // harfrust can panic on a malformed font, as reading files is, so that's the file's doing.
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

/// Bakes the engine's font, as [`bake`] does.
fn bake_inter() -> Bake {
    let path = Path::new("gg-engine/assets/fonts/Inter/Inter-Regular.ttf");
    bake_bytes("Inter", path, INTER.into()).expect("the engine's font bakes")
}

/// Bakes the font in `bytes`, font `name`'s, read from `path`, as [`bake`] does.
fn bake_bytes(name: &str, path: &Path, bytes: Box<[u8]>) -> Result<Bake> {
    let start = Instant::now();
    let bad = |problem: String| {
        let message = format!("font {name:?} at {} {problem}", path.display());
        Error::new("engine.bad_font", message)
    };
    // fdsm can panic on a malformed font, as reading files is, so that's the file's doing: the font
    // fails as one that doesn't read does. gg-core's panic hook still logs it.
    let field = panic::catch_unwind(AssertUnwindSafe(|| field(&bytes)));
    let field = field
        .unwrap_or_else(|_| Err("its reading panicked on it".to_owned()))
        .map_err(bad)?;
    let (shaping, plan) = shaping(&bytes).map_err(bad)?;
    let [width, height] = field.size;
    debug!(
        font = name,
        path = %path.display(),
        glyphs = field.glyphs.len(),
        width,
        height,
        ms = start.elapsed().as_millis() as u64,
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

/// What a font bakes into, before its field is a texture: the field's size and its rows of RGBA8,
/// tightly packed, top row first, each texel the distances to a glyph's nearest edges in its red,
/// green and blue, multi-channel, and to its outline in its alpha; and where its glyphs are in it,
/// and its lines' metrics.
struct Field {
    size: [u32; 2],
    pixels: Vec<u8>,
    glyphs: Glyphs,
    units_per_em: f32,
    lines: Lines,
}

/// A glyph's outline in its cell's texels, before its field is generated: so that every cell's size
/// is known, and checked, before any is made.
struct Outline {
    id: u32,
    /// Its contours, +Y up from its cell's bottom row, `RANGE / 2` texels in from its edges.
    shape: Shape<Contour>,
    /// Where its cell's texels start about its origin on the baseline, in ems.
    low: Vec2,
    /// Its cell's width and height, in texels.
    size: [u32; 2],
}

/// A glyph's field, as its outline generates it, its rows from its bottom up.
struct Cell {
    id: u32,
    /// Where its texels start about its origin on the baseline, in ems.
    low: Vec2,
    image: ImageBuffer<Rgba<f32>, Vec<f32>>,
}

/// Bakes the font in `bytes` into its field, as [`Field`] says, of the glyphs the font maps
/// [`characters`] to and its `.notdef`, glyph 0; a glyph whose outline doesn't draw goes without,
/// and draws as the `.notdef` does. The error says what's wrong with the font.
///
/// Fails, before any glyph's field is made, if a glyph's outline has a point that isn't a finite
/// number, if a glyph's cell would be wider or taller than [`MAX_TEXTURE_SIZE`], or if the cells
/// would come to more texels than a field that size holds, as a crafted font's can, by gigabytes;
/// and after, if they don't pack into one.
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
    // No field holds more texels than a texture of the largest size, so cells that come to more
    // couldn't pack into one; and they're refused before any is made, as each texel takes 16 bytes
    // until it's packed, so that a crafted font's glyphs, each of a size a field can be, don't take
    // hundreds of gigabytes together. The cells, all made at once, take 1 GiB at most.
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

/// The fields of the glyphs of `outlines`, in their order, as [`cell`] makes each, on the job
/// system's threads, which each glyph's is apart from the others'.
fn bake_glyphs(mut outlines: Vec<Outline>) -> Vec<Cell> {
    // A few glyphs a job, about as long as a job takes to hand out.
    if outlines.len() > 4 {
        let rest = outlines.split_off(outlines.len() / 2);
        let first = || bake_glyphs(outlines);
        let (mut baked, rest) = jobs::join(first, || bake_glyphs(rest));
        baked.extend(rest);
        return baked;
    }
    outlines.into_iter().map(cell).collect()
}

/// The outline of glyph `id`, `contours` in font units, `units` to an em, in its cell's texels:
/// `None` if it has none, as a space has.
///
/// Fails if a point of it isn't a finite number, or if its cell would be wider or taller than
/// [`MAX_TEXTURE_SIZE`], which no field can be, as a crafted font's glyph's can be by far.
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
    // From font units to the cell's texels, +Y up from its bottom row, its outline `RANGE / 2`
    // texels in from its edges.
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
    // Finite, as the points are: a side that wasn't a number would pass the check below.
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

/// The field of the glyph of `outline`, whose cell's size [`field`] has checked.
fn cell(outline: Outline) -> Cell {
    let Outline {
        id,
        shape,
        low,
        size: [width, height],
    } = outline;
    // A fixed seed, so that a font bakes the same field every time.
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

/// Packs `cells` into a field, on shelves, the tallest first, [`GAP`] texels apart: the field's
/// size, its pixels, as [`Field`] has them, and where each glyph is in it.
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
    // About square, a power of two across, which a shelf packing fills some way short of.
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
            // Its rows go from its bottom up, and the field's from its top down.
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
    /// Where the contour drawn started, and where it's got to.
    start: Option<Point>,
    last: Option<Point>,
}

/// A part of a contour, from where the one before ended.
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

    /// The segment it is, its points taken by `to`; `None` for a line of no length, which has no
    /// direction to color it by.
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
    /// Adds `command` to the contour drawn, ending at `to`; ignored before any contour starts, as a
    /// malformed outline's may be.
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

    /// Closes the contour drawn with a line back to its start, if it ends elsewhere.
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
mod tests {
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
}
