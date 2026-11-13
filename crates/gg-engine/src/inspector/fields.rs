//! The Inspector panel's fields: a widget for each field of a component, for its type and hints,
//! which draws it as the sets waiting for a tick will make it, and keeps the edits it makes.

use std::borrow::Cow;
use std::f64::consts::PI;

use egui::color_picker::Alpha;
use egui::ecolor::{Hsva, HsvaGamma};
use gg_core::math::{self, DVec3, EulerRot, Quat, Vec2, Vec3, Vec4};
use tracing::warn;

use super::commands::Command;
use super::inspected::Inspected;
use super::{Edit, Refusal, shown_right};
use crate::EntityId;
use crate::reflect::{self, Bound, Field, Ident, Meta, Shape, Show, Type, Types, Value, Variant};

/// Draws the fields of one component, and keeps the edits they make.
pub(super) struct Fields<'a> {
    pub(super) world: &'a dyn Inspected,
    pub(super) types: &'a Types,
    /// The ID of the entity whose component it is.
    pub(super) entity: EntityId,
    pub(super) component: &'a str,
    pub(super) refusals: &'a [Refusal],
    pub(super) edits: &'a mut Vec<Edit>,
    /// The sets of the entity's fields that wait for a tick, in the order they were made.
    pub(super) waiting: &'a [Edit],
    /// The component's own fields that differ from those of the prefab of the instance whose entity
    /// it is, `prefab`, by name, each marked, its label's context menu reverting it, the revert
    /// going in `operations`; none for an entity no instance gives.
    pub(super) overridden: &'a [String],
    pub(super) prefab: &'a str,
    pub(super) operations: &'a mut Vec<Command>,
}

/// What's drawn at a path: its type; the hints of the field it's in, which its parts and elements
/// inherit; and its default there, if one is left, by reference, so that a frame clones none: the
/// default of what holds it, narrowed to it, or else its field's own.
#[derive(Clone, Copy)]
pub(super) struct Slot<'s> {
    pub(super) ty: &'s Type,
    pub(super) meta: &'s Meta,
    pub(super) default: Option<&'s Value>,
}

impl<'s> Slot<'s> {
    /// Where `field` is drawn, its default the one of its name in `defaults`, the fields of the
    /// default of what holds it, or else its own.
    fn of(field: &'s Field, defaults: &'s [(Ident, Value)]) -> Self {
        let given = defaults.iter().find(|(name, _)| *name == field.name);
        Self {
            ty: &field.ty,
            meta: &field.meta,
            default: given.map(|(_, default)| default).or(field.default.as_ref()),
        }
    }
}

/// Each axis of a vector, and of a rotation's angles: its component's name, as a path names it;
/// its button's label; and the button's color, then its color hovered, in RGB: Hazel's, and grays
/// for `w`.
pub(super) const AXES: [(&str, &str, [u8; 3], [u8; 3]); 4] = [
    ("x", "X", [204, 25, 38], [230, 51, 51]),
    ("y", "Y", [51, 179, 51], [77, 204, 77]),
    ("z", "Z", [25, 64, 204], [51, 89, 230]),
    ("w", "W", [102, 102, 102], [128, 128, 128]),
];

impl Fields<'_> {
    /// Draws `value`, the field at `path`, as `slot` says, and returns its new value if a widget
    /// changed it and it's set `whole`, as the inside of an option or an enum is; otherwise its
    /// parts set themselves, each at its own path. A field is set only when its widget says it
    /// changed, never by comparing values: drawing it changes nothing. A field that a set waits for
    /// a tick to set is drawn as the last such set will make it, so that its widget, which keeps
    /// what it shows from frame to frame, goes on from there; and a vector as the sets of it and of
    /// its components will make it, in the order they were made, since recordings hold sets of it
    /// whole. Other than a vector, a field is set at its own path or whole with what's around it,
    /// never both, so the sets of one field and of what's around it never wait together.
    pub(super) fn value(
        &mut self,
        ui: &mut egui::Ui,
        slot: Slot<'_>,
        value: &Value,
        path: &str,
        whole: bool,
    ) -> Option<Value> {
        let (waiting, component) = (self.waiting, self.component);
        let sets = waiting.iter().filter(|set| set.component == component);
        // By the type, so that a struct's field named `x` isn't taken for a component.
        if !matches!(slot.ty, Type::Vec2 | Type::Vec3 | Type::Vec4 | Type::DVec3) {
            let set = sets.rev().find(|set| set.path == path);
            let value = set.map_or(value, |set| &set.value);
            return self.draw(ui, slot, value, path, whole);
        }
        let mut drawn = Cow::Borrowed(value);
        for set in sets {
            if set.path == path {
                drawn = Cow::Borrowed(&set.value);
            } else if let Some(axis) = axis_of(path, &set.path) {
                match set.value {
                    Value::F32(x) => set_component(drawn.to_mut(), axis, x.into()),
                    Value::F64(x) => set_component(drawn.to_mut(), axis, x),
                    _ => {}
                }
            }
        }
        self.draw(ui, slot, &drawn, path, whole)
    }

    /// Draws `value` as [`value`](Self::value) does, as it's given.
    fn draw(
        &mut self,
        ui: &mut egui::Ui,
        slot: Slot<'_>,
        value: &Value,
        path: &str,
        whole: bool,
    ) -> Option<Value> {
        let Slot { ty, meta, default } = slot;
        match (ty, value) {
            (Type::Named(index), _) => {
                let info = self.types.info(*index);
                match (&info.shape, value) {
                    (Shape::Struct { fields, tuple }, Value::Struct(given)) => {
                        let defaults = match default {
                            Some(Value::Struct(defaults)) => defaults.as_slice(),
                            _ => &[],
                        };
                        let changed = match fields.as_slice() {
                            // A tuple struct's only field needs no label: it would say `0`.
                            [field] if *tuple => {
                                self.only_field(ui, field, given, defaults, path, whole)
                            }
                            _ => self.fields(ui, fields, given, defaults, path, whole),
                        };
                        changed.map(Value::Struct)
                    }
                    (Shape::Enum(variants), Value::Variant(name, given)) => {
                        if variants.iter().all(|variant| variant.fields.is_empty()) {
                            let names = variants.iter().map(|variant| &variant.name);
                            self.choice(ui, names, name, path)
                        } else {
                            let variant = variants.iter().find(|variant| variant.name == *name)?;
                            // The default's fields, if it's of this variant.
                            let defaults = match default {
                                Some(Value::Variant(of, defaults)) if of == name => {
                                    defaults.as_slice()
                                }
                                _ => &[],
                            };
                            let new = ui.vertical(|ui| {
                                let picked = self.variant(ui, variants, default, name, given, path);
                                let fields = &variant.fields;
                                let changed = self.fields(ui, fields, given, defaults, path, true);
                                picked.or_else(|| Some(Value::Variant(name.clone(), changed?)))
                            });
                            new.inner
                        }
                    }
                    _ => opaque(ui),
                }
            }
            (Type::Option(inner), Value::Option(given)) => {
                self.option(ui, slot, inner, given.as_deref(), path)
            }
            (Type::List(inner) | Type::Array(inner, _), Value::List(items)) => {
                let defaults = match default {
                    Some(Value::List(defaults)) => defaults.as_slice(),
                    _ => &[],
                };
                let item = Slot {
                    ty: inner,
                    meta,
                    default: None,
                };
                self.elements(ui, item, defaults, items, path, whole)
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
                let v = self.vector(ui, slot, v.as_dvec2().to_array(), path, whole, true)?;
                Some(Value::Vec2(Vec2::from(v.map(|x| x as f32))))
            }
            (Type::Vec3, &Value::Vec3(v)) if meta.show == Show::Color => {
                let rgba = self.color(ui, v.extend(1.0).to_array(), Alpha::Opaque, path)?;
                Some(Value::Vec3(Vec4::from(rgba).truncate()))
            }
            (Type::Vec3, &Value::Vec3(v)) => {
                let v = self.vector(ui, slot, v.as_dvec3().to_array(), path, whole, true)?;
                Some(Value::Vec3(Vec3::from(v.map(|x| x as f32))))
            }
            (Type::DVec3, &Value::DVec3(v)) => {
                let v = self.vector(ui, slot, v.to_array(), path, whole, false)?;
                Some(Value::DVec3(DVec3::from(v)))
            }
            (Type::Vec4, &Value::Vec4(v)) if meta.show == Show::Color => {
                let rgba = self.color(ui, v.to_array(), Alpha::OnlyBlend, path)?;
                Some(Value::Vec4(Vec4::from(rgba)))
            }
            (Type::Vec4, &Value::Vec4(v)) => {
                let v = self.vector(ui, slot, v.as_dvec4().to_array(), path, whole, true)?;
                Some(Value::Vec4(Vec4::from(v.map(|x| x as f32))))
            }
            (Type::Quat, &Value::Quat(q)) => self.rotation(ui, slot, q, path),
            // By the type, as another process's world keeps them otherwise than its own.
            (Type::Entity, value) => {
                ui.label(self.world.entity_label(value));
                None
            }
            (Type::Texture | Type::Model | Type::Font | Type::Prefab, value) => {
                ui.label(self.world.asset_label(value));
                None
            }
            _ => opaque(ui),
        }
    }

    /// Draws `given`, the values of `fields` at `path`, a row each, with the doc of each field on
    /// its name, and its default the one of its name in `defaults`, the fields of the default of
    /// what holds them, or else its own. Returns them, with what changed, if they're set `whole`;
    /// otherwise each field that changed is set at its own path.
    fn fields(
        &mut self,
        ui: &mut egui::Ui,
        fields: &[Field],
        given: &[(Ident, Value)],
        defaults: &[(Ident, Value)],
        path: &str,
        whole: bool,
    ) -> Option<Vec<(Ident, Value)>> {
        let mut changed: Option<Vec<(Ident, Value)>> = None;
        egui::Grid::new((self.component, path))
            .num_columns(2)
            .show(ui, |ui| {
                for (i, (field, (_, value))) in fields.iter().zip(given).enumerate() {
                    let own = path.is_empty();
                    let path = reflect::join(path, &field.name);
                    let overridden = own && self.overridden.iter().any(|f| *f == *field.name);
                    if overridden {
                        self.overridden_label(ui, field, &path);
                    } else {
                        let name = ui.label(field.name.as_ref());
                        if !field.meta.doc.is_empty() {
                            name.on_hover_text(field.meta.doc.as_ref());
                        }
                    }
                    let new = self.value(ui, Slot::of(field, defaults), value, &path, whole);
                    ui.end_row();
                    self.refusal(ui, &field.ty, &path);
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

    /// Draws the label of `field`, at `path`, one of the component's own that differs from its
    /// prefab's: strong, marked, saying so on hover, above the field's doc, and with a context menu
    /// whose Revert to Prefab sets it back to the prefab's.
    fn overridden_label(&mut self, ui: &mut egui::Ui, field: &Field, path: &str) {
        let text = egui::RichText::new(field.name.as_ref()).strong();
        let label = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
        super::mark(ui, label.rect);
        let said = format!("Differs from {}: its context menu reverts it", self.prefab);
        let label = match field.meta.doc.is_empty() {
            true => label.on_hover_text(said),
            false => label.on_hover_text(format!("{said}\n\n{}", field.meta.doc)),
        };
        let menu = egui::Popup::context_menu(&label).style(crate::menu_style);
        menu.show(|ui| {
            if ui.button("Revert to Prefab").clicked() {
                let component = self.component.to_owned();
                let revert = Command::Revert(self.entity, component, path.to_owned());
                self.operations.push(revert);
            }
        });
    }

    /// Draws the only field of a tuple struct, `field`, with its value from `given`, and its
    /// default as [`fields`](Self::fields) finds it in `defaults`, as that draws a field, but
    /// without a label, which would only say `0`, and in a group of its own, so that a refusal goes
    /// under it, not into the row of a grid it's in.
    fn only_field(
        &mut self,
        ui: &mut egui::Ui,
        field: &Field,
        given: &[(Ident, Value)],
        defaults: &[(Ident, Value)],
        path: &str,
        whole: bool,
    ) -> Option<Vec<(Ident, Value)>> {
        let [(name, value)] = given else {
            return None;
        };
        let path = reflect::join(path, &field.name);
        let new = ui.vertical(|ui| {
            let new = self.value(ui, Slot::of(field, defaults), value, &path, whole);
            self.refusal(ui, &field.ty, &path);
            new
        });
        let new = new.inner?;
        if whole {
            Some(vec![(name.clone(), new)])
        } else {
            self.edit(&path, new);
            None
        }
    }

    /// Draws `items`, the elements at `path` of a list or an array, each as `item` says but for its
    /// default, which is the one of its index in `defaults`, the default's elements, if there's
    /// one; a row each, as [`fields`](Self::fields) does.
    fn elements(
        &mut self,
        ui: &mut egui::Ui,
        item: Slot<'_>,
        defaults: &[Value],
        items: &[Value],
        path: &str,
        whole: bool,
    ) -> Option<Value> {
        let mut changed: Option<Vec<Value>> = None;
        egui::Grid::new((self.component, path))
            .num_columns(2)
            .show(ui, |ui| {
                for (i, value) in items.iter().enumerate() {
                    let path = format!("{path}[{i}]");
                    ui.label(format!("[{i}]"));
                    let slot = Slot {
                        default: defaults.get(i),
                        ..item
                    };
                    let new = self.value(ui, slot, value, &path, whole);
                    ui.end_row();
                    self.refusal(ui, item.ty, &path);
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
        &self,
        ui: &mut egui::Ui,
        names: impl Iterator<Item = &'n Ident>,
        picked: &Ident,
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

    /// A choice of the variants of an enum with fields, `variants`, at `path`, where its default is
    /// `default`, of which `name` is the one it is, with fields `given`; returns the one picked, as
    /// [`switched`](Self::switched) makes it. One that can't be switched to shows disabled, saying
    /// why on hover. The fields each variant last had are kept as they're drawn, which a set
    /// waiting for a tick makes them, under an id of the component's field, not the entity's, so
    /// that what's kept doesn't grow as entities are picked: they're the entity's, and go as
    /// another is drawn.
    fn variant(
        &self,
        ui: &mut egui::Ui,
        variants: &[Variant],
        default: Option<&Value>,
        name: &Ident,
        given: &[(Ident, Value)],
        path: &str,
    ) -> Option<Value> {
        type Kept = (EntityId, Vec<(Ident, Vec<(Ident, Value)>)>);
        let id = egui::Id::unique(("inspector variants", self.component, path));
        let entity = self.entity;
        // Written only as they differ, so that a frame clones nothing.
        ui.data_mut(|data| {
            let kept = data.get_temp_mut_or_insert_with(id, || -> Kept { (entity, Vec::new()) });
            if kept.0 != entity {
                *kept = (entity, Vec::new());
            }
            match kept.1.iter_mut().find(|(of, _)| of == name) {
                Some((_, fields)) if fields.as_slice() == given => {}
                Some((_, fields)) => *fields = given.to_vec(),
                None => kept.1.push((name.clone(), given.to_vec())),
            }
        });
        let mut picked = None;
        egui::ComboBox::from_id_salt(("variant", self.component, path))
            .selected_text(name.as_ref())
            .show_ui(ui, |ui| {
                // Read only while it's open.
                let kept = ui.data(|data| data.get_temp::<Kept>(id));
                let kept = kept.map_or_else(Vec::new, |(_, kept)| kept);
                for variant in variants {
                    let current = variant.name == *name;
                    let item = egui::Button::selectable(current, variant.name.as_ref());
                    if current {
                        ui.add(item);
                        continue;
                    }
                    match self.switched(variant, &kept, default) {
                        Ok(value) => {
                            if ui.add(item).clicked() {
                                picked = Some(value);
                            }
                        }
                        Err(why) => {
                            ui.add_enabled(false, item).on_disabled_hover_text(why);
                        }
                    }
                }
            });
        picked
    }

    /// `variant`, as a switch to it makes it: with the fields it last had on the entity, which
    /// `kept` holds, if it had any; or else `default`, if it's of that variant; or else with its
    /// fields' defaults. Fails, saying why, if its default fails, or if it holds what's opaque,
    /// which reflection doesn't make, so that a set of it would be refused.
    fn switched(
        &self,
        variant: &Variant,
        kept: &[(Ident, Vec<(Ident, Value)>)],
        default: Option<&Value>,
    ) -> Result<Value, String> {
        let last = kept.iter().find(|(name, _)| *name == variant.name);
        let made = match (last, default) {
            (Some((name, fields)), _) => Value::Variant(name.clone(), fields.clone()),
            (None, Some(default @ Value::Variant(name, _))) if *name == variant.name => {
                default.clone()
            }
            _ => self
                .types
                .variant_default(variant)
                .map_err(|error| error.to_string())?,
        };
        makeable(made)
    }

    /// An option, `given`, at `path`, holding `inner` when it's on, under the hints and default at
    /// `slot`: a checkbox, labeled `Some` or `None`, that turns it on or off, and, while it's on,
    /// what it holds, which is set whole with it. Turned on, it holds what it last held while the
    /// entity was drawn, kept as [`variant`](Self::variant) keeps a variant's fields; or else its
    /// default's, if that's on; or else its inside's type's default under the hints. One that can't
    /// be turned on, as its inside has no default, as a texture, or holds what's opaque, shows its
    /// checkbox disabled, saying why on hover: what it would hold is made each frame it's off, to
    /// know.
    fn option(
        &mut self,
        ui: &mut egui::Ui,
        slot: Slot<'_>,
        inner: &Type,
        given: Option<&Value>,
        path: &str,
    ) -> Option<Value> {
        type Kept = (EntityId, Value);
        let id = egui::Id::unique(("inspector option", self.component, path));
        let entity = self.entity;
        let Some(value) = given else {
            let kept = ui.data(|data| data.get_temp::<Kept>(id));
            let kept = kept.filter(|(of, _)| *of == entity).map(|(_, kept)| kept);
            let mut on = false;
            return match self.turned_on(slot, inner, kept) {
                Ok(held) => {
                    let turned = ui.checkbox(&mut on, "None").changed();
                    turned.then(|| Value::Option(Some(Box::new(held))))
                }
                Err(why) => {
                    let checkbox = egui::Checkbox::new(&mut on, "None");
                    ui.add_enabled(false, checkbox).on_disabled_hover_text(why);
                    None
                }
            };
        };
        // Written only as it differs, so that a frame clones nothing while it's on.
        ui.data_mut(|data| {
            let kept = data.get_temp_mut_or_insert_with(id, || -> Kept { (entity, value.clone()) });
            if kept.0 != entity || kept.1 != *value {
                *kept = (entity, value.clone());
            }
        });
        let default = match slot.default {
            Some(Value::Option(Some(default))) => Some(&**default),
            _ => None,
        };
        let inside = Slot {
            ty: inner,
            meta: slot.meta,
            default,
        };
        // In a vertical, which starts at the top of a grid's row: the grid centers a horizontal in
        // the row's height of the frame before as if it were a line high, which put what's taller,
        // as a struct's fields, half a row down, over the rows below.
        let new = ui.vertical(|ui| {
            let new = ui.horizontal(|ui| {
                let mut on = true;
                if ui.checkbox(&mut on, "Some").changed() {
                    return Some(Value::Option(None));
                }
                // As it's given: a set waiting at its path is the option's, which drew it.
                let held = self.draw(ui, inside, value, path, true)?;
                Some(Value::Option(Some(Box::new(held))))
            });
            new.inner
        });
        new.inner
    }

    /// What an option holding `inner`, under the hints and default at `slot`, holds as it's turned
    /// on, as [`option`](Self::option) says: `kept`, what it last held on the entity, if it held
    /// anything; or else its default's inside, if that's on; or else `inner`'s default under the
    /// hints. Fails, saying why, as [`switched`](Self::switched) does.
    fn turned_on(
        &self,
        slot: Slot<'_>,
        inner: &Type,
        kept: Option<Value>,
    ) -> Result<Value, String> {
        let made = match (kept, slot.default) {
            (Some(kept), _) => kept,
            (None, Some(Value::Option(Some(default)))) => (**default).clone(),
            _ => (self.types.default_value(inner, slot.meta)).map_err(|error| error.to_string())?,
        };
        makeable(made)
    }

    /// A rotation as angles about x, y and z, in degrees, applied z first, each joined to a button
    /// of its axis that sets it to the angle of the default rotation at `slot`; it's set whole. The
    /// angles are kept with the rotation they made, so that those typed stay as they are, rather
    /// than jump to the same rotation's other angles, while the rotation stays within float noise
    /// of it: a system that renormalizes it at each tick can change its last bits, which would
    /// reset the text being typed. They're kept under an id of the component's field, not the
    /// entity's, so that what's kept doesn't grow as entities are picked.
    fn rotation(&self, ui: &mut egui::Ui, slot: Slot<'_>, q: Quat, path: &str) -> Option<Value> {
        type Kept = (Quat, [f32; 3]);
        let id = egui::Id::unique(("inspector rotation", self.component, path));
        let kept = ui.data(|data| data.get_temp::<Kept>(id));
        let mut angles = match kept {
            Some((kept, angles)) if same_rotation(kept, q) => angles,
            _ => {
                let angles = angles_of(q);
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
            let width = axes_row(ui, angles.len());
            for (axis, angle) in angles.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    axis_spacing(ui, width);
                    if axis_button(ui, axis)
                        && let Some(Value::Quat(default)) = self.default_at(slot, path).as_deref()
                    {
                        *angle = angles_of(*default)[axis];
                        changed = true;
                    }
                    let drag = egui::DragValue::new(angle)
                        .speed(0.5)
                        .suffix("°")
                        .custom_formatter(signed);
                    changed |= ui.add(drag).changed();
                });
            }
        });
        if !changed {
            return None;
        }
        let [x, y, z] = angles.map(math::to_radians);
        let q = Quat::from_euler(EulerRot::ZYX, z, y, x).normalize();
        ui.data_mut(|data| data.insert_temp(id, (q, angles)));
        Some(Value::Quat(q))
    }

    /// Drags each of a vector's `components`, at `path`, under the hints at `slot`, each joined to
    /// a button of its axis that sets it to the default's there. Each that changes is set at its
    /// own path, as `x` is, unless they're set `whole`: then they're returned, if one changed.
    fn vector<const N: usize>(
        &mut self,
        ui: &mut egui::Ui,
        slot: Slot<'_>,
        mut components: [f64; N],
        path: &str,
        whole: bool,
        is_f32: bool,
    ) -> Option<[f64; N]> {
        let mut changed = false;
        ui.horizontal(|ui| {
            let width = axes_row(ui, N);
            for (axis, component) in components.iter_mut().enumerate() {
                let new = ui.horizontal(|ui| {
                    axis_spacing(ui, width);
                    let reset = axis_button(ui, axis);
                    let dragged = float(ui, *component, slot.meta, is_f32);
                    if reset {
                        let default = self.default_at(slot, path)?;
                        component_of(&default, axis)
                    } else {
                        dragged.map(|x| if is_f32 { f64::from(x as f32) } else { x })
                    }
                });
                let Some(new) = new.inner else {
                    continue;
                };
                *component = new;
                if whole {
                    changed = true;
                } else {
                    let new = if is_f32 {
                        Value::F32(new as f32)
                    } else {
                        Value::F64(new)
                    };
                    self.edit(&reflect::join(path, AXES[axis].0), new);
                }
            }
        });
        changed.then_some(components)
    }

    /// The default at `slot`, of the field at `path`: the one there, or else its type's under its
    /// field's bounds, made as it's asked for, which a reset does, so that a frame makes none;
    /// `None` if its type has none there, which is logged.
    fn default_at<'s>(&self, slot: Slot<'s>, path: &str) -> Option<Cow<'s, Value>> {
        if let Some(default) = slot.default {
            return Some(Cow::Borrowed(default));
        }
        match self.types.default_value(slot.ty, slot.meta) {
            Ok(default) => Some(Cow::Owned(default)),
            Err(error) => {
                warn!(
                    code = error.code(),
                    entity = %self.entity,
                    component = self.component,
                    path,
                    "{error}"
                );
                None
            }
        }
    }

    /// A linear color, alpha not multiplied in, `rgba`, the field at `path`, with alpha as `alpha`
    /// says. It's edited as `Hsva`, which keeps the color of what's transparent, where egui's
    /// button of unmultiplied RGBA multiplies alpha in and loses it; and the `Hsva` is kept with
    /// the color it made, as a rotation's angles are, so that a gray's hue, which its color doesn't
    /// hold, stays as it's picked. What egui's popup gives back for it untouched isn't an edit: it
    /// passes the color through gamma and back each frame it's open, which moves its last bits.
    fn color(
        &self,
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
    pub(super) fn edit(&mut self, path: &str, value: Value) {
        self.edits.push(Edit {
            component: self.component.to_owned(),
            path: path.to_owned(),
            value,
        });
    }

    /// Shows, each on a row of its own, why the field at `path`, of type `ty`, last refused a
    /// value, if it did, and, if it's a vector, why each of its components did.
    pub(super) fn refusal(&self, ui: &mut egui::Ui, ty: &Type, path: &str) {
        let vector = matches!(ty, Type::Vec2 | Type::Vec3 | Type::Vec4 | Type::DVec3);
        let refused = self.refusals.iter().filter(|refusal| {
            refusal.component == self.component
                && (refusal.path == path || (vector && axis_of(path, &refusal.path).is_some()))
        });
        for refusal in refused {
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

/// `q`'s angles about x, y and z, in degrees, applied z first.
fn angles_of(q: Quat) -> [f32; 3] {
    let (z, y, x) = q.to_euler(EulerRot::ZYX);
    [x, y, z].map(math::to_degrees)
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
    // A value outside the bounds, which it keeps, may be negative too.
    let negative = range.start().to_f64() < 0.0 || n.to_f64() < 0.0;
    let mut drag = egui::DragValue::new(n)
        .range(range)
        .clamp_existing_to_range(false)
        .speed(speed);
    if negative {
        drag = drag.custom_formatter(signed);
    }
    ui.add(drag).changed()
}

/// A number as egui shows one, with the fewest decimals in `decimals` that give it back, after a
/// minus sign, U+2212, if it's negative, or else a figure space, U+2007, which egui's parser
/// skips: each as wide as a digit in the UI's Inter, so that its digits hold still as its sign
/// comes and goes. A negative number that shows as zero shows as zero, without a sign that would
/// flicker as it crosses it.
fn signed(n: f64, decimals: std::ops::RangeInclusive<usize>) -> String {
    let text = egui::emath::format_with_decimals_in_range(n.abs(), decimals);
    let zero = text.bytes().all(|byte| matches!(byte, b'0' | b'.'));
    let sign = if n < 0.0 && !zero {
        '\u{2212}'
    } else {
        '\u{2007}'
    };
    format!("{sign}{text}")
}

/// How far apart the axes of a row are, and how far a number is padded across in its field, at
/// the least, in points: half egui's spacing and a quarter of its padding, which leaves the fields
/// of a panel as narrow as the editor's Inspector is at first wide enough for numbers such as
/// −1.875, each centered in its field.
const AXIS_GAP: f32 = 4.0;
const FIELD_PADDING: f32 = 2.0;

/// Spaces `ui`, a row of `axes` axes, each a button and a field, [`AXIS_GAP`] apart; returns how
/// wide each field is, after its axis's button: what shows of the row, from `ui`'s cursor, shared
/// out equally, as Unity's vectors' fields are, so that a number that widens doesn't push the
/// fields after it; or egui's narrowest, in a row too narrow for that, which runs past its edge.
fn axes_row(ui: &mut egui::Ui, axes: usize) -> f32 {
    ui.spacing_mut().item_spacing.x = AXIS_GAP;
    let spacing = ui.spacing();
    let gaps = AXIS_GAP * (axes - 1) as f32;
    let row = shown_right(ui) - ui.cursor().min.x;
    let axis = (row - gaps) / axes as f32;
    (axis - spacing.interact_size.y).max(spacing.interact_size.x)
}

/// Spaces `ui`, an axis of a row whose fields are `width` wide: its button against its field, and
/// its field's number padded [`FIELD_PADDING`] across; a field takes the least size it may be
/// from egui's `interact_size`.
fn axis_spacing(ui: &mut egui::Ui, width: f32) {
    let spacing = ui.spacing_mut();
    spacing.item_spacing.x = 0.0;
    spacing.interact_size.x = width;
    spacing.button_padding.x = FIELD_PADDING;
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
/// further than the field's bounds, as [`World::set_field`](crate::World::set_field) compares
/// them, and finite, so that what's dragged or typed is a value the field takes. NaN is shown as
/// text, as wide as a field: a drag over it would change it every frame.
fn float(ui: &mut egui::Ui, x: f64, meta: &Meta, is_f32: bool) -> Option<f64> {
    if x.is_nan() {
        ui.add_sized(ui.spacing().interact_size, egui::Label::new(x.to_string()));
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
        .speed(speed(meta, if degrees { 0.5 / scale } else { 0.01 }) * scale)
        .custom_formatter(signed);
    if degrees {
        drag = drag.suffix("°");
    }
    ui.add(drag).changed().then(|| shown / scale)
}

/// The button of axis `axis`, in its color, as high and as wide as a field's widget is high, its
/// label egui's strong text, the brightest against the color, in the family [`crate::BOLD`] names
/// where the context binds it: a game may set fonts of its own, and egui draws text in a family it
/// hasn't bound as boxes. Whether it was clicked.
fn axis_button(ui: &mut egui::Ui, axis: usize) -> bool {
    let (_, label, fill, hovered) = AXES[axis];
    let rgb = |[r, g, b]: [u8; 3]| egui::Color32::from_rgb(r, g, b);
    let size = egui::Vec2::splat(ui.spacing().interact_size.y);
    // Found among the few families bound, rather than looked up by a key made of the name, which
    // would allocate for each button each frame; the family found is shared, not copied.
    let bold = ui.ctx().fonts(|fonts| {
        let mut families = fonts.definitions().families.keys();
        let bold = |family: &&egui::FontFamily| {
            matches!(family, egui::FontFamily::Name(name) if **name == *crate::BOLD)
        };
        families.find(bold).cloned()
    });
    let label = egui::RichText::new(label).strong();
    let label = match bold {
        Some(bold) => label.family(bold),
        None => label,
    };
    // Its own colors, in a scope of its own: `Button::fill` would keep it from lighting up as
    // it's hovered; and no padding beside its label, which would widen it past its height.
    ui.scope(|ui| {
        ui.spacing_mut().button_padding.x = 0.0;
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.weak_bg_fill = rgb(fill);
        widgets.hovered.weak_bg_fill = rgb(hovered);
        widgets.active.weak_bg_fill = rgb(fill);
        let button = egui::Button::new(label).min_size(size);
        ui.add(button).clicked()
    })
    .inner
}

/// Which component of the vector at `path` the path `at` leads to: 0 for `x` to 3 for `w`; `None`
/// if it leads elsewhere.
fn axis_of(path: &str, at: &str) -> Option<usize> {
    let rest = at.strip_prefix(path)?;
    let name = if path.is_empty() {
        rest
    } else {
        rest.strip_prefix('.')?
    };
    AXES.iter().position(|&(axis, ..)| axis == name)
}

/// Component `axis` of `value`, a vector; `None` if it's of another kind, or hasn't that one.
fn component_of(value: &Value, axis: usize) -> Option<f64> {
    match value {
        Value::Vec2(v) if axis < 2 => Some(v[axis].into()),
        Value::Vec3(v) if axis < 3 => Some(v[axis].into()),
        Value::Vec4(v) if axis < 4 => Some(v[axis].into()),
        Value::DVec3(v) if axis < 3 => Some(v[axis]),
        _ => None,
    }
}

/// Sets component `axis` of `value`, a vector, to `x`; leaves it as it is if it's of another kind,
/// or hasn't that one.
fn set_component(value: &mut Value, axis: usize, x: f64) {
    match value {
        Value::Vec2(v) if axis < 2 => v[axis] = x as f32,
        Value::Vec3(v) if axis < 3 => v[axis] = x as f32,
        Value::Vec4(v) if axis < 4 => v[axis] = x as f32,
        Value::DVec3(v) if axis < 3 => v[axis] = x,
        _ => {}
    }
}

/// Where in `value` the first thing it holds that's opaque is, which reflection keeps but doesn't
/// make, as a path, such as `rect[1]`; `None` if it holds nothing opaque.
fn opaque_at(value: &Value) -> Option<String> {
    match value {
        Value::Opaque => Some(String::new()),
        Value::Option(Some(inner)) => opaque_at(inner),
        Value::List(items) => items.iter().enumerate().find_map(|(i, item)| {
            let at = opaque_at(item)?;
            Some(reflect::join(&format!("[{i}]"), &at))
        }),
        Value::Struct(fields) | Value::Variant(_, fields) => {
            fields.iter().find_map(|(name, field)| {
                let at = opaque_at(field)?;
                Some(reflect::join(name, &at))
            })
        }
        _ => None,
    }
}

/// `made`, as a switch to it or a turn on of it makes it, or why it can't be: it holds what's
/// opaque, which reflection doesn't make, so that a set of it would be refused, as a make of it is.
fn makeable(made: Value) -> Result<Value, String> {
    match opaque_at(&made) {
        Some(at) => {
            reflect::make_opaque(&made).map_err(|error| reflect::within(error, &at).to_string())
        }
        None => Ok(made),
    }
}
