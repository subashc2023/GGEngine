//! Telling an edit inside function bodies, which a patch carries, from one that needs a rebuild.

use syn::visit_mut::{self, VisitMut};
use syn::{
    Attribute, Block, File, ImplItem, ImplItemFn, Item, ItemFn, ItemImpl, ItemMod, Stmt,
    TraitItemFn,
};

/// What an edit to a module is, as a patch sees it.
#[derive(Debug, PartialEq)]
pub(crate) enum Edit {
    /// Inside function bodies, which a patch carries. `attach` if among them is a `Layer` impl's
    /// `attach`, which builds the world only as a run starts afresh, not in one that keeps it.
    Bodies { attach: bool },
    /// Only items under `#[cfg(test)]` differ, which the game's binary doesn't hold: nothing to
    /// patch.
    Same,
    /// The new text doesn't parse. A patch's build shows what rustc makes of it; if it builds,
    /// rustc read what gg can't, so the game is rebuilt.
    Unreadable,
    /// A patch can't carry it, for the reason.
    Rebuild(&'static str),
}

/// What the edit from `old`, a module's text as of the game's build, to `new` is.
///
/// `Same` if the two differ only in items under `#[cfg(test)]`. Else inside function bodies if the
/// two are the same token for token, but for non-`const` function
/// bodies (their declared items aside) and doc comments, leaving out items under `#[cfg(test)]`.
///
/// An edit inside code that runs as the game starts (`main`, a `Layer`'s `load` and `restored`)
/// needs a rebuild: a patch would come after they ran. So does an edit to code only they call,
/// which gg can't tell. An edit inside `attach` is patched and told of: no patch builds the world
/// again.
///
/// Old text that doesn't parse can't be compared with: any edit to it rebuilds.
pub(crate) fn classify(old: &str, new: &str) -> Edit {
    let Ok(old) = syn::parse_file(old) else {
        return Edit::Rebuild("an edit to a module gg can't read as it was built");
    };
    let Ok(new) = syn::parse_file(new) else {
        return Edit::Unreadable;
    };
    if old != new {
        let (mut old_bare, mut new_bare) = (old.clone(), new.clone());
        untested(&mut old_bare);
        untested(&mut new_bare);
        if old_bare == new_bare {
            return Edit::Same;
        }
    }
    if bare(old.clone(), Keep::None) != bare(new.clone(), Keep::None) {
        return Edit::Rebuild("an edit outside function bodies");
    }
    if bare(old.clone(), Keep::Startup) != bare(new.clone(), Keep::Startup) {
        return Edit::Rebuild(
            "an edit to code that runs as the game starts, as a patch comes after",
        );
    }
    Edit::Bodies {
        attach: bare(old, Keep::Attach) != bare(new, Keep::Attach),
    }
}

/// The bodies [`bare`] keeps, beside those of `const fn`s.
#[derive(Clone, Copy, PartialEq)]
enum Keep {
    None,
    /// `main`, and a `Layer` impl's `load` and `restored`.
    Startup,
    /// A `Layer` impl's `attach`.
    Attach,
}

/// `file` without doc comments (`#[doc]` attributes), `#[cfg(test)]` items and non-`const`
/// function bodies, but for the items declared in them and the bodies `keep` keeps.
fn bare(mut file: File, keep: Keep) -> File {
    untested(&mut file);
    let mut bare = Bare { keep, layer: false };
    bare.visit_file_mut(&mut file);
    file
}

/// Empties function bodies (free, methods, default methods) but for `const fn`s' (the build may
/// evaluate them into a type or a static) and those `keep` keeps. Keeps items declared in a body,
/// however deep: the running game may hold values of a type among them. Drops doc comments. Items
/// a macro declares in a body (`thread_local!`) go with it.
struct Bare {
    keep: Keep,
    /// Whether the impl being visited is one of `Layer`.
    layer: bool,
}

impl VisitMut for Bare {
    fn visit_attributes_mut(&mut self, attrs: &mut Vec<Attribute>) {
        attrs.retain(|attr| !attr.path().is_ident("doc"));
    }

    fn visit_item_fn_mut(&mut self, item: &mut ItemFn) {
        let kept = self.keep == Keep::Startup && item.sig.ident == "main";
        if item.sig.constness.is_none() && !kept {
            keep_items(&mut item.block);
        }
        visit_mut::visit_item_fn_mut(self, item);
    }

    fn visit_item_impl_mut(&mut self, item: &mut ItemImpl) {
        let layer = item.trait_.as_ref().is_some_and(|(path, _)| {
            path.segments
                .last()
                .is_some_and(|last| last.ident == "Layer")
        });
        let outer = std::mem::replace(&mut self.layer, layer);
        visit_mut::visit_item_impl_mut(self, item);
        self.layer = outer;
    }

    fn visit_impl_item_fn_mut(&mut self, item: &mut ImplItemFn) {
        let name = &item.sig.ident;
        let kept = self.layer
            && match self.keep {
                Keep::None => false,
                Keep::Startup => name == "load" || name == "restored",
                Keep::Attach => name == "attach",
            };
        if item.sig.constness.is_none() && !kept {
            keep_items(&mut item.block);
        }
        visit_mut::visit_impl_item_fn_mut(self, item);
    }

    fn visit_trait_item_fn_mut(&mut self, item: &mut TraitItemFn) {
        if item.sig.constness.is_none()
            && let Some(body) = &mut item.default
        {
            keep_items(body);
        }
        visit_mut::visit_trait_item_fn_mut(self, item);
    }
}

/// Takes the items under `#[cfg(test)]` out of `file`, however deep in modules and impls (not in
/// function bodies, which [`Items`] sees to).
fn untested(file: &mut File) {
    Untested.visit_file_mut(file);
}

struct Untested;

impl VisitMut for Untested {
    fn visit_file_mut(&mut self, file: &mut File) {
        file.items.retain(|item| !tested(item));
        visit_mut::visit_file_mut(self, file);
    }

    fn visit_item_mod_mut(&mut self, item: &mut ItemMod) {
        if let Some((_, items)) = &mut item.content {
            items.retain(|item| !tested(item));
        }
        visit_mut::visit_item_mod_mut(self, item);
    }

    fn visit_item_impl_mut(&mut self, item: &mut ItemImpl) {
        item.items.retain(|item| !tested_impl(item));
        visit_mut::visit_item_impl_mut(self, item);
    }
}

/// Whether `attrs` hold `#[cfg(test)]`.
fn cfg_test(attrs: &[Attribute]) -> bool {
    let test = |attr: &Attribute| {
        attr.path().is_ident("cfg") && attr.parse_args::<syn::Ident>().is_ok_and(|id| id == "test")
    };
    attrs.iter().any(test)
}

/// Whether `item` is under `#[cfg(test)]`.
fn tested(item: &Item) -> bool {
    match item {
        Item::Const(item) => cfg_test(&item.attrs),
        Item::Enum(item) => cfg_test(&item.attrs),
        Item::ExternCrate(item) => cfg_test(&item.attrs),
        Item::Fn(item) => cfg_test(&item.attrs),
        Item::ForeignMod(item) => cfg_test(&item.attrs),
        Item::Impl(item) => cfg_test(&item.attrs),
        Item::Macro(item) => cfg_test(&item.attrs),
        Item::Mod(item) => cfg_test(&item.attrs),
        Item::Static(item) => cfg_test(&item.attrs),
        Item::Struct(item) => cfg_test(&item.attrs),
        Item::Trait(item) => cfg_test(&item.attrs),
        Item::TraitAlias(item) => cfg_test(&item.attrs),
        Item::Type(item) => cfg_test(&item.attrs),
        Item::Union(item) => cfg_test(&item.attrs),
        Item::Use(item) => cfg_test(&item.attrs),
        _ => false,
    }
}

/// Whether `item`, of an impl, is under `#[cfg(test)]`.
fn tested_impl(item: &ImplItem) -> bool {
    match item {
        ImplItem::Const(item) => cfg_test(&item.attrs),
        ImplItem::Fn(item) => cfg_test(&item.attrs),
        ImplItem::Type(item) => cfg_test(&item.attrs),
        ImplItem::Macro(item) => cfg_test(&item.attrs),
        _ => false,
    }
}

/// Whether the module `name`, in the file whose text is `parent`, is declared apart from its
/// `parent` and under `#[cfg(test)]`, as `#[cfg(test)] mod tests;`: its file isn't in the game's
/// binary.
pub(crate) fn declares_test(parent: &str, name: &str) -> bool {
    struct Find<'a>(&'a str, bool);
    impl VisitMut for Find<'_> {
        fn visit_item_mod_mut(&mut self, item: &mut ItemMod) {
            let apart = item.content.is_none() && item.ident == self.0;
            self.1 |= apart && cfg_test(&item.attrs);
            visit_mut::visit_item_mod_mut(self, item);
        }
    }
    let Ok(mut file) = syn::parse_file(parent) else {
        return false;
    };
    let mut find = Find(name, false);
    find.visit_file_mut(&mut file);
    find.1
}

/// Empties `block` but for the items declared in it, however deep, in order.
fn keep_items(block: &mut Block) {
    let mut items = Items(Vec::new());
    items.visit_block_mut(block);
    block.stmts = items.0.into_iter().map(Stmt::Item).collect();
}

/// Collects the items a block declares, each whole.
struct Items(Vec<Item>);

impl VisitMut for Items {
    fn visit_item_mut(&mut self, item: &mut Item) {
        if !tested(item) {
            self.0.push(item.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A game's module: types, a static, a `const fn`, functions, a trait with a default method,
    /// impls (one with methods named as a `Layer`'s are, one of `Layer`), and `main`.
    const MODULE: &str = r#"
//! The game.

use std::time::Duration;

static SPEED: f32 = 1.0;

/// Where it is.
struct Ship {
    x: f32,
}

const fn lanes() -> u32 {
    3
}

fn steer(ship: &mut Ship, by: f32) {
    // Right is positive.
    ship.x += by * SPEED;
}

fn launch() -> u32 {
    struct Burst {
        count: u32,
    }
    fn count(burst: &Burst) -> u32 {
        burst.count
    }
    count(&Burst { count: 3 })
}

trait Turn {
    fn turn(&mut self) {
        println!("turning");
    }
}

impl Turn for Ship {}

impl Ship {
    fn stop(&mut self) {
        self.x = 0.0;
    }

    fn load(&mut self) {
        self.x = 2.0;
    }

    fn attach(&mut self) {
        self.x = 1.0;
    }
}

impl Layer for Ship {
    fn load(&mut self, cx: &mut Context) {
        cx.add_texture("ship.png");
    }

    fn attach(&mut self, cx: &mut Context) {
        cx.world_mut().spawn(Ship { x: 0.0 });
    }

    fn restored(&mut self, cx: &mut Context) {
        cx.play_music("theme.ogg");
    }

    fn tick(&mut self, world: &mut World, tick: Tick) {
        self.x += 1.0;
    }
}

fn main() {
    App::new("Ships").run();
}
"#;

    /// `MODULE` with `from` replaced by `to`, which it has once.
    fn edited(from: &str, to: &str) -> String {
        assert_eq!(MODULE.matches(from).count(), 1, "{from}");
        MODULE.replace(from, to)
    }

    #[test]
    fn edits_inside_function_bodies_are_patchable() {
        for (from, to) in [
            // Bodies of: a method, a default method, a `Layer`'s `tick`, a function declared in a
            // body, and another impl's method named as a `Layer`'s is.
            ("ship.x += by * SPEED;", "ship.x -= by * SPEED * 2.0;"),
            (
                "self.x = 0.0;",
                "self.x = 0.0;\n        println!(\"stopped\");",
            ),
            ("println!(\"turning\");", "println!(\"turning left\");"),
            ("self.x += 1.0;", "self.x += 2.0;"),
            ("burst.count\n", "burst.count * 2\n"),
            ("self.x = 2.0;", "self.x = 3.0;"),
            // Doc comments, comments and whitespace.
            (
                "/// Where it is.",
                "/// Where it is, in meters.\n/// Right is positive.",
            ),
            ("//! The game.", "//! The game, a ship."),
            ("// Right is positive.", "// Left is negative."),
            (
                "fn steer(ship: &mut Ship, by: f32) {",
                "fn steer(ship:&mut Ship,by:f32){",
            ),
        ] {
            let edit = classify(MODULE, &edited(from, to));
            assert_eq!(edit, Edit::Bodies { attach: false }, "{from} -> {to}");
        }
    }

    /// An edit inside a `Layer`'s `attach` is patched and told of, beside other edits too. One to
    /// another impl's method of that name isn't told of.
    #[test]
    fn edits_inside_a_layers_attach_are_patched_and_told_of() {
        let attached = edited("Ship { x: 0.0 }", "Ship { x: 5.0 }");
        assert_eq!(classify(MODULE, &attached), Edit::Bodies { attach: true });
        let beside = attached.replace("self.x += 1.0;", "self.x += 2.0;");
        assert_eq!(classify(MODULE, &beside), Edit::Bodies { attach: true });
        let other = classify(MODULE, &edited("self.x = 1.0;", "self.x = 4.0;"));
        assert_eq!(other, Edit::Bodies { attach: false });
    }

    /// Items under `#[cfg(test)]`, a module of tests and a declaration of one apart aren't in the
    /// game's binary: an edit to them, or adding or removing one, is no change.
    #[test]
    fn edits_to_test_code_are_no_change() {
        let tested = format!(
            "{MODULE}
#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn steers() {{
        assert_eq!(1, 1);
    }}
}}

#[cfg(test)]
mod more;

#[cfg(test)]
fn helper() -> u32 {{
    1
}}

#[cfg(test)]
impl Ship {{
    fn at(&self) -> f32 {{
        self.x
    }}
}}

impl Ship {{
    #[cfg(test)]
    fn peek(&self) -> f32 {{
        self.x
    }}
}}
"
        );
        for (from, to) in [
            ("assert_eq!(1, 1);", "assert_eq!(2, 2);\n        helper();"),
            ("fn steers()", "fn steers_right()"),
            ("use super::*;", "use super::*;\n    use std::fmt;"),
            ("mod more;", "mod fewer;"),
            (
                "fn helper() -> u32 {\n    1",
                "fn helper() -> f32 {\n    1.0",
            ),
            ("self.x\n    }\n}\n\nimpl", "self.x * 2.0\n    }\n}\n\nimpl"),
            ("fn peek(&self) -> f32", "fn peek(&self) -> f64"),
        ] {
            assert_eq!(tested.matches(from).count(), 1, "{from}");
            let edited = tested.replace(from, to);
            assert_eq!(classify(&tested, &edited), Edit::Same, "{from} -> {to}");
        }
        // Added or removed.
        let module = format!("{MODULE}\n#[cfg(test)]\nmod tests {{\n    fn one() {{}}\n}}\n");
        assert_eq!(classify(MODULE, &module), Edit::Same);
        assert_eq!(classify(&module, MODULE), Edit::Same);
        // Beside an edit inside a function body, it's that edit alone.
        let beside = tested.replace("assert_eq!(1, 1);", "assert_eq!(3, 3);");
        let beside = beside.replace("self.x += 1.0;", "self.x += 2.0;");
        assert_eq!(classify(&tested, &beside), Edit::Bodies { attach: false });
        // Not under `#[cfg(test)]`, a module of that name is the game's.
        let plain = MODULE.replace(
            "fn main()",
            "mod tests {\n    pub fn one() {}\n}\n\nfn main()",
        );
        let outside = Edit::Rebuild("an edit outside function bodies");
        assert_eq!(classify(MODULE, &plain), outside);
    }

    #[test]
    fn a_module_declared_apart_under_cfg_test_is_found() {
        let parent = "mod ship;\n\n#[cfg(test)]\nmod tests;\n\nmod inner {\n    #[cfg(test)]\n    mod checks;\n}\n";
        for name in ["tests", "checks"] {
            assert!(declares_test(parent, name), "{name}");
        }
        for name in ["ship", "nope"] {
            assert!(!declares_test(parent, name), "{name}");
        }
        assert!(!declares_test("mod tests {}", "tests"));
        assert!(!declares_test("#[cfg(test)] mod tests {}", "tests"));
        assert!(!declares_test("fn {", "tests"));
    }

    #[test]
    fn edits_outside_function_bodies_need_a_rebuild() {
        for (from, to) in [
            ("    x: f32,", "    x: f32,\n    y: f32,"),
            (
                "fn steer(ship: &mut Ship, by: f32)",
                "fn steer(ship: &mut Ship, by: f64)",
            ),
            ("    3\n", "    4\n"),
            (
                "impl Turn for Ship {}",
                "impl Turn for Ship {}\n\nfn dock() {}",
            ),
            ("static SPEED: f32 = 1.0;", "static SPEED: f32 = 2.0;"),
            (
                "use std::time::Duration;",
                "use std::time::{Duration, Instant};",
            ),
            (
                "    fn stop(&mut self) {",
                "    #[inline]\n    fn stop(&mut self) {",
            ),
            // A type declared in a body: the running game may hold its values.
            (
                "        count: u32,",
                "        count: u32,\n        size: f32,",
            ),
        ] {
            let edit = classify(MODULE, &edited(from, to));
            let outside = Edit::Rebuild("an edit outside function bodies");
            assert_eq!(edit, outside, "{from} -> {to}");
        }
    }

    #[test]
    fn edits_to_code_that_runs_as_the_game_starts_need_a_rebuild() {
        for (from, to) in [
            (
                "cx.add_texture(\"ship.png\");",
                "cx.add_texture(\"boat.png\");",
            ),
            (
                "cx.play_music(\"theme.ogg\");",
                "cx.play_music(\"calm.ogg\");",
            ),
            ("App::new(\"Ships\")", "App::new(\"Boats\")"),
        ] {
            let edit = classify(MODULE, &edited(from, to));
            assert!(
                matches!(edit, Edit::Rebuild(why) if why.contains("starts")),
                "{edit:?}"
            );
        }
        // Beside an edit inside `attach`.
        let both = edited("Ship { x: 0.0 }", "Ship { x: 5.0 }").replace("ship.png", "boat.png");
        let edit = classify(MODULE, &both);
        assert!(
            matches!(edit, Edit::Rebuild(why) if why.contains("starts")),
            "{edit:?}"
        );
    }

    #[test]
    fn text_that_doesnt_parse_is_patched_to_hear_rustc_unless_its_the_builds() {
        let broken = edited("ship.x += by * SPEED;", "ship.x += by *;");
        assert_eq!(classify(MODULE, &broken), Edit::Unreadable);
        // Build text in syntax syn doesn't know can't be compared with.
        for new in [MODULE, broken.as_str()] {
            assert!(matches!(classify("fn steer( {", new), Edit::Rebuild(_)));
        }
    }
}
