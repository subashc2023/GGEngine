//! Telling an edit inside function bodies, which a patch carries, from one that needs a rebuild.

use syn::visit_mut::{self, VisitMut};
use syn::{Attribute, Block, File, ImplItemFn, Item, ItemFn, ItemImpl, Stmt, TraitItemFn};

/// What an edit to a module is, as a patch sees it.
#[derive(Debug, PartialEq)]
pub(crate) enum Edit {
    /// Inside function bodies, which a patch carries. `attach` if among them is a `Layer` impl's
    /// `attach`, which builds the world only as a run starts afresh, not in one that keeps it.
    Bodies { attach: bool },
    /// The new text doesn't parse. A patch's build shows what rustc makes of it; if it builds,
    /// rustc read what gg can't, so the game is rebuilt.
    Unreadable,
    /// A patch can't carry it, for the reason.
    Rebuild(&'static str),
}

/// What the edit from `old`, a module's text as of the game's build, to `new` is.
///
/// Inside function bodies if the two are the same token for token, but for non-`const` function
/// bodies (their declared items aside) and doc comments.
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

/// `file` without doc comments (`#[doc]` attributes) and without non-`const` function bodies,
/// but for the items declared in them and the bodies `keep` keeps.
fn bare(mut file: File, keep: Keep) -> File {
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
        self.0.push(item.clone());
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
