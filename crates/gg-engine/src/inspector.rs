//! The inspector: the Entities panel, which lists the world's entities, and the Inspector panel,
//! which shows the picked entity's components and edits their fields. It reaches the world only
//! through its by-name calls, its IDs and its hierarchy, as the tool API will, so that an editor
//! can show it for a game in another process, and names entities by their IDs.

use std::f64::consts::PI;

use egui::color_picker::Alpha;
use egui::ecolor::{Hsva, HsvaGamma};
use gg_core::math::{EulerRot, Quat, Vec2, Vec3, Vec4};
use tracing::warn;

use crate::reflect::{self, Bound, Field, Meta, Name, Shape, Show, Type, Types, Value};
use crate::{Context, Entity, EntityId, Layer, Ui, World};

/// The Entities and Inspector panels, which an app adds as an overlay,
/// `.overlay(gg_engine::Inspector::default())`. The Entities panel lists the world's entities, all
/// of them or those with one component, to pick one from; the Inspector panel shows the picked
/// entity's components, each field with a widget for its type and hints, and sets a field as its
/// widget changes it, through [`World::set_field`]. That checks the value: a value refused shows
/// beside its field, and is logged. Edits apply at once, with no undo yet.
#[derive(Default)]
pub struct Inspector {
    /// By its ID, which outlives it, so that a pick despawned is still named as it was shown.
    picked: Option<EntityId>,
    /// The component type the Entities panel lists the entities of; all of them if `None`.
    with: Option<String>,
    /// The last value refused of each field of the picked entity.
    refusals: Vec<Refusal>,
}

/// A value [`World::set_field`] refused, at `path` of `component`.
struct Refusal {
    component: String,
    path: String,
    message: String,
}

/// A field to set, found as the Inspector panel draws, and set once it has.
struct Edit {
    component: String,
    path: String,
    value: Value,
}

impl Layer for Inspector {
    fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
        let _zone = gg_core::zone!("inspector");
        let world = cx.world();
        ui.panel("Entities", |ui| self.entities(world, ui));
        let mut edits = Vec::new();
        ui.panel("Inspector", |ui| {
            // A new panel floats smaller than its contents.
            egui::ScrollArea::vertical().show(ui, |ui| self.inspect(world, ui, &mut edits));
        });
        let world = cx.world_mut();
        for edit in edits {
            self.apply(world, edit);
        }
    }
}

impl Inspector {
    /// The Entities panel: which component the entities listed have, how many there are, and a
    /// row for each that picks it, of which only those in view are drawn.
    fn entities(&mut self, world: &World, ui: &mut egui::Ui) {
        egui::ComboBox::from_label("With")
            .selected_text(self.with.as_deref().unwrap_or("Any component"))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.with, None, "Any component");
                for name in world.component_types() {
                    let picked = self.with.as_deref() == Some(name);
                    if ui.selectable_label(picked, name).clicked() {
                        self.with = Some(name.to_owned());
                    }
                }
            });
        let entities = match world.entities(self.with.as_deref()) {
            Ok(entities) => entities,
            Err(error) => {
                ui.label(error.to_string());
                return;
            }
        };
        ui.label(match entities.len() {
            1 => "1 entity".to_owned(),
            n => format!("{n} entities"),
        });
        let height = ui.spacing().interact_size.y;
        let rows = entities.len();
        egui::ScrollArea::vertical().show_rows(ui, height, rows, |ui, shown| {
            for entity in entities.skip(shown.start).take(shown.len()) {
                let id = world.id(entity).expect("the world lists living entities");
                let components = world.components_of(entity).into_iter().flatten();
                let label = format!("{id}  {}", components.collect::<Vec<_>>().join(", "));
                if ui
                    .selectable_label(self.picked == Some(id), label)
                    .clicked()
                {
                    self.pick(id);
                }
            }
        });
    }

    fn pick(&mut self, id: EntityId) {
        if self.picked != Some(id) {
            self.picked = Some(id);
            self.refusals.clear();
        }
    }

    /// The Inspector panel: the picked entity's components, each under a header that collapses,
    /// with its fields' widgets; the fields they change go in `edits`.
    fn inspect(&self, world: &World, ui: &mut egui::Ui, edits: &mut Vec<Edit>) {
        let Some(id) = self.picked else {
            ui.label("Nothing picked: pick an entity in the Entities panel");
            return;
        };
        let Some(entity) = world.entity(id) else {
            ui.label(format!("Entity {id} was despawned"));
            return;
        };
        let components = world.components_of(entity);
        let components = components.expect("the entity of an ID is alive");
        ui.label(format!("Entity {id}"));
        if let Some(children) = children(world, entity) {
            ui.label(children);
        }
        // So that what's typed into a field of one entity never lands in the field of the next
        // one picked.
        ui.push_id(id, |ui| {
            for component in components {
                let types = world.types();
                let (Ok(value), Some(index)) = (
                    world.get_field(entity, component, ""),
                    types.find(component),
                ) else {
                    continue;
                };
                let header = egui::CollapsingHeader::new(component)
                    .id_salt(component)
                    .default_open(true)
                    .show(ui, |ui| {
                        let mut fields = Fields {
                            world,
                            types,
                            component,
                            refusals: &self.refusals,
                            edits: &mut *edits,
                        };
                        let ty = Type::Named(index);
                        let meta = Meta::default();
                        if let Some(value) = fields.value(ui, &ty, &meta, &value, "", false) {
                            fields.edit("", value);
                        }
                        fields.refusal(ui, "");
                    });
                let doc = &types.info(index).doc;
                if !doc.is_empty() {
                    header.header_response.on_hover_text(doc.as_ref());
                }
            }
        });
    }

    /// Sets the field `edit` changed, and keeps the refusal if it's refused.
    fn apply(&mut self, world: &mut World, edit: Edit) {
        let Some((id, entity)) = self.picked.and_then(|id| Some((id, world.entity(id)?))) else {
            return;
        };
        let Edit {
            component,
            path,
            value,
        } = edit;
        self.refusals
            .retain(|refusal| refusal.component != component || refusal.path != path);
        if let Err(error) = world.set_field(entity, &component, &path, &value) {
            warn!(
                code = error.code(),
                entity = %id,
                component = component.as_str(),
                path = path.as_str(),
                "{error}"
            );
            self.refusals.push(Refusal {
                component,
                path,
                message: error.to_string(),
            });
        }
    }
}

/// Draws the fields of one component, and keeps the edits they make.
struct Fields<'a> {
    world: &'a World,
    types: &'a Types,
    component: &'a str,
    refusals: &'a [Refusal],
    edits: &'a mut Vec<Edit>,
}

impl Fields<'_> {
    /// Draws `value`, of type `ty`, the field at `path` with hints `meta`, and returns its new
    /// value if a widget changed it and it's set `whole`, as the inside of an option or an enum
    /// is; otherwise its parts set themselves, each at its own path. A field is set only when its
    /// widget says it changed, never by comparing values: drawing it changes nothing.
    fn value(
        &mut self,
        ui: &mut egui::Ui,
        ty: &Type,
        meta: &Meta,
        value: &Value,
        path: &str,
        whole: bool,
    ) -> Option<Value> {
        match (ty, value) {
            (Type::Named(index), _) => {
                let info = self.types.info(*index);
                match (&info.shape, value) {
                    (Shape::Struct { fields, .. }, Value::Struct(given)) => self
                        .fields(ui, fields, given, path, whole)
                        .map(Value::Struct),
                    (Shape::Enum(variants), Value::Variant(name, given)) => {
                        if variants.iter().all(|variant| variant.fields.is_empty()) {
                            let names = variants.iter().map(|variant| &variant.name);
                            self.choice(ui, names, name, path)
                        } else {
                            let variant = variants.iter().find(|variant| variant.name == *name)?;
                            let fields = ui.vertical(|ui| {
                                ui.label(name.as_ref());
                                self.fields(ui, &variant.fields, given, path, true)
                            });
                            let fields = fields.inner?;
                            Some(Value::Variant(name.clone(), fields))
                        }
                    }
                    _ => opaque(ui),
                }
            }
            (Type::Option(inner), Value::Option(Some(value))) => {
                let inner = ui.horizontal(|ui| {
                    ui.label("Some");
                    self.value(ui, inner, meta, value, path, true)
                });
                Some(Value::Option(Some(Box::new(inner.inner?))))
            }
            (Type::Option(_), Value::Option(None)) => {
                ui.label("None");
                None
            }
            (Type::List(inner) | Type::Array(inner, _), Value::List(items)) => {
                self.elements(ui, inner, meta, items, path, whole)
            }
            (Type::Bool, &Value::Bool(mut on)) => {
                let changed = ui.checkbox(&mut on, "").changed();
                changed.then_some(Value::Bool(on))
            }
            (&Type::Int { bits, signed: true }, &Value::Int(n)) => {
                let (min, max) = int_range(bits, true, meta);
                let mut n = n;
                drag(ui, &mut n, min as i64..=max as i64, meta).then_some(Value::Int(n))
            }
            (
                &Type::Int {
                    bits,
                    signed: false,
                },
                &Value::UInt(n),
            ) => {
                let (min, max) = int_range(bits, false, meta);
                let mut n = n;
                drag(ui, &mut n, min as u64..=max as u64, meta).then_some(Value::UInt(n))
            }
            (Type::F32, &Value::F32(x)) => {
                let x = float(ui, x.into(), meta, true)?;
                Some(Value::F32(x as f32))
            }
            (Type::F64, &Value::F64(x)) => float(ui, x, meta, false).map(Value::F64),
            (Type::String, Value::String(text)) => {
                let mut text = text.clone();
                let changed = ui.text_edit_singleline(&mut text).changed();
                changed.then_some(Value::String(text))
            }
            (Type::Vec2, &Value::Vec2(v)) => {
                vector(ui, v.to_array(), meta).map(|v| Value::Vec2(Vec2::from(v)))
            }
            (Type::Vec3, &Value::Vec3(v)) if meta.show == Show::Color => {
                let rgba = self.color(ui, v.extend(1.0).to_array(), Alpha::Opaque, path)?;
                Some(Value::Vec3(Vec4::from(rgba).truncate()))
            }
            (Type::Vec3, &Value::Vec3(v)) => {
                vector(ui, v.to_array(), meta).map(|v| Value::Vec3(Vec3::from(v)))
            }
            (Type::Vec4, &Value::Vec4(v)) if meta.show == Show::Color => {
                let rgba = self.color(ui, v.to_array(), Alpha::OnlyBlend, path)?;
                Some(Value::Vec4(Vec4::from(rgba)))
            }
            (Type::Vec4, &Value::Vec4(v)) => {
                vector(ui, v.to_array(), meta).map(|v| Value::Vec4(Vec4::from(v)))
            }
            (Type::Quat, &Value::Quat(q)) => self.rotation(ui, q, path),
            (Type::Entity, Value::Entity(entity)) => {
                // A despawned entity has no ID, only its handle.
                match self.world.id(*entity) {
                    Some(id) => ui.label(id.to_string()),
                    None => ui.label(entity.to_string()),
                };
                None
            }
            _ => opaque(ui),
        }
    }

    /// Draws `given`, the values of `fields` at `path`, a row each, with the doc of each field on
    /// its name. Returns them, with what changed, if they're set `whole`; otherwise each field that
    /// changed is set at its own path.
    fn fields(
        &mut self,
        ui: &mut egui::Ui,
        fields: &[Field],
        given: &[(Name, Value)],
        path: &str,
        whole: bool,
    ) -> Option<Vec<(Name, Value)>> {
        let mut changed: Option<Vec<(Name, Value)>> = None;
        egui::Grid::new((self.component, path))
            .num_columns(2)
            .show(ui, |ui| {
                for (i, (field, (_, value))) in fields.iter().zip(given).enumerate() {
                    let path = reflect::join(path, &field.name);
                    let name = ui.label(field.name.as_ref());
                    if !field.meta.doc.is_empty() {
                        name.on_hover_text(field.meta.doc.as_ref());
                    }
                    let new = self.value(ui, &field.ty, &field.meta, value, &path, whole);
                    ui.end_row();
                    self.refusal(ui, &path);
                    if let Some(new) = new {
                        if whole {
                            changed.get_or_insert_with(|| given.to_vec())[i].1 = new;
                        } else {
                            self.edit(&path, new);
                        }
                    }
                }
            });
        changed
    }

    /// Draws `items`, the elements at `path` of a list or an array of `ty` with hints `meta`, a
    /// row each; as [`fields`](Self::fields) does.
    fn elements(
        &mut self,
        ui: &mut egui::Ui,
        ty: &Type,
        meta: &Meta,
        items: &[Value],
        path: &str,
        whole: bool,
    ) -> Option<Value> {
        let mut changed: Option<Vec<Value>> = None;
        egui::Grid::new((self.component, path))
            .num_columns(2)
            .show(ui, |ui| {
                for (i, item) in items.iter().enumerate() {
                    let path = format!("{path}[{i}]");
                    ui.label(format!("[{i}]"));
                    let new = self.value(ui, ty, meta, item, &path, whole);
                    ui.end_row();
                    self.refusal(ui, &path);
                    if let Some(new) = new {
                        if whole {
                            changed.get_or_insert_with(|| items.to_vec())[i] = new;
                        } else {
                            self.edit(&path, new);
                        }
                    }
                }
            });
        changed.map(Value::List)
    }

    /// A choice of an enum's unit variants, `names`, of which `picked` is the one it is.
    fn choice<'n>(
        &mut self,
        ui: &mut egui::Ui,
        names: impl Iterator<Item = &'n Name>,
        picked: &Name,
        path: &str,
    ) -> Option<Value> {
        let mut chosen = picked.clone();
        let changed = egui::ComboBox::from_id_salt((self.component, path))
            .selected_text(picked.as_ref())
            .show_ui(ui, |ui| {
                let mut changed = false;
                for name in names {
                    let response = ui.selectable_value(&mut chosen, name.clone(), name.as_ref());
                    changed |= response.changed();
                }
                changed
            });
        changed
            .inner
            .unwrap_or(false)
            .then(|| Value::Variant(chosen, Vec::new()))
    }

    /// A rotation as angles about x, y and z, in degrees, applied z first. The angles are kept
    /// with the rotation they made, so that those typed stay as they are, rather than jump to the
    /// same rotation's other angles, while the rotation stays within float noise of it: a system
    /// that renormalizes it every frame can change its last bits, which would reset the text being
    /// typed. They're kept under an id of the component's field, not the entity's, so that what's
    /// kept doesn't grow as entities are picked.
    fn rotation(&mut self, ui: &mut egui::Ui, q: Quat, path: &str) -> Option<Value> {
        type Kept = (Quat, [f32; 3]);
        let id = egui::Id::unique(("inspector rotation", self.component, path));
        let kept = ui.data(|data| data.get_temp::<Kept>(id));
        let mut angles = match kept {
            Some((kept, angles)) if same_rotation(kept, q) => angles,
            _ => {
                let (z, y, x) = q.to_euler(EulerRot::ZYX);
                let angles = [x, y, z].map(f32::to_degrees);
                // Shown as text, as NaN is: a drag over NaN would change it every frame. A
                // rotation that isn't finite has NaN angles.
                if !angles.iter().all(|angle| angle.is_finite()) {
                    ui.label(q.to_string());
                    return None;
                }
                ui.data_mut(|data| data.insert_temp(id, (q, angles)));
                angles
            }
        };
        let mut changed = false;
        ui.horizontal(|ui| {
            for angle in &mut angles {
                changed |= ui
                    .add(egui::DragValue::new(angle).speed(0.5).suffix("°"))
                    .changed();
            }
        });
        if !changed {
            return None;
        }
        let [x, y, z] = angles.map(f32::to_radians);
        let q = Quat::from_euler(EulerRot::ZYX, z, y, x).normalize();
        ui.data_mut(|data| data.insert_temp(id, (q, angles)));
        Some(Value::Quat(q))
    }

    /// A linear color, alpha not multiplied in, `rgba`, the field at `path`, with alpha as `alpha`
    /// says. It's edited as `Hsva`, which keeps the color of what's transparent, where egui's
    /// button of unmultiplied RGBA multiplies alpha in and loses it; and the `Hsva` is kept with
    /// the color it made, as a rotation's angles are, so that a gray's hue, which its color doesn't
    /// hold, stays as it's picked. What egui's popup gives back for it untouched isn't an edit: it
    /// passes the color through gamma and back each frame it's open, which moves its last bits.
    fn color(
        &mut self,
        ui: &mut egui::Ui,
        rgba: [f32; 4],
        alpha: Alpha,
        path: &str,
    ) -> Option<[f32; 4]> {
        type Kept = ([u32; 4], Hsva);
        let id = egui::Id::unique(("inspector color", self.component, path));
        let bits = rgba.map(f32::to_bits);
        let mut hsva = match ui.data(|data| data.get_temp::<Kept>(id)) {
            Some((kept, hsva)) if kept == bits => hsva,
            _ => {
                let [r, g, b, a] = rgba;
                Hsva::from_rgba_unmultiplied(r, g, b, a)
            }
        };
        let untouched = Hsva::from(HsvaGamma::from(hsva));
        let button = egui::color_picker::color_edit_button_hsva(ui, &mut hsva, alpha);
        if !button.changed() || hsva == untouched {
            return None;
        }
        let new = hsva.to_rgba_unmultiplied();
        let made = new.map(f32::to_bits);
        ui.data_mut(|data| data.insert_temp(id, (made, hsva)));
        // A hue picked for a gray changes none of its color's bits: it's only kept.
        (made != bits).then_some(new)
    }

    /// Sets the field at `path` to `value`, once the panel has drawn.
    fn edit(&mut self, path: &str, value: Value) {
        self.edits.push(Edit {
            component: self.component.to_owned(),
            path: path.to_owned(),
            value,
        });
    }

    /// Shows, on a row of its own, why the field at `path` last refused a value, if it did.
    fn refusal(&self, ui: &mut egui::Ui, path: &str) {
        let refused = self
            .refusals
            .iter()
            .find(|refusal| refusal.component == self.component && refusal.path == path);
        if let Some(refusal) = refused {
            ui.label("");
            ui.colored_label(ui.visuals().error_fg_color, &refusal.message);
            ui.end_row();
        }
    }
}

/// Whether `a` and `b` are one rotation, within float noise: a millionth in each component, about
/// a ten-thousandth of a degree, below the three decimals shown. `q` and `-q` are one rotation.
fn same_rotation(a: Quat, b: Quat) -> bool {
    let near = |b: Quat| (Vec4::from(a) - Vec4::from(b)).abs().max_element() < 1e-6;
    near(b) || near(-b)
}

/// `entity`'s children, by their IDs, as "Children: 5f3a9c0e12b47d68, 0c41b2e79d6a3f15": the
/// first 16 of them, then how many more there are; `None` if it has none.
fn children(world: &World, entity: Entity) -> Option<String> {
    let mut children = world.children(entity).ok()?.peekable();
    children.peek()?;
    let shown: Vec<String> = children
        .by_ref()
        .take(16)
        .map(|child| world.id(child).expect("a child is alive").to_string())
        .collect();
    let mut line = format!("Children: {}", shown.join(", "));
    let more = children.count();
    if more > 0 {
        line += &format!(" and {more} more");
    }
    Some(line)
}

/// What's opaque: shown, but not read or set.
fn opaque(ui: &mut egui::Ui) -> Option<Value> {
    ui.add_enabled(false, egui::Label::new("opaque"));
    None
}

/// The least and the most an integer of `bits` may be, within `meta`'s bounds.
fn int_range(bits: u8, signed: bool, meta: &Meta) -> (i128, i128) {
    let (mut min, mut max) = if signed {
        (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
    } else {
        (0, (1i128 << bits) - 1)
    };
    match meta.min {
        Some(Bound::Int(bound)) => min = min.max(bound),
        Some(Bound::Float(bound)) => min = min.max(bound.ceil() as i128),
        None => {}
    }
    match meta.max {
        Some(Bound::Int(bound)) => max = max.min(bound),
        Some(Bound::Float(bound)) => max = max.min(bound.floor() as i128),
        None => {}
    }
    (min, max.max(min))
}

/// Drags `n` within `range`, which the field's bounds give, a speed for which they pick. Showing a
/// value outside them leaves it as it is.
fn drag<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    n: &mut N,
    range: std::ops::RangeInclusive<N>,
    meta: &Meta,
) -> bool {
    let speed = speed(meta, 0.25);
    let drag = egui::DragValue::new(n)
        .range(range)
        .clamp_existing_to_range(false)
        .speed(speed);
    ui.add(drag).changed()
}

/// How much a drag of a pixel changes a number of a field with `meta`: a 200th of its range if it
/// has bounds both ways, and `otherwise` if it hasn't.
fn speed(meta: &Meta, otherwise: f64) -> f64 {
    match (meta.min, meta.max) {
        (Some(min), Some(max)) if max.as_f64() > min.as_f64() => {
            (max.as_f64() - min.as_f64()) / 200.0
        }
        _ => otherwise,
    }
}

/// Drags `x`, an `f32`'s if `is_f32`, in degrees if `meta` says, and returns it if it changed: no
/// further than the field's bounds, as [`World::set_field`] compares them, and finite, so that
/// what's dragged or typed is a value the field takes. NaN is shown as text: a drag over it would
/// change it every frame.
fn float(ui: &mut egui::Ui, x: f64, meta: &Meta, is_f32: bool) -> Option<f64> {
    if x.is_nan() {
        ui.label(x.to_string());
        return None;
    }
    let degrees = meta.show == Show::Degrees;
    // From the field's unit to what's shown, which the bounds are in too.
    let scale = if degrees { 180.0 / PI } else { 1.0 };
    let (most, bound): (f64, fn(Bound) -> f64) = if is_f32 {
        (f32::MAX.into(), |bound| bound.as_f32().into())
    } else {
        (f64::MAX, Bound::as_f64)
    };
    let end = |given: Option<Bound>, otherwise: f64| {
        given.map_or(otherwise, bound).clamp(-most, most) * scale
    };
    let range = end(meta.min, -most)..=end(meta.max, most);
    let mut shown = x * scale;
    let mut drag = egui::DragValue::new(&mut shown)
        .range(range)
        .clamp_existing_to_range(false)
        .speed(speed(meta, if degrees { 0.5 / scale } else { 0.01 }) * scale);
    if degrees {
        drag = drag.suffix("°");
    }
    ui.add(drag).changed().then(|| shown / scale)
}

/// Drags each of a vector's `components`, and returns them if one changed.
fn vector<const N: usize>(
    ui: &mut egui::Ui,
    mut components: [f32; N],
    meta: &Meta,
) -> Option<[f32; N]> {
    let mut changed = false;
    ui.horizontal(|ui| {
        for component in &mut components {
            if let Some(x) = float(ui, (*component).into(), meta, true) {
                *component = x as f32;
                changed = true;
            }
        }
    });
    changed.then_some(components)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflect::tests::{Everything, Form, Inner, everything, same_snapshots, snapshot};
    use crate::{Sprite, Transform};

    /// `entity`'s ID, which the Inspector picks it by and names it by.
    fn id(world: &World, entity: Entity) -> EntityId {
        world.id(entity).expect("alive")
    }

    /// A text drawn, and where.
    type Text = (String, egui::Rect);

    /// A frame of the two panels' contents, on a screen big enough for all of them, given
    /// `events`, whose edits are then set; returns its texts, and how many edits it made.
    fn frame(
        inspector: &mut Inspector,
        world: &mut World,
        ctx: &egui::Context,
        time: f64,
        events: Vec<egui::Event>,
    ) -> (Vec<Text>, usize) {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 4000.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(time),
            events,
            ..egui::RawInput::default()
        };
        let mut edits = Vec::new();
        let mut output = ctx.run_ui(input, |ui| {
            inspector.entities(world, ui);
            inspector.inspect(world, ui, &mut edits);
        });
        output.textures_delta.clear();
        let made = edits.len();
        for edit in edits {
            inspector.apply(world, edit);
        }
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            find_texts(&clipped.shape, &mut texts);
        }
        (texts, made)
    }

    fn find_texts(shape: &egui::Shape, texts: &mut Vec<Text>) {
        match shape {
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    find_texts(shape, texts);
                }
            }
            egui::Shape::Text(text) => {
                texts.push((text.galley.text().to_owned(), text.visual_bounding_rect()));
            }
            _ => {}
        }
    }

    /// Three frames, the last's text: a grid lays itself out unseen in its first.
    fn frames(inspector: &mut Inspector, world: &mut World, ctx: &egui::Context) -> Vec<String> {
        let mut texts = Vec::new();
        for time in 0..3 {
            let (shown, edits) = frame(inspector, world, ctx, f64::from(time), Vec::new());
            assert_eq!(edits, 0, "drawing edits nothing");
            texts = shown;
        }
        texts.into_iter().map(|(text, _)| text).collect()
    }

    #[test]
    fn drawing_the_panels_changes_nothing() {
        let mut world = World::default();
        let turned = Transform {
            rotation: Quat::from_rotation_z(1.0),
            ..Transform::IDENTITY
        };
        let turned = world.spawn(turned);
        let lost = Transform {
            rotation: Quat::from_xyzw(f32::NAN, 0.0, 0.0, 1.0),
            ..Transform::IDENTITY
        };
        let lost = world.spawn(lost);
        // Transparent, so that its color is lost if alpha is multiplied in.
        let sprite = Sprite {
            color: Vec4::new(0.3, 0.6, 0.9, 0.0),
            ..Sprite::default()
        };
        let tinted = world.spawn((Transform::IDENTITY, sprite));
        let odd = Everything {
            count: 200,
            angle: f32::NAN,
            offset: Vec3::new(f32::INFINITY, 0.0, 0.0),
            maybe: Some(Inner {
                level: 0,
                name: "kept".to_owned(),
            }),
            form: Form::Box {
                size: Vec2::ONE,
                turn: 0.5,
            },
            list: vec![1, 2, 3],
            ..everything()
        };
        let odd = world.spawn(odd);
        let [first, second] = [0, 1].map(|_| world.spawn(Transform::IDENTITY));
        for child in [first, second] {
            world.set_parent(child, Some(turned)).expect("no cycle");
        }
        let before = snapshot(&world);
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        let texts = frames(&mut inspector, &mut world, &ctx);
        let row = format!("{}  Transform", id(&world, turned));
        for text in [
            "6 entities",
            row.as_str(),
            "Nothing picked: pick an entity in the Entities panel",
        ] {
            assert!(texts.iter().any(|shown| shown == text), "{text}: {texts:?}");
        }
        let shown: [(Entity, &[&str]); 4] = [
            (turned, &["Transform", "translation", "rotation", "scale"]),
            (lost, &["rotation", "[NaN, 0, 0, 1]"]),
            (
                tinted,
                &["Sprite", "color", "texture", "rect", "[1]", "LinearClamp"],
            ),
            (
                odd,
                &[
                    "Everything",
                    "count",
                    "200",
                    "angle",
                    "NaN",
                    "offset",
                    "inf",
                    "Box",
                    "size",
                    "maybe",
                    "Some",
                    "kept",
                    "[2]",
                    "level",
                    "pair",
                    "0",
                    "handle",
                    "opaque",
                ],
            ),
        ];
        for (entity, names) in shown {
            inspector.pick(id(&world, entity));
            let texts = frames(&mut inspector, &mut world, &ctx);
            let header = format!("Entity {}", id(&world, entity));
            for name in names.iter().copied().chain([header.as_str()]) {
                assert!(texts.iter().any(|text| text == name), "{name}: {texts:?}");
            }
            let clash = texts.iter().find(|text| text.contains(" use of "));
            assert_eq!(clash, None, "egui saw an ID twice");
        }
        // A parent lists its children, and a child shows its parent, by their IDs.
        let [one, two] = [first, second].map(|child| id(&world, child));
        let children = format!("Children: {one}, {two}");
        let parent = id(&world, turned).to_string();
        for (entity, names) in [
            (turned, [children.as_str(), "Transform"]),
            (first, ["Parent", parent.as_str()]),
        ] {
            inspector.pick(id(&world, entity));
            let texts = frames(&mut inspector, &mut world, &ctx);
            for name in names {
                assert!(texts.iter().any(|text| text == name), "{name}: {texts:?}");
            }
        }
        assert!(same_snapshots(&snapshot(&world), &before));
    }

    #[test]
    fn a_parent_lists_its_first_16_children() {
        let mut world = World::default();
        let parent = world.spawn(Transform::IDENTITY);
        assert_eq!(children(&world, parent), None);
        let kids: Vec<Entity> = (0..18).map(|_| world.spawn(Transform::IDENTITY)).collect();
        for &kid in &kids {
            world.set_parent(kid, Some(parent)).expect("no cycle");
        }
        let shown = kids[..16].iter().map(|&kid| id(&world, kid).to_string());
        let shown: Vec<String> = shown.collect();
        let shown = format!("Children: {}", shown.join(", "));
        let listed = children(&world, parent);
        assert_eq!(listed, Some(format!("{shown} and 2 more")));
        for &kid in &kids[16..] {
            world.set_parent(kid, None).expect("a root");
        }
        assert_eq!(children(&world, parent), Some(shown));
    }

    /// A click at `at`: the pointer there, pressed and released in one frame, as egui takes a tap.
    fn click(at: egui::Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(at), button(true), button(false)]
    }

    fn enter() -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]
    }

    /// The pointer pressed at `from`, moved to `to` and released, each in a frame of its own.
    fn drag(from: egui::Pos2, to: egui::Pos2) -> [Vec<egui::Event>; 3] {
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        [
            vec![egui::Event::PointerMoved(from), button(from, true)],
            vec![egui::Event::PointerMoved(to)],
            vec![button(to, false)],
        ]
    }

    /// Where the text `name` is, the first drawn at or after the text `after`.
    fn find(texts: &[Text], after: &str, name: &str) -> egui::Rect {
        let start = texts.iter().position(|(text, _)| text == after);
        let start = start.unwrap_or_else(|| panic!("no {after}: {texts:?}"));
        let found = texts[start..].iter().find(|(text, _)| text == name);
        found.unwrap_or_else(|| panic!("no {name}: {texts:?}")).1
    }

    /// The middle of what's drawn just right of `label`, on its row: the widget of its field.
    fn beside(texts: &[Text], label: egui::Rect) -> egui::Pos2 {
        let row = |rect: &egui::Rect| (rect.center().y - label.center().y).abs() < 4.0;
        let right = texts.iter().map(|(_, rect)| rect);
        let right = right.filter(|rect| row(rect) && rect.min.x > label.max.x);
        let nearest = right.min_by(|a, b| a.min.x.total_cmp(&b.min.x));
        nearest.expect("a widget beside the label").center()
    }

    #[test]
    fn widgets_set_the_fields_they_show() {
        let mut world = World::default();
        let odd = Everything {
            maybe: Some(Inner {
                level: 0,
                name: "kept".to_owned(),
            }),
            form: Form::Box {
                size: Vec2::ONE,
                turn: 0.5,
            },
            // Infinite, as code may leave it, which a number typed sets back.
            offset: Vec3::new(f32::INFINITY, -0.0, 3.0),
            ..everything()
        };
        let entity = world.spawn(odd.clone());
        // Transparent, so that its color is lost if alpha is multiplied in; one whose bits egui's
        // popup moves as it passes it through gamma.
        let tint = Vec4::new(0.3, 0.6, 0.9, 0.0);
        let sprite = Sprite {
            color: tint,
            ..Sprite::default()
        };
        let sprite = world.spawn(sprite);
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        let mut time = 0.0;
        // A frame given `events`, then two that settle what they did; the last one's texts.
        let mut run = |inspector: &mut Inspector, world: &mut World, events| {
            let mut texts = Vec::new();
            for events in [events, Vec::new(), Vec::new()] {
                time += 1.0;
                texts = frame(inspector, world, &ctx, time, events).0;
            }
            texts
        };
        inspector.pick(id(&world, entity));
        let mut texts = run(&mut inspector, &mut world, Vec::new());
        // Each number typed in as a person would: clicked, which selects its text, typed over and
        // entered.
        for (after, name, typed) in [
            ("maybe", "level", "1"),
            ("rect", "[1]", "0.75"),
            ("form", "turn", "90"),
            ("angle", "angle", "999"),
            ("offset", "offset", "1e39"),
        ] {
            let at = beside(&texts, find(&texts, after, name));
            run(&mut inspector, &mut world, click(at));
            let text = vec![egui::Event::Text(typed.to_owned())];
            run(&mut inspector, &mut world, text);
            texts = run(&mut inspector, &mut world, enter());
        }
        assert!(inspector.refusals.is_empty(), "{texts:?}");
        let all = world.get::<Everything>(entity).expect("everything");
        let Form::Box { turn, .. } = all.form else {
            panic!("still a box: {:?}", all.form);
        };
        assert!((turn - std::f32::consts::FRAC_PI_2).abs() < 1e-6, "{turn}");
        let expected = Everything {
            maybe: Some(Inner {
                level: 1,
                name: "kept".to_owned(),
            }),
            rect: [Vec2::ZERO, Vec2::new(0.75, 1.0)],
            form: Form::Box {
                size: Vec2::ONE,
                turn,
            },
            // Typed past its max, it stops there, at the f32 the field takes.
            angle: 3.2,
            // Past what an f32 holds, it stops at the most one does, rather than become infinite.
            offset: Vec3::new(f32::MAX, -0.0, 3.0),
            ..odd
        };
        assert_eq!(all, &expected);

        inspector.pick(id(&world, sprite));
        let texts = run(&mut inspector, &mut world, Vec::new());
        // Its color's button, in the column the texture's "opaque" starts: its popup opens.
        let at = egui::pos2(
            find(&texts, "texture", "opaque").min.x + 10.0,
            find(&texts, "Sprite", "color").center().y,
        );
        let opened = run(&mut inspector, &mut world, click(at));
        // Open a while, as egui passes it through gamma each frame.
        for _ in 0..3 {
            run(&mut inspector, &mut world, Vec::new());
        }
        let bits = |color: Vec4| color.to_array().map(f32::to_bits);
        let set = world.get::<Sprite>(sprite).expect("a sprite").color;
        assert_eq!(bits(set), bits(tint));
        // Closed by a click away from it, the sampler's choice is opened and picked from.
        let away = click(egui::pos2(1500.0, 3900.0));
        run(&mut inspector, &mut world, away);
        let at = beside(&texts, find(&texts, "Sprite", "sampler"));
        let texts = run(&mut inspector, &mut world, click(at));
        let at = find(&texts, "sampler", "NearestRepeat").center();
        run(&mut inspector, &mut world, click(at));
        let set = world.get::<Sprite>(sprite).expect("a sprite");
        assert_eq!(set.sampler, crate::Sampler::NearestRepeat);
        assert_eq!(bits(set.color), bits(tint));
        let copy = opened.iter().any(|(text, _)| text == "📋");
        assert!(copy, "the color's popup opened: {opened:?}");
    }

    #[test]
    fn a_gray_keeps_the_hue_picked_for_it() {
        // White, as a sprite is untinted, and a gray whose value gamma's round trip moves.
        for gray in [Vec4::ONE, Vec4::new(0.9, 0.9, 0.9, 1.0)] {
            a_gray_keeps_its_hue(gray);
        }
    }

    fn a_gray_keeps_its_hue(gray: Vec4) {
        let mut world = World::default();
        let sprite = Sprite {
            color: gray,
            ..Sprite::default()
        };
        let sprite = world.spawn(sprite);
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        let mut time = 0.0;
        // A frame given `events`, then two that settle what they did; the last one's texts.
        let mut run = |inspector: &mut Inspector, world: &mut World, events| {
            let mut texts = Vec::new();
            for events in [events, Vec::new(), Vec::new()] {
                time += 1.0;
                texts = frame(inspector, world, &ctx, time, events).0;
            }
            texts
        };
        inspector.pick(id(&world, sprite));
        let texts = run(&mut inspector, &mut world, Vec::new());
        let at = egui::pos2(
            find(&texts, "texture", "opaque").min.x + 10.0,
            find(&texts, "Sprite", "color").center().y,
        );
        let opened = run(&mut inspector, &mut world, click(at));
        // The popup's hue slider, found by its tooltip below its first row, about halfway along.
        let copy = find(&opened, "📋", "📋");
        let x = copy.center().x + 70.0;
        let below = (0..100).map(|step| copy.max.y + 4.0 * step as f32);
        let hue = below.into_iter().find(|&y| {
            let moved = vec![egui::Event::PointerMoved(egui::pos2(x, y))];
            let texts = run(&mut inspector, &mut world, moved);
            texts.iter().any(|(text, _)| text == "Hue")
        });
        let hue = egui::pos2(x, hue.expect("the hue's slider") + 4.0);
        // Clicked, so that it's picked once: a drag would pick it again each frame it's held.
        run(&mut inspector, &mut world, click(hue));
        // A hue alone leaves it gray, though it may be one whose last bits gamma moves.
        let color = world.get::<Sprite>(sprite).expect("a sprite").color;
        let still = color.x == color.y && color.y == color.z;
        assert!(
            still && (color - gray).abs().max_element() < 1e-6,
            "{color}"
        );
        // Saturated from the square above the slider, it takes the hue picked, not red.
        let square = egui::pos2(x, hue.y - 250.0);
        for events in drag(square, square + egui::vec2(5.0, 0.0)) {
            run(&mut inspector, &mut world, events);
        }
        let [r, g, b, a] = world.get::<Sprite>(sprite).expect("a sprite").color.into();
        let picked = Hsva::from_rgba_unmultiplied(r, g, b, a);
        assert!(
            picked.s > 0.3 && (0.2..0.8).contains(&picked.h),
            "{gray}: {picked:?}"
        );
    }

    #[test]
    fn angles_typed_stay_while_a_system_renormalizes_the_rotation() {
        let typed = Quat::from_rotation_z(200f32.to_radians());
        let mut world = World::default();
        let turned = Transform {
            rotation: typed,
            ..Transform::IDENTITY
        };
        let entity = world.spawn(turned);
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        // As the widget keeps them once 200° is typed: from the rotation alone, it shows -160°.
        let id = egui::Id::unique(("inspector rotation", "Transform", "rotation"));
        ctx.data_mut(|data| data.insert_temp(id, (typed, [0.0f32, 0.0, 200.0])));
        // A bit off in each component, as renormalizing it every frame leaves it.
        let nudged = typed.to_array().map(|x| f32::from_bits(x.to_bits() + 1));
        world
            .get_mut::<Transform>(entity)
            .expect("a transform")
            .rotation = Quat::from_array(nudged).normalize();
        // egui draws the number apart from its suffix.
        let texts = frames(&mut inspector, &mut world, &ctx);
        assert!(texts.iter().any(|text| text == "200.0"), "{texts:?}");
        // Turned by something else, it shows the new rotation's own angles.
        world
            .get_mut::<Transform>(entity)
            .expect("a transform")
            .rotation = Quat::from_rotation_z(10f32.to_radians());
        let texts = frames(&mut inspector, &mut world, &ctx);
        assert!(texts.iter().any(|text| text == "10.0"), "{texts:?}");
    }

    #[test]
    fn a_value_refused_shows_beside_its_field_until_one_is_set() {
        let mut world = World::default();
        let entity = world.spawn(everything());
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        let count = |value| Edit {
            component: "Everything".to_owned(),
            path: "count".to_owned(),
            value,
        };
        inspector.apply(&mut world, count(Value::UInt(200)));
        let refused = format!(
            "can't set `Everything.count` of entity {}: 200 is above the field's max, 100",
            id(&world, entity)
        );
        let texts = frames(&mut inspector, &mut world, &ctx);
        assert!(texts.contains(&refused), "{texts:?}");
        assert_eq!(
            world.get::<Everything>(entity).expect("everything").count,
            42
        );
        inspector.apply(&mut world, count(Value::UInt(99)));
        assert_eq!(
            world.get::<Everything>(entity).expect("everything").count,
            99
        );
        let texts = frames(&mut inspector, &mut world, &ctx);
        assert!(!texts.contains(&refused), "{texts:?}");
        // Another entity picked starts without the last one's.
        inspector.apply(&mut world, count(Value::UInt(200)));
        let other = world.spawn(everything());
        inspector.pick(id(&world, other));
        assert!(inspector.refusals.is_empty());
    }

    /// A pick despawned is named by the ID it was shown by, and an entity field holding a handle
    /// whose entity is gone shows the handle, not the ID of the entity now in its slot.
    #[test]
    fn the_despawned_are_named_by_what_they_were_shown_by() {
        let mut world = World::default();
        let gone = world.spawn(Transform::IDENTITY);
        world.despawn(gone).expect("alive");
        // In the slot `gone` left, the last freed being taken first.
        let occupant = world.spawn(Transform::IDENTITY);
        let holder = world.spawn(Everything {
            target: Some(gone),
            ..everything()
        });
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, holder));
        let texts = frames(&mut inspector, &mut world, &ctx);
        assert!(texts.contains(&gone.to_string()), "{texts:?}");
        let taken = id(&world, occupant).to_string();
        assert!(!texts.contains(&taken), "{texts:?}");
        let picked = id(&world, holder);
        world.despawn(holder).expect("alive");
        let texts = frames(&mut inspector, &mut world, &ctx);
        let despawned = format!("Entity {picked} was despawned");
        assert!(texts.contains(&despawned), "{texts:?}");
    }
}
