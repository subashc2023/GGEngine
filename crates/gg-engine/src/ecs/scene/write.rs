//! The canonical writer: a scene's text as the engine writes it, so that its bytes depend on the
//! entities alone, never on a library's formatting, the platform's line endings or the escapes of
//! a Rust release.

use std::collections::{HashMap, HashSet};
use std::fmt::{self, Write as _};

use gg_core::{Error, Result};

use super::{Expanding, MAX_DEPTH, NO_HINTS};
use crate::ecs::by_name::about;
use crate::ecs::diff::same;
use crate::ecs::id::Named;
use crate::ecs::prefab::Claims;
use crate::ecs::table::{AnyColumn, ComponentId};
use crate::ecs::{Entity, EntityId, Parent, World};
use crate::reflect::{self, Field, Shape, Type, Types, Value};
use crate::{ModelNode, PrefabRoot};

/// RON's words, which no name a scene writes is, so that a name never reads as a value.
pub(in crate::ecs) const WORDS: [&str; 10] = [
    "true", "false", "Some", "None", "inf", "inff32", "inff64", "NaN", "NaNf32", "NaNf64",
];

/// `roots`, and their descendants if `descendants`, as a scene's text, as [`World::save_scene`]
/// says.
pub(super) fn save(world: &World, roots: &[Entity], descendants: bool) -> Result<String> {
    for &root in roots {
        world.locate(root)?;
        world.not_a_node(root)?;
    }
    // Each once.
    let mut saved = Vec::new();
    let mut seen = HashSet::new();
    let mut next = roots.to_vec();
    while let Some(entity) = next.pop() {
        if seen.insert(entity) {
            saved.push((world.id(entity).expect("a saved entity is alive"), entity));
            if descendants {
                for child in world.children(entity).expect("a saved entity is alive") {
                    match world.get::<ModelNode>(child) {
                        Some(_) => only_nodes(world, child)?,
                        None => next.push(child),
                    }
                }
            }
        }
    }
    let claims = instances(world, &mut saved)?;
    // In the order of their IDs.
    saved.sort_unstable_by_key(|&(id, _)| id);
    let mut writer = Writer {
        world,
        types: world.types(),
        saved: &saved,
        claims: &claims,
        out: String::new(),
        built: HashMap::new(),
    };
    writer.scene()?;
    Ok(writer.out)
}

/// The instances among `saved` whose roots are, each by the outermost; and the entities they give,
/// which go into `saved` with their roots.
///
/// Fails as an instance's template does, as [`Expanding::template`] says, and with
/// `scene.prefab_missing` for an entity an instance's prefab gives that isn't alive, as one code
/// despawned, which a scene can't save the instance without.
fn instances(world: &World, saved: &mut Vec<(EntityId, Entity)>) -> Result<Claims> {
    let depth = |mut entity: Entity| {
        let mut depth = 0;
        while let Some(up) = world.parent(entity).expect("a saved entity is alive") {
            entity = up;
            depth += 1;
        }
        depth
    };
    let mut linked: Vec<(usize, EntityId, Entity)> = (saved.iter())
        .filter(|&&(_, entity)| world.get::<PrefabRoot>(entity).is_some())
        .map(|&(id, entity)| (depth(entity), id, entity))
        .collect();
    linked.sort_unstable();
    let mut claims = Claims::default();
    let mut expanding = Expanding::default();
    for (_, id, root) in linked {
        if claims.has(id) {
            continue;
        }
        let instance = world.instance(root, &mut expanding)?.expect("linked");
        if let Some(gone) = instance.ids[1..]
            .iter()
            .find(|&&id| world.entity(id).is_none())
        {
            return Err(Error::new(
                "scene.prefab_missing",
                format!(
                    "entity {gone}, which the prefab of the instance whose root is entity {id} \
                     gives, isn't in the world, but a scene saves an instance with every entity \
                     its prefab gives"
                ),
            ));
        }
        claims.claim(root, instance);
    }
    let seen: HashSet<EntityId> = saved.iter().map(|&(id, _)| id).collect();
    for (_, instance) in &claims.instances {
        for &id in &instance.ids[1..] {
            if !seen.contains(&id) {
                saved.push((id, world.entity(id).expect("checked alive")));
            }
        }
    }
    Ok(claims)
}

/// Fails with `scene.model_node` if anything under `node`, a model's node, which the world spawns
/// under the model's root and scenes don't save, isn't a node of the model too: what a scene can't
/// save, as its parent isn't saved.
fn only_nodes(world: &World, node: Entity) -> Result<()> {
    let mut next = vec![node];
    while let Some(entity) = next.pop() {
        for child in world
            .children(entity)
            .expect("a saved entity's descendant is alive")
        {
            if world.get::<ModelNode>(child).is_none() {
                let (child, entity) = (world.name_of(child), world.name_of(entity));
                return Err(Error::new(
                    "scene.model_node",
                    format!(
                        "entity {child} is a child of entity {entity}, a node of a model, which \
                         the world spawns under the model's root and scenes don't save: make it a \
                         child of the root, or of another entity"
                    ),
                ));
            }
            next.push(child);
        }
    }
    Ok(())
}

struct Writer<'w> {
    world: &'w World,
    types: &'w Types,
    /// The entities saved, by ID.
    saved: &'w [(EntityId, Entity)],
    /// The instances whose roots are saved, and the entities they give.
    claims: &'w Claims,
    out: String,
    /// A column of each component type saved, in which each component is made again, as a load
    /// makes it, and taken out of again.
    built: HashMap<ComponentId, Box<dyn AnyColumn>>,
}

impl Writer<'_> {
    fn scene(&mut self) -> Result<()> {
        if self.saved.is_empty() {
            self.out.push_str("(\n    entities: {},\n)\n");
            return Ok(());
        }
        self.out.push_str("(\n    entities: {\n");
        for &(id, entity) in self.saved {
            self.entity(id, entity)?;
        }
        self.out.push_str("    },\n)\n");
        Ok(())
    }

    /// Writes entity `id` and its components, in the order of their names: of an instance's root,
    /// or of an entity an instance saved gives, those that differ from what the prefab gives it,
    /// each struct of named fields with the fields that differ alone, and of an entity an instance
    /// gives where none does, nothing.
    fn entity(&mut self, id: EntityId, entity: Entity) -> Result<()> {
        let world = self.world;
        let at = world.location(entity).expect("a saved entity is alive");
        let table = &world.tables[at.table as usize];
        let parent = world.component::<Parent>();
        // What its prefab gives it, if an instance saved does: the instance, and its part.
        let claims = self.claims;
        let roots = claims.instances.iter();
        let base = (claims.of(id)).or_else(|| {
            let root = roots.clone().position(|&(root, _)| root == entity);
            root.map(|at| (at, 0))
        });
        let instance = base.map(|(at, part)| (&claims.instances[at].1, part));
        // An entity an instance gives is saved under a parent the scene saves: a load puts one
        // whose parent the text leaves out under its prefab's.
        if let Some((at, part)) = base
            && part > 0
        {
            let up = world.parent(entity).expect("a saved entity is alive");
            if up.is_none_or(|up| !self.is_saved(up)) {
                let root = world.name_of(claims.instances[at].0);
                let place = match up {
                    None => "has no parent".to_owned(),
                    Some(up) => format!("is under entity {}, which isn't saved", world.name_of(up)),
                };
                return Err(Error::new(
                    "scene.prefab_parent",
                    format!(
                        "entity {id}, which the prefab of the instance whose root is entity {root} \
                         gives, {place}, but a scene loads it under its prefab's parent where it \
                         saves none: give it a parent the scene saves"
                    ),
                ));
            }
        }
        let given = instance.map_or_else(Vec::new, |(of, part)| of.components(world, part));
        if let Some(&gone) = given.iter().find(|&&component| !table.has(component)) {
            let name = world.components.name(gone);
            let error = Error::new(
                "scene.prefab_missing",
                format!(
                    "its prefab gives it a `{name}`, which it hasn't, but a scene saves an \
                     instance's entities with every component their prefab gives them"
                ),
            );
            return Err(about(error, "save", Named::Id(id), name, ""));
        }
        let mut components = Vec::new();
        for (column, &component) in table.components().iter().enumerate() {
            if world.components.ty(component).is_opaque(self.types) {
                continue;
            }
            if Some(component) == parent {
                let up = world
                    .get::<Parent>(entity)
                    .expect("the entity has a parent")
                    .0;
                let placed = instance
                    .is_some_and(|(of, part)| part > 0 && of.parent(world, part) == Some(up));
                if !self.is_saved(up) || placed {
                    continue;
                }
            }
            let value = table.column_at(column).value(at.row as usize);
            let base = (instance.filter(|_| given.contains(&component)))
                .and_then(|(of, part)| of.value(world, part, component));
            let written = match &base {
                None => Written::Whole,
                Some(base) if same(&value, world, base, world) => continue,
                Some(base) => match (&value, base) {
                    (Value::Struct(fields), Value::Struct(was))
                        if world.named_fields(component) && fields.len() == was.len() =>
                    {
                        let fields = fields.iter().zip(was);
                        let differ =
                            fields.filter(|((_, now), (_, was))| !same(now, world, was, world));
                        Written::Fields(differ.map(|((name, _), _)| name.clone()).collect())
                    }
                    _ => Written::Whole,
                },
            };
            components.push((world.components.name(component), column, value, written));
        }
        components.sort_unstable_by_key(|&(name, ..)| name);
        if components.is_empty() && base.is_some_and(|(_, part)| part > 0) {
            return Ok(());
        }
        write!(self.out, "        \"{id}\": ").expect("a string takes any text");
        if components.is_empty() {
            self.out.push_str("(),\n");
            return Ok(());
        }
        self.out.push_str("(\n");
        for (name, column, value, written) in components {
            let about = |error: Error| about(error, "save", Named::Id(id), name, "");
            let component = table.components()[column];
            let ty = world.components.ty(component);
            reflect::check(self.types, &ty, &NO_HINTS, &value).map_err(about)?;
            let built = self
                .built
                .entry(component)
                .or_insert_with(|| world.components.new_column(component));
            built.push_value(&value).map_err(about)?;
            built.swap_remove(0);
            writable(name).map_err(about)?;
            pad(&mut self.out, 3);
            self.out.push_str(name);
            self.out.push_str(": ");
            match written {
                Written::Whole => self.value(&ty, &value, 3, 1).map_err(about)?,
                Written::Fields(differ) => {
                    let (Type::Named(index), Value::Struct(given)) = (&ty, &value) else {
                        unreachable!("a struct's fields are written of a struct");
                    };
                    let Shape::Struct { fields, .. } = &self.types.info(*index).shape else {
                        unreachable!("a struct's fields are written of a struct");
                    };
                    let parts = self.saved_parts(fields, given).map_err(about)?;
                    let parts = parts
                        .into_iter()
                        .filter(|(field, _)| differ.contains(&field.name));
                    let parts: Vec<(&Field, &Value)> = parts.collect();
                    self.block(&parts, false, 3, 1).map_err(about)?;
                }
            }
            self.out.push_str(",\n");
        }
        self.out.push_str("        ),\n");
        Ok(())
    }

    /// Whether `entity` is alive and saved.
    fn is_saved(&self, entity: Entity) -> bool {
        let id = self.world.id(entity);
        id.is_some_and(|id| self.saved.binary_search_by_key(&id, |&(id, _)| id).is_ok())
    }

    /// Writes `value`, of type `ty`, which is `depth` levels deep, from a line indented `indent`
    /// levels: a block, which a struct or a variant with saved fields and a list with elements
    /// are, opens at the line's end, its parts on their own lines a level deeper, and closes on a
    /// line of its own, so that each part has lines of its own.
    fn value(&mut self, ty: &Type, value: &Value, indent: usize, depth: u32) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(Error::new(
                "scene.too_deep",
                format!("it's more than {MAX_DEPTH} levels deep, the most a scene nests"),
            ));
        }
        let types = self.types;
        let out = &mut self.out;
        let text = "a string takes any text";
        match (ty, value) {
            (_, Value::Bool(b)) => write!(out, "{b}").expect(text),
            (_, Value::Int(n)) => write!(out, "{n}").expect(text),
            (_, Value::UInt(n)) => write!(out, "{n}").expect(text),
            (_, &Value::F32(x)) => float(out, x, x.is_nan(), x.is_sign_negative(), x.is_finite()),
            (_, &Value::F64(x)) => float(out, x, x.is_nan(), x.is_sign_negative(), x.is_finite()),
            (_, Value::String(string)) => quote(out, string),
            (_, Value::Vec2(v)) => floats(out, v.as_ref()),
            (_, Value::Vec3(v)) => floats(out, v.as_ref()),
            (_, Value::Vec4(v)) => floats(out, v.as_ref()),
            (_, Value::DVec3(v)) => floats(out, v.as_ref()),
            (_, Value::Quat(q)) => floats(out, &q.to_array()),
            (_, &Value::Entity(entity)) => self.reference(entity)?,
            (_, &Value::Texture(texture)) => {
                let Some(name) = self.world.assets().name(texture) else {
                    return Err(Error::new(
                        "asset.unnamed",
                        "it's a texture that wasn't loaded from a file, so it has no name to save",
                    ));
                };
                quote(out, name);
            }
            (_, &Value::Model(model)) => quote(out, self.world.assets().model_name(model)),
            (_, &Value::Font(font)) => quote(out, self.world.assets().font_name(font)),
            (_, &Value::Prefab(prefab)) => quote(out, self.world.assets().prefab_name(prefab)),
            (Type::Option(_), Value::Option(None)) => out.push_str("None"),
            (Type::Option(inner), Value::Option(Some(value))) => {
                if **value == Value::Opaque {
                    return Err(opaque_part());
                }
                out.push_str("Some(");
                self.value(inner, value, indent, depth + 1)?;
                self.out.push(')');
            }
            (Type::List(_) | Type::Array(..), Value::List(items)) if items.is_empty() => {
                out.push_str("[]");
            }
            (Type::List(inner) | Type::Array(inner, _), Value::List(items)) => {
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    let at = |error| reflect::within(error, &format!("[{i}]"));
                    if *item == Value::Opaque {
                        return Err(at(opaque_part()));
                    }
                    pad(&mut self.out, indent + 1);
                    self.value(inner, item, indent + 1, depth + 1).map_err(at)?;
                    self.out.push_str(",\n");
                }
                pad(&mut self.out, indent);
                self.out.push(']');
            }
            (&Type::Named(index), Value::Struct(given)) => {
                let Shape::Struct { fields, tuple } = &types.info(index).shape else {
                    return Err(mismatch(value));
                };
                let parts = self.saved_parts(fields, given)?;
                if parts.is_empty() {
                    self.out.push_str("()");
                } else {
                    self.block(&parts, *tuple, indent, depth)?;
                }
            }
            (&Type::Named(index), Value::Variant(name, given)) => {
                let Shape::Enum(variants) = &types.info(index).shape else {
                    return Err(mismatch(value));
                };
                let Some(variant) = variants.iter().find(|variant| variant.name == *name) else {
                    let names: Vec<&str> = variants.iter().map(|variant| &*variant.name).collect();
                    return Err(reflect::no_variant(name, &names));
                };
                writable(name)?;
                let parts = self.saved_parts(&variant.fields, given)?;
                self.out.push_str(name);
                if !parts.is_empty() {
                    self.block(&parts, variant.tuple, indent, depth)?;
                }
            }
            _ => return Err(mismatch(value)),
        }
        Ok(())
    }

    /// The fields of `given` that a scene saves, those of `fields` that aren't opaque, in their
    /// order.
    fn saved_parts<'v>(
        &self,
        fields: &'v [Field],
        given: &'v [(reflect::Ident, Value)],
    ) -> Result<Vec<(&'v Field, &'v Value)>> {
        let saved = fields
            .iter()
            .filter(|field| !field.ty.is_opaque(self.types));
        let parts = saved.map(|field| {
            let found = given.iter().find(|(name, _)| *name == field.name);
            let missing = || {
                Error::new(
                    "reflect.missing_field",
                    format!("field `{}` isn't given, but each field is", field.name),
                )
            };
            found.map(|(_, value)| (field, value)).ok_or_else(missing)
        });
        parts.collect()
    }

    /// Writes `parts`, at least one, of a struct or a variant, a tuple's by position: one part of a
    /// tuple around it, keeping its own form, as `(v)`; more as a block.
    fn block(
        &mut self,
        parts: &[(&Field, &Value)],
        tuple: bool,
        indent: usize,
        depth: u32,
    ) -> Result<()> {
        if let [(field, value)] = parts
            && tuple
        {
            self.out.push('(');
            self.value(&field.ty, value, indent, depth + 1)
                .map_err(|error| reflect::within(error, &field.name))?;
            self.out.push(')');
            return Ok(());
        }
        self.out.push_str("(\n");
        for (field, value) in parts {
            let at = |error| reflect::within(error, &field.name);
            pad(&mut self.out, indent + 1);
            if !tuple {
                writable(&field.name).map_err(at)?;
                self.out.push_str(&field.name);
                self.out.push_str(": ");
            }
            self.value(&field.ty, value, indent + 1, depth + 1)
                .map_err(at)?;
            self.out.push_str(",\n");
        }
        pad(&mut self.out, indent);
        self.out.push(')');
        Ok(())
    }

    /// Writes a reference to `entity` as its ID, which it must be saved for.
    fn reference(&mut self, entity: Entity) -> Result<()> {
        let Some(id) = self.world.id(entity) else {
            return Err(Error::new(
                "ecs.no_entity",
                format!(
                    "it refers to entity {entity}, which isn't in the world: it was despawned, or \
                     is another world's"
                ),
            ));
        };
        if !self.is_saved(entity) {
            return Err(Error::new(
                "scene.outside",
                format!(
                    "it refers to entity {id}, which isn't saved with it: a scene's entities refer \
                     only to each other"
                ),
            ));
        }
        write!(self.out, "\"{id}\"").expect("a string takes any text");
        Ok(())
    }
}

/// What a scene writes of a component: all of it, or the fields of a struct of named fields that
/// differ from its prefab's, by their names.
enum Written {
    Whole,
    Fields(Vec<reflect::Ident>),
}

/// Indents a line `indent` levels, of 4 spaces each.
fn pad(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push_str("    ");
    }
}

/// Writes a float as Rust's `Display` does, the shortest text that reads back to the same bits and
/// never an exponent, with `.0` if it has no point; `inf` and `-inf`; and `NaN` and `-NaN`, which
/// keep a NaN's sign but not its payload.
pub(crate) fn float(
    out: &mut String,
    x: impl fmt::Display,
    nan: bool,
    negative: bool,
    finite: bool,
) {
    if nan {
        out.push_str(if negative { "-NaN" } else { "NaN" });
        return;
    }
    let start = out.len();
    write!(out, "{x}").expect("a string takes any text");
    if finite && !out[start..].contains('.') {
        out.push_str(".0");
    }
}

/// Writes a vector's or a quaternion's components, as `(x, y, z)`.
fn floats<F: Float>(out: &mut String, xs: &[F]) {
    out.push('(');
    for (i, &x) in xs.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        float(out, x, x.is_nan(), x.is_sign_negative(), x.is_finite());
    }
    out.push(')');
}

/// An `f32` or an `f64`, as a vector's components are.
trait Float: Copy + fmt::Display {
    fn is_nan(self) -> bool;
    fn is_sign_negative(self) -> bool;
    fn is_finite(self) -> bool;
}

macro_rules! floats {
    ($($F:ty),*) => {$(
        impl Float for $F {
            fn is_nan(self) -> bool {
                <$F>::is_nan(self)
            }

            fn is_sign_negative(self) -> bool {
                <$F>::is_sign_negative(self)
            }

            fn is_finite(self) -> bool {
                <$F>::is_finite(self)
            }
        }
    )*};
}

floats!(f32, f64);

/// Writes `text` quoted, by the scene's own escapes: `\\`, `\"`, `\n`, `\r` and `\t`; `\u{..}` for
/// the other characters that are [`hidden`]; and everything else as it is.
pub(crate) fn quote(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if hidden(c) => {
                write!(out, "\\u{{{:x}}}", u32::from(c)).expect("a string takes any text")
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Whether `c` draws nothing of its own, or changes how the text around it reads, so that a scene
/// writes it as an escape, where it shows: a control (Unicode's category Cc); a format character
/// (Cf), such as a bidirectional control, a joiner or a tag; the line and paragraph separators (Zl
/// and Zp); or a default-ignorable code point, which renderers may drop, such as a variation
/// selector, a filler or a reserved code point among them. By Unicode 18.0's tables, kept here
/// rather than read from a crate's, so that the bytes depend on no library's Unicode: a character
/// a later Unicode adds to them stays bare until the arms below are updated, which changes the
/// bytes of the strings that hold it.
fn hidden(c: char) -> bool {
    match c {
        // The C0 and C1 controls, and DEL.
        '\0'..='\u{1f}' | '\u{7f}'..='\u{9f}' => true,
        // The soft hyphen and the combining grapheme joiner.
        '\u{ad}' | '\u{34f}' => true,
        // Arabic's and Syriac's format characters: number signs, the letter mark, ends of ayah
        // and the abbreviation mark.
        '\u{600}'..='\u{605}'
        | '\u{61c}'
        | '\u{6dd}'
        | '\u{70f}'
        | '\u{890}'..='\u{891}'
        | '\u{8e2}' => true,
        // Hangul's fillers.
        '\u{115f}'..='\u{1160}' | '\u{3164}' | '\u{ffa0}' => true,
        // Khmer's inherent vowels, and Mongolian's variation selectors and vowel separator.
        '\u{17b4}'..='\u{17b5}' | '\u{180b}'..='\u{180f}' => true,
        // The zero-width characters, bidirectional controls, line and paragraph separators, word
        // joiner, invisible operators and deprecated format characters.
        '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}' => true,
        // Variation selectors, the byte order mark, and the reserved and interlinear annotation
        // characters among the specials.
        '\u{fe00}'..='\u{fe0f}' | '\u{feff}' | '\u{fff0}'..='\u{fffb}' => true,
        // Kaithi's number signs, and the Egyptian hieroglyph, shorthand and musical format
        // characters.
        '\u{110bd}'
        | '\u{110cd}'
        | '\u{13430}'..='\u{1343f}'
        | '\u{1bca0}'..='\u{1bca3}'
        | '\u{1d173}'..='\u{1d17a}' => true,
        // Tags, variation selectors 17 to 256, and the reserved code points among them.
        '\u{e0000}'..='\u{e0fff}' => true,
        _ => false,
    }
}

/// Refuses a name that a scene can't write: one that isn't an ASCII identifier, or is one of RON's
/// words.
fn writable(name: &str) -> Result<()> {
    let mut bytes = name.bytes();
    let identifier = bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if identifier && !WORDS.contains(&name) {
        return Ok(());
    }
    Err(Error::new(
        "scene.bad_name",
        format!(
            "`{name}` isn't a name a scene writes: those are ASCII identifiers, and none of RON's \
             words, `true`, `false`, `Some`, `None`, `inf` and `NaN`"
        ),
    ))
}

/// The error that something opaque is where a scene can't leave it out.
fn opaque_part() -> Error {
    Error::new(
        "reflect.opaque",
        "it's opaque, which a scene leaves out, but can't in a list or an option",
    )
}

/// The error that `value` isn't of its type, as a hand-written description's might not be.
fn mismatch(value: &Value) -> Error {
    Error::new(
        "reflect.wrong_kind",
        format!("it's {}, which isn't of its type", reflect::kind(value)),
    )
}
