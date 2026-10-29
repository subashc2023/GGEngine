//! The engine's components that `gg-renderer` defines, and the sampler a sprite holds, which
//! `gg-gpu` does, described to reflection here, since neither crate can derive it.

use gg_core::Error;
use gg_core::math::{Vec2, Vec4};
use gg_renderer::{Circle, MeshDraw, Sampler, Sprite};

use crate::reflect::{
    self, Bound, Digest, Field, Meta, Reflect, Result, Shape, Show, Type, Types, Value,
};
use crate::{Component, Writable};

/// Built on the default sprite, so that a scene's sprite, which leaves its texture out, shows the
/// white texture until code sets another.
impl Component for Sprite {
    fn base() -> Option<Self> {
        Some(Self::default())
    }
}

impl Writable for Sprite {}

/// Taken apart whole, so that a field `gg-renderer` adds fails to compile here until it's
/// described. Its texture is opaque, so that no value makes the index of a texture, which shaders
/// sample unchecked. Its fields' defaults are the default sprite's.
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
                    Type::Opaque,
                    Show::Plain,
                    None,
                    "The texture it shows, which code sets.",
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
            texture: _,
            rect,
            sampler,
        } = self;
        Value::Struct(vec![
            ("color".into(), color.to_value()),
            ("texture".into(), Value::Opaque),
            ("rect".into(), rect.to_value()),
            ("sampler".into(), sampler.to_value()),
        ])
    }

    fn from_value(value: &Value) -> Result<Self> {
        let [_, texture, ..] = reflect::fields(value, SPRITE_FIELDS)?;
        reflect::make_opaque(texture).map_err(|error| reflect::within(error, "texture"))
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
            texture: reflect::keep(texture, new_texture).map_err(at("texture"))?,
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
            texture: _,
            rect,
            sampler,
        } = self;
        match index {
            0 => Some(color),
            2 => Some(rect),
            3 => Some(sampler),
            _ => None,
        }
    }

    fn part_mut(&mut self, index: u32) -> Option<&mut dyn Reflect> {
        let Sprite {
            color,
            texture: _,
            rect,
            sampler,
        } = self;
        match index {
            0 => Some(color),
            2 => Some(rect),
            3 => Some(sampler),
            _ => None,
        }
    }

    fn digest(&self, to: &mut Digest<'_>) {
        let Sprite {
            color,
            texture: _,
            rect,
            sampler,
        } = self;
        color.digest(to);
        rect.digest(to);
        sampler.digest(to);
    }
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
}

impl Component for MeshDraw {}
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
}
