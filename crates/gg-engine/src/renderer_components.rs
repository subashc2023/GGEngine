//! The engine's components that `gg-renderer` defines, the tonemapper, antialiasing and grade an
//! environment holds, which it does too, and the sampler a sprite holds, which `gg-gpu` does,
//! described to reflection here, since neither crate can derive it.

use std::fmt::Write as _;

use gg_core::Error;
use gg_core::math::{Vec2, Vec4};
use gg_renderer::{Antialiasing, Circle, Grade, MeshDraw, Sampler, Sprite, Texture, Tonemapper};

use crate::reflect::{
    self, Bound, Digest, Field, Meta, Reflect, Result, Shape, Show, Type, Types, Value,
};
use crate::{Component, Entity, Writable};

impl Component for Sprite {}
impl Writable for Sprite {}

/// Taken apart whole, so that a field `gg-renderer` adds fails to compile here until it's
/// described. Its texture is named by its path, as [`Value::Texture`] says, and only a texture the
/// renderer handed out sets it, so that no value makes an index that shaders would sample
/// unchecked. Its fields' defaults are the default sprite's.
impl Reflect for Sprite {
    fn describe(types: &mut Types) -> Result<Type> {
        let field = |name: &'static str, ty, show, default, doc: &'static str| Field {
            name: name.into(),
            ty,
            meta: Meta {
                show,
                doc: doc.into(),
                ..Meta::default()
            },
            default,
        };
        let base = Sprite::default();
        let doc =
            "What a sprite shows: a rectangle of a texture, tinted, on a square a unit across.";
        types.add::<Self>("Sprite".into(), doc.into(), |types| {
            let fields = vec![
                field(
                    "color",
                    Type::Vec4,
                    Show::Color,
                    Some(base.color.to_value()),
                    "In linear RGBA, alpha not multiplied in, which multiplies the texture's \
                     colors and alpha.",
                ),
                field(
                    "texture",
                    <Option<Texture>>::describe(types)?,
                    Show::Plain,
                    Some(base.texture.to_value()),
                    "The texture it shows, by its path: none for the engine's white texture.",
                ),
                field(
                    "rect",
                    <[Vec2; 2]>::describe(types)?,
                    Show::Plain,
                    Some(base.rect.to_value()),
                    "The part of the texture it shows, its top left then its bottom right, from \
                     (0, 0) at the texture's top left to (1, 1) at its bottom right.",
                ),
                field(
                    "sampler",
                    Sampler::describe(types)?,
                    Show::Plain,
                    Some(base.sampler.to_value()),
                    "How the texture is sampled.",
                ),
            ];
            Ok(Shape::Struct {
                fields,
                tuple: false,
            })
        })
    }

    fn to_value(&self) -> Value {
        let Sprite {
            color,
            texture,
            rect,
            sampler,
        } = self;
        Value::Struct(vec![
            ("color".into(), color.to_value()),
            ("texture".into(), texture.to_value()),
            ("rect".into(), rect.to_value()),
            ("sampler".into(), sampler.to_value()),
        ])
    }

    fn from_value(value: &Value) -> Result<Self> {
        Self::default().with_value(value)
    }

    fn with_value(&self, value: &Value) -> Result<Self> {
        let Sprite {
            color,
            texture,
            rect,
            sampler,
        } = self;
        let [new_color, new_texture, new_rect, new_sampler] =
            reflect::fields(value, SPRITE_FIELDS)?;
        let at = |name| move |error: Error| reflect::within(error, name);
        Ok(Sprite {
            color: color.with_value(new_color).map_err(at("color"))?,
            texture: texture.with_value(new_texture).map_err(at("texture"))?,
            rect: rect.with_value(new_rect).map_err(at("rect"))?,
            sampler: sampler.with_value(new_sampler).map_err(at("sampler"))?,
        })
    }

    fn set_value(&mut self, value: &Value) -> Result<()> {
        *self = self.with_value(value)?;
        Ok(())
    }

    fn part(&self, index: u32) -> Option<&dyn Reflect> {
        let Sprite {
            color,
            texture,
            rect,
            sampler,
        } = self;
        match index {
            0 => Some(color),
            1 => Some(texture),
            2 => Some(rect),
            3 => Some(sampler),
            _ => None,
        }
    }

    fn part_mut(&mut self, index: u32) -> Option<&mut dyn Reflect> {
        let Sprite {
            color,
            texture,
            rect,
            sampler,
        } = self;
        match index {
            0 => Some(color),
            1 => Some(texture),
            2 => Some(rect),
            3 => Some(sampler),
            _ => None,
        }
    }

    fn digest(&self, to: &mut Digest<'_>) {
        let Sprite {
            color,
            texture,
            rect,
            sampler,
        } = self;
        color.digest(to);
        texture.digest(to);
        rect.digest(to);
        sampler.digest(to);
    }

    /// None of its fields holds one.
    fn entities_mut(&mut self, _: &mut dyn FnMut(&mut Entity)) {}
}

/// A sprite's fields, in the order its description gives them.
const SPRITE_FIELDS: [&str; 4] = ["color", "texture", "rect", "sampler"];

impl Component for Circle {}
impl Writable for Circle {}

/// Taken apart whole, as a sprite is, so that a field `gg-renderer` adds fails to compile here until
/// it's described. Its fields' defaults are the default circle's, and its thickness and its fade
/// run from 0 to 1, a whole radius.
impl Reflect for Circle {
    fn describe(types: &mut Types) -> Result<Type> {
        let field = |name: &'static str, ty, show, unit: bool, default, doc: &'static str| Field {
            name: name.into(),
            ty,
            meta: Meta {
                min: unit.then_some(Bound::Float(0.0)),
                max: unit.then_some(Bound::Float(1.0)),
                show,
                doc: doc.into(),
            },
            default: Some(default),
        };
        let base = Circle::default();
        let doc = "A disc or a ring of a color, as large as fits on a square a unit across.";
        types.add::<Self>("Circle".into(), doc.into(), |_| {
            let fields = vec![
                field(
                    "color",
                    Type::Vec4,
                    Show::Color,
                    false,
                    base.color.to_value(),
                    "In linear RGBA, alpha not multiplied in.",
                ),
                field(
                    "thickness",
                    Type::F32,
                    Show::Plain,
                    true,
                    base.thickness.to_value(),
                    "How far in from its edge it's filled, as a fraction of its radius: 1 fills it \
                     all, a disc, and less draws a ring that thick.",
                ),
                field(
                    "fade",
                    Type::F32,
                    Show::Plain,
                    true,
                    base.fade.to_value(),
                    "How far in from each of its edges it fades, as a fraction of its radius, \
                     beyond the pixel its edges are smoothed over.",
                ),
            ];
            Ok(Shape::Struct {
                fields,
                tuple: false,
            })
        })
    }

    fn to_value(&self) -> Value {
        let Circle {
            color,
            thickness,
            fade,
        } = self;
        Value::Struct(vec![
            ("color".into(), color.to_value()),
            ("thickness".into(), thickness.to_value()),
            ("fade".into(), fade.to_value()),
        ])
    }

    fn from_value(value: &Value) -> Result<Self> {
        let [color, thickness, fade] = reflect::fields(value, CIRCLE_FIELDS)?;
        let at = |name| move |error: Error| reflect::within(error, name);
        Ok(Circle {
            color: Vec4::from_value(color).map_err(at("color"))?,
            thickness: f32::from_value(thickness).map_err(at("thickness"))?,
            fade: f32::from_value(fade).map_err(at("fade"))?,
        })
    }

    fn with_value(&self, value: &Value) -> Result<Self> {
        let Circle {
            color,
            thickness,
            fade,
        } = self;
        let [new_color, new_thickness, new_fade] = reflect::fields(value, CIRCLE_FIELDS)?;
        let at = |name| move |error: Error| reflect::within(error, name);
        Ok(Circle {
            color: color.with_value(new_color).map_err(at("color"))?,
            thickness: (thickness.with_value(new_thickness)).map_err(at("thickness"))?,
            fade: fade.with_value(new_fade).map_err(at("fade"))?,
        })
    }

    fn set_value(&mut self, value: &Value) -> Result<()> {
        *self = self.with_value(value)?;
        Ok(())
    }

    fn part(&self, index: u32) -> Option<&dyn Reflect> {
        let Circle {
            color,
            thickness,
            fade,
        } = self;
        match index {
            0 => Some(color),
            1 => Some(thickness),
            2 => Some(fade),
            _ => None,
        }
    }

    fn part_mut(&mut self, index: u32) -> Option<&mut dyn Reflect> {
        let Circle {
            color,
            thickness,
            fade,
        } = self;
        match index {
            0 => Some(color),
            1 => Some(thickness),
            2 => Some(fade),
            _ => None,
        }
    }

    fn digest(&self, to: &mut Digest<'_>) {
        let Circle {
            color,
            thickness,
            fade,
        } = self;
        color.digest(to);
        thickness.digest(to);
        fade.digest(to);
    }

    /// None of its fields holds one.
    fn entities_mut(&mut self, _: &mut dyn FnMut(&mut Entity)) {}
}

/// A circle's fields, in the order its description gives them.
const CIRCLE_FIELDS: [&str; 3] = ["color", "thickness", "fade"];

/// A sampler's variant name, by a match that names each, so that a sampler `gg-gpu` adds fails to
/// compile here until it's named.
fn sampler_name(sampler: Sampler) -> &'static str {
    match sampler {
        Sampler::LinearClamp => "LinearClamp",
        Sampler::NearestClamp => "NearestClamp",
        Sampler::LinearRepeat => "LinearRepeat",
        Sampler::NearestRepeat => "NearestRepeat",
        Sampler::LinearMirror => "LinearMirror",
        Sampler::NearestMirror => "NearestMirror",
    }
}

/// An enum of unit variants, one for each sampler.
impl Reflect for Sampler {
    fn describe(types: &mut Types) -> Result<Type> {
        let variants = Sampler::ALL.map(|sampler| reflect::Variant {
            name: sampler_name(sampler).into(),
            fields: Vec::new(),
            tuple: false,
        });
        let doc = "How a texture is sampled: its filter, and what lies past its edges.";
        types.add::<Self>("Sampler".into(), doc.into(), |_| {
            Ok(Shape::Enum(variants.into()))
        })
    }

    fn to_value(&self) -> Value {
        Value::Variant(sampler_name(*self).into(), Vec::new())
    }

    fn from_value(value: &Value) -> Result<Self> {
        let (name, given) = reflect::variant(value)?;
        let Some(sampler) = Sampler::ALL.into_iter().find(|&s| sampler_name(s) == name) else {
            return Err(reflect::no_variant(name, &Sampler::ALL.map(sampler_name)));
        };
        reflect::pick(given, [])?;
        Ok(sampler)
    }

    fn with_value(&self, value: &Value) -> Result<Self> {
        Self::from_value(value)
    }

    fn set_value(&mut self, value: &Value) -> Result<()> {
        *self = self.with_value(value)?;
        Ok(())
    }

    fn part(&self, _: u32) -> Option<&dyn Reflect> {
        None
    }

    fn part_mut(&mut self, _: u32) -> Option<&mut dyn Reflect> {
        None
    }

    /// By its index, which is its variant's in its description.
    fn digest(&self, to: &mut Digest<'_>) {
        self.index().digest(to);
    }

    fn entities_mut(&mut self, _: &mut dyn FnMut(&mut Entity)) {}
}

/// A tonemapper's variant name, by a match that names each, so that one `gg-renderer` adds fails to
/// compile here until it's named.
fn tonemapper_name(tonemapper: Tonemapper) -> &'static str {
    match tonemapper {
        Tonemapper::PbrNeutral => "PbrNeutral",
        Tonemapper::Aces => "Aces",
        Tonemapper::Agx => "Agx",
        Tonemapper::None => "None",
    }
}

/// An enum of unit variants, one for each tonemapper.
impl Reflect for Tonemapper {
    fn describe(types: &mut Types) -> Result<Type> {
        let variants = Tonemapper::ALL.map(|tonemapper| reflect::Variant {
            name: tonemapper_name(tonemapper).into(),
            fields: Vec::new(),
            tuple: false,
        });
        let doc = "How the view's colors, drawn in high dynamic range, map into the range it shows: \
                   Khronos PBR Neutral, which keeps materials' colors; ACES, the film look; AgX, \
                   Blender's; or none, each channel clamped.";
        types.add::<Self>("Tonemapper".into(), doc.into(), |_| {
            Ok(Shape::Enum(variants.into()))
        })
    }

    fn to_value(&self) -> Value {
        Value::Variant(tonemapper_name(*self).into(), Vec::new())
    }

    fn from_value(value: &Value) -> Result<Self> {
        let (name, given) = reflect::variant(value)?;
        let Some(tonemapper) = (Tonemapper::ALL.into_iter()).find(|&t| tonemapper_name(t) == name)
        else {
            return Err(reflect::no_variant(
                name,
                &Tonemapper::ALL.map(tonemapper_name),
            ));
        };
        reflect::pick(given, [])?;
        Ok(tonemapper)
    }

    fn with_value(&self, value: &Value) -> Result<Self> {
        Self::from_value(value)
    }

    fn set_value(&mut self, value: &Value) -> Result<()> {
        *self = self.with_value(value)?;
        Ok(())
    }

    fn part(&self, _: u32) -> Option<&dyn Reflect> {
        None
    }

    fn part_mut(&mut self, _: u32) -> Option<&mut dyn Reflect> {
        None
    }

    /// By its index, which is its variant's in its description.
    fn digest(&self, to: &mut Digest<'_>) {
        self.index().digest(to);
    }

    fn entities_mut(&mut self, _: &mut dyn FnMut(&mut Entity)) {}
}

/// A way of antialiasing's variant name, by a match that names each, as [`tonemapper_name`] does.
fn antialiasing_name(antialiasing: Antialiasing) -> &'static str {
    match antialiasing {
        Antialiasing::Taa => "Taa",
        Antialiasing::None => "None",
    }
}

/// An enum of unit variants, one for each way of antialiasing.
impl Reflect for Antialiasing {
    fn describe(types: &mut Types) -> Result<Type> {
        let variants = Antialiasing::ALL.map(|antialiasing| reflect::Variant {
            name: antialiasing_name(antialiasing).into(),
            fields: Vec::new(),
            tuple: false,
        });
        let doc = "How the view is antialiased: temporal antialiasing, each frame drawn a fraction \
                   of a pixel off and resolved against those before it; or none, each pixel what's \
                   at its center.";
        types.add::<Self>("Antialiasing".into(), doc.into(), |_| {
            Ok(Shape::Enum(variants.into()))
        })
    }

    fn to_value(&self) -> Value {
        Value::Variant(antialiasing_name(*self).into(), Vec::new())
    }

    fn from_value(value: &Value) -> Result<Self> {
        let (name, given) = reflect::variant(value)?;
        let found = (Antialiasing::ALL.into_iter()).find(|&a| antialiasing_name(a) == name);
        let Some(antialiasing) = found else {
            return Err(reflect::no_variant(
                name,
                &Antialiasing::ALL.map(antialiasing_name),
            ));
        };
        reflect::pick(given, [])?;
        Ok(antialiasing)
    }

    fn with_value(&self, value: &Value) -> Result<Self> {
        Self::from_value(value)
    }

    fn set_value(&mut self, value: &Value) -> Result<()> {
        *self = self.with_value(value)?;
        Ok(())
    }

    fn part(&self, _: u32) -> Option<&dyn Reflect> {
        None
    }

    fn part_mut(&mut self, _: u32) -> Option<&mut dyn Reflect> {
        None
    }

    /// By its index, which is its variant's in its description.
    fn digest(&self, to: &mut Digest<'_>) {
        let index = Antialiasing::ALL.iter().position(|way| way == self);
        (index.expect("every way is among them") as u32).digest(to);
    }

    fn entities_mut(&mut self, _: &mut dyn FnMut(&mut Entity)) {}
}

/// Taken apart whole, as a sprite is, so that a field `gg-renderer` adds fails to compile here until
/// it's described. Its fields' defaults are the identity's; its white balance runs from -1 to 1,
/// its saturation from 0, and its contrast, slope and power from 0 to [`Grade::MOST`].
impl Reflect for Grade {
    fn describe(types: &mut Types) -> Result<Type> {
        let field =
            |name: &'static str, ty, [min, max]: [Option<f64>; 2], default, doc: &'static str| {
                Field {
                    name: name.into(),
                    ty,
                    meta: Meta {
                        min: min.map(Bound::Float),
                        max: max.map(Bound::Float),
                        show: Show::Plain,
                        doc: doc.into(),
                    },
                    default: Some(default),
                }
            };
        let balance = [Some(-1.0), Some(1.0)];
        let positive = [Some(0.0), None];
        let bounded = [Some(0.0), Some(f64::from(Grade::MOST))];
        let base = Grade::IDENTITY;
        let doc = "How the view's light is graded before a game's canvas and before it's \
                   tonemapped: white balanced, its contrast raised or lowered round mid gray, \
                   through ASC CDL's slope, offset and power, and saturated, in that order.";
        types.add::<Self>("Grade".into(), doc.into(), |_| {
            let fields = vec![
                field(
                    "temperature",
                    Type::F32,
                    balance,
                    base.temperature.to_value(),
                    "The white balance's temperature, from -1, cooler, bluer, to 1, warmer, more \
                     amber: balanced from daylight's white of 4,000 K at -1, about 17,400 K at 1.",
                ),
                field(
                    "tint",
                    Type::F32,
                    balance,
                    base.tint.to_value(),
                    "The white balance's tint, from -1, greener, to 1, more magenta.",
                ),
                field(
                    "contrast",
                    Type::F32,
                    bounded,
                    base.contrast.to_value(),
                    "What each channel's light is raised to round mid gray, 0.18, which stays as \
                     it is, up to 4: above 1, more contrast; below, less.",
                ),
                field(
                    "saturation",
                    Type::F32,
                    positive,
                    base.saturation.to_value(),
                    "How far each color is from gray, as ASC CDL's saturation weighs it: 0 makes \
                     the view gray, and above 1 more colorful.",
                ),
                field(
                    "slope",
                    Type::Vec3,
                    bounded,
                    base.slope.to_value(),
                    "ASC CDL's slope: what each channel's light is multiplied by, up to 4.",
                ),
                field(
                    "offset",
                    Type::Vec3,
                    [None, None],
                    base.offset.to_value(),
                    "ASC CDL's offset: what's added to each channel's light after its slope.",
                ),
                field(
                    "power",
                    Type::Vec3,
                    bounded,
                    base.power.to_value(),
                    "ASC CDL's power: what each channel's light is raised to after its offset, \
                     round 1, which stays as it is, up to 4.",
                ),
            ];
            Ok(Shape::Struct {
                fields,
                tuple: false,
            })
        })
    }

    fn to_value(&self) -> Value {
        let Grade {
            temperature,
            tint,
            contrast,
            saturation,
            slope,
            offset,
            power,
        } = self;
        Value::Struct(vec![
            ("temperature".into(), temperature.to_value()),
            ("tint".into(), tint.to_value()),
            ("contrast".into(), contrast.to_value()),
            ("saturation".into(), saturation.to_value()),
            ("slope".into(), slope.to_value()),
            ("offset".into(), offset.to_value()),
            ("power".into(), power.to_value()),
        ])
    }

    fn from_value(value: &Value) -> Result<Self> {
        Grade::IDENTITY.with_value(value)
    }

    fn with_value(&self, value: &Value) -> Result<Self> {
        let Grade {
            temperature,
            tint,
            contrast,
            saturation,
            slope,
            offset,
            power,
        } = self;
        let [
            new_temperature,
            new_tint,
            new_contrast,
            new_saturation,
            new_slope,
            new_offset,
            new_power,
        ] = reflect::fields(value, GRADE_FIELDS)?;
        let at = |name| move |error: Error| reflect::within(error, name);
        Ok(Grade {
            temperature: (temperature.with_value(new_temperature)).map_err(at("temperature"))?,
            tint: tint.with_value(new_tint).map_err(at("tint"))?,
            contrast: contrast.with_value(new_contrast).map_err(at("contrast"))?,
            saturation: (saturation.with_value(new_saturation)).map_err(at("saturation"))?,
            slope: slope.with_value(new_slope).map_err(at("slope"))?,
            offset: offset.with_value(new_offset).map_err(at("offset"))?,
            power: power.with_value(new_power).map_err(at("power"))?,
        })
    }

    fn set_value(&mut self, value: &Value) -> Result<()> {
        *self = self.with_value(value)?;
        Ok(())
    }

    fn part(&self, index: u32) -> Option<&dyn Reflect> {
        let Grade {
            temperature,
            tint,
            contrast,
            saturation,
            slope,
            offset,
            power,
        } = self;
        match index {
            0 => Some(temperature),
            1 => Some(tint),
            2 => Some(contrast),
            3 => Some(saturation),
            4 => Some(slope),
            5 => Some(offset),
            6 => Some(power),
            _ => None,
        }
    }

    fn part_mut(&mut self, index: u32) -> Option<&mut dyn Reflect> {
        let Grade {
            temperature,
            tint,
            contrast,
            saturation,
            slope,
            offset,
            power,
        } = self;
        match index {
            0 => Some(temperature),
            1 => Some(tint),
            2 => Some(contrast),
            3 => Some(saturation),
            4 => Some(slope),
            5 => Some(offset),
            6 => Some(power),
            _ => None,
        }
    }

    fn digest(&self, to: &mut Digest<'_>) {
        let Grade {
            temperature,
            tint,
            contrast,
            saturation,
            slope,
            offset,
            power,
        } = self;
        temperature.digest(to);
        tint.digest(to);
        contrast.digest(to);
        saturation.digest(to);
        slope.digest(to);
        offset.digest(to);
        power.digest(to);
    }

    /// None of its fields holds one.
    fn entities_mut(&mut self, _: &mut dyn FnMut(&mut Entity)) {}
}

/// A grade's fields, in the order its description gives them.
const GRADE_FIELDS: [&str; 7] = [
    "temperature",
    "tint",
    "contrast",
    "saturation",
    "slope",
    "offset",
    "power",
];

/// A snapshot keeps its mesh's and its shader's indices, whether its shader is opaque and whether
/// it culls, and its parameters' bytes in hex, as `1 2 true false x0a0b`: what a restart takes back
/// into a run whose layers added the same assets in the same order, which the indices name there
/// too, as the snapshot's `asset` lines check.
impl Component for MeshDraw {
    fn save_opaque(&self) -> Option<String> {
        let (mesh, shader, opaque, culls, params) = self.parts();
        let mut text = format!("{mesh} {shader} {opaque} {culls} x");
        for byte in params {
            write!(text, "{byte:02x}").expect("a string takes any text");
        }
        Some(text)
    }

    fn load_opaque(text: &str) -> Result<Self> {
        let bad = || {
            let message = format!("`{text}` isn't a mesh draw's parts");
            Error::new("snapshot.opaque", message)
        };
        let parts: Vec<&str> = text.split(' ').collect();
        let [mesh, shader, opaque, culls, params] = parts[..] else {
            return Err(bad());
        };
        let hex = params.strip_prefix('x').filter(|hex| hex.len() % 2 == 0);
        let hex = hex.ok_or_else(bad)?;
        let bytes: Option<Vec<u8>> = (0..hex.len() / 2)
            .map(|at| u8::from_str_radix(hex.get(at * 2..at * 2 + 2)?, 16).ok())
            .collect();
        MeshDraw::from_parts(
            mesh.parse().map_err(|_| bad())?,
            shader.parse().map_err(|_| bad())?,
            opaque.parse().map_err(|_| bad())?,
            culls.parse().map_err(|_| bad())?,
            &bytes.ok_or_else(bad)?,
        )
        .ok_or_else(bad)
    }
}

impl Writable for MeshDraw {}

/// Opaque: its mesh and its shader are indices private to `gg-renderer`, which no value may make.
impl Reflect for MeshDraw {
    fn describe(types: &mut Types) -> Result<Type> {
        let doc = "A mesh drawn with a shader and its parameters, which code sets.";
        types.add::<Self>("MeshDraw".into(), doc.into(), |_| Ok(Shape::Opaque))
    }

    fn to_value(&self) -> Value {
        Value::Opaque
    }

    fn from_value(value: &Value) -> Result<Self> {
        reflect::make_opaque(value)
    }

    fn with_value(&self, value: &Value) -> Result<Self> {
        reflect::keep(self, value)
    }

    fn set_value(&mut self, value: &Value) -> Result<()> {
        *self = self.with_value(value)?;
        Ok(())
    }

    fn part(&self, _: u32) -> Option<&dyn Reflect> {
        None
    }

    fn part_mut(&mut self, _: u32) -> Option<&mut dyn Reflect> {
        None
    }

    fn digest(&self, _: &mut Digest<'_>) {}

    fn entities_mut(&mut self, _: &mut dyn FnMut(&mut Entity)) {}
}
