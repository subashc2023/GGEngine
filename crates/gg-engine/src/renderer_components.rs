//! Reflection for what `gg-renderer` defines: the sprite and circle components, the tonemapper,
//! antialiasing, grade, ambient occlusion, fog and reflections an environment holds, and
//! `gg-gpu`'s sampler. Neither crate can derive it.

use std::fmt::Write as _;

use gg_core::Error;
use gg_core::math::{Vec2, Vec4};
use gg_renderer::{
    AmbientOcclusion, Antialiasing, Circle, Fog, Grade, MeshDraw, Reflections, Sampler, Sprite,
    Texture, Tonemapper,
};

use crate::ecs::Writable;
use crate::reflect::{
    self, Bound, Digest, Field, Meta, Reflect, Result, Shape, Show, Type, Types, Value,
};
use crate::{Component, Entity};

impl Component for Sprite {}
impl Writable for Sprite {}

/// Taken apart whole, so a field `gg-renderer` adds fails to compile here until described. The
/// texture is named by its path ([`Value::Texture`]); only a texture the renderer handed out sets
/// it, so no value makes an index shaders would sample unchecked. Defaults are the default
/// sprite's.
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

    fn entities_mut(&mut self, _: &mut dyn FnMut(&mut Entity)) {}
}

const SPRITE_FIELDS: [&str; 4] = ["color", "texture", "rect", "sampler"];

impl Component for Circle {}
impl Writable for Circle {}

/// Taken apart whole, like a sprite. Defaults are the default circle's; thickness and fade run
/// from 0 to 1, a whole radius.
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

    fn entities_mut(&mut self, _: &mut dyn FnMut(&mut Entity)) {}
}

const CIRCLE_FIELDS: [&str; 3] = ["color", "thickness", "fade"];

/// Taken apart whole, like a sprite. Defaults are the default fog's; amounts are at least 0;
/// opacity and sharpness stay within what [`Fog`] takes.
impl Reflect for Fog {
    fn describe(types: &mut Types) -> Result<Type> {
        let field =
            |name: &'static str, ty, show, bounds: [Option<f64>; 2], default, doc: &'static str| {
                Field {
                    name: name.into(),
                    ty,
                    meta: Meta {
                        min: bounds[0].map(Bound::Float),
                        max: bounds[1].map(Bound::Float),
                        show,
                        doc: doc.into(),
                    },
                    default: Some(default),
                }
            };
        let base = Fog::default();
        let doc = "Height fog: densest low down and thinning exponentially with height, lit by the \
                   sky from all around, or by a luminance of its own without one, and by the sun.";
        let (amount, unbounded) = ([Some(0.0), None], [None, None]);
        types.add::<Self>("Fog".into(), doc.into(), |_| {
            let fields = vec![
                field(
                    "density",
                    Type::F32,
                    Show::Plain,
                    amount,
                    base.density.to_value(),
                    "The share of the light it takes out over a unit of the world at its height, \
                     its extinction there; 0 for no fog.",
                ),
                field(
                    "falloff",
                    Type::F32,
                    Show::Plain,
                    amount,
                    base.falloff.to_value(),
                    "How fast it thins with height, per unit: halving every 0.69 / falloff units \
                     up, and doubling as far down; 0 fills the world alike at every height.",
                ),
                field(
                    "height",
                    Type::F32,
                    Show::Plain,
                    unbounded,
                    base.height.to_value(),
                    "The height, along +Y, at which its density is its density.",
                ),
                field(
                    "start",
                    Type::F32,
                    Show::Plain,
                    amount,
                    base.start.to_value(),
                    "How far from the eye it starts: what's nearer shows unfogged.",
                ),
                field(
                    "opacity",
                    Type::F32,
                    Show::Plain,
                    [Some(0.0), Some(1.0)],
                    base.opacity.to_value(),
                    "The most of the light from behind it it takes out: 1 hides what's far enough \
                     away, the sky among it; less lets that much show through, however far.",
                ),
                field(
                    "color",
                    Type::Vec3,
                    Show::Color,
                    amount,
                    base.color.to_value(),
                    "The share of the light reaching it that it scatters toward the eye, in linear \
                     RGB: white as water's droplets.",
                ),
                field(
                    "luminance",
                    Type::F32,
                    Show::Plain,
                    amount,
                    base.luminance.to_value(),
                    "The luminance, in candela per square meter, of the light from all around that \
                     reaches it in a world without a sky, before the exposure.",
                ),
                field(
                    "sun",
                    Type::F32,
                    Show::Plain,
                    amount,
                    base.sun.to_value(),
                    "How much of the sun's light reaches it: 1 as out in the open, less where \
                     shadows it doesn't take would keep much of it out.",
                ),
                field(
                    "sharpness",
                    Type::F32,
                    Show::Plain,
                    [Some(0.0), Some(0.99)],
                    base.sharpness.to_value(),
                    "How much more of the sun's light it scatters on along the way the light goes, \
                     Henyey-Greenstein's g: 0 alike every way, toward 1 a narrow glow round the \
                     sun.",
                ),
            ];
            Ok(Shape::Struct {
                fields,
                tuple: false,
            })
        })
    }

    fn to_value(&self) -> Value {
        let parts = FOG_FIELDS.into_iter().zip(fog_parts(self));
        Value::Struct(
            parts
                .map(|(name, part)| (name.into(), part.to_value()))
                .collect(),
        )
    }

    fn from_value(value: &Value) -> Result<Self> {
        Self::default().with_value(value)
    }

    fn with_value(&self, value: &Value) -> Result<Self> {
        let given = reflect::fields(value, FOG_FIELDS)?;
        let mut fog = *self;
        for (i, (name, given)) in (0..).zip(FOG_FIELDS.into_iter().zip(given)) {
            let part = fog.part_mut(i).expect("a part for each field");
            (part.set_value(given)).map_err(|error| reflect::within(error, name))?;
        }
        Ok(fog)
    }

    fn set_value(&mut self, value: &Value) -> Result<()> {
        *self = self.with_value(value)?;
        Ok(())
    }

    fn part(&self, index: u32) -> Option<&dyn Reflect> {
        fog_parts(self).into_iter().nth(index as usize)
    }

    fn part_mut(&mut self, index: u32) -> Option<&mut dyn Reflect> {
        let Fog {
            density,
            falloff,
            height,
            start,
            opacity,
            color,
            luminance,
            sun,
            sharpness,
        } = self;
        let parts: [&mut dyn Reflect; 9] = [
            density, falloff, height, start, opacity, color, luminance, sun, sharpness,
        ];
        parts.into_iter().nth(index as usize)
    }

    fn digest(&self, to: &mut Digest<'_>) {
        for part in fog_parts(self) {
            part.digest(to);
        }
    }

    fn entities_mut(&mut self, _: &mut dyn FnMut(&mut Entity)) {}
}

const FOG_FIELDS: [&str; 9] = [
    "density",
    "falloff",
    "height",
    "start",
    "opacity",
    "color",
    "luminance",
    "sun",
    "sharpness",
];

/// `fog`'s fields in [`FOG_FIELDS`]' order, taken apart whole, so a field `gg-renderer` adds fails
/// to compile here until described.
fn fog_parts(fog: &Fog) -> [&dyn Reflect; 9] {
    let Fog {
        density,
        falloff,
        height,
        start,
        opacity,
        color,
        luminance,
        sun,
        sharpness,
    } = fog;
    [
        density, falloff, height, start, opacity, color, luminance, sun, sharpness,
    ]
}

/// A sampler's variant name, by a match naming each, so a sampler `gg-gpu` adds fails to compile
/// here until named.
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

/// A tonemapper's variant name, by a match naming each, so one `gg-renderer` adds fails to compile
/// here until named.
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

/// An antialiasing's variant name, like [`tonemapper_name`].
fn antialiasing_name(antialiasing: Antialiasing) -> &'static str {
    match antialiasing {
        Antialiasing::Taa => "Taa",
        Antialiasing::None => "None",
        Antialiasing::Smaa => "Smaa",
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
                   of a pixel off and resolved against those before it; none, each pixel what's \
                   at its center; or SMAA, each frame alone, its pixels blended across the edges \
                   the lines of their edges stand for, sharper and without ghosts.";
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

/// Taken apart whole, like a sprite. Defaults are the identity's. White balance runs from -1 to 1,
/// saturation from 0, contrast, slope and power from 0 to [`Grade::MOST`].
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

    fn entities_mut(&mut self, _: &mut dyn FnMut(&mut Entity)) {}
}

const GRADE_FIELDS: [&str; 7] = [
    "temperature",
    "tint",
    "contrast",
    "saturation",
    "slope",
    "offset",
    "power",
];

/// Taken apart whole, like a circle. Defaults are XeGTAO's (the default's); neither field goes
/// below 0, which darkens nothing.
impl Reflect for AmbientOcclusion {
    fn describe(types: &mut Types) -> Result<Type> {
        let field = |name: &'static str, default: f32, doc: &'static str| Field {
            name: name.into(),
            ty: Type::F32,
            meta: Meta {
                min: Some(Bound::Float(0.0)),
                doc: doc.into(),
                ..Meta::default()
            },
            default: Some(default.to_value()),
        };
        let base = AmbientOcclusion::default();
        let doc = "How what's near each point the view shows darkens the light that reaches it \
                   from all around, the sky's, as XeGTAO's ground-truth ambient occlusion finds it \
                   from the view's depth.";
        types.add::<Self>("AmbientOcclusion".into(), doc.into(), |_| {
            let fields = vec![
                field(
                    "radius",
                    base.radius,
                    "How far from a point what darkens it reaches, in the world's units.",
                ),
                field(
                    "strength",
                    base.strength,
                    "How much it darkens: 1 as XeGTAO does, more for darker, 0 for nothing.",
                ),
            ];
            Ok(Shape::Struct {
                fields,
                tuple: false,
            })
        })
    }

    fn to_value(&self) -> Value {
        let AmbientOcclusion { radius, strength } = self;
        Value::Struct(vec![
            ("radius".into(), radius.to_value()),
            ("strength".into(), strength.to_value()),
        ])
    }

    fn from_value(value: &Value) -> Result<Self> {
        Self::default().with_value(value)
    }

    fn with_value(&self, value: &Value) -> Result<Self> {
        let AmbientOcclusion { radius, strength } = self;
        let [new_radius, new_strength] = reflect::fields(value, AMBIENT_OCCLUSION_FIELDS)?;
        let at = |name| move |error: Error| reflect::within(error, name);
        Ok(AmbientOcclusion {
            radius: radius.with_value(new_radius).map_err(at("radius"))?,
            strength: (strength.with_value(new_strength)).map_err(at("strength"))?,
        })
    }

    fn set_value(&mut self, value: &Value) -> Result<()> {
        *self = self.with_value(value)?;
        Ok(())
    }

    fn part(&self, index: u32) -> Option<&dyn Reflect> {
        let AmbientOcclusion { radius, strength } = self;
        match index {
            0 => Some(radius),
            1 => Some(strength),
            _ => None,
        }
    }

    fn part_mut(&mut self, index: u32) -> Option<&mut dyn Reflect> {
        let AmbientOcclusion { radius, strength } = self;
        match index {
            0 => Some(radius),
            1 => Some(strength),
            _ => None,
        }
    }

    fn digest(&self, to: &mut Digest<'_>) {
        let AmbientOcclusion { radius, strength } = self;
        radius.digest(to);
        strength.digest(to);
    }

    fn entities_mut(&mut self, _: &mut dyn FnMut(&mut Entity)) {}
}

const AMBIENT_OCCLUSION_FIELDS: [&str; 2] = ["radius", "strength"];

/// Taken apart whole, like ambient occlusion. Defaults are the default reflections'; each field is
/// at least 0, roughness at most 1.
impl Reflect for Reflections {
    fn describe(types: &mut Types) -> Result<Type> {
        let field =
            |name: &'static str, most: Option<f64>, default: f32, doc: &'static str| Field {
                name: name.into(),
                ty: Type::F32,
                meta: Meta {
                    min: Some(Bound::Float(0.0)),
                    max: most.map(Bound::Float),
                    doc: doc.into(),
                    ..Meta::default()
                },
                default: Some(default.to_value()),
            };
        let base = Reflections::default();
        let doc = "How what's opaque reflects what the view shows: each point seen in its mirror, \
                   by a ray traced through the view's depth, in place of the sky's specular light \
                   where it finds something, blurred as the point's roughness spreads it.";
        types.add::<Self>("Reflections".into(), doc.into(), |_| {
            let fields = vec![
                field(
                    "distance",
                    None,
                    base.distance,
                    "How far a ray reaches, in the world's units, fading over its last quarter.",
                ),
                field(
                    "thickness",
                    None,
                    base.thickness,
                    "How far behind what the view shows a ray may pass and still find it, in the \
                     world's units: how thick what the view's depth shows is taken to be.",
                ),
                field(
                    "roughness",
                    Some(1.0),
                    base.roughness,
                    "The roughness past which a surface takes the sky's specular light alone, \
                     fading over its last quarter.",
                ),
            ];
            Ok(Shape::Struct {
                fields,
                tuple: false,
            })
        })
    }

    fn to_value(&self) -> Value {
        let parts = REFLECTIONS_FIELDS.into_iter().zip(reflections_parts(self));
        Value::Struct(
            parts
                .map(|(name, part)| (name.into(), part.to_value()))
                .collect(),
        )
    }

    fn from_value(value: &Value) -> Result<Self> {
        Self::default().with_value(value)
    }

    fn with_value(&self, value: &Value) -> Result<Self> {
        let given = reflect::fields(value, REFLECTIONS_FIELDS)?;
        let mut reflections = *self;
        for (i, (name, given)) in (0..).zip(REFLECTIONS_FIELDS.into_iter().zip(given)) {
            let part = reflections.part_mut(i).expect("a part for each field");
            (part.set_value(given)).map_err(|error| reflect::within(error, name))?;
        }
        Ok(reflections)
    }

    fn set_value(&mut self, value: &Value) -> Result<()> {
        *self = self.with_value(value)?;
        Ok(())
    }

    fn part(&self, index: u32) -> Option<&dyn Reflect> {
        reflections_parts(self).into_iter().nth(index as usize)
    }

    fn part_mut(&mut self, index: u32) -> Option<&mut dyn Reflect> {
        let Reflections {
            distance,
            thickness,
            roughness,
        } = self;
        let parts: [&mut dyn Reflect; 3] = [distance, thickness, roughness];
        parts.into_iter().nth(index as usize)
    }

    fn digest(&self, to: &mut Digest<'_>) {
        for part in reflections_parts(self) {
            part.digest(to);
        }
    }

    fn entities_mut(&mut self, _: &mut dyn FnMut(&mut Entity)) {}
}

const REFLECTIONS_FIELDS: [&str; 3] = ["distance", "thickness", "roughness"];

/// `reflections`' fields in [`REFLECTIONS_FIELDS`]' order, taken apart whole, so a field
/// `gg-renderer` adds fails to compile here until described.
fn reflections_parts(reflections: &Reflections) -> [&dyn Reflect; 3] {
    let Reflections {
        distance,
        thickness,
        roughness,
    } = reflections;
    [distance, thickness, roughness]
}

/// A snapshot keeps the mesh's and shader's indices, whether the shader is opaque, whether it
/// culls, and the parameters' bytes in hex, as `1 2 true false x0a0b`. A restart takes it back
/// into a run whose layers added the same assets in the same order, so the indices name the same
/// ones; the snapshot's `asset` lines check that.
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

/// Opaque: the mesh and shader are `gg-renderer`'s private indices; no value may make them.
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
