//! Built-in shapes: models made in code, so a game places a cube or a sphere without a model file.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use gg_core::math::{DVec3, Vec3, libm};
use gg_core::{Error, Result};
use gg_renderer::Workflow;

use crate::Transform;
use crate::gltf::{self, Imported, Part, Primitive};
#[cfg(doc)]
use crate::{Context, MeshCollider3D, Model};

/// A built-in shape: a [`Model`] made in code, white, of one node.
///
/// Each is centered on its origin, Y up, and 1 unit across:
///
/// | Shape | Size | Normals |
/// | :- | :- | :- |
/// | `Cube` | 1 by 1 by 1 | flat |
/// | `Sphere` | 1 across, 32 segments around and 16 down | smooth |
/// | `Capsule` | 1 across and 2 tall along Y, hemispheres of radius 0.5 | smooth |
/// | `Plane` | 1 by 1 in XZ at y 0, facing +Y, drawn from above alone | flat |
///
/// Each has texture coordinates and tangents, and a white material of metallic 0 and roughness
/// 0.5, opaque and one-sided. As a model it spawns nodes, collides through [`MeshCollider3D`] and is
/// picked like a glTF one.
///
/// Code gets one through [`Context::shape`]. Scenes, prefabs and the tool API name it by
/// [`name`](Self::name), as `ModelRoot("gg:cube")`, which loads it as a file's model loads. It has
/// no file, so no asset ID, no hot reload and no place in a package: every run makes the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shape {
    Cube,
    Sphere,
    Capsule,
    Plane,
}

/// What a shape's name starts with, reserved: a path that every OS reads alike holds no `:`.
const PREFIX: &str = "gg:";

/// Segments around a sphere or capsule.
const AROUND: u32 = 32;

/// Rings from pole to pole of a sphere; each hemisphere of a capsule has half as many.
const DOWN: u32 = 16;

impl Shape {
    /// Every shape, in the order of its variants.
    pub const ALL: [Self; 4] = [Self::Cube, Self::Sphere, Self::Capsule, Self::Plane];

    /// The name text gives it: `gg:cube`, `gg:sphere`, `gg:capsule` or `gg:plane`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Cube => "gg:cube",
            Self::Sphere => "gg:sphere",
            Self::Capsule => "gg:capsule",
            Self::Plane => "gg:plane",
        }
    }

    /// The shape text names `name`; `None` for a name without the `gg:` prefix, an asset's.
    ///
    /// Fails with `asset.unknown_shape` for a `gg:` name that isn't a shape's.
    pub(crate) fn named(name: &str) -> Option<Result<Self>> {
        if !name.starts_with(PREFIX) {
            return None;
        }
        let found = Self::ALL.into_iter().find(|shape| shape.name() == name);
        Some(found.ok_or_else(|| {
            let message = format!(
                "`{name}` names no built-in shape: names starting `{PREFIX}` are the engine's, \
                 gg:cube, gg:sphere, gg:capsule and gg:plane"
            );
            Error::new("asset.unknown_shape", message)
        }))
    }

    /// The model it is, as reading a glTF file would give it.
    pub(crate) fn imported(self) -> Imported {
        let mut mesh = Builder::default();
        match self {
            Self::Cube => {
                // Each face's normal, then its across and up, across × up = normal.
                for (normal, across, up) in [
                    (DVec3::X, DVec3::NEG_Z, DVec3::Y),
                    (DVec3::NEG_X, DVec3::Z, DVec3::Y),
                    (DVec3::Y, DVec3::X, DVec3::NEG_Z),
                    (DVec3::NEG_Y, DVec3::X, DVec3::Z),
                    (DVec3::Z, DVec3::X, DVec3::Y),
                    (DVec3::NEG_Z, DVec3::NEG_X, DVec3::Y),
                ] {
                    mesh.square(normal * 0.5, normal, across, up);
                }
            }
            Self::Plane => mesh.square(DVec3::ZERO, DVec3::Y, DVec3::X, DVec3::NEG_Z),
            Self::Sphere => {
                let rings = (0..=DOWN).map(|i| (PI * f64::from(i) / f64::from(DOWN), 0.0));
                mesh.lathe(&rings.collect::<Vec<_>>());
            }
            Self::Capsule => {
                let half = DOWN / 2;
                let bottom = (0..=half).map(|i| (FRAC_PI_2 * f64::from(i) / f64::from(half), -0.5));
                let top =
                    (0..=half).map(|i| (FRAC_PI_2 * (1.0 + f64::from(i) / f64::from(half)), 0.5));
                mesh.lathe(&bottom.chain(top).collect::<Vec<_>>());
            }
        }
        mesh.imported(self.name().trim_start_matches(PREFIX))
    }
}

/// A primitive's vertices and triangles as they're made.
#[derive(Default)]
struct Builder {
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    normals: Vec<[f32; 3]>,
    tangents: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl Builder {
    /// Adds a vertex at `at` with `normal`, `tangent` along the texture's U, and texture
    /// coordinates `uv`. W is 1: cross(normal, tangent) points up the texture, as V falls.
    fn vertex(&mut self, at: DVec3, normal: DVec3, tangent: DVec3, uv: [f64; 2]) -> u32 {
        let index = u32::try_from(self.positions.len()).expect("a shape's few vertices");
        let float = |v: DVec3| v.as_vec3().to_array();
        self.positions.push(float(at));
        self.normals.push(float(normal));
        self.tangents
            .push(Vec3::from(float(tangent)).extend(1.0).to_array());
        self.uvs.push(uv.map(|x| x as f32));
        index
    }

    /// Adds a square 1 across around `center`, facing `normal`, its texture's U along `across` and
    /// V down `up` (glTF's texture coordinates start at the image's top left).
    fn square(&mut self, center: DVec3, normal: DVec3, across: DVec3, up: DVec3) {
        let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let [a, b, c, d] = corners.map(|[s, t]: [f64; 2]| {
            let at = center + across * (s - 0.5) + up * (t - 0.5);
            self.vertex(at, normal, across, [s, 1.0 - t])
        });
        // Counterclockwise seen from in front, as glTF's front faces are.
        self.indices.extend([a, b, c, a, c, d]);
    }

    /// Adds a surface turned about Y through `rings`, from the bottom up: each the angle from the
    /// downward pole of its normals and the height of the center they point from, the surface 0.5
    /// from it. A ring of angle 0 or π is a pole. U goes once around, V down the profile by length.
    fn lathe(&mut self, rings: &[(f64, f64)]) {
        // Sine and cosine through libm, so the vertices are the same bits on every platform.
        let sin_cos = |angle: f64| {
            let (sin, cos) = (libm::sin(angle), libm::cos(angle));
            // Exact at the poles and the equator, so a pole's vertices meet.
            let exact = |x: f64| if x.abs() < 1e-12 { 0.0 } else { x };
            (exact(sin), exact(cos))
        };
        // Each ring's distance from the bottom pole along the profile.
        let mut along = vec![0.0];
        for pair in rings.windows(2) {
            let [(angle, center), (next, next_center)] = [pair[0], pair[1]];
            let length = 0.5 * (next - angle) + (next_center - center);
            along.push(along.last().expect("one") + length);
        }
        let total = along.last().copied().expect("rings");
        let first = u32::try_from(self.positions.len()).expect("a shape's few vertices");
        for (&(angle, center), &along) in rings.iter().zip(&along) {
            let (sin, cos) = sin_cos(angle);
            for i in 0..=AROUND {
                let turned = TAU * f64::from(i) / f64::from(AROUND);
                let (turn_sin, turn_cos) = sin_cos(turned);
                // Around clockwise seen from above, so the surface faces out.
                let normal = DVec3::new(sin * turn_cos, -cos, -sin * turn_sin);
                let tangent = DVec3::new(-turn_sin, 0.0, -turn_cos);
                let at = DVec3::new(0.0, center, 0.0) + normal * 0.5;
                let uv = [f64::from(i) / f64::from(AROUND), 1.0 - along / total];
                self.vertex(at, normal, tangent, uv);
            }
        }
        let stride = AROUND + 1;
        let last = rings.len() - 1;
        let pole = |ring: usize| matches!(sin_cos(rings[ring].0).0, 0.0);
        for ring in 0..last {
            let low = first + u32::try_from(ring).expect("few rings") * stride;
            let high = low + stride;
            for i in 0..AROUND {
                let [a, b, c, d] = [low + i, low + i + 1, high + i + 1, high + i];
                // A pole's corners meet: its band is triangles alone.
                if !pole(ring) {
                    self.indices.extend([a, b, c]);
                }
                if !pole(ring + 1) {
                    self.indices.extend([a, c, d]);
                }
            }
        }
    }

    /// The model of one node drawing what's built, its primitive named after `name`.
    fn imported(self, name: &str) -> Imported {
        let count = self.positions.len();
        let material = gltf::Material {
            workflow: Workflow::MetallicRoughness {
                metallic: 0.0,
                roughness: 0.5,
            },
            ..gltf::Material::default()
        };
        let primitive = Primitive {
            name: format!("{name}/0"),
            positions: self.positions,
            uvs: self.uvs,
            colors: vec![[1.0; 4]; count],
            normals: self.normals,
            tangents: self.tangents,
            joints: Vec::new(),
            weights: Vec::new(),
            reach: None,
            indices: self.indices,
            mesh: None,
            material,
        };
        let part = Part {
            name: None,
            transform: Transform::IDENTITY,
            parent: None,
            primitive: Some(0),
            light: None,
            skin: None,
        };
        Imported {
            parts: vec![part],
            primitives: vec![primitive],
            ..Imported::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use gg_core::math::Vec4;
    use gg_renderer::MeshDraw;

    use super::*;
    use crate::dirs::tests::Scratch;
    use crate::package::{Files, Kind, Package, Writer};
    use crate::{Context, Entity, ModelRoot, World};

    /// A scene of one entity whose `ModelRoot` names `model`.
    fn scene(model: &str) -> String {
        format!(
            "(\n    entities: {{\n        \"0000000000000100\": (\n            ModelRoot: \
             (\"{model}\"),\n            Transform: (),\n        ),\n    }},\n)\n"
        )
    }

    /// `root`'s one node, which draws.
    fn drawing_node(world: &World, root: Entity) -> Entity {
        let nodes: Vec<Entity> = world.children(root).expect("alive").collect();
        let [node] = nodes[..] else {
            panic!("one node, not {}", nodes.len());
        };
        assert!(world.get::<MeshDraw>(node).is_some(), "a draw");
        node
    }

    /// Each shape's mesh holds together: as many of each attribute as vertices, unit normals,
    /// tangents at right angles to them, texture coordinates within the image, and every triangle
    /// covering something, wound counterclockwise seen from where its corners' normals point
    /// (outward), as glTF's front faces are. Its bounds are its size.
    #[test]
    fn each_shapes_mesh_faces_out_and_has_its_size() {
        for shape in Shape::ALL {
            let imported = shape.imported();
            let [primitive] = &imported.primitives[..] else {
                panic!("{shape:?} has one primitive");
            };
            let count = primitive.positions.len();
            assert_eq!(
                [
                    primitive.uvs.len(),
                    primitive.normals.len(),
                    primitive.tangents.len()
                ],
                [count; 3],
                "{shape:?}"
            );
            for ((&normal, &tangent), &uv) in (primitive.normals.iter())
                .zip(&primitive.tangents)
                .zip(&primitive.uvs)
            {
                let (normal, tangent) = (Vec3::from(normal), Vec4::from(tangent));
                assert!((normal.length() - 1.0).abs() < 1e-5, "{shape:?}: {normal}");
                assert!(
                    (tangent.truncate().length() - 1.0).abs() < 1e-5,
                    "{shape:?}"
                );
                assert!(normal.dot(tangent.truncate()).abs() < 1e-5, "{shape:?}");
                assert!(
                    uv.iter().all(|x| (0.0..=1.0).contains(x)),
                    "{shape:?}: {uv:?}"
                );
            }
            assert_eq!(primitive.indices.len() % 3, 0);
            for triangle in primitive.indices.chunks(3) {
                let [a, b, c] =
                    [0, 1, 2].map(|i| Vec3::from(primitive.positions[triangle[i] as usize]));
                let facing = (b - a).cross(c - a);
                assert!(
                    facing.length() > 1e-6,
                    "{shape:?}: a triangle covering nothing"
                );
                let normals = triangle
                    .iter()
                    .map(|&i| Vec3::from(primitive.normals[i as usize]));
                for normal in normals {
                    assert!(facing.dot(normal) > 0.0, "{shape:?}: a triangle facing in");
                }
                // Outward: away from the center, or up for the plane.
                let middle = (a + b + c) / 3.0;
                let out = match shape {
                    Shape::Plane => Vec3::Y,
                    Shape::Capsule => middle - Vec3::Y * middle.y.clamp(-0.5, 0.5),
                    _ => middle,
                };
                assert!(facing.dot(out) > 0.0, "{shape:?}: a triangle facing in");
            }
            let (least, most) = (primitive.positions.iter()).fold(
                (Vec3::INFINITY, Vec3::NEG_INFINITY),
                |(least, most), &at| (least.min(at.into()), most.max(at.into())),
            );
            let size = match shape {
                Shape::Cube | Shape::Sphere => Vec3::ONE,
                Shape::Capsule => Vec3::new(1.0, 2.0, 1.0),
                Shape::Plane => Vec3::new(1.0, 0.0, 1.0),
            };
            assert!(least.abs_diff_eq(-size / 2.0, 1e-6), "{shape:?}: {least}");
            assert!(most.abs_diff_eq(size / 2.0, 1e-6), "{shape:?}: {most}");
        }
    }

    #[test]
    fn a_shape_is_found_by_its_name_and_a_gg_name_that_isnt_one_is_refused() {
        for shape in Shape::ALL {
            let found = Shape::named(shape.name()).expect("a gg: name");
            assert_eq!(found.expect("a shape"), shape);
        }
        assert!(Shape::named("models/cube.glb").is_none(), "an asset's");
        let error = Shape::named("gg:cone")
            .expect("a gg: name")
            .expect_err("no shape");
        assert_eq!(error.code(), "asset.unknown_shape");
        assert!(error.to_string().contains("`gg:cone`"), "{error}");
    }

    /// A shape is added once, its add noted each time as a file's is; its name and path are its
    /// `gg:` name, which text finds it by.
    #[test]
    fn a_shape_is_added_once_and_named_by_its_gg_name() {
        let mut cx = Context::seeded(1);
        let cube = cx.shape(Shape::Cube);
        let notes = cx.world().assets().added().len();
        assert_eq!(cx.shape(Shape::Cube), cube);
        assert_eq!(cx.world().assets().added().len(), notes + 1);
        assert_ne!(cx.shape(Shape::Sphere), cube);
        let assets = cx.world().assets();
        assert_eq!(assets.model("gg:cube"), Some(cube));
        let named = (assets.model_name(cube), assets.model_path(cube));
        assert_eq!(named, ("gg:cube", "gg:cube"));
        let root = cube.spawn(cx.world_mut(), Transform::IDENTITY);
        drawing_node(cx.world(), root);
    }

    /// A scene file naming a shape loads it first, as a file's model, and its root spawns the
    /// shape's node; saved, it names the shape again. A `gg:` name that's no shape's is refused,
    /// saying why.
    #[test]
    fn a_scene_naming_a_shape_loads_it_and_saves_its_name() {
        let dir = Scratch::new("shape-scene");
        let path = dir.0.join("scenes/s.ron");
        fs::create_dir_all(dir.0.join("scenes")).expect("a directory");
        fs::write(&path, scene("gg:cube")).expect("a scene");
        let mut cx = Context::seeded(1).root(dir.0.clone());
        let roots = cx.load_scene_file(&path).expect("loaded");
        let world = cx.world();
        let cube = world.assets().model("gg:cube").expect("added");
        assert_eq!(world.get::<ModelRoot>(roots[0]), Some(&ModelRoot(cube)));
        drawing_node(world, roots[0]);
        let saved = world.save_scene(&roots).expect("saved");
        assert!(saved.contains("ModelRoot: (\"gg:cube\")"), "{saved}");

        fs::write(&path, scene("gg:cone")).expect("a scene");
        let mut cx = Context::seeded(1).root(dir.0.clone());
        let error = cx.load_scene_file(&path).expect_err("no such shape");
        assert_eq!(error.code(), "asset.no_model", "{error}");
        assert!(error.to_string().contains("asset.unknown_shape"), "{error}");
    }

    /// A run from a game's package, which holds no shape, makes those its scenes name.
    #[test]
    fn a_run_from_a_package_makes_the_shapes_its_scenes_name() {
        let dir = Scratch::new("shape-package");
        let mut writer = Writer::new(".", Some("scenes/main.ron"), None);
        let text = scene("gg:capsule").into_bytes();
        let added = writer.add("scenes/main.ron", None, Kind::File, text);
        added.expect("added");
        fs::create_dir_all(&dir.0).expect("a directory");
        let path = dir.0.join("game.ggpk");
        writer.write(&path).expect("written");
        // Never made: read from the package.
        let root = dir.0.join("game");
        let mut cx = Context::seeded(1).root(root.clone());
        let package = Arc::new(Package::open(&path).expect("opened"));
        cx.files = Files::package(package, root.clone());
        let roots = cx.load_scene_file(root.join("scenes/main.ron"));
        let roots = roots.expect("loaded");
        drawing_node(cx.world(), roots[0]);
    }
}
