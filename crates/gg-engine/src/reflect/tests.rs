use super::*;
use crate::{Component, EntityId, MeshDraw, Name, Sampler, Sprite, Transform, World};

/// A struct a field holds.
#[derive(Clone, Debug, PartialEq, Reflect)]
pub(crate) struct Inner {
    #[reflect(min = -1, max = 1)]
    pub(crate) level: i8,
    pub(crate) name: String,
}

/// Kept, but not reflected.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Handle(pub(crate) u32);

#[derive(Clone, Debug, PartialEq, Reflect)]
pub(crate) enum Form {
    Point,
    Circle(#[reflect(min = 0.0)] f32),
    Box {
        size: Vec2,
        #[reflect(degrees)]
        turn: f32,
    },
    Held(#[reflect(opaque)] Handle),
}

/** Two numbers,
 * in a block comment, whose stars aren't its text. */
#[derive(Clone, Debug, PartialEq, Reflect)]
pub(crate) struct Pair(pub(crate) u16, pub(crate) i64);

#[derive(Clone, Debug, PartialEq, Reflect)]
pub(crate) struct Unit;

/// A field of everything reflection describes.
///
/// Its doc is two paragraphs.
#[derive(Clone, Debug, PartialEq, Reflect)]
pub(crate) struct Everything {
    /// Whether it's on.
    pub(crate) on: bool,
    #[reflect(min = 0, max = 100)]
    pub(crate) count: u8,
    pub(crate) byte: u8,
    #[reflect(min = 0.0)]
    pub(crate) weight: f64,
    #[reflect(degrees, min = -3.2, max = 3.2)]
    pub(crate) angle: f32,
    #[reflect(color)]
    pub(crate) tint: Vec4,
    pub(crate) offset: Vec3,
    pub(crate) turn: Quat,
    pub(crate) target: Option<Entity>,
    pub(crate) inner: Inner,
    pub(crate) form: Form,
    pub(crate) maybe: Option<Inner>,
    #[reflect(max = 10)]
    pub(crate) list: Vec<u32>,
    pub(crate) rect: [Vec2; 2],
    pub(crate) pair: Pair,
    pub(crate) unit: Unit,
    #[reflect(opaque)]
    pub(crate) handle: Handle,
}

/// Built on [`everything`], which gives it its handle, as a component with an opaque field is.
impl Component for Everything {
    fn base() -> Option<Self> {
        Some(everything())
    }
}

impl crate::Writable for Everything {}

pub(crate) fn everything() -> Everything {
    Everything {
        on: true,
        count: 42,
        byte: 7,
        weight: 2.5,
        angle: 0.5,
        // Transparent, so that its color is lost if alpha is multiplied in.
        tint: Vec4::new(0.3, 0.6, 0.9, 0.0),
        offset: Vec3::new(1.0, -0.0, 3.0),
        turn: Quat::from_rotation_x(0.25),
        target: None,
        inner: Inner {
            level: -1,
            name: "first".to_owned(),
        },
        form: Form::Point,
        maybe: None,
        list: vec![1, 2],
        rect: [Vec2::ZERO, Vec2::ONE],
        pair: Pair(3, -4),
        unit: Unit,
        handle: Handle(7),
    }
}

/// Whether `a` and `b` are the same, their numbers compared by their bits.
pub(crate) fn same(a: &Value, b: &Value) -> bool {
    let floats = |a: &[f32], b: &[f32]| {
        a.iter()
            .map(|x| x.to_bits())
            .eq(b.iter().map(|x| x.to_bits()))
    };
    let fields = |a: &[(Ident, Value)], b: &[(Ident, Value)]| {
        a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.0 == b.0 && same(&a.1, &b.1))
    };
    match (a, b) {
        (Value::F32(a), Value::F32(b)) => a.to_bits() == b.to_bits(),
        (Value::F64(a), Value::F64(b)) => a.to_bits() == b.to_bits(),
        (Value::Vec2(a), Value::Vec2(b)) => floats(a.as_ref(), b.as_ref()),
        (Value::Vec3(a), Value::Vec3(b)) => floats(a.as_ref(), b.as_ref()),
        (Value::Vec4(a), Value::Vec4(b)) => floats(a.as_ref(), b.as_ref()),
        (Value::Quat(a), Value::Quat(b)) => floats(&a.to_array(), &b.to_array()),
        (Value::Option(Some(a)), Value::Option(Some(b))) => same(a, b),
        (Value::List(a), Value::List(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
        }
        (Value::Struct(a), Value::Struct(b)) => fields(a, b),
        (Value::Variant(m, a), Value::Variant(n, b)) => m == n && fields(a, b),
        _ => a == b,
    }
}

/// Every component of every entity of `world`, read by name, beside the entity's handle and ID.
pub(crate) fn snapshot(world: &World) -> Vec<(Entity, EntityId, String, Value)> {
    let mut all = Vec::new();
    for entity in world.entities(None).expect("every entity") {
        let id = world.id(entity).expect("alive");
        for component in world.components_of(entity).expect("alive") {
            let value = world.get_field(entity, component, "").expect("readable");
            all.push((entity, id, component.to_owned(), value));
        }
    }
    all
}

pub(crate) fn same_snapshots(
    a: &[(Entity, EntityId, String, Value)],
    b: &[(Entity, EntityId, String, Value)],
) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| a.0 == b.0 && a.1 == b.1 && a.2 == b.2 && same(&a.3, &b.3))
}

fn field<'a>(types: &'a Types, ty: &Type, name: &str) -> &'a Field {
    let Type::Named(index) = ty else {
        panic!("{ty:?} isn't named");
    };
    let Shape::Struct { fields, .. } = &types.info(*index).shape else {
        panic!("{ty:?} isn't a struct");
    };
    fields
        .iter()
        .find(|field| field.name == name)
        .unwrap_or_else(|| panic!("no field {name}"))
}

#[test]
fn types_describe_their_fields_once_each() {
    let mut world = World::default();
    for registered in [
        world.register::<Transform>(),
        world.register::<Sprite>(),
        world.register::<MeshDraw>(),
        world.register::<Everything>(),
        world.register::<Everything>(),
    ] {
        registered.expect("described");
    }
    let types = world.types();
    // Each as it's first reached, the types of a type's fields after it.
    let names: Vec<&str> = types.iter().map(|(_, info)| &*info.name).collect();
    let expected = [
        "Parent",
        "Transform",
        "Sprite",
        "Sampler",
        "MeshDraw",
        "Everything",
        "Inner",
        "Form",
        "Pair",
        "Unit",
    ];
    assert_eq!(names, expected);
    let named = |name| Type::Named(types.find(name).expect(name));

    let transform = named("Transform");
    let ty = |name| field(types, &transform, name).ty.clone();
    assert_eq!(
        [ty("translation"), ty("rotation"), ty("scale")],
        [Type::Vec3, Type::Quat, Type::Vec3]
    );
    assert_eq!(
        field(types, &transform, "rotation").meta.doc,
        "As a unit quaternion."
    );
    let doc = |name| &types.info(types.find(name).expect(name)).doc;
    assert!(doc("Transform").starts_with("Where an entity is"));
    assert_eq!(
        doc("Everything"),
        "A field of everything reflection describes.\n\nIts doc is two paragraphs."
    );
    assert_eq!(
        doc("Pair"),
        "Two numbers, in a block comment, whose stars aren't its text."
    );

    let sprite = named("Sprite");
    let color = field(types, &sprite, "color");
    assert_eq!((&color.ty, color.meta.show), (&Type::Vec4, Show::Color));
    assert!(field(types, &sprite, "texture").ty.is_opaque(types));
    let rect = &field(types, &sprite, "rect").ty;
    assert_eq!(*rect, Type::Array(Box::new(Type::Vec2), 2));
    assert_eq!(field(types, &sprite, "sampler").ty, named("Sampler"));
    let Shape::Enum(samplers) = &types.info(types.find("Sampler").expect("described")).shape else {
        panic!("a sampler is an enum");
    };
    let samplers: Vec<&str> = samplers.iter().map(|variant| &*variant.name).collect();
    assert_eq!(samplers.len(), 6);
    assert!(samplers.contains(&"NearestRepeat"));
    assert!(named("MeshDraw").is_opaque(types));

    let everything = named("Everything");
    let meta = |name| field(types, &everything, name).meta.clone();
    let ty = |name| field(types, &everything, name).ty.clone();
    let u8 = Type::Int {
        bits: 8,
        signed: false,
    };
    assert_eq!(meta("on").doc, "Whether it's on.");
    assert_eq!(
        (ty("count"), meta("count").min, meta("count").max),
        (u8, Some(Bound::Int(0)), Some(Bound::Int(100)))
    );
    assert_eq!(
        (meta("weight").min, meta("weight").max),
        (Some(Bound::Float(0.0)), None)
    );
    assert_eq!(meta("angle").show, Show::Degrees);
    assert_eq!(meta("angle").min, Some(Bound::Float(-3.2)));
    assert_eq!(meta("tint").show, Show::Color);
    assert_eq!(ty("target"), Type::Option(Box::new(Type::Entity)));
    assert_eq!(ty("inner"), named("Inner"));
    assert_eq!(ty("maybe"), Type::Option(Box::new(named("Inner"))));
    let u32 = Type::Int {
        bits: 32,
        signed: false,
    };
    assert_eq!(
        (ty("list"), meta("list").max),
        (Type::List(Box::new(u32)), Some(Bound::Int(10)))
    );
    assert_eq!(ty("handle"), Type::Opaque);
    let level = field(types, &named("Inner"), "level");
    assert_eq!(level.meta.min, Some(Bound::Int(-1)));
    let Shape::Struct { fields, tuple } = &types.info(types.find("Pair").expect("described")).shape
    else {
        panic!("a pair is a struct");
    };
    let names: Vec<&str> = fields.iter().map(|field| &*field.name).collect();
    assert_eq!((names, *tuple), (vec!["0", "1"], true));
    let unit = &types.info(types.find("Unit").expect("described")).shape;
    assert_eq!(
        *unit,
        Shape::Struct {
            fields: Vec::new(),
            tuple: false
        }
    );
    let Shape::Enum(forms) = &types.info(types.find("Form").expect("described")).shape else {
        panic!("a form is an enum");
    };
    let shapes: Vec<(&str, usize, bool)> = forms
        .iter()
        .map(|variant| (&*variant.name, variant.fields.len(), variant.tuple))
        .collect();
    assert_eq!(
        shapes,
        [
            ("Point", 0, false),
            ("Circle", 1, true),
            ("Box", 2, false),
            ("Held", 1, true)
        ]
    );
    assert_eq!(forms[2].fields[1].meta.show, Show::Degrees);
    assert_eq!(forms[3].fields[0].ty, Type::Opaque);
}

#[derive(Clone, Debug, PartialEq, Reflect)]
struct Node {
    kids: Vec<Node>,
}

#[test]
fn a_recursive_type_describes_itself_once() {
    let mut types = Types::default();
    let ty = Node::describe(&mut types).expect("described");
    assert_eq!(types.iter().len(), 1);
    let kids = field(&types, &ty, "kids");
    assert_eq!(kids.ty, Type::List(Box::new(ty.clone())));
    let tree = Node {
        kids: vec![
            Node { kids: Vec::new() },
            Node {
                kids: vec![Node { kids: Vec::new() }],
            },
        ],
    };
    assert_eq!(Node::from_value(&tree.to_value()).expect("a tree"), tree);
}

/// Derived beside consts named as what the derives' code binds, which would turn those
/// bindings into patterns of the consts, which don't compile.
mod beside_lowercase_consts {
    #![allow(
        dead_code,
        non_upper_case_globals,
        reason = "unused, and named as the derives' bindings"
    )]

    use crate::reflect::{Reflect, Types};

    const value: u8 = 0;
    const index: u8 = 0;
    const types: u8 = 0;
    const to: u8 = 0;
    const name: u8 = 0;
    const given: u8 = 0;
    const error: u8 = 0;
    const default: u8 = 0;

    /// A field with a default, an enum's and an opaque type's.
    #[derive(Clone, Debug, PartialEq, crate::Component)]
    struct Held {
        #[reflect(default = 3)]
        count: u8,
        mode: Mode,
        sealed: Sealed,
    }

    #[derive(Clone, Debug, PartialEq, Reflect)]
    enum Mode {
        Idle,
        Wait { ticks: u8 },
    }

    #[derive(Clone, Debug, PartialEq, Reflect)]
    #[reflect(opaque)]
    struct Sealed(u8);

    #[test]
    fn the_derives_work_beside_them() {
        let mut described = Types::default();
        Held::describe(&mut described).expect("described");
        let mut held = Held {
            count: 1,
            mode: Mode::Idle,
            sealed: Sealed(2),
        };
        let set = Held {
            count: 4,
            mode: Mode::Wait { ticks: 5 },
            sealed: Sealed(6),
        };
        held.set_value(&set.to_value()).expect("set");
        let kept = Held {
            sealed: Sealed(2),
            ..set
        };
        assert_eq!(held, kept, "what's opaque kept");
    }
}

mod other {
    /// Named as the outer module's `Inner` is.
    #[derive(Clone, super::Reflect)]
    pub(super) struct Inner {
        pub(super) x: f32,
    }
}

/// Its second field's type takes a name that's taken, after its first's is described.
#[derive(Clone, Reflect)]
struct Holder {
    pair: Pair,
    inner: other::Inner,
}

#[test]
fn a_type_that_fails_to_describe_itself_leaves_the_types_as_they_were() {
    let mut types = Types::default();
    Inner::describe(&mut types).expect("described");
    for _ in 0..2 {
        let error = Holder::describe(&mut types).expect_err("a name taken");
        assert_eq!(error.code(), "reflect.name_taken");
        let message = error.to_string();
        assert!(
            message.contains("reflect::tests::Inner")
                && message.contains("reflect::tests::other::Inner")
                && message.contains("#[reflect(name"),
            "{message}"
        );
        let names: Vec<&str> = types.iter().map(|(_, info)| &*info.name).collect();
        assert_eq!(names, ["Inner"]);
        assert_eq!(types.find("Holder").or(types.find("Pair")), None);
    }
    // Undone by its type too, so that it describes itself afresh.
    let pair = Pair::describe(&mut types).expect("described");
    assert_eq!(pair, Type::Named(TypeIndex(1)));
    let Shape::Struct { fields, .. } = &types.info(TypeIndex(1)).shape else {
        panic!("a pair is a struct");
    };
    assert_eq!(fields.len(), 2);
}

#[test]
fn paths_lead_to_fields_and_their_hints() {
    let mut world = World::default();
    world.register::<Transform>().expect("described");
    world.register::<Sprite>().expect("described");
    world.register::<Everything>().expect("described");
    let types = world.types();
    let resolve = |root: &str, path| {
        let root = Type::Named(types.find(root).expect(root));
        resolve(types, &root, path)
    };
    let at = |root, path| {
        let to = resolve(root, path).unwrap_or_else(|error| panic!("{path}: {error}"));
        (to.steps, to.ty)
    };
    let everything = |name| {
        let Shape::Struct { fields, .. } = &types
            .info(types.find("Everything").expect("described"))
            .shape
        else {
            unreachable!()
        };
        fields
            .iter()
            .position(|field| field.name == name)
            .expect(name) as u32
    };
    assert_eq!(
        at("Transform", ""),
        (
            vec![],
            Type::Named(types.find("Transform").expect("described"))
        )
    );
    assert_eq!(at("Transform", "translation.x"), (vec![0, 0], Type::F32));
    assert_eq!(at("Transform", "scale.z"), (vec![2, 2], Type::F32));
    assert_eq!(at("Sprite", "rect[1].y"), (vec![2, 1, 1], Type::F32));
    let u16 = Type::Int {
        bits: 16,
        signed: false,
    };
    assert_eq!(
        at("Everything", "pair.0"),
        (vec![everything("pair"), 0], u16)
    );
    assert_eq!(
        at("Everything", "turn"),
        (vec![everything("turn")], Type::Quat)
    );
    // Elements and components keep their field's bounds.
    let list = resolve("Everything", "list[7]").expect("an element");
    assert_eq!(list.steps, [everything("list"), 7]);
    assert_eq!(list.meta.max, Some(Bound::Int(10)));
    assert_eq!(
        resolve("Everything", "angle").expect("a field").meta.show,
        Show::Degrees
    );
    assert_eq!(
        resolve("Everything", "inner.level")
            .expect("a field")
            .meta
            .min,
        Some(Bound::Int(-1))
    );
    assert!(
        resolve("Sprite", "texture")
            .expect("a field")
            .ty
            .is_opaque(types)
    );

    for (root, path, says) in [
        (
            "Transform",
            "translation.q",
            "whose components are `x`, `y` and `z`",
        ),
        ("Transform", "rotation.x", "a quaternion, set whole"),
        ("Sprite", "sampler.x", "an enum, Sampler, set whole"),
        ("Sprite", "rect[2]", "whose elements are `[0]` to `[1]`"),
        ("Sprite", "rect.x", "which has no `x`"),
        (
            "Transform",
            "spin",
            "its fields are `translation`, `rotation` and `scale`",
        ),
        (
            "Everything",
            "maybe.level",
            "an option of a struct, Inner, set whole",
        ),
        ("Everything", "form.size", "an enum, Form, set whole"),
        ("Everything", "on.x", "a bool, which has no `x`"),
        ("Everything", "[0]", "which has no `[0]`"),
    ] {
        let error = resolve(root, path).expect_err(path);
        assert_eq!(error.code(), "reflect.no_field", "{path}: {error}");
        assert!(error.to_string().contains(says), "{path}: {error}");
    }
    for path in [
        "rect[",
        "rect[]",
        "rect[x]",
        "rect[-1]",
        "rect[+1]",
        "rect[4294967296]",
        "rect..x",
        "rect.",
        "rect[1]y",
        " rect",
        "re-ct",
    ] {
        let error = resolve("Sprite", path).expect_err(path);
        assert_eq!(error.code(), "reflect.bad_path", "{path}: {error}");
        assert!(error.to_string().contains(&format!("`{path}`")), "{error}");
    }

    // A field's name is any identifier's.
    #[derive(Clone, Reflect)]
    struct Measure {
        größe: f32,
    }
    let mut types = Types::default();
    let measure = Measure::describe(&mut types).expect("described");
    let to = super::resolve(&types, &measure, "größe").expect("a field");
    assert_eq!((to.steps, to.ty), (vec![0], Type::F32));
}

/// `value` read and made again, which must give back the same value, bit for bit.
fn round_trip<T: Reflect>(value: T) {
    let read = value.to_value();
    let made = T::from_value(&read).unwrap_or_else(|error| panic!("{read:?}: {error}"));
    assert!(same(&made.to_value(), &read), "{read:?}");
}

#[test]
fn values_come_back_bit_for_bit() {
    round_trip(true);
    round_trip(i8::MIN);
    round_trip(i16::MAX);
    round_trip(-5i32);
    round_trip(i64::MIN);
    round_trip(u8::MAX);
    round_trip(u16::MAX);
    round_trip(7u32);
    round_trip(u64::MAX);
    round_trip(-0.0f32);
    round_trip(f32::from_bits(0x7fc0_1234));
    round_trip(f64::from_bits(0xfff8_0000_0000_4321));
    round_trip("text".to_owned());
    round_trip(Vec2::new(1.0, -0.0));
    round_trip(Vec3::new(f32::INFINITY, 2.0, 3.0));
    round_trip(Vec4::new(1.0, 2.0, 3.0, f32::from_bits(0x7fc0_0001)));
    round_trip(Quat::from_rotation_y(1.0));
    round_trip(World::default().spawn(()));
    round_trip(Some(3u8));
    round_trip(None::<u8>);
    round_trip(vec![1u32, 2, 3]);
    round_trip(Vec::<String>::new());
    round_trip([Vec2::X, Vec2::Y]);
    round_trip(Inner {
        level: 1,
        name: "x".to_owned(),
    });
    round_trip(Form::Point);
    round_trip(Form::Circle(f32::from_bits(0xffc0_0002)));
    round_trip(Form::Box {
        size: Vec2::ONE,
        turn: -0.0,
    });
    round_trip(Pair(1, -1));
    round_trip(Unit);
    round_trip(Transform {
        translation: Vec3::new(1.0, -0.0, 2.0),
        rotation: Quat::from_rotation_z(0.5),
        scale: Vec3::splat(2.0),
    });
    for name in Sampler::ALL {
        round_trip(name);
    }
    // An integer of either sign sets one of either, if it fits.
    assert_eq!(u8::from_value(&Value::Int(5)).expect("fits"), 5);
    assert_eq!(i8::from_value(&Value::UInt(5)).expect("fits"), 5);
    // What's opaque comes back from what it was.
    let all = everything();
    let kept = all.with_value(&all.to_value()).expect("its own value");
    assert!(same(&kept.to_value(), &all.to_value()));
    assert_eq!(kept.handle, all.handle);
    let error = Everything::from_value(&all.to_value()).expect_err("an opaque field");
    assert_eq!(error.code(), "reflect.opaque");
    assert!(error.to_string().starts_with("at `handle`: "), "{error}");
}

#[derive(Clone, Debug, PartialEq, Reflect)]
pub(crate) struct WithOpaque {
    #[reflect(opaque)]
    pub(crate) handle: Handle,
}

#[derive(Clone, Debug, PartialEq, Reflect)]
struct Partly {
    a: f32,
    b: Option<WithOpaque>,
}

#[test]
fn a_value_refused_partway_changes_nothing() {
    let mut partly = Partly { a: 1.0, b: None };
    let inner = Value::Struct(vec![("handle".into(), Value::Opaque)]);
    let value = Value::Struct(vec![
        ("a".into(), Value::F32(2.0)),
        ("b".into(), Value::Option(Some(Box::new(inner)))),
    ]);
    let error = partly.set_value(&value).expect_err("an opaque field made");
    assert_eq!(error.code(), "reflect.opaque");
    assert!(error.to_string().starts_with("at `b.handle`: "), "{error}");
    assert_eq!(partly, Partly { a: 1.0, b: None });
    // Kept, it's set.
    let mut partly = Partly {
        a: 1.0,
        b: Some(WithOpaque { handle: Handle(3) }),
    };
    partly.set_value(&value).expect("the opaque field kept");
    assert_eq!(partly.a, 2.0);
    assert_eq!(partly.b, Some(WithOpaque { handle: Handle(3) }));
}

#[test]
fn bounds_apply_to_numbers_components_and_elements() {
    let meta = |min, max| Meta {
        min,
        max,
        ..Meta::default()
    };
    let types = Types::default();
    let int = Type::Int {
        bits: 64,
        signed: true,
    };
    let check = |ty: &Type, meta: &Meta, value: Value| check(&types, ty, meta, &value);
    let within = meta(Some(Bound::Int(-2)), Some(Bound::Int(2)));
    // Integers compare exactly, past what an f64 holds.
    let huge = meta(None, Some(Bound::Int(i128::from(i64::MAX) - 1)));
    assert!(check(&int, &huge, Value::Int(i64::MAX - 1)).is_ok());
    assert!(check(&int, &huge, Value::Int(i64::MAX)).is_err());
    assert!(check(&int, &within, Value::Int(-2)).is_ok());
    for (ty, value, at) in [
        (int.clone(), Value::Int(3), ""),
        (int.clone(), Value::Int(-3), ""),
        (Type::F32, Value::F32(f32::NAN), ""),
        (Type::F32, Value::F32(f32::INFINITY), ""),
        (Type::F64, Value::F64(2.5), ""),
        (
            Type::Vec3,
            Value::Vec3(Vec3::new(0.0, 0.0, 9.0)),
            "at `z`: ",
        ),
        (
            Type::Option(Box::new(Type::F32)),
            Value::Option(Some(Box::new(Value::F32(5.0)))),
            "",
        ),
        (
            Type::List(Box::new(int.clone())),
            Value::List(vec![Value::Int(0), Value::Int(5)]),
            "at `[1]`: ",
        ),
    ] {
        let error = check(&ty, &within, value.clone()).expect_err("out of range");
        assert_eq!(error.code(), "reflect.out_of_range", "{value:?}");
        assert!(error.to_string().starts_with(at), "{error}");
    }
    // An f32 compares with its bounds rounded to an f32: 3.2f32, a little more than 3.2, is
    // the f32 that `max = 3.2` means. An f64 compares with them as they are.
    let decimal = meta(Some(Bound::Float(-3.2)), Some(Bound::Float(3.2)));
    assert!(check(&Type::F32, &decimal, Value::F32(3.2)).is_ok());
    assert!(check(&Type::F32, &decimal, Value::F32(-3.2)).is_ok());
    let corner = Value::Vec2(Vec2::new(3.2, -3.2));
    assert!(check(&Type::Vec2, &decimal, corner).is_ok());
    assert!(check(&Type::F32, &decimal, Value::F32(3.2f32.next_up())).is_err());
    assert!(check(&Type::F64, &decimal, Value::F64(3.2f32.into())).is_err());
    // Without bounds, any number is in range.
    assert!(check(&Type::F32, &Meta::default(), Value::F32(f32::NAN)).is_ok());
    let half = meta(Some(Bound::Float(0.5)), None);
    assert!(check(&int, &half, Value::Int(0)).is_err());
    assert!(check(&int, &half, Value::Int(1)).is_ok());
}

#[test]
fn a_type_defaults_to_zero_or_the_bound_nearest_it() {
    let types = Types::default();
    let meta = |min, max| Meta {
        min,
        max,
        ..Meta::default()
    };
    let none = Meta::default;
    let int = |bits, signed| Type::Int { bits, signed };
    let entities = |len| Type::Array(Box::new(Type::Entity), len);
    for (ty, meta, expected) in [
        (Type::Bool, none(), Value::Bool(false)),
        (int(8, true), none(), Value::Int(0)),
        (int(8, false), none(), Value::UInt(0)),
        (
            int(32, true),
            meta(Some(Bound::Int(3)), None),
            Value::Int(3),
        ),
        (
            int(32, true),
            meta(None, Some(Bound::Int(-3))),
            Value::Int(-3),
        ),
        // Rounded to an integer inside them.
        (
            int(8, false),
            meta(Some(Bound::Float(0.5)), None),
            Value::UInt(1),
        ),
        (
            int(8, true),
            meta(None, Some(Bound::Float(-0.5))),
            Value::Int(-1),
        ),
        (
            int(16, true),
            meta(Some(Bound::Int(-5)), Some(Bound::Float(5.5))),
            Value::Int(0),
        ),
        (
            int(64, false),
            meta(Some(Bound::Int(u64::MAX.into())), None),
            Value::UInt(u64::MAX),
        ),
        (
            int(64, true),
            meta(None, Some(Bound::Int(i64::MIN.into()))),
            Value::Int(i64::MIN),
        ),
        (Type::F32, none(), Value::F32(0.0)),
        // As `check` compares with it: rounded to an f32.
        (
            Type::F32,
            meta(Some(Bound::Float(0.1)), None),
            Value::F32(0.1),
        ),
        (
            Type::F64,
            meta(Some(Bound::Float(0.1)), None),
            Value::F64(0.1),
        ),
        (
            Type::F64,
            meta(None, Some(Bound::Int(-2))),
            Value::F64(-2.0),
        ),
        (
            Type::F32,
            meta(Some(Bound::Int(-1)), Some(Bound::Int(1))),
            Value::F32(0.0),
        ),
        (Type::Vec2, none(), Value::Vec2(Vec2::ZERO)),
        (
            Type::Vec3,
            meta(Some(Bound::Int(1)), None),
            Value::Vec3(Vec3::ONE),
        ),
        (
            Type::Vec4,
            meta(None, Some(Bound::Float(-0.5))),
            Value::Vec4(Vec4::splat(-0.5)),
        ),
        (Type::Quat, none(), Value::Quat(Quat::IDENTITY)),
        (Type::String, none(), Value::String(String::new())),
        (
            Type::Option(Box::new(Type::Entity)),
            none(),
            Value::Option(None),
        ),
        (
            Type::List(Box::new(Type::Entity)),
            none(),
            Value::List(Vec::new()),
        ),
        (entities(0), none(), Value::List(Vec::new())),
        (
            Type::Array(Box::new(Type::F32), 3),
            meta(Some(Bound::Int(2)), None),
            Value::List(vec![Value::F32(2.0); 3]),
        ),
        (Type::Opaque, none(), Value::Opaque),
    ] {
        let made = types.default_value(&ty, &meta);
        let made = made.unwrap_or_else(|error| panic!("{ty:?}, {meta:?}: {error}"));
        assert!(same(&made, &expected), "{ty:?}, {meta:?}: {made:?}");
        let checked = check(&types, &ty, &meta, &made);
        assert!(checked.is_ok(), "{ty:?}, {meta:?}: {checked:?}");
    }
    for (ty, meta, says) in [
        (
            int(8, false),
            meta(Some(Bound::Int(300)), None),
            "a u8 has no value in the field's range, of 300 and up, to be its default",
        ),
        (int(8, false), meta(None, Some(Bound::Int(-1))), "up to -1"),
        (
            int(8, true),
            meta(Some(Bound::Int(-200)), Some(Bound::Int(-150))),
            "an i8 has no value in the field's range, from -200 to -150",
        ),
        (
            int(32, true),
            meta(Some(Bound::Float(0.5)), Some(Bound::Float(0.7))),
            "from 0.5 to 0.7",
        ),
        (
            int(64, true),
            meta(Some(Bound::Int(i128::from(i64::MAX) + 1)), None),
            "an i64",
        ),
        (
            Type::F32,
            meta(Some(Bound::Int(2)), Some(Bound::Int(1))),
            "an f32 has no value in the field's range, from 2 to 1",
        ),
        (
            Type::Vec2,
            meta(Some(Bound::Int(2)), Some(Bound::Int(1))),
            "a Vec2",
        ),
        // Beyond an f32's range, which rounds it to an infinite bound, outside every range.
        (
            Type::F32,
            meta(Some(Bound::Float(1e39)), None),
            "an f32 has no value in the field's range",
        ),
        (Type::Vec3, meta(None, Some(Bound::Float(-1e39))), "a Vec3"),
        (Type::Entity, none(), "is an `Option<Entity>`"),
        (entities(2), none(), "at `[0]`: an entity has no default"),
    ] {
        let error = types.default_value(&ty, &meta).expect_err(says);
        assert_eq!(error.code(), "reflect.no_default", "{error}");
        assert!(error.to_string().contains(says), "{error}");
    }
}

/// Lit by its hints' defaults, but for `spare`'s, which is its type's, as `shade`'s is.
#[derive(Clone, Debug, PartialEq, Reflect)]
struct Lamp {
    #[reflect(min = 0.0, default = 2.5)]
    intensity: f32,
    #[reflect(degrees, default = std::f32::consts::FRAC_PI_4)]
    cone: f32,
    shade: Shade,
    spare: Shade,
    watts: Watts,
}

#[derive(Clone, Debug, PartialEq, Reflect)]
struct Watts(#[reflect(min = 1, default = 60)] u16);

#[derive(Clone, Debug, PartialEq, Reflect)]
enum Shade {
    Ring {
        #[reflect(default = Vec2::ONE)]
        size: Vec2,
        #[reflect(min = 1)]
        rings: u8,
    },
    Dome(#[reflect(min = 0.5)] f32),
}

/// A struct's default is its fields', each the one its hint gives, or else its type's under
/// its bounds; an enum's is its first variant, with its fields' defaults; and a type held
/// twice is no cycle.
#[test]
fn defaults_are_the_hints_or_else_the_types() {
    let mut types = Types::default();
    let lamp = Lamp::describe(&mut types).expect("described");
    let ring = Value::Variant(
        "Ring".into(),
        vec![
            ("size".into(), Value::Vec2(Vec2::ONE)),
            ("rings".into(), Value::UInt(1)),
        ],
    );
    let expected = Value::Struct(vec![
        ("intensity".into(), Value::F32(2.5)),
        ("cone".into(), Value::F32(std::f32::consts::FRAC_PI_4)),
        ("shade".into(), ring.clone()),
        ("spare".into(), ring.clone()),
        (
            "watts".into(),
            Value::Struct(vec![("0".into(), Value::UInt(60))]),
        ),
    ]);
    let made = types
        .default_value(&lamp, &Meta::default())
        .expect("a default");
    assert!(same(&made, &expected), "{made:?}");
    let made = Lamp::from_value(&made).expect("made");
    assert_eq!(made.watts, Watts(60));
    // The hints are the fields' defaults, as their types' own values.
    let intensity = field(&types, &lamp, "intensity");
    assert_eq!(intensity.default, Some(Value::F32(2.5)));
    assert_eq!(field(&types, &lamp, "shade").default, None);
    let watts = Type::Named(types.find("Watts").expect("described"));
    assert_eq!(field(&types, &watts, "0").default, Some(Value::UInt(60)));
    let shade = types.find("Shade").expect("described");
    let Shape::Enum(variants) = &types.info(shade).shape else {
        panic!("a shade is an enum");
    };
    assert_eq!(variants[0].fields[0].default, Some(Value::Vec2(Vec2::ONE)));
    assert_eq!(
        types.variant_default(&variants[0]).expect("a default"),
        ring
    );
    let dome = Value::Variant("Dome".into(), vec![("0".into(), Value::F32(0.5))]);
    assert_eq!(
        types.variant_default(&variants[1]).expect("a default"),
        dome
    );
    // A field without one has its type's, under its own bounds.
    let rings = types.field_default(&variants[0].fields[1]);
    assert_eq!(rings.expect("a default"), Value::UInt(1));
}

/// What a description written by hand makes a struct that holds itself, without end.
enum Loop {}

/// What a description written by hand makes an enum without variants.
enum Never {}

#[test]
fn a_default_without_end_or_without_a_variant_is_refused() {
    let mut types = Types::default();
    let looped = types.add::<Loop>("Loop".into(), "".into(), |types| {
        let index = types.find("Loop").expect("reserved");
        let next = Field {
            name: "next".into(),
            ty: Type::Named(index),
            meta: Meta::default(),
            default: None,
        };
        Ok(Shape::Struct {
            fields: vec![next],
            tuple: false,
        })
    });
    let looped = looped.expect("described");
    let error = types.default_value(&looped, &Meta::default());
    let error = error.expect_err("without end");
    assert_eq!(error.code(), "reflect.no_default");
    assert_eq!(
        error.to_string(),
        "at `next`: `Loop`'s default holds a `Loop`, without end"
    );
    let never = types.add::<Never>("Never".into(), "".into(), |_| Ok(Shape::Enum(Vec::new())));
    let never = never.expect("described");
    let error = types.default_value(&never, &Meta::default());
    let error = error.expect_err("no variants");
    assert_eq!(error.code(), "reflect.no_default");
    assert!(
        error.to_string().contains("`Never` has no variants"),
        "{error}"
    );
}

#[derive(Clone, Reflect)]
struct Light {
    #[reflect(min = 0, default = -1.0)]
    intensity: f32,
}

/// Registered, it describes its field's type, whose default its own bounds refuse.
#[derive(Clone, crate::Component)]
struct Dim {
    light: Light,
}

#[derive(Clone, Reflect)]
enum Glow {
    Off,
    Tinted(#[reflect(color, max = 1, default = Vec3::new(1.0, 2.0, 1.0))] Vec3),
}

/// Registered, it describes `Moons`, whose default holds an orbit its bounds refuse, which
/// only `Orbit`'s description, finished after it, says.
#[derive(Clone, crate::Component)]
struct Orbit {
    moons: Moons,
    #[reflect(min = 0)]
    radius: i32,
}

#[derive(Clone, Reflect)]
struct Moons {
    #[reflect(default = vec![Orbit { moons: Moons { orbits: Vec::new() }, radius: -1 }])]
    orbits: Vec<Orbit>,
}

/// Opaque as a whole, so that a default gives it nothing.
#[derive(Clone, Reflect)]
#[reflect(opaque)]
struct Socket;

#[derive(Clone, crate::Component)]
struct Plugged {
    #[reflect(default = Socket)]
    socket: Socket,
}

/// A default its field's bounds refuse is refused as its type registers, even one holding the
/// type registering, checked once that's described; and so is one opaque as a whole, which
/// would give nothing. Either leaves the type unregistered, and every type described with it.
#[test]
fn a_bad_default_is_refused_as_its_type_registers() {
    let mut world = World::default();
    let error = world.register::<Dim>().expect_err("a bad default");
    assert_eq!(error.code(), "reflect.bad_default");
    assert_eq!(
        error.to_string(),
        "`Light`'s field `intensity` defaults to a value it refuses: -1 is below the \
         field's min, 0"
    );
    let error = world.register::<Orbit>().expect_err("a bad default");
    assert_eq!(error.code(), "reflect.bad_default");
    assert_eq!(
        error.to_string(),
        "`Moons`'s field `orbits[0].radius` defaults to a value it refuses: -1 is below the \
         field's min, 0"
    );
    let error = world.register::<Plugged>().expect_err("an opaque default");
    assert_eq!(error.code(), "reflect.bad_default");
    assert_eq!(
        error.to_string(),
        "`Plugged`'s field `socket` is opaque, which reflection keeps but doesn't make, so it \
         takes no default"
    );
    let names: Vec<&str> = world.types().iter().map(|(_, info)| &*info.name).collect();
    assert_eq!(names, ["Parent"]);
    assert_eq!(world.component_types().collect::<Vec<_>>(), ["Parent"]);
    let mut types = Types::default();
    let error = Glow::describe(&mut types).expect_err("a bad default");
    assert_eq!(error.code(), "reflect.bad_default");
    assert_eq!(
        error.to_string(),
        "`Glow::Tinted`'s field `0.y` defaults to a value it refuses: 2 is above the \
         field's max, 1"
    );
    assert_eq!(types.iter().len(), 0);
}

/// Each of the engine's components' default makes it, as an add makes it: a transform the
/// identity, a sprite the default one and a name empty, and one with an opaque field, from
/// its base; while a mesh, opaque as a whole without a base, and a parent, which holds an
/// entity, have none.
#[test]
fn the_engines_components_defaults_make_them() {
    let mut world = World::default();
    for registered in [
        world.register::<Transform>(),
        world.register::<Sprite>(),
        world.register::<MeshDraw>(),
        world.register::<Name>(),
        world.register::<Everything>(),
    ] {
        registered.expect("registered");
    }
    let default = |name| {
        let default = world.default_of(name);
        default.unwrap_or_else(|error| panic!("{name}: {error}"))
    };
    assert!(same(&default("Transform"), &Transform::IDENTITY.to_value()));
    assert!(same(&default("Sprite"), &Sprite::default().to_value()));
    let unnamed = Name(String::new()).to_value();
    assert!(same(&default("Name"), &unnamed));
    let Value::Struct(fields) = default("Everything") else {
        panic!("everything is a struct");
    };
    assert_eq!(fields.last(), Some(&("handle".into(), Value::Opaque)));
    for (name, code, at) in [
        ("MeshDraw", "reflect.opaque", "`MeshDraw`"),
        ("Parent", "reflect.no_default", "`Parent.0`"),
    ] {
        let error = world.default_of(name).expect_err(name);
        assert_eq!(error.code(), code, "{error}");
        let says = format!("can't make a default {at}: ");
        assert!(error.to_string().starts_with(&says), "{error}");
    }
    let error = world.default_of("Inner").expect_err("a type of fields");
    assert_eq!(error.code(), "reflect.unknown_type");
}

#[test]
fn an_error_keeps_its_cause_as_it_says_where() {
    let cause = std::io::Error::other("the disk is full");
    let error = Error::new("game.unsaved", "it isn't saved").with_source(cause);
    let error = within(within(error, "slot"), "saves[2]");
    assert_eq!(error.code(), "game.unsaved");
    assert_eq!(
        format!("{error:#}"),
        "at `saves[2].slot`: it isn't saved: the disk is full"
    );
}
