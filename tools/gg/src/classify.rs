//! Telling an edit inside function bodies, which a patch carries, from one that needs a rebuild, as
//! `gg run`'s patches do for each of a game's modules that changed.

use syn::visit_mut::{self, VisitMut};
use syn::{Attribute, Block, File, ImplItemFn, Item, ItemFn, ItemImpl, Stmt, TraitItemFn};

/// What an edit to a module is, as a patch sees it.
#[derive(Debug, PartialEq)]
pub(crate) enum Edit {
    /// Inside the bodies of functions, which a patch carries.
    Bodies,
    /// One whose new text doesn't parse: a patch's build then says what rustc makes of it while the
    /// game runs on, but if it builds, rustc reading what gg can't, the game is rebuilt, as gg
    /// can't tell what changed.
    Unreadable,
    /// One that a patch can't carry, for a reason.
    Rebuild(&'static str),
}

/// What the edit from `old`, a module's text as of the game's build, to `new` is. It's inside
/// function bodies if the two are the same, token for token, but for the bodies of functions that
/// aren't `const`, the items declared in them aside, and for doc comments; comments and whitespace
/// aren't tokens. But an edit inside the bodies of code that runs as the game starts, `main` and a
/// `Layer`'s `load` and `restored`, needs a rebuild, which runs them again, as a patch would come
/// after they ran: as an edit to code only they call, in a function of its own, would, which gg
/// can't tell. Old text that doesn't parse can't be compared with, so any edit to it rebuilds.
pub(crate) fn classify(old: &str, new: &str) -> Edit {
    let Ok(old) = syn::parse_file(old) else {
        return Edit::Rebuild("an edit to a module gg can't read as it was built");
    };
    let Ok(new) = syn::parse_file(new) else {
        return Edit::Unreadable;
    };
    if bare(old.clone(), true) != bare(new.clone(), true) {
        return Edit::Rebuild("an edit outside function bodies");
    }
    if bare(old, false) != bare(new, false) {
        return Edit::Rebuild(
            "an edit to code that runs as the game starts, as a patch comes after",
        );
    }
    Edit::Bodies
}

/// `file` without doc comments, which are `#[doc]` attributes, and without the bodies of the
/// functions that aren't `const` but for the items declared in them, and with `startup`, without
/// those of the code that runs as the game starts too.
fn bare(mut file: File, startup: bool) -> File {
    let mut bare = Bare {
        startup,
        layer: false,
    };
    bare.visit_file_mut(&mut file);
    file
}

/// Empties the bodies of free functions, methods and traits' default methods, but for a `const
/// fn`'s, which the crate's build may evaluate into a type or a static, and, unless `startup`, those
/// of `main` and of a `Layer` impl's `load` and `restored`; keeps the items declared in a body,
/// however deep, as a type whose values the running game holds; and drops doc comments. An item
/// that a macro invoked in a body declares, as `thread_local!` does, goes with the body.
struct Bare {
    /// Whether the bodies of the code that runs as the game starts are emptied too.
    startup: bool,
    /// Whether the impl being visited is one of `Layer`.
    layer: bool,
}

impl VisitMut for Bare {
    fn visit_attributes_mut(&mut self, attrs: &mut Vec<Attribute>) {
        attrs.retain(|attr| !attr.path().is_ident("doc"));
    }

    fn visit_item_fn_mut(&mut self, item: &mut ItemFn) {
        let startup = !self.startup && item.sig.ident == "main";
        if item.sig.constness.is_none() && !startup {
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
        let startup = !self.startup && self.layer && (name == "load" || name == "restored");
        if item.sig.constness.is_none() && !startup {
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

/// Empties `block` but for the items declared in it, however deep, as in a nested block or a
/// closure's body, in the order they come.
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

    /// A module with what a game's has: a type, a static, a `const fn`, free functions, one with a
    /// type of its own, a trait with a default method, impls of it and of `Layer`, and `main`.
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
}

impl Layer for Ship {
    fn load(&mut self, cx: &mut Context) {
        cx.add_texture("ship.png");
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
            // A body, a method's, a trait's default method's, a `Layer`'s tick's, a function's
            // declared in a body, and a method's of another impl named as a `Layer`'s is.
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
            assert_eq!(edit, Edit::Bodies, "{from} -> {to}");
        }
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
            // A type declared in a body, whose values the running game may hold.
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
            ("App::new(\"Ships\")", "App::new(\"Boats\")"),
        ] {
            let edit = classify(MODULE, &edited(from, to));
            assert!(
                matches!(edit, Edit::Rebuild(why) if why.contains("starts")),
                "{edit:?}"
            );
        }
    }

    #[test]
    fn text_that_doesnt_parse_is_patched_to_hear_rustc_unless_its_the_builds() {
        let broken = edited("ship.x += by * SPEED;", "ship.x += by *;");
        assert_eq!(classify(MODULE, &broken), Edit::Unreadable);
        // The build's own text, as one in syntax syn doesn't know, can't be compared with.
        for new in [MODULE, broken.as_str()] {
            assert!(matches!(classify("fn steer( {", new), Edit::Rebuild(_)));
        }
    }
}
