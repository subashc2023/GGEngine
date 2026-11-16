//! The Inspector panel's fields: a widget per field by its type and hints, drawn as the sets
//! waiting for a tick will make it, keeping the edits it makes.

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
    /// The component's own fields, by name, that differ from the instance's `prefab`: marked, and
    /// reverted from the label's context menu (into `operations`). None if no instance.
    pub(super) overridden: &'a [String],
    pub(super) prefab: &'a str,
    pub(super) operations: &'a mut Vec<Command>,
}

/// What's drawn at a path: its type, its field's hints (inherited by parts and elements), and its
/// default if any, by reference so a frame clones none: the container's default narrowed to it,
/// else its field's own.
#[derive(Clone, Copy)]
pub(super) struct Slot<'s> {
    pub(super) ty: &'s Type,
    pub(super) meta: &'s Meta,
    pub(super) default: Option<&'s Value>,
}

impl<'s> Slot<'s> {
    /// Where `field` is drawn; its default is the one named in `defaults` (the container's), else
    /// its own.
    fn of(field: &'s Field, defaults: &'s [(Ident, Value)]) -> Self {
        let given = defaults.iter().find(|(name, _)| *name == field.name);
        Self {
            ty: &field.ty,
            meta: &field.meta,
            default: given.map(|(_, default)| default).or(field.default.as_ref()),
        }
    }
}

/// Each axis of a vector or a rotation's angles: its path name, its button's label, and the
/// button's RGB color, then hovered. Hazel's colors, grays for `w`.
pub(super) const AXES: [(&str, &str, [u8; 3], [u8; 3]); 4] = [
    ("x", "X", [204, 25, 38], [230, 51, 51]),
    ("y", "Y", [51, 179, 51], [77, 204, 77]),
    ("z", "Z", [25, 64, 204], [51, 89, 230]),
    ("w", "W", [102, 102, 102], [128, 128, 128]),
];

impl Fields<'_> {
    /// Draws `value`, the field at `path`, as `slot` says. Returns its new value if a widget
    /// changed it and it's set `whole` (inside an option or enum); otherwise parts set themselves
    /// at their own paths.
    ///
    /// - A field is set only when its widget says it changed, never by comparing values.
    /// - A field with a set waiting for a tick is drawn as the last such set will make it, so its
    ///   widget, which keeps its shown state between frames, goes on from there.
    /// - A vector is drawn as the sets of it and of its components will make it, in order, since
    ///   recordings hold sets of it whole.
    /// - Other than a vector, a field is set at its own path or whole with its container, never
    ///   both, so their sets never wait together.
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

    /// Like [`value`](Self::value), but draws `value` as given.
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

    /// Draws `given`, the values of `fields` at `path`, a row each, the doc on the name. Defaults
    /// are from `defaults` (the container's), else the field's own. Returns the values with
    /// changes if set `whole`; else each changed field is set at its own path.
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

    /// Draws the label of `field` at `path`, which differs from the prefab's: strong, marked, a
    /// hover saying so above the doc, and a context menu whose Revert to Prefab sets it back.
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

    /// Like [`fields`](Self::fields) for a tuple struct's only field, with no label (it would say
    /// `0`) and in its own group, so a refusal goes under it, not into a grid row.
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

    /// Like [`fields`](Self::fields) for `items`, the elements at `path` of a list or array, each
    /// as `item` says but with its default from `defaults` by index, if any.
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

    /// A choice among an enum's unit variants `names`; `picked` is the current one.
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

    /// A choice among an enum's variants at `path`; `name` is the current one, with fields
    /// `given`. Returns the one picked, made by [`switched`](Self::switched). A variant that can't
    /// be switched to is disabled, saying why on hover.
    ///
    /// Each variant's last fields are kept as drawn (as a waiting set makes them), under an id of
    /// the component's field, not the entity's, so the store doesn't grow as entities are picked.
    /// They're the entity's, and go as another is drawn.
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

    /// `variant` as a switch makes it: with its last fields on the entity (`kept`); else
    /// `default`, if of that variant; else its fields' defaults. Fails, saying why, if its default
    /// fails or it holds what's opaque, which reflection can't make, so a set would be refused.
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

    /// An option `given` at `path`, holding `inner` when on: a `Some`/`None` checkbox and, while
    /// on, what it holds, set whole with it.
    ///
    /// Turned on, it holds what it last held for the entity (kept like
    /// [`variant`](Self::variant)'s fields); else its default's, if on; else `inner`'s default
    /// under the hints. One that can't be turned on (no default inside, as a texture, or opaque)
    /// has its checkbox disabled, saying why on hover. What it would hold is made each frame it's
    /// off, to know.
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
        // In a vertical, which starts at the top of a grid's row: a grid centers a horizontal as if
        // a line high, which put taller content (a struct's fields) half a row down, over the
        // rows below.
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

    /// What an option holds when turned on ([`option`](Self::option)): `kept`; else its default's
    /// inside, if on; else `inner`'s default. Fails like [`switched`](Self::switched).
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

    /// A rotation as angles about x, y and z, in degrees, applied z first, each with a button that
    /// sets it to the default rotation's angle at `slot`. Set whole.
    ///
    /// The angles are kept with the rotation they made, so typed ones stay rather than jump to the
    /// same rotation's other angles, while it stays within float noise of it: a system
    /// renormalizing it each tick changes its last bits, which would reset the text being typed.
    /// They're kept under an id of the component's field, not the entity's, so the store doesn't
    /// grow as entities are picked.
    fn rotation(&self, ui: &mut egui::Ui, slot: Slot<'_>, q: Quat, path: &str) -> Option<Value> {
        type Kept = (Quat, [f32; 3]);
        let id = egui::Id::unique(("inspector rotation", self.component, path));
        let kept = ui.data(|data| data.get_temp::<Kept>(id));
        let mut angles = match kept {
            Some((kept, angles)) if same_rotation(kept, q) => angles,
            _ => {
                let angles = angles_of(q);
                // Text, as NaN is: a drag over NaN would change it every frame. A rotation that
                // isn't finite has NaN angles.
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

    /// Drags a vector's `components` at `path`, each with a button that sets it to the default's.
    /// A changed one is set at its own path (`x`), unless set `whole`: then all are returned.
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

    /// The default at `slot`: the one there, else its type's under the field's bounds, made on
    /// request (a reset), so a frame makes none. `None`, logged, if its type has none.
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

    /// A linear color `rgba` (alpha not multiplied in) at `path`, with `alpha` mode.
    ///
    /// Edited as `Hsva`, which keeps a transparent color (egui's unmultiplied RGBA button
    /// multiplies alpha in and loses it). The `Hsva` is kept with the color it made, like a
    /// rotation's angles, so a gray's hue stays as picked. What egui's popup returns untouched
    /// isn't an edit: it round-trips the color through gamma each frame it's open, moving its
    /// last bits.
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

    /// Shows, a row each, why the field at `path` (and, for a vector, each component) last
    /// refused a value.
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

/// Whether `a` and `b` are one rotation within float noise: 1e-6 in each component, about a
/// ten-thousandth of a degree, below the three decimals shown. `q` and `-q` are one rotation.
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

/// The range an integer of `bits` may have, within `meta`'s bounds.
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

/// Drags `n` within `range`, the field's bounds, at the speed they pick. A value outside them is
/// shown as it is.
fn drag<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    n: &mut N,
    range: std::ops::RangeInclusive<N>,
    meta: &Meta,
) -> bool {
    let speed = speed(meta, 0.25);
    // A kept value outside the bounds may be negative too.
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
/// minus sign (U+2212) if negative, else a figure space (U+2007), which egui's parser skips. Both
/// are as wide as an Inter digit, so digits hold still as the sign comes and goes. A negative
/// number showing as zero has no sign, which would flicker as it crosses zero.
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

/// The gap between a row's axes and the least padding of a number in its field, in points: half
/// egui's spacing and a quarter of its padding, so the editor's Inspector is at first just wide
/// enough for numbers such as −1.875, centered in their fields.
const AXIS_GAP: f32 = 4.0;
const FIELD_PADDING: f32 = 2.0;

/// Spaces `ui`, a row of `axes` axes (button and field), [`AXIS_GAP`] apart. Returns each field's
/// width: the visible row from `ui`'s cursor shared equally, as Unity does, so a widening number
/// doesn't push later fields; or egui's narrowest, running past the edge of a too-narrow row.
fn axes_row(ui: &mut egui::Ui, axes: usize) -> f32 {
    ui.spacing_mut().item_spacing.x = AXIS_GAP;
    let spacing = ui.spacing();
    let gaps = AXIS_GAP * (axes - 1) as f32;
    let row = shown_right(ui) - ui.cursor().min.x;
    let axis = (row - gaps) / axes as f32;
    (axis - spacing.interact_size.y).max(spacing.interact_size.x)
}

/// Spaces `ui`, an axis of a row whose fields are `width` wide: button against field, the number
/// padded [`FIELD_PADDING`]. A field takes its least size from egui's `interact_size`.
fn axis_spacing(ui: &mut egui::Ui, width: f32) {
    let spacing = ui.spacing_mut();
    spacing.item_spacing.x = 0.0;
    spacing.interact_size.x = width;
    spacing.button_padding.x = FIELD_PADDING;
}

/// How much a drag of a pixel changes a field's number: a 200th of its range if bounded both
/// ways, else `otherwise`.
fn speed(meta: &Meta, otherwise: f64) -> f64 {
    match (meta.min, meta.max) {
        (Some(min), Some(max)) if max.as_f64() > min.as_f64() => {
            (max.as_f64() - min.as_f64()) / 200.0
        }
        _ => otherwise,
    }
}

/// Drags `x` (an `f32`'s if `is_f32`, in degrees if `meta` says), returning it if changed. It's
/// within the bounds as [`World::set_field`](crate::World::set_field) compares them, and finite,
/// so the field takes it. NaN shows as text as wide as a field: a drag over it would change it
/// every frame.
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

/// The axis `axis` button, in its color, square at a field's height, labeled in strong text in
/// the family [`crate::BOLD`] names where the context binds it (a game may set its own fonts, and
/// egui draws an unbound family as boxes). Whether it was clicked.
fn axis_button(ui: &mut egui::Ui, axis: usize) -> bool {
    let (_, label, fill, hovered) = AXES[axis];
    let rgb = |[r, g, b]: [u8; 3]| egui::Color32::from_rgb(r, g, b);
    let size = egui::Vec2::splat(ui.spacing().interact_size.y);
    // Found among the few families bound: a key made of the name would allocate per button per
    // frame.
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
    // Colors in a scope of their own: `Button::fill` would stop it lighting up on hover. No
    // padding beside the label, which would widen it past its height.
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

/// Which component of the vector at `path` the path `at` leads to, 0 for `x` to 3 for `w`; `None`
/// if elsewhere.
fn axis_of(path: &str, at: &str) -> Option<usize> {
    let rest = at.strip_prefix(path)?;
    let name = if path.is_empty() {
        rest
    } else {
        rest.strip_prefix('.')?
    };
    AXES.iter().position(|&(axis, ..)| axis == name)
}

/// Component `axis` of the vector `value`; `None` if it's another kind or lacks that one.
fn component_of(value: &Value, axis: usize) -> Option<f64> {
    match value {
        Value::Vec2(v) if axis < 2 => Some(v[axis].into()),
        Value::Vec3(v) if axis < 3 => Some(v[axis].into()),
        Value::Vec4(v) if axis < 4 => Some(v[axis].into()),
        Value::DVec3(v) if axis < 3 => Some(v[axis]),
        _ => None,
    }
}

/// Sets component `axis` of the vector `value` to `x`; no change if it's another kind or lacks
/// that one.
fn set_component(value: &mut Value, axis: usize, x: f64) {
    match value {
        Value::Vec2(v) if axis < 2 => v[axis] = x as f32,
        Value::Vec3(v) if axis < 3 => v[axis] = x as f32,
        Value::Vec4(v) if axis < 4 => v[axis] = x as f32,
        Value::DVec3(v) if axis < 3 => v[axis] = x,
        _ => {}
    }
}

/// The path, such as `rect[1]`, of the first opaque thing in `value`, which reflection keeps but
/// doesn't make; `None` if there is none.
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

/// `made`, as a switch or turn on makes it, or why it can't be: it holds what's opaque, which
/// reflection doesn't make, so a set of it would be refused, as a make of it is.
fn makeable(made: Value) -> Result<Value, String> {
    match opaque_at(&made) {
        Some(at) => {
            reflect::make_opaque(&made).map_err(|error| reflect::within(error, &at).to_string())
        }
        None => Ok(made),
    }
}
