//! Telling an edit inside function bodies, which a patch carries, from one that needs a rebuild.

use std::collections::BTreeMap;

use syn::visit_mut::{self, VisitMut};
use syn::{
    Attribute, Block, Expr, ExprInfer, File, GenericArgument, Ident, ImplItem, ImplItemConst,
    ImplItemFn, Item, ItemConst, ItemFn, ItemImpl, ItemMod, ItemUse, Macro, MetaList, Signature,
    Stmt, TraitItemConst, TraitItemFn, Type, UseRename, UseTree, Visibility,
};

/// What an edit to a module is, as a patch sees it.
#[derive(Debug, PartialEq)]
pub(crate) enum Edit {
    /// Inside what a patch carries: function bodies, constants' values, `use`s, and functions
    /// added, removed or re-signed. `attach` if among them is a `Layer` impl's `attach`, which
    /// builds the world only as a run starts afresh, not in one that keeps it. `consts` names the
    /// constants whose values differ, sorted: a patch carries them only where [`uses`] says.
    Bodies { attach: bool, consts: Vec<String> },
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
/// two are the same token for token, but for non-`const` function bodies (their declared items
/// aside), constants' values, doc comments, `use`s and the functions [`free`] lets come and go,
/// leaving out items under `#[cfg(test)]`.
///
/// An edit inside code that runs as the game starts (`main`, a `Layer`'s `load` and `restored`)
/// needs a rebuild: a patch would come after they ran. So does an edit to code only they call,
/// which gg can't tell. An edit inside `attach` is patched and told of: no patch builds the world
/// again.
///
/// A patch carries a changed `use` where function bodies alone use the names it brings in, matched
/// as words, as [`uses`] matches constants. A `pub use` or a glob's could change names gg can't
/// see: they rebuild.
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
    let old_bare = bare(old.clone(), Keep::None);
    let new_bare = bare(new.clone(), Keep::None);
    if old_bare.file != new_bare.file || old_bare.values.len() != new_bare.values.len() {
        return Edit::Rebuild("an edit outside function bodies");
    }
    let names = match renamed(&old_bare.uses, &new_bare.uses) {
        Ok(names) => names,
        Err(why) => return Edit::Rebuild(why),
    };
    if bare(old.clone(), Keep::Startup).file != bare(new.clone(), Keep::Startup).file {
        return Edit::Rebuild(STARTS);
    }
    let mut attach = bare(old, Keep::Attach).file != bare(new.clone(), Keep::Attach).file;
    if !names.is_empty() {
        match named(new, names) {
            Ok(in_attach) => attach |= in_attach,
            Err(why) => return Edit::Rebuild(why),
        }
    }
    // The trees match, so their constants come in the same order.
    let values = old_bare.values.into_iter().zip(new_bare.values);
    let mut consts: Vec<String> = values
        .filter(|(old, new)| old != new)
        .map(|((name, _), _)| name)
        .collect();
    consts.sort();
    consts.dedup();
    Edit::Bodies { attach, consts }
}

/// The names `use`s bring in that differ from `old` to `new`, each with index 0 as [`Find`] takes
/// them; or why a patch can't carry the change.
fn renamed(old: &[Binding], new: &[Binding]) -> Result<BTreeMap<String, usize>, &'static str> {
    // Counted, as one of two alike, in different modules or bodies, may change alone.
    let mut counts: BTreeMap<&Binding, isize> = BTreeMap::new();
    for binding in old {
        *counts.entry(binding).or_default() += 1;
    }
    for binding in new {
        *counts.entry(binding).or_default() -= 1;
    }
    let mut names = BTreeMap::new();
    for (binding, _) in counts.into_iter().filter(|&(_, count)| count != 0) {
        if binding.public {
            return Err(PUBLIC_USE);
        }
        match &binding.name {
            None => return Err(GLOB),
            Some(name) if name != "_" => {
                names.insert(name.clone(), 0);
            }
            Some(_) => {}
        }
    }
    Ok(names)
}

/// Whether code a patch doesn't carry uses any of `names`, which changed `use`s bring in: `Ok`
/// with whether a `Layer` impl's `attach` does, or why a patch can't carry the change.
fn named(mut file: File, names: BTreeMap<String, usize>) -> Result<bool, &'static str> {
    untested(&mut file);
    Free(Vec::new()).visit_file_mut(&mut file);
    let mut find = Find {
        names,
        kind: Kind::Use,
        place: Place::Item,
        types: 0,
        layer: false,
        attach: Vec::new(),
        found: None,
    };
    find.visit_file_mut(&mut file);
    match find.found {
        Some((_, why)) => Err(why),
        None => Ok(!find.attach.is_empty()),
    }
}

const STARTS: &str = "an edit to code that runs as the game starts, as a patch comes after";
const PUBLIC_USE: &str = "an edit to a `pub use`, which other modules may import through";
const GLOB: &str = "an edit to a glob `use`, whose names gg can't tell";
const USED_OUTSIDE: &str = "an edit to a `use` of a name used outside function bodies";
const USED_AT_START: &str =
    "an edit to a `use` of a name code that runs as the game starts uses, as a patch comes after";
pub(crate) const IN_A_TYPE: &str =
    "an edit to a constant a type uses, as an array's length or a generic argument";
pub(crate) const OUTSIDE_BODIES: &str =
    "an edit to a constant used outside function bodies, as in a static or a `const fn`";
pub(crate) const AT_START: &str =
    "an edit to a constant code that runs as the game starts uses, as a patch comes after";
pub(crate) const UNREAD: &str = "an edit to a constant beside a module gg can't read";

/// Where an edit to the constants `edited`, by name, reaches in the game's modules, `texts`.
///
/// rustc copies a constant's value into the code that uses it, so a patch of that code carries the
/// edit. Constants whose values name an edited one are followed too, as are names a `use` gives
/// one. A use anywhere else needs a rebuild:
/// - in a type, as an array's length or a const generic argument: values the game holds keep the
///   old layout, and components of the old type no longer match;
/// - outside function bodies, as in a static, an enum's discriminant or a `const fn`: the build
///   evaluated it into data or types a patch doesn't replace;
/// - in code run as the game starts, as a patch comes after.
///
/// Names match as words, in macros' tokens too, whatever they resolve to: a word that only shares a
/// name rebuilds needlessly. Items under `#[cfg(test)]` aren't the game's. A text that doesn't
/// parse could hide a use: it rebuilds.
pub(crate) fn uses(texts: &[&str], edited: &[String]) -> Uses {
    let mut files = Vec::new();
    for text in texts {
        let Ok(mut file) = syn::parse_file(text) else {
            return Uses::Rebuild(0, UNREAD);
        };
        untested(&mut file);
        files.push(file);
    }
    let mut links = Links(Vec::new());
    for file in &mut files {
        links.visit_file_mut(file);
    }
    let edited = edited.iter().enumerate();
    let mut names: BTreeMap<String, usize> = edited.map(|(at, name)| (name.clone(), at)).collect();
    loop {
        let reached = names.len();
        for (name, words) in &links.0 {
            if !names.contains_key(name)
                && let Some(from) = words.iter().find_map(|word| names.get(word).copied())
            {
                names.insert(name.clone(), from);
            }
        }
        if names.len() == reached {
            break;
        }
    }
    let mut find = Find {
        names,
        kind: Kind::Const,
        place: Place::Item,
        types: 0,
        layer: false,
        attach: Vec::new(),
        found: None,
    };
    for file in &mut files {
        find.visit_file_mut(file);
    }
    match find.found {
        Some((at, why)) => Uses::Rebuild(at, why),
        None => {
            find.attach.sort_unstable();
            find.attach.dedup();
            Uses::Bodies(find.attach)
        }
    }
}

/// Where edited constants are used, as [`uses`] tells.
#[derive(Debug, PartialEq)]
pub(crate) enum Uses {
    /// In function bodies alone, which a patch carries: the edited constants, by index, that a
    /// `Layer` impl's `attach` uses.
    Bodies(Vec<usize>),
    /// Where a patch can't carry the edited constant, by index, for the reason.
    Rebuild(usize, &'static str),
}

/// Each constant with the words its value holds, and each name a `use` gives with the name it
/// renames.
struct Links(Vec<(String, Vec<String>)>);

impl VisitMut for Links {
    fn visit_item_const_mut(&mut self, item: &mut ItemConst) {
        self.0.push((item.ident.to_string(), words(&mut item.expr)));
        visit_mut::visit_item_const_mut(self, item);
    }

    fn visit_impl_item_const_mut(&mut self, item: &mut ImplItemConst) {
        self.0.push((item.ident.to_string(), words(&mut item.expr)));
        visit_mut::visit_impl_item_const_mut(self, item);
    }

    fn visit_trait_item_const_mut(&mut self, item: &mut TraitItemConst) {
        if let Some((_, value)) = &mut item.default {
            self.0.push((item.ident.to_string(), words(value)));
        }
        visit_mut::visit_trait_item_const_mut(self, item);
    }

    fn visit_use_rename_mut(&mut self, item: &mut UseRename) {
        let renamed = vec![item.ident.to_string()];
        self.0.push((item.rename.to_string(), renamed));
    }
}

/// The words in `expr`: its identifiers and its macros' tokens'.
fn words(expr: &mut Expr) -> Vec<String> {
    struct Words(Vec<String>);
    impl VisitMut for Words {
        fn visit_ident_mut(&mut self, ident: &mut Ident) {
            self.0.push(ident.to_string());
        }

        fn visit_macro_mut(&mut self, mac: &mut Macro) {
            self.0.extend(tokens(&mac.tokens));
        }
    }
    let mut words = Words(Vec::new());
    words.visit_expr_mut(expr);
    words.0
}

/// The words in a macro's or an attribute's tokens, inside their literals too.
fn tokens(tokens: &impl ToString) -> Vec<String> {
    let text = tokens.to_string();
    let words = text.split(|c: char| !(c.is_alphanumeric() || c == '_'));
    words
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Where a word stands, as [`Find`] sees it.
#[derive(Clone, Copy, PartialEq)]
enum Place {
    /// Outside function bodies.
    Item,
    /// A constant's value: where the constant is used tells.
    Value,
    /// A non-`const` function's body, which a patch carries.
    Body,
    /// `main`'s body, or a `Layer` impl's `load` or `restored`.
    Startup,
    /// A `Layer` impl's `attach`.
    Attach,
}

/// What [`Find`] looks for the uses of.
#[derive(Clone, Copy)]
enum Kind {
    /// Edited constants, whose uses in a type rebuild even in a body.
    Const,
    /// Names changed `use`s bring in, which a body may use anywhere.
    Use,
}

/// Finds the first use of a name a patch can't carry, and the uses in `attach`.
struct Find {
    /// Each name sought, with the index of the edited constant it reaches.
    names: BTreeMap<String, usize>,
    kind: Kind,
    place: Place,
    /// How many types the word is inside.
    types: u32,
    /// Whether the impl being visited is one of `Layer`.
    layer: bool,
    attach: Vec<usize>,
    found: Option<(usize, &'static str)>,
}

impl Find {
    fn word(&mut self, word: &str) {
        let Some(&at) = self.names.get(word) else {
            return;
        };
        let why = match (self.kind, self.place) {
            (Kind::Const, _) if self.types > 0 => IN_A_TYPE,
            (Kind::Const, Place::Item) => OUTSIDE_BODIES,
            (Kind::Const, Place::Startup) => AT_START,
            (Kind::Use, Place::Item | Place::Value) => USED_OUTSIDE,
            (Kind::Use, Place::Startup) => USED_AT_START,
            (_, Place::Attach) => {
                self.attach.push(at);
                return;
            }
            (_, Place::Value | Place::Body) => return,
        };
        self.found.get_or_insert((at, why));
    }

    /// Visits with `place` as the place.
    fn within(&mut self, place: Place, visit: impl FnOnce(&mut Self)) {
        let outer = std::mem::replace(&mut self.place, place);
        visit(self);
        self.place = outer;
    }

    /// Visits a constant's type and value, but not its name.
    fn constant(&mut self, ty: &mut Type, value: Option<&mut Expr>) {
        self.visit_type_mut(ty);
        if let Some(value) = value {
            self.within(Place::Value, |find| find.visit_expr_mut(value));
        }
    }
}

impl VisitMut for Find {
    fn visit_item_mut(&mut self, item: &mut Item) {
        self.within(Place::Item, |find| visit_mut::visit_item_mut(find, item));
    }

    /// A `use` names what it brings in; [`Links`] follows a rename.
    fn visit_item_use_mut(&mut self, _: &mut ItemUse) {}

    fn visit_item_const_mut(&mut self, item: &mut ItemConst) {
        self.visit_generics_mut(&mut item.generics);
        self.constant(&mut item.ty, Some(&mut item.expr));
    }

    fn visit_impl_item_const_mut(&mut self, item: &mut ImplItemConst) {
        self.visit_generics_mut(&mut item.generics);
        self.constant(&mut item.ty, Some(&mut item.expr));
    }

    fn visit_trait_item_const_mut(&mut self, item: &mut TraitItemConst) {
        self.visit_generics_mut(&mut item.generics);
        let value = item.default.as_mut().map(|(_, value)| value);
        self.constant(&mut item.ty, value);
    }

    fn visit_item_fn_mut(&mut self, item: &mut ItemFn) {
        self.visit_signature_mut(&mut item.sig);
        let place = match () {
            _ if item.sig.constness.is_some() => Place::Item,
            _ if item.sig.ident == "main" => Place::Startup,
            _ => Place::Body,
        };
        self.within(place, |find| find.visit_block_mut(&mut item.block));
    }

    fn visit_item_impl_mut(&mut self, item: &mut ItemImpl) {
        let outer = std::mem::replace(&mut self.layer, layer(item));
        visit_mut::visit_item_impl_mut(self, item);
        self.layer = outer;
    }

    fn visit_impl_item_fn_mut(&mut self, item: &mut ImplItemFn) {
        self.visit_signature_mut(&mut item.sig);
        let name = &item.sig.ident;
        let place = match () {
            _ if item.sig.constness.is_some() => Place::Item,
            _ if self.layer && (name == "load" || name == "restored") => Place::Startup,
            _ if self.layer && name == "attach" => Place::Attach,
            _ => Place::Body,
        };
        self.within(place, |find| find.visit_block_mut(&mut item.block));
    }

    fn visit_trait_item_fn_mut(&mut self, item: &mut TraitItemFn) {
        self.visit_signature_mut(&mut item.sig);
        let place = match item.sig.constness {
            Some(_) => Place::Item,
            None => Place::Body,
        };
        if let Some(body) = &mut item.default {
            self.within(place, |find| find.visit_block_mut(body));
        }
    }

    fn visit_type_mut(&mut self, ty: &mut Type) {
        self.types += 1;
        visit_mut::visit_type_mut(self, ty);
        self.types -= 1;
    }

    fn visit_generic_argument_mut(&mut self, arg: &mut GenericArgument) {
        self.types += 1;
        visit_mut::visit_generic_argument_mut(self, arg);
        self.types -= 1;
    }

    fn visit_ident_mut(&mut self, ident: &mut Ident) {
        self.word(&ident.to_string());
    }

    fn visit_macro_mut(&mut self, mac: &mut Macro) {
        for word in tokens(&mac.tokens) {
            self.word(&word);
        }
    }

    /// An attribute's arguments, as the names a derive takes.
    fn visit_meta_list_mut(&mut self, list: &mut MetaList) {
        visit_mut::visit_meta_list_mut(self, list);
        for word in tokens(&list.tokens) {
            self.word(&word);
        }
    }
}

/// Whether `item` is an impl of `Layer`.
fn layer(item: &ItemImpl) -> bool {
    item.trait_.as_ref().is_some_and(|(path, _)| {
        path.segments
            .last()
            .is_some_and(|last| last.ident == "Layer")
    })
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

/// `file` without doc comments (`#[doc]` attributes), `#[cfg(test)]` items, `use`s, the functions
/// [`free`] lets come and go, constants' values and non-`const` function bodies, but for the items
/// declared in them and the bodies `keep` keeps.
fn bare(mut file: File, keep: Keep) -> Bared {
    untested(&mut file);
    let mut bare = Bare {
        keep,
        layer: false,
        values: Vec::new(),
    };
    bare.visit_file_mut(&mut file);
    let mut free = Free(Vec::new());
    free.visit_file_mut(&mut file);
    Bared {
        file,
        values: bare.values,
        uses: free.0,
    }
}

/// A module as [`bare`] leaves it.
struct Bared {
    file: File,
    /// Each constant's name and value, in the order the tree holds them.
    values: Vec<(String, Expr)>,
    /// What its `use`s bring in.
    uses: Vec<Binding>,
}

/// A name a `use` brings in.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Binding {
    /// Whether it's more than private, so other modules may import through it.
    public: bool,
    /// `None` for a glob's; `_` for a trait's brought in for its methods alone.
    name: Option<String>,
    path: String,
}

/// Collects into `into` what `item` brings in.
fn bindings(item: &ItemUse, into: &mut Vec<Binding>) {
    fn walk(tree: &UseTree, path: String, public: bool, into: &mut Vec<Binding>) {
        let mut bind = |name: Option<String>, path: String| {
            into.push(Binding { public, name, path });
        };
        match tree {
            UseTree::Path(step) => {
                walk(&step.tree, format!("{path}{}::", step.ident), public, into)
            }
            UseTree::Name(leaf) if leaf.ident == "self" => {
                let parent = path.trim_end_matches("::").rsplit("::").next();
                bind(parent.map(str::to_owned), format!("{path}self"));
            }
            UseTree::Name(leaf) => bind(
                Some(leaf.ident.to_string()),
                format!("{path}{}", leaf.ident),
            ),
            UseTree::Rename(leaf) => {
                bind(
                    Some(leaf.rename.to_string()),
                    format!("{path}{}", leaf.ident),
                );
            }
            UseTree::Glob(_) => bind(None, path),
            UseTree::Group(group) => {
                for tree in &group.items {
                    walk(tree, path.clone(), public, into);
                }
            }
        }
    }
    let public = !matches!(item.vis, Visibility::Inherited);
    let root = if item.leading_colon.is_some() {
        "::"
    } else {
        ""
    };
    walk(&item.tree, root.to_owned(), public, into);
}

/// Takes `use`s out of a tree, collecting what they bring in, and the functions [`free`] lets come
/// and go, with the inherent impls they leave empty.
struct Free(Vec<Binding>);

impl Free {
    /// Whether `item` stays in the tree.
    fn stays(&mut self, item: &mut Item) -> bool {
        match item {
            Item::Use(item) => {
                bindings(item, &mut self.0);
                false
            }
            Item::Fn(item) => !free(&item.attrs, &item.sig, &mut item.block),
            Item::Impl(item) => {
                !(item.trait_.is_none() && item.items.is_empty() && plain(&item.attrs))
            }
            _ => true,
        }
    }
}

impl VisitMut for Free {
    fn visit_file_mut(&mut self, file: &mut File) {
        visit_mut::visit_file_mut(self, file);
        file.items.retain_mut(|item| self.stays(item));
    }

    fn visit_item_mod_mut(&mut self, item: &mut ItemMod) {
        visit_mut::visit_item_mod_mut(self, item);
        if let Some((_, items)) = &mut item.content {
            items.retain_mut(|item| self.stays(item));
        }
    }

    fn visit_block_mut(&mut self, block: &mut Block) {
        visit_mut::visit_block_mut(self, block);
        block.stmts.retain_mut(|stmt| match stmt {
            Stmt::Item(item) => self.stays(item),
            _ => true,
        });
    }

    fn visit_item_impl_mut(&mut self, item: &mut ItemImpl) {
        visit_mut::visit_item_impl_mut(self, item);
        if item.trait_.is_none() && plain(&item.attrs) {
            item.items.retain_mut(|item| match item {
                ImplItem::Fn(item) => !free(&item.attrs, &item.sig, &mut item.block),
                _ => true,
            });
        }
    }
}

/// Whether a patch may add, remove or re-sign the function, free or an inherent impl's. A patch
/// recompiles all its callers in the game's crate. `HotFn`, the one way into patched code, calls
/// only `Layer` methods, whose trait fixes them. Not if it's:
/// - a `const fn`, which the build may evaluate into a type or a static;
/// - `main`, or of another ABI, which code outside the crate calls;
/// - given an attribute that may add code or change its symbol, as a macro's or `#[no_mangle]`;
/// - declaring items, as the running game may hold values of a type among them.
fn free(attrs: &[Attribute], sig: &Signature, block: &mut Block) -> bool {
    sig.constness.is_none()
        && sig.abi.is_none()
        && sig.ident != "main"
        && plain(attrs)
        && !declares(block)
}

/// Whether `attrs` only document, configure, hint or lint.
fn plain(attrs: &[Attribute]) -> bool {
    const PLAIN: [&str; 12] = [
        "allow",
        "cfg",
        "cold",
        "deny",
        "deprecated",
        "doc",
        "expect",
        "forbid",
        "inline",
        "must_use",
        "track_caller",
        "warn",
    ];
    attrs.iter().all(|attr| {
        let path = attr.path();
        let first = path.segments.first().map(|first| first.ident.to_string());
        let tool =
            path.segments.len() > 1 && matches!(first.as_deref(), Some("clippy" | "rustfmt"));
        tool || PLAIN.iter().any(|name| path.is_ident(name))
    })
}

/// Whether `block` declares an item, however deep, but for test code.
fn declares(block: &mut Block) -> bool {
    struct Declares(bool);
    impl VisitMut for Declares {
        fn visit_item_mut(&mut self, item: &mut Item) {
            self.0 |= !tested(item);
        }
    }
    let mut declares = Declares(false);
    declares.visit_block_mut(block);
    declares.0
}

/// Empties function bodies (free, methods, default methods) but for `const fn`s' (the build may
/// evaluate them into a type or a static) and those `keep` keeps. Keeps items declared in a body,
/// however deep: the running game may hold values of a type among them. Drops doc comments. Items
/// a macro declares in a body (`thread_local!`) go with it. Takes out constants' values, which
/// [`uses`] judges.
struct Bare {
    keep: Keep,
    /// Whether the impl being visited is one of `Layer`.
    layer: bool,
    values: Vec<(String, Expr)>,
}

impl Bare {
    fn take(&mut self, name: &Ident, value: &mut Expr) {
        let infer = Expr::Infer(ExprInfer {
            attrs: Vec::new(),
            underscore_token: Default::default(),
        });
        let value = std::mem::replace(value, infer);
        self.values.push((name.to_string(), value));
    }
}

impl VisitMut for Bare {
    fn visit_attributes_mut(&mut self, attrs: &mut Vec<Attribute>) {
        attrs.retain(|attr| !attr.path().is_ident("doc"));
    }

    fn visit_item_const_mut(&mut self, item: &mut ItemConst) {
        self.take(&item.ident, &mut item.expr);
        visit_mut::visit_item_const_mut(self, item);
    }

    fn visit_impl_item_const_mut(&mut self, item: &mut ImplItemConst) {
        self.take(&item.ident, &mut item.expr);
        visit_mut::visit_impl_item_const_mut(self, item);
    }

    fn visit_trait_item_const_mut(&mut self, item: &mut TraitItemConst) {
        if let Some((_, value)) = &mut item.default {
            self.take(&item.ident, value);
        }
        visit_mut::visit_trait_item_const_mut(self, item);
    }

    fn visit_item_fn_mut(&mut self, item: &mut ItemFn) {
        let kept = self.keep == Keep::Startup && item.sig.ident == "main";
        if item.sig.constness.is_none() && !kept {
            keep_items(&mut item.block);
        }
        visit_mut::visit_item_fn_mut(self, item);
    }

    fn visit_item_impl_mut(&mut self, item: &mut ItemImpl) {
        let outer = std::mem::replace(&mut self.layer, layer(item));
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

    /// An edit inside function bodies, but for constants.
    fn bodies(attach: bool) -> Edit {
        Edit::Bodies {
            attach,
            consts: Vec::new(),
        }
    }

    /// Constants: free, an impl's, a trait's default, one declared in a body, and one an array's
    /// length uses.
    const CONSTS: &str = r#"
const SPEED: f32 = 2.0;
const LANES: usize = 3;

struct Road {
    lanes: [f32; LANES],
}

impl Ship {
    const TURN: f32 = 0.5;
}

trait Grip {
    const GRIP: f32 = 0.9;
}

fn steer() -> f32 {
    const DRAG: f32 = 0.1;
    DRAG * SPEED
}
"#;

    #[test]
    fn edits_to_constants_values_name_them() {
        for (from, to, consts) in [
            ("2.0", "3.5", &["SPEED"][..]),
            ("0.5", "0.5 * 2.0", &["TURN"]),
            ("0.9", "0.8", &["GRIP"]),
            ("0.1", "0.2", &["DRAG"]),
            ("= 3;", "= 4;", &["LANES"]),
        ] {
            assert_eq!(CONSTS.matches(from).count(), 1, "{from}");
            let edit = classify(CONSTS, &CONSTS.replace(from, to));
            let consts = consts.iter().map(|name| name.to_string()).collect();
            let named = Edit::Bodies {
                attach: false,
                consts,
            };
            assert_eq!(edit, named, "{from} -> {to}");
        }
        // Two at once, beside an edit inside a function body, sorted.
        let both = CONSTS.replace("2.0", "1.0").replace("0.1", "0.3");
        let both = both.replace("DRAG * SPEED", "DRAG * SPEED * 2.0");
        let named = Edit::Bodies {
            attach: false,
            consts: vec!["DRAG".into(), "SPEED".into()],
        };
        assert_eq!(classify(CONSTS, &both), named);
        // A constant's type, name or visibility is outside function bodies; so is one added.
        for (from, to) in [
            ("SPEED: f32", "SPEED: f64"),
            ("const TURN", "const TURNING"),
            ("const SPEED", "pub const SPEED"),
            (
                "    const DRAG",
                "    const PULL: f32 = 1.0;\n    const DRAG",
            ),
        ] {
            assert_eq!(CONSTS.matches(from).count(), 1, "{from}");
            let edit = classify(CONSTS, &CONSTS.replace(from, to));
            let outside = Edit::Rebuild("an edit outside function bodies");
            assert_eq!(edit, outside, "{from} -> {to}");
        }
        // One in code run as the game starts.
        let main = "fn main() {\n    const TITLE: &str = \"Ships\";\n    run(TITLE);\n}\n";
        let edit = classify(main, &main.replace("Ships", "Boats"));
        let titled = Edit::Bodies {
            attach: false,
            consts: vec!["TITLE".into()],
        };
        assert_eq!(edit, titled);
    }

    /// Modules using `SPEED`, edited, and `HALF`, which its value names.
    const USED: &str = "pub const SPEED: f32 = 2.0;\npub const HALF: f32 = SPEED / 2.0;\n";

    #[test]
    fn an_edited_constant_patches_where_function_bodies_alone_use_it() {
        let speed = ["SPEED".to_string()];
        let patchable = [
            // Function bodies, a macro's tokens among them, and through a `use`'s rename.
            "use crate::consts::HALF as H;\n\nfn steer(x: &mut f32) {\n    *x += H * 2.0;\n    \
             println!(\"{}\", SPEED);\n}\n",
            // A repeat's length in a body, a pattern, a closure and a default method.
            "fn lanes() -> usize {\n    let lanes = [0.0; SPEED as usize];\n    match 1.0 {\n        \
             SPEED => 1,\n        _ => (|| HALF as usize)(),\n    }\n}\n",
            "trait Turn {\n    fn turn(&self) -> f32 {\n        SPEED\n    }\n}\n",
            // Not uses: a `use`, a doc comment and a word in a string. Uses a patch carries: in
            // a `load` of an impl not of `Layer`, and in test code, which the game doesn't hold.
            "/// Twice SPEED.\nuse crate::consts::SPEED;\n\nfn speed() -> &'static str {\n    \
             \"HALF\"\n}\n",
            "impl Ship {\n    fn load(&self) -> f32 {\n        SPEED\n    }\n}\n",
            "#[cfg(test)]\nstatic TOP: f32 = SPEED;\n\n#[cfg(test)]\nmod tests {\n    \
             struct Lanes([f32; SPEED as usize]);\n}\n",
        ];
        for module in patchable {
            assert_eq!(
                uses(&[USED, module], &speed),
                Uses::Bodies(Vec::new()),
                "{module}"
            );
        }
        // A layer's `attach`, told of by the edited constant's index.
        let attach = "impl Layer for Game {\n    fn attach(&mut self) {\n        spawn(HALF);\n    \
                      }\n}\n";
        let both = ["LANES".to_string(), "SPEED".to_string()];
        assert_eq!(uses(&[USED, attach], &both), Uses::Bodies(vec![1]));
        for (module, why) in [
            // In a type, outside function bodies or in one.
            (
                "struct Road {\n    lanes: [f32; HALF as usize],\n}\n",
                IN_A_TYPE,
            ),
            (
                "fn spawn() {\n    let lanes: [f32; SPEED as usize] = todo!();\n}\n",
                IN_A_TYPE,
            ),
            (
                "fn grid() -> usize {\n    size::<Grid<{ H }>>()\n}\n",
                IN_A_TYPE,
            ),
            ("fn steer(by: [f32; SPEED as usize]) {}\n", IN_A_TYPE),
            // Outside function bodies.
            ("static TOP: f32 = SPEED;\n", OUTSIDE_BODIES),
            (
                "enum Lane {\n    Left = SPEED as isize,\n}\n",
                OUTSIDE_BODIES,
            ),
            ("const fn top() -> f32 {\n    HALF\n}\n", OUTSIDE_BODIES),
            (
                "thread_local! {\n    static TOP: f32 = SPEED;\n}\n",
                OUTSIDE_BODIES,
            ),
            (
                "fn steer() {\n    static TOP: f32 = SPEED;\n}\n",
                OUTSIDE_BODIES,
            ),
            // In code run as the game starts.
            ("fn main() {\n    run(SPEED);\n}\n", AT_START),
            (
                "impl Layer for Game {\n    fn load(&mut self) {\n        speed(HALF);\n    }\n}\n",
                AT_START,
            ),
        ] {
            let module = format!("use crate::consts::HALF as H;\n\n{module}");
            assert_eq!(
                uses(&[USED, &module], &speed),
                Uses::Rebuild(0, why),
                "{module}"
            );
        }
        // The edited constant named in the rebuild is the one the use reaches.
        let typed = "struct Road {\n    lanes: [f32; HALF as usize],\n}\n";
        assert_eq!(uses(&[USED, typed], &both), Uses::Rebuild(1, IN_A_TYPE));
        // A module that doesn't parse could hide a use.
        assert_eq!(uses(&[USED, "fn {"], &speed), Uses::Rebuild(0, UNREAD));
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
            assert_eq!(edit, bodies(false), "{from} -> {to}");
        }
    }

    /// An edit inside a `Layer`'s `attach` is patched and told of, beside other edits too. One to
    /// another impl's method of that name isn't told of.
    #[test]
    fn edits_inside_a_layers_attach_are_patched_and_told_of() {
        let attached = edited("Ship { x: 0.0 }", "Ship { x: 5.0 }");
        assert_eq!(classify(MODULE, &attached), bodies(true));
        let beside = attached.replace("self.x += 1.0;", "self.x += 2.0;");
        assert_eq!(classify(MODULE, &beside), bodies(true));
        let other = classify(MODULE, &edited("self.x = 1.0;", "self.x = 4.0;"));
        assert_eq!(other, bodies(false));
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
        assert_eq!(classify(&tested, &beside), bodies(false));
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
    fn functions_and_uses_patch_as_they_come_go_and_change() {
        for (from, to) in [
            // A function re-signed, made public, added or removed.
            (
                "fn steer(ship: &mut Ship, by: f32)",
                "fn steer(ship: &mut Ship, by: f64)",
            ),
            ("fn steer(", "pub(crate) fn steer("),
            (
                "impl Turn for Ship {}",
                "impl Turn for Ship {}\n\nfn dock(ship: &mut Ship) -> Duration {\n    \
                 ship.x = 0.0;\n    Duration::ZERO\n}",
            ),
            (
                "fn steer(ship: &mut Ship, by: f32) {\n    // Right is positive.\n    \
                 ship.x += by * SPEED;\n}\n",
                "",
            ),
            // Methods: given hints, added, and in an impl of their own.
            (
                "    fn stop(&mut self) {",
                "    #[inline]\n    #[allow(dead_code)]\n    fn stop(&mut self) {",
            ),
            (
                "    fn stop(&mut self) {",
                "    fn dock(&mut self) {}\n\n    fn stop(&mut self) {",
            ),
            (
                "impl Turn for Ship {}",
                "impl Turn for Ship {}\n\nimpl Ship {\n    fn dock(&self) {}\n}",
            ),
            // One declared in a body.
            (
                "    count(&Burst { count: 3 })",
                "    fn twice(n: u32) -> u32 {\n        n * 2\n    }\n    \
                 twice(count(&Burst { count: 3 }))",
            ),
            // A `use` added, removed, in a body, and of a trait for its methods.
            (
                "use std::time::Duration;",
                "use std::time::{Duration, Instant};",
            ),
            ("use std::time::Duration;\n", ""),
            (
                "self.x += 1.0;",
                "use std::f32::consts::PI;\n        self.x += PI;",
            ),
            (
                "use std::time::Duration;",
                "use std::time::Duration;\nuse std::fmt::Write as _;",
            ),
        ] {
            let edit = classify(MODULE, &edited(from, to));
            assert_eq!(edit, bodies(false), "{from} -> {to}");
        }
        // A function added with the `use` its signature needs.
        let since = edited(
            "use std::time::Duration;",
            "use std::time::{Duration, Instant};\n\nfn since(at: Instant) -> Duration {\n    \
             at.elapsed()\n}",
        );
        assert_eq!(classify(MODULE, &since), bodies(false));
    }

    #[test]
    fn a_changed_use_rebuilds_where_code_a_patch_doesnt_carry_uses_its_names() {
        // Each module's `use` changes from `gg` to `other`.
        let changed = |module: &str| classify(module, &module.replace("gg::", "other::"));
        for (module, why) in [
            // In a type, a derive, a `Layer` method's signature and a constant's value.
            ("use gg::Wait;\n\nstruct Ship(Wait);\n", USED_OUTSIDE),
            (
                "use gg::Component;\n\n#[derive(Component)]\nstruct Ship;\n",
                USED_OUTSIDE,
            ),
            (
                "use gg::Context;\n\nimpl Layer for Game {\n    fn tick(&mut self, cx: &mut \
                 Context) {}\n}\n",
                USED_OUTSIDE,
            ),
            (
                "use gg::LANES;\n\nconst ROAD: usize = LANES * 2;\n",
                USED_OUTSIDE,
            ),
            // In code run as the game starts.
            (
                "use gg::App;\n\nfn main() {\n    App::new().run();\n}\n",
                USED_AT_START,
            ),
            // One other modules may import through, or a glob's, whose names gg can't tell.
            ("pub use gg::Wait;\n", PUBLIC_USE),
            ("pub(crate) use gg::Wait;\n", PUBLIC_USE),
            ("use gg::*;\n", GLOB),
        ] {
            assert_eq!(changed(module), Edit::Rebuild(why), "{module}");
        }
        // One of two alike, removed alone.
        let twice = "use gg::Wait;\n\nstruct Ship(Wait);\n\nfn wait() {\n    use gg::Wait;\n}\n";
        let once = twice.replacen("use gg::Wait;\n", "", 1);
        assert_eq!(classify(twice, &once), Edit::Rebuild(USED_OUTSIDE));
        // Function bodies alone, types in them and functions' signatures among them; `attach`, told
        // of.
        let aim =
            "use gg::Vec2;\n\nfn aim() -> Vec2 {\n    let at: Vec2 = Vec2::ZERO;\n    at\n}\n";
        assert_eq!(changed(aim), bodies(false));
        let attach = "use gg::spawn;\n\nimpl Layer for Game {\n    fn attach(&mut self) {\n        spawn();\n    \
             }\n}\n";
        assert_eq!(changed(attach), bodies(true));
    }

    #[test]
    fn edits_outside_function_bodies_need_a_rebuild() {
        for (from, to) in [
            ("    x: f32,", "    x: f32,\n    y: f32,"),
            ("    3\n", "    4\n"),
            ("static SPEED: f32 = 1.0;", "static SPEED: f32 = 2.0;"),
            // A type declared in a body: the running game may hold its values.
            (
                "        count: u32,",
                "        count: u32,\n        size: f32,",
            ),
            // Functions a patch can't add, remove or re-sign: `const`, of another ABI, given an
            // attribute that may add code or change a symbol, or declaring a type.
            ("fn steer(", "const fn steer("),
            ("fn steer(", "extern \"C\" fn steer("),
            ("fn steer(", "#[no_mangle]\nfn steer("),
            (
                "impl Turn for Ship {}",
                "impl Turn for Ship {}\n\n#[system]\nfn dock() {}",
            ),
            (
                "impl Turn for Ship {}",
                "impl Turn for Ship {}\n\nfn dock() {\n    struct Bay;\n}",
            ),
            ("impl Ship {", "#[methods]\nimpl Ship {"),
            // A method added to an impl of a trait, a `Layer`'s too.
            (
                "impl Turn for Ship {}",
                "impl Turn for Ship {\n    fn turn(&mut self) {}\n}",
            ),
            (
                "    fn tick(&mut self",
                "    fn detach(&mut self, cx: &mut Context) {}\n\n    fn tick(&mut self",
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
