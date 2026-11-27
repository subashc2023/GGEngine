//! Where a game's functions run from, as names reach them from what the engine calls: as the game
//! starts (`main`, a layer's `load` and `restored`), as a layer's `attach` builds the world, or as
//! the game runs. A patch reaches code only as the game runs, through a layer's methods, which the
//! engine calls through subsecond's `HotFn`. So `gg run` rebuilds for an edit to code only startup
//! reaches, and tells of one to code only `attach` reaches.
//!
//! Calls are followed by name, as rustc's resolution isn't at hand:
//! - `step` names a free function, through a `use`'s rename too;
//! - `Game::new`, and `Self::new` in `Game`'s impls, name the method;
//! - `ship::sail` names any free `sail`, and `x.fire()` every method `fire`;
//! - a macro's arguments are read as expressions; otherwise each word of its tokens, as of an
//!   attribute's, names every function of that name.
//!
//! Every mention counts, as a function taken as a value may be called later. A name outside
//! function bodies, as in a static, reaches code as the game runs. So does a trait impl's method
//! but `Layer`'s, as code calls them without naming them (`+`, `{}`, `for`). Code nothing names is
//! taken to run as the game runs too: a trait object, a function pointer or the engine may call it.
//! A name that matches too much leaves code reached as the game runs, which a patch carries.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use syn::punctuated::Punctuated;
use syn::visit_mut::{self, VisitMut};
use syn::{
    Attribute, Expr, ExprMethodCall, ExprPath, File, Ident, ImplItemConst, ImplItemFn, ItemConst,
    ItemFn, ItemImpl, ItemTrait, ItemUse, Macro, MetaList, Signature, Token, TraitItemConst,
    TraitItemFn, Type,
};

use crate::classify::{self, Binding};

/// Runs as the game starts: `main`, a layer's `load` and `restored`.
const STARTUP: u8 = 1;
/// Runs as a layer's `attach` builds the world, which only a start afresh does.
const ATTACH: u8 = 2;
/// Runs as the game runs, where a patch reaches.
const RUNTIME: u8 = 4;

/// A function, by its module and name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Code {
    pub(crate) module: PathBuf,
    /// As a path in its module names it: `step`, `Game::new`, `<Game as Layer>::attach`. Functions
    /// of one name in a module, as two declared in bodies, are one.
    pub(crate) name: String,
}

/// What a function holds, as a hash, and what reaches it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Print {
    /// Of its text but for doc comments, of the values of the constants it names, however deep,
    /// and of what its module's `use`s bring in under the names it holds.
    hash: u64,
    /// Which of [`STARTUP`], [`ATTACH`] and [`RUNTIME`] reach it.
    from: u8,
}

/// The game's functions' prints, as [`prints`] finds them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Prints(BTreeMap<Code, Print>);

impl Prints {
    /// The first function only startup reaches whose print differs from `built`'s: one an edit
    /// changed that a patch would come too late for.
    pub(crate) fn started<'a>(&'a self, built: &'a Prints) -> Option<&'a Code> {
        self.differ(STARTUP, built).next()
    }

    /// The functions only `attach` reaches, or it and startup, whose prints differ from `world`'s,
    /// the build's that built the world, and from `told`'s, which they then join.
    pub(crate) fn attached(&self, world: &Prints, told: &mut BTreeMap<Code, u64>) -> Vec<Code> {
        let new: Vec<Code> = (self.differ(ATTACH, world))
            .filter(|code| told.get(*code) != Some(&self.0[*code].hash))
            .cloned()
            .collect();
        for code in &new {
            told.insert(code.clone(), self.0[code].hash);
        }
        new
    }

    /// The functions `from` reaches, but nothing as the game runs, whose prints differ from
    /// `other`'s.
    fn differ<'a>(&'a self, from: u8, other: &'a Prints) -> impl Iterator<Item = &'a Code> {
        self.0.iter().filter_map(move |(code, print)| {
            let differs = print.from & from != 0
                && print.from & RUNTIME == 0
                && other.0.get(code).map(|other| other.hash) != Some(print.hash);
            differs.then_some(code)
        })
    }
}

/// The prints of the functions of a game's `modules`, each its path and text: the modules of its
/// binary, but for its test code. `None` if a text doesn't parse, as it could hide a call.
pub(crate) fn prints(modules: &[(&Path, &str)]) -> Option<Prints> {
    let mut collect = Collect::default();
    for (path, text) in modules {
        let mut file = syn::parse_file(text).ok()?;
        classify::untested(&mut file);
        Undoc.visit_file_mut(&mut file);
        let mut uses = Uses(Vec::new());
        uses.visit_file_mut(&mut file);
        collect.module(path, uses.0, &mut file);
    }
    Some(collect.prints())
}

/// A function as [`Collect`] finds it.
struct Def {
    code: Code,
    callee: Callee,
    /// What reaches it, whatever names it: its role, if the engine calls it, or [`RUNTIME`] if
    /// code may call it unnamed.
    from: u8,
    /// Of its text.
    hash: u64,
    /// The names its body mentions.
    names: Vec<Name>,
    /// The words it holds, its macros' and literals' too: what may name a constant or a `use`.
    words: BTreeSet<String>,
    /// Its module, by index in [`Collect::uses`].
    module: usize,
}

/// How calls name a function.
enum Callee {
    Free(String),
    /// A method, of the type by its last name if gg can tell it.
    Method(Option<String>, String),
}

/// A mention of a name that may be a function's.
enum Name {
    /// `step`: a free function.
    Free(String),
    /// `Game::new`, `ship::sail`: a method of the type, or a free function in a module.
    Path(String, String),
    /// `x.fire()`, `<T as Fire>::fire`: any method.
    Method(String),
    /// A word of a macro's or an attribute's tokens: any function.
    Word(String),
}

/// Collects the functions of the modules it visits.
#[derive(Default)]
struct Collect {
    defs: Vec<Def>,
    /// The names mentioned outside function bodies, as in a static: they run as the game runs.
    elsewhere: Vec<Name>,
    /// Each constant's name, the hash of its value and the words its value holds.
    consts: Vec<(String, u64, Vec<String>)>,
    /// What each module's `use`s bring in, under each name.
    uses: Vec<HashMap<String, Vec<String>>>,
    /// The module being visited.
    module: PathBuf,
    /// The names its `use`s rename, with the names they rename.
    renames: HashMap<String, String>,
    /// The functions being visited, innermost last, by index in `defs`.
    open: Vec<usize>,
    /// The impl being visited: its type's last name, if gg can tell it, and its trait's.
    imp: Option<(Option<String>, Option<String>)>,
    /// The trait being visited.
    trait_name: Option<String>,
}

impl Collect {
    /// Collects the functions of `file`, at `path`, whose `use`s bring in `bindings`.
    fn module(&mut self, path: &Path, bindings: Vec<Binding>, file: &mut File) {
        self.module = path.to_owned();
        self.renames.clear();
        let mut uses: HashMap<String, Vec<String>> = HashMap::new();
        for binding in bindings {
            let Some(name) = binding.name else {
                continue;
            };
            let last = binding.path.rsplit("::").next().unwrap_or_default();
            if last != name && last != "self" {
                self.renames.insert(name.clone(), last.to_owned());
            }
            uses.entry(name).or_default().push(binding.path);
        }
        self.uses.push(uses);
        self.visit_file_mut(file);
    }

    /// Visits a function's body, as `visit` does, collecting what it names.
    fn def(
        &mut self,
        name: String,
        callee: Callee,
        from: u8,
        hash: u64,
        visit: impl FnOnce(&mut Self),
    ) {
        self.defs.push(Def {
            code: Code {
                module: self.module.clone(),
                name,
            },
            callee,
            from,
            hash,
            names: Vec::new(),
            words: BTreeSet::new(),
            module: self.uses.len() - 1,
        });
        self.open.push(self.defs.len() - 1);
        visit(self);
        self.open.pop();
    }

    fn name(&mut self, name: Name) {
        match self.open.last() {
            Some(&def) => self.defs[def].names.push(name),
            None => self.elsewhere.push(name),
        }
    }

    fn words(&mut self, words: impl IntoIterator<Item = String>) {
        if let Some(&def) = self.open.last() {
            self.defs[def].words.extend(words);
        }
    }

    /// `name`, and the name it renames if a `use` renames it.
    fn renamed(&self, name: &str) -> Vec<String> {
        let renamed = self.renames.get(name).cloned();
        [Some(name.to_owned()), renamed]
            .into_iter()
            .flatten()
            .collect()
    }

    fn constant(&mut self, name: &Ident, value: &mut Expr) {
        let hash = hash_of(&*value);
        let words = classify::words(value);
        self.consts.push((name.to_string(), hash, words));
    }

    /// What reaches each function, by index in `defs`.
    fn reach(&self) -> Vec<u8> {
        let mut free: HashMap<&str, Vec<usize>> = HashMap::new();
        let mut methods: HashMap<&str, Vec<(Option<&str>, usize)>> = HashMap::new();
        for (at, def) in self.defs.iter().enumerate() {
            match &def.callee {
                Callee::Free(name) => free.entry(name).or_default().push(at),
                Callee::Method(of, name) => {
                    methods.entry(name).or_default().push((of.as_deref(), at));
                }
            }
        }
        let frees = |name: &str| free.get(name).into_iter().flatten().copied();
        let methods = |name: &str| methods.get(name).into_iter().flatten().copied();
        let named = |name: &Name, into: &mut Vec<usize>| match name {
            Name::Free(name) => into.extend(frees(name)),
            Name::Path(ty, name) => {
                into.extend(frees(name));
                let of = methods(name).filter(|(of, _)| of.is_none_or(|of| of == ty));
                into.extend(of.map(|(_, at)| at));
            }
            Name::Method(name) => into.extend(methods(name).map(|(_, at)| at)),
            Name::Word(name) => {
                into.extend(frees(name));
                into.extend(methods(name).map(|(_, at)| at));
            }
        };
        let calls: Vec<Vec<usize>> = (self.defs.iter())
            .map(|def| {
                let mut calls = Vec::new();
                for name in &def.names {
                    named(name, &mut calls);
                }
                calls.sort_unstable();
                calls.dedup();
                calls
            })
            .collect();
        let mut from: Vec<u8> = self.defs.iter().map(|def| def.from).collect();
        let mut elsewhere = Vec::new();
        for name in &self.elsewhere {
            named(name, &mut elsewhere);
        }
        for at in elsewhere {
            from[at] |= RUNTIME;
        }
        spread(&calls, &mut from);
        // What nothing reaches, a trait object, a function pointer or the engine may call.
        for from in &mut from {
            if *from == 0 {
                *from = RUNTIME;
            }
        }
        spread(&calls, &mut from);
        from
    }

    fn prints(self) -> Prints {
        let from = self.reach();
        let mut consts: HashMap<&str, Vec<usize>> = HashMap::new();
        for (at, (name, ..)) in self.consts.iter().enumerate() {
            consts.entry(name).or_default().push(at);
        }
        let mut prints: BTreeMap<Code, Print> = BTreeMap::new();
        for (def, from) in self.defs.iter().zip(from) {
            let mut hasher = DefaultHasher::new();
            def.hash.hash(&mut hasher);
            // The constants it names, and those their values name, however deep.
            let mut named = BTreeSet::new();
            let mut words: Vec<&str> = def.words.iter().map(String::as_str).collect();
            while let Some(word) = words.pop() {
                for &at in consts.get(word).into_iter().flatten() {
                    if named.insert(at) {
                        words.extend(self.consts[at].2.iter().map(String::as_str));
                    }
                }
            }
            for at in named {
                (&self.consts[at].0, self.consts[at].1).hash(&mut hasher);
            }
            let uses = &self.uses[def.module];
            for word in &def.words {
                if let Some(paths) = uses.get(word) {
                    (word, paths).hash(&mut hasher);
                }
            }
            let print = Print {
                hash: hasher.finish(),
                from,
            };
            prints
                .entry(def.code.clone())
                .and_modify(|was| {
                    was.hash = hash_of(&(was.hash, print.hash));
                    was.from |= print.from;
                })
                .or_insert(print);
        }
        Prints(prints)
    }
}

/// Spreads what reaches each function to those it names, `calls` by index, until nothing changes.
fn spread(calls: &[Vec<usize>], from: &mut [u8]) {
    loop {
        let mut changed = false;
        for (caller, callees) in calls.iter().enumerate() {
            let reach = from[caller];
            for &callee in callees {
                if from[callee] | reach != from[callee] {
                    from[callee] |= reach;
                    changed = true;
                }
            }
        }
        if !changed {
            return;
        }
    }
}

impl VisitMut for Collect {
    fn visit_item_fn_mut(&mut self, item: &mut ItemFn) {
        let name = item.sig.ident.to_string();
        let from = match () {
            _ if name == "main" => STARTUP,
            _ if unnamed(&item.attrs, &item.sig) => RUNTIME,
            _ => 0,
        };
        let hash = hash_of(&*item);
        let callee = Callee::Free(name.clone());
        self.def(name, callee, from, hash, |collect| {
            visit_mut::visit_item_fn_mut(collect, item);
        });
    }

    fn visit_item_impl_mut(&mut self, item: &mut ItemImpl) {
        let of = type_name(&item.self_ty);
        let trait_name = classify::trait_name(item).map(ToString::to_string);
        let outer = self.imp.replace((of, trait_name));
        visit_mut::visit_item_impl_mut(self, item);
        self.imp = outer;
    }

    fn visit_impl_item_fn_mut(&mut self, item: &mut ImplItemFn) {
        let (of, trait_name) = self.imp.clone().unwrap_or_default();
        let method = item.sig.ident.to_string();
        let shown = of.as_deref().unwrap_or("_");
        let (name, from) = match trait_name.as_deref() {
            None => {
                let from = if unnamed(&item.attrs, &item.sig) {
                    RUNTIME
                } else {
                    0
                };
                (format!("{shown}::{method}"), from)
            }
            Some(trait_name) => {
                let from = match (trait_name, method.as_str()) {
                    ("Layer", "load" | "restored") => STARTUP,
                    ("Layer", "attach") => ATTACH,
                    _ => RUNTIME,
                };
                (format!("<{shown} as {trait_name}>::{method}"), from)
            }
        };
        let hash = hash_of(&*item);
        self.def(name, Callee::Method(of, method), from, hash, |collect| {
            visit_mut::visit_impl_item_fn_mut(collect, item);
        });
    }

    fn visit_item_trait_mut(&mut self, item: &mut ItemTrait) {
        let outer = self.trait_name.replace(item.ident.to_string());
        visit_mut::visit_item_trait_mut(self, item);
        self.trait_name = outer;
    }

    fn visit_trait_item_fn_mut(&mut self, item: &mut TraitItemFn) {
        if item.default.is_none() {
            return visit_mut::visit_trait_item_fn_mut(self, item);
        }
        let trait_name = self.trait_name.clone().unwrap_or_default();
        let method = item.sig.ident.to_string();
        let name = format!("{trait_name}::{method}");
        let hash = hash_of(&*item);
        self.def(
            name,
            Callee::Method(None, method),
            RUNTIME,
            hash,
            |collect| {
                visit_mut::visit_trait_item_fn_mut(collect, item);
            },
        );
    }

    fn visit_item_const_mut(&mut self, item: &mut ItemConst) {
        self.constant(&item.ident, &mut item.expr);
        visit_mut::visit_item_const_mut(self, item);
    }

    fn visit_impl_item_const_mut(&mut self, item: &mut ImplItemConst) {
        self.constant(&item.ident, &mut item.expr);
        visit_mut::visit_impl_item_const_mut(self, item);
    }

    fn visit_trait_item_const_mut(&mut self, item: &mut TraitItemConst) {
        if let Some((_, value)) = &mut item.default {
            self.constant(&item.ident, value);
        }
        visit_mut::visit_trait_item_const_mut(self, item);
    }

    fn visit_expr_path_mut(&mut self, expr: &mut ExprPath) {
        let segments = expr.path.segments.iter();
        let path: Vec<String> = segments.map(|segment| segment.ident.to_string()).collect();
        match (&expr.qself, path.as_slice()) {
            (Some(_), [.., last]) => self.name(Name::Method(last.clone())),
            (None, [one]) => {
                for name in self.renamed(one) {
                    self.name(Name::Free(name));
                }
            }
            (None, [.., of, last]) => {
                let of = match (of.as_str(), &self.imp) {
                    ("Self", Some((Some(own), _))) => own.clone(),
                    _ => of.clone(),
                };
                for of in self.renamed(&of) {
                    self.name(Name::Path(of, last.clone()));
                }
            }
            _ => {}
        }
        visit_mut::visit_expr_path_mut(self, expr);
    }

    fn visit_expr_method_call_mut(&mut self, call: &mut ExprMethodCall) {
        self.name(Name::Method(call.method.to_string()));
        visit_mut::visit_expr_method_call_mut(self, call);
    }

    fn visit_macro_mut(&mut self, mac: &mut Macro) {
        let words = classify::tokens(&mac.tokens);
        match mac.parse_body_with(Punctuated::<Expr, Token![,]>::parse_terminated) {
            Ok(exprs) => {
                for mut expr in exprs {
                    self.visit_expr_mut(&mut expr);
                }
            }
            Err(_) => {
                for word in &words {
                    self.name(Name::Word(word.clone()));
                }
            }
        }
        self.words(words);
    }

    fn visit_meta_list_mut(&mut self, list: &mut MetaList) {
        visit_mut::visit_meta_list_mut(self, list);
        let words = classify::tokens(&list.tokens);
        for word in &words {
            self.name(Name::Word(word.clone()));
        }
        self.words(words);
    }

    fn visit_ident_mut(&mut self, ident: &mut Ident) {
        self.words([ident.to_string()]);
    }
}

/// Whether code outside the game may call the function by its symbol, or a macro's attribute add
/// callers: of another ABI, or given an attribute that does more than document, configure, hint or
/// lint.
fn unnamed(attrs: &[Attribute], sig: &Signature) -> bool {
    sig.abi.is_some() || !classify::plain(attrs)
}

/// The last name of `ty`, through references and parentheses, if it's a path.
fn type_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(path) => (path.path.segments.last()).map(|last| last.ident.to_string()),
        Type::Reference(reference) => type_name(&reference.elem),
        Type::Paren(paren) => type_name(&paren.elem),
        Type::Group(group) => type_name(&group.elem),
        _ => None,
    }
}

/// Takes doc comments (`#[doc]` attributes) out of a tree.
struct Undoc;

impl VisitMut for Undoc {
    fn visit_attributes_mut(&mut self, attrs: &mut Vec<Attribute>) {
        attrs.retain(|attr| !attr.path().is_ident("doc"));
    }
}

/// Collects what a tree's `use`s bring in.
struct Uses(Vec<Binding>);

impl VisitMut for Uses {
    fn visit_item_use_mut(&mut self, item: &mut ItemUse) {
        classify::bindings(item, &mut self.0);
    }
}

fn hash_of(value: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A game's main module: `main` reaching `Game::new` through `app`, a layer, functions only
    /// `attach` or the running game reaches, one both do, and one nothing names.
    const GAME: &str = r#"
mod ship;

use ship::sail as go;
use ship::Lanes;

const SPEED: f32 = 2.0;
const NAME: &str = "Ships";

fn main() {
    ship::chart();
    wave();
    run(app());
}

fn app() -> App {
    let later = paint;
    println!("{}", greet());
    App::new(NAME).layer(Game::new()).on_frame(later).on_end(spawned)
}

struct Game;

impl Game {
    /// A new game.
    fn new() -> Self {
        let walk = format!("walks at {SPEED}");
        describe(walk, Lanes::count());
        Self::bind();
        Game
    }

    fn bind() {}

    fn fire(&mut self) {
        go();
    }
}

impl Layer for Game {
    fn load(&mut self, cx: &mut Context) {
        cx.add_texture(NAME);
    }

    fn attach(&mut self, cx: &mut Context) {
        walls(cx);
        both();
    }

    fn tick(&mut self, world: &mut World, tick: Tick) {
        self.fire();
        steer(SPEED);
        both();
        println!("{}", helper());
        waves!(wave => 3);
    }
}

impl Default for Game {
    fn default() -> Self {
        Self::bind();
        Self
    }
}

static WAVE: fn() = spawned;

fn walls(cx: &mut Context) {
    cx.spawn(Wall);
}

fn both() {}

fn steer(by: f32) {}

fn helper() -> u32 {
    1
}

fn wave() {}

fn spawned() {}

fn paint() {}

fn greet() -> &'static str {
    "hello"
}

fn unnamed() {}

fn describe(text: String, lanes: u32) {}
"#;

    const SHIP: &str = "pub fn sail() {}\n\npub fn chart() {}\n";

    /// The prints of `game` beside `SHIP`.
    fn game(game: &str) -> Prints {
        let modules = [
            (Path::new("src/main.rs"), game),
            (Path::new("src/ship.rs"), SHIP),
        ];
        prints(&modules).expect("prints")
    }

    /// `GAME` with `from` replaced by `to`, which it has once.
    fn edited(from: &str, to: &str) -> Prints {
        assert_eq!(GAME.matches(from).count(), 1, "{from}");
        game(&GAME.replace(from, to))
    }

    fn code(module: &str, name: &str) -> Code {
        Code {
            module: PathBuf::from(module),
            name: name.to_owned(),
        }
    }

    #[test]
    fn functions_are_reached_by_name_from_what_the_engine_calls() {
        let prints = game(GAME);
        let from = |module: &str, name: &str| prints.0[&code(module, name)].from;
        for (name, reached) in [
            // `main`, what it names however deep: by path, by `Self::`, as a value and in a macro
            // read as expressions. `load`.
            ("main", STARTUP),
            ("app", STARTUP),
            ("Game::new", STARTUP),
            ("describe", STARTUP),
            ("paint", STARTUP),
            ("greet", STARTUP),
            ("<Game as Layer>::load", STARTUP),
            // `attach`, and what it names.
            ("<Game as Layer>::attach", ATTACH),
            ("walls", ATTACH),
            ("both", ATTACH | RUNTIME),
            // A layer's other methods, and what they name: by method, in a macro read as
            // expressions, as a word in one that isn't.
            ("<Game as Layer>::tick", RUNTIME),
            ("Game::fire", RUNTIME),
            ("steer", RUNTIME),
            ("helper", RUNTIME),
            ("wave", STARTUP | RUNTIME),
            // Another trait's method, which may be called unnamed, and what it names; a static's
            // value; what nothing names.
            ("<Game as Default>::default", RUNTIME),
            ("Game::bind", STARTUP | RUNTIME),
            ("spawned", STARTUP | RUNTIME),
            ("unnamed", RUNTIME),
        ] {
            assert_eq!(from("src/main.rs", name), reached, "{name}");
        }
        // Another module's, by its module's path and through a `use`'s rename.
        assert_eq!(from("src/ship.rs", "chart"), STARTUP);
        assert_eq!(from("src/ship.rs", "sail"), RUNTIME);
    }

    #[test]
    fn an_edit_to_code_only_startup_reaches_is_found() {
        let built = game(GAME);
        let started = |prints: Prints| prints.started(&built).map(|code| code.name.clone());
        for (from, to, name) in [
            // Its body, a constant it names in a format string, a `use` it names.
            ("walks at", "runs at", "Game::new"),
            ("SPEED: f32 = 2.0", "SPEED: f32 = 2.5", "Game::new"),
            ("use ship::Lanes;", "use road::Lanes;", "Game::new"),
            (
                "fn describe(text: String, lanes: u32) {}",
                "fn describe() {}",
                "describe",
            ),
            ("fn paint() {}", "fn paint() {\n    dab();\n}", "paint"),
        ] {
            assert_eq!(started(edited(from, to)), Some(name.to_owned()), "{from}");
        }
        // Not what the running game or `attach` reaches, nor a doc comment.
        for (from, to) in [
            ("steer(SPEED);", "steer(SPEED * 2.0);"),
            ("fn bind() {}", "fn bind() {\n        tie();\n    }"),
            ("cx.spawn(Wall);", "cx.spawn(Tower);"),
            ("/// A new game.", "/// A game, new."),
        ] {
            assert_eq!(started(edited(from, to)), None, "{from}");
        }
        // Another module's.
        let ship = SHIP.replace("chart() {}", "chart() {\n    plot();\n}");
        let modules = [
            (Path::new("src/main.rs"), GAME),
            (Path::new("src/ship.rs"), ship.as_str()),
        ];
        let prints = prints(&modules).expect("prints");
        assert_eq!(prints.started(&built), Some(&code("src/ship.rs", "chart")));
    }

    #[test]
    fn code_only_attach_reaches_is_told_once_against_the_world() {
        let world = game(GAME);
        let mut told = BTreeMap::new();
        let walls = vec![code("src/main.rs", "walls")];
        let towers = edited("cx.spawn(Wall);", "cx.spawn(Tower);");
        assert_eq!(towers.attached(&world, &mut told), walls);
        assert_eq!(towers.attached(&world, &mut told), Vec::new(), "told once");
        let gates = edited("cx.spawn(Wall);", "cx.spawn(Gate);");
        assert_eq!(gates.attached(&world, &mut told), walls, "edited again");
        // As the world was built, or reached as the game runs too: nothing to tell.
        assert_eq!(world.attached(&world, &mut told), Vec::new());
        let both = edited("fn both() {}", "fn both() {\n    log();\n}");
        assert_eq!(both.attached(&world, &mut told), Vec::new());
        let attach = edited("both();\n    }\n\n    fn tick", "    }\n\n    fn tick");
        let told_of = attach.attached(&world, &mut told);
        assert_eq!(told_of, [code("src/main.rs", "<Game as Layer>::attach")]);
    }

    #[test]
    fn a_module_that_doesnt_parse_has_no_prints() {
        let modules = [
            (Path::new("src/main.rs"), GAME),
            (Path::new("src/ship.rs"), "fn {"),
        ];
        assert_eq!(prints(&modules), None);
    }
}
