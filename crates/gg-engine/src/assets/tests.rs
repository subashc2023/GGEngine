use super::*;
use crate::dirs::tests::Scratch;

/// A path inside the root is named relative to it, with `/` between parts on every platform; one
/// outside it as given.
#[test]
fn files_are_named_by_their_paths_from_the_root() {
    let root = Path::new("game");
    let inside = root.join("assets").join("logo.png");
    assert_eq!(&*name_of(Some(root), &inside), "assets/logo.png");
    let outside = Path::new("elsewhere").join("logo.png");
    assert_eq!(&*name_of(Some(root), &outside), "elsewhere/logo.png");
    assert_eq!(&*name_of(None, &inside), "game/assets/logo.png");
}

/// A file's name is its path as written, whatever case the disk keeps: a case-ignoring file system
/// keeps the case an editor last saved, which git doesn't see, so names from the disk would differ
/// between checkouts. The file itself is one however written: `.`, `..`, and on Windows `\` or
/// another case.
#[test]
fn a_file_is_named_as_its_path_is_written() {
    let scratch = Scratch::new("named-as-written");
    let root = scratch.0.join("game");
    fs::create_dir_all(root.join("assets")).expect("a directory");
    fs::write(root.join("assets/Logo.png"), "").expect("a file");
    let logo = root.join("assets").join("logo.png");
    assert_eq!(&*name_of(Some(&root), &logo), "assets/logo.png");
    let real = root.join("assets").join("Logo.png");
    let file = file_of(&real).expect("the file");
    let about = root
        .join(".")
        .join("assets")
        .join("..")
        .join("assets/Logo.png");
    assert_eq!(file_of(&about).as_ref(), Some(&file));
    #[cfg(windows)]
    for spelled in [logo, root.join(r"assets\LOGO.PNG")] {
        assert_eq!(file_of(&spelled).as_ref(), Some(&file), "{spelled:?}");
    }
    assert_eq!(file_of(&root.join("assets/none.png")), None);
}

/// A scan gives each asset the ID its ID file holds, writes missing ID files where the run may
/// (else says why there's none), warns of each problem once a run, never writes over an ID file or
/// writes back an ID another holds. An asset keeps its ID for the run, whatever its ID file comes
/// to hold, until renamed with its ID file.
#[cfg(feature = "dev")]
#[test]
fn a_scan_writes_missing_id_files_where_the_run_may_and_each_problem_warns_once() {
    let logged = crate::testing::logged();
    let scratch = Scratch::new("scan-catalog");
    let root = scratch.0.as_path();
    fs::create_dir_all(root).expect("a directory");
    let id = |text: &str| -> AssetId { text.parse().expect("an ID") };
    let write = |name: &str, text: &str| fs::write(root.join(name), text).expect("written");
    let meta = |text: &str| Meta { id: id(text) }.text();
    let read = |name: &str| fs::read_to_string(root.join(name)).expect("an ID file");
    let names = || -> Vec<String> {
        let listed = fs::read_dir(root).expect("listed").map(|entry| {
            let name = entry.expect("an entry").file_name();
            name.into_string().expect("Unicode")
        });
        listed.collect()
    };
    let rescan = |catalog: &mut Catalog, writes| {
        if let Some(next) = scan(catalog, root, &mut Prints::default(), writes) {
            *catalog = next;
        }
    };
    let codes = || logged.codes.try_iter().collect::<Vec<_>>();
    // An asset's ID this run and the code of what's wrong with its ID file.
    let state = |catalog: &Catalog, path: &str| {
        let found = catalog.listed().find(|&(listed, _)| listed == path);
        let (_, cataloged) = found.unwrap_or_else(|| panic!("{path} listed"));
        (
            cataloged.id,
            cataloged.problem.as_ref().map(|problem| problem.code),
        )
    };
    let mut catalog = Catalog::default();

    write("a.png", "");
    write("b.png", "");
    write("b.png.meta", &meta("5f3c9a1e07b2d846"));
    rescan(&mut catalog, false);
    assert_eq!(state(&catalog, "a.png"), (None, Some("asset.no_id")));
    assert!(!root.join("a.png.meta").exists(), "a run that writes none");
    let b = Some(id("5f3c9a1e07b2d846"));
    assert_eq!(state(&catalog, "b.png"), (b, None));
    let unchanged = scan(&catalog, root, &mut Prints::default(), false);
    assert_eq!(unchanged, None, "nothing changed");
    rescan(&mut catalog, true);
    let (a, problem) = state(&catalog, "a.png");
    let a = a.expect("an ID written");
    assert_eq!((read("a.png.meta"), problem), (Meta { id: a }.text(), None));
    assert_eq!(codes(), ["asset.no_id", "asset.id_written"]);

    // The ID it had, whatever its ID file holds now.
    write("b.png.meta", &meta("0a1b2c3d4e5f6071"));
    rescan(&mut catalog, true);
    rescan(&mut catalog, true);
    assert_eq!(state(&catalog, "b.png"), (b, Some("asset.id_changed")));
    assert_eq!(codes(), ["asset.id_changed"]);

    // An ID file that doesn't read, and one named for it but for case, which stay as they are.
    let broken = "(id: \"5f3c";
    write("bad.png", "");
    write("bad.png.meta", broken);
    write("Case.png", "");
    write("case.png.meta", &meta("1d2c3b4a59687f0e"));
    rescan(&mut catalog, true);
    rescan(&mut catalog, true);
    assert_eq!(state(&catalog, "bad.png"), (None, Some("asset.bad_meta")));
    assert_eq!(read("bad.png.meta"), broken);
    assert_eq!(state(&catalog, "Case.png"), (None, Some("asset.meta_case")));
    assert!(
        !names().contains(&"Case.png.meta".to_owned()),
        "{:?}",
        names()
    );
    assert_eq!(codes(), ["asset.meta_case", "asset.bad_meta"]);

    // Two that hold one ID, the first by path keeping it; and one whose asset isn't there.
    let shared = "2b3c4d5e6f708192";
    for name in ["c.png", "d.png"] {
        write(name, "");
        write(&format!("{name}.meta"), &meta(shared));
    }
    write("gone.png.meta", &meta("3c4d5e6f708192a3"));
    rescan(&mut catalog, true);
    rescan(&mut catalog, true);
    assert_eq!(state(&catalog, "c.png"), (Some(id(shared)), None));
    assert_eq!(state(&catalog, "d.png"), (None, Some("asset.duplicate_id")));
    let gone = Some(id("3c4d5e6f708192a3"));
    let orphan = (Arc::from("gone.png.meta"), AssetKind::Texture, gone);
    assert_eq!(catalog.orphans(), [orphan]);
    assert_eq!(codes(), ["asset.duplicate_id", "asset.orphaned"]);
    // Its ID file removed, the other gets a fresh one.
    fs::remove_file(root.join("d.png.meta")).expect("removed");
    rescan(&mut catalog, true);
    let (d, problem) = state(&catalog, "d.png");
    let d = d.expect("a fresh ID");
    assert_ne!(d, id(shared));
    assert_eq!((read("d.png.meta"), problem), (Meta { id: d }.text(), None));
    assert_eq!(codes(), ["asset.id_written"]);

    // Renamed with its ID file, which takes its ID along; then a new file at the old name,
    // which gets a fresh one.
    write("e.png", "");
    rescan(&mut catalog, true);
    let e = state(&catalog, "e.png").0.expect("an ID written");
    for (from, to) in [("e.png", "f.png"), ("e.png.meta", "f.png.meta")] {
        fs::rename(root.join(from), root.join(to)).expect("renamed");
    }
    rescan(&mut catalog, true);
    assert_eq!(state(&catalog, "f.png"), (Some(e), None));
    assert!(catalog.listed().all(|(path, _)| path != "e.png"), "gone");
    write("e.png", "");
    rescan(&mut catalog, true);
    let (fresh, problem) = state(&catalog, "e.png");
    assert!(fresh.is_some_and(|fresh| fresh != e) && problem.is_none());
    let renamed = ["asset.id_written", "asset.renamed", "asset.id_written"];
    assert_eq!(codes(), renamed);
    // Its ID file moved beside another asset, whose ID this run another ID file holds: nothing
    // written back, as its ID would be in two files.
    fs::remove_file(root.join("e.png.meta")).expect("removed");
    fs::rename(root.join("f.png.meta"), root.join("e.png.meta")).expect("moved");
    rescan(&mut catalog, true);
    assert_eq!(
        state(&catalog, "f.png"),
        (Some(e), Some("asset.duplicate_id"))
    );
    assert_eq!(state(&catalog, "e.png"), (fresh, Some("asset.id_changed")));
    assert!(!names().contains(&"f.png.meta".to_owned()), "{:?}", names());
    assert_eq!(codes(), ["asset.id_changed", "asset.duplicate_id"]);
    let mut held: Vec<AssetId> = (names().iter())
        .filter(|name| name.ends_with(".meta"))
        .filter_map(|name| Meta::read(&root.join(name)).ok())
        .map(|meta| meta.id)
        .collect();
    let files = held.len();
    held.sort_unstable();
    held.dedup();
    assert_eq!(held.len(), files, "no two ID files hold one ID");
}

/// An asset renamed without its ID file, its file as it was, keeps its ID where the run writes ID
/// files: the ID file is moved beside it, logging `asset.renamed`, as the rename is looked at or,
/// by a cache's prints, as the next run starts. One renamed and edited at once, and twins of a
/// renamed one, get fresh IDs, the ID file left behind warned of. A run that writes none moves
/// none.
#[cfg(feature = "dev")]
#[test]
fn an_asset_renamed_without_its_id_file_keeps_its_id_by_what_its_file_holds() {
    let logged = crate::testing::logged();
    let scratch = Scratch::new("scan-renamed");
    let root = scratch.0.join("game");
    fs::create_dir_all(&root).expect("a directory");
    let write = |name: &str, text: &str| fs::write(root.join(name), text).expect("written");
    let rename = |from: &str, to: &str| fs::rename(root.join(from), root.join(to)).expect("");
    let there = |name: &str| root.join(name).exists();
    let held = |name: &str| Meta::read(&root.join(name)).expect("an ID file").id;
    let codes = || logged.codes.try_iter().collect::<Vec<_>>();
    // A look at the root, as the thread that watches it takes one.
    let look = |catalog: &mut Catalog, prints: &mut Prints, writes| {
        let found = asset_files::scan(&root);
        if let Some(next) = scanned(catalog, &found, prints, writes) {
            *catalog = next;
        }
        prints.update(catalog, &found);
    };
    let (mut catalog, mut prints) = (Catalog::default(), Prints::default());
    for (name, text) in [("a.png", "alpha"), ("b.png", "beta"), ("e.png", "twin")] {
        write(name, text);
    }
    write("h.png", "kept");
    look(&mut catalog, &mut prints, true);
    let [a, b, e, h] = ["a", "b", "e", "h"].map(|name| held(&format!("{name}.png.meta")));
    let _ = codes();

    rename("a.png", "c.png");
    look(&mut catalog, &mut prints, true);
    assert_eq!(catalog.id("c.png"), Some(a));
    assert_eq!(catalog.path(a), Some("c.png"));
    assert_eq!((held("c.png.meta"), there("a.png.meta")), (a, false));
    assert_eq!(codes(), ["asset.renamed"]);

    // Renamed and edited at once; and renamed beside a copy of what it held.
    rename("b.png", "d.png");
    write("d.png", "BETA");
    fs::copy(root.join("e.png"), root.join("f.png")).expect("copied");
    rename("e.png", "g.png");
    look(&mut catalog, &mut prints, true);
    for (name, was) in [("d.png", b), ("f.png", e), ("g.png", e)] {
        let id = catalog.id(name).expect("a fresh ID");
        assert!(id != was && held(&format!("{name}.meta")) == id, "{name}");
    }
    assert_eq!(
        (held("b.png.meta"), held("e.png.meta")),
        (b, e),
        "left behind"
    );
    let fresh = ["asset.id_written"; 3];
    assert_eq!(codes(), [&fresh[..], &["asset.orphaned"; 2]].concat());
    // The prints of what the assets left behind held are kept; those of an asset gone with its ID
    // file are forgotten.
    assert!([b, e].iter().all(|id| prints.by_id.contains_key(id)));
    let d = catalog.id("d.png").expect("an ID");
    for name in ["d.png", "d.png.meta"] {
        fs::remove_file(root.join(name)).expect("removed");
    }
    look(&mut catalog, &mut prints, true);
    assert!(!prints.by_id.contains_key(&d), "forgotten");

    // An ID file left behind whose ID another ID file holds (a copy beside the renamed asset)
    // pairs with none, though a new file holds what its asset held.
    write("x.png", "ex");
    look(&mut catalog, &mut prints, true);
    let x = held("x.png.meta");
    fs::copy(root.join("x.png.meta"), root.join("y.png.meta")).expect("copied");
    rename("x.png", "y.png");
    write("z.png", "ex");
    look(&mut catalog, &mut prints, true);
    assert_eq!((catalog.id("y.png"), held("x.png.meta")), (Some(x), x));
    let z = catalog.id("z.png").expect("a fresh ID");
    assert!(z != x && held("z.png.meta") == z);
    let _ = codes();

    // A run that writes none moves none.
    rename("c.png", "c2.png");
    look(&mut catalog, &mut prints, false);
    assert_eq!((catalog.id("c2.png"), there("c.png.meta")), (None, true));
    rename("c2.png", "c.png");
    look(&mut catalog, &mut prints, true);

    // Renamed while no run watched: the next run's start pairs it by the prints the last kept.
    let cache = scratch.0.join("cache").join("prints.json");
    prints.save(&cache, &root);
    rename("h.png", "i.png");
    let mut next = Prints::read(&cache, &root);
    let started = scan(&Catalog::default(), &root, &mut next, true).expect("found");
    assert_eq!(started.id("i.png"), Some(h));
    assert_eq!((held("i.png.meta"), there("h.png.meta")), (h, false));
    assert!(codes().contains(&"asset.renamed".to_owned()));
    let other = Prints::read(&cache, &scratch.0);
    assert!(other.by_id.is_empty(), "a cache is of its root alone");
}

/// A file's print is its length and its bytes mixed 8 at a time, however it's read in parts.
#[cfg(feature = "dev")]
#[test]
fn a_print_mixes_every_byte_of_its_file_however_its_read() {
    let scratch = Scratch::new("print");
    fs::create_dir_all(&scratch.0).expect("a directory");
    let path = scratch.0.join("long.png");
    let bytes: Vec<u8> = (0..Print::CHUNK as u32 + 5)
        .map(|i| (i % 251) as u8)
        .collect();
    fs::write(&path, &bytes).expect("written");
    let print = Print::of(&path).expect("read");
    let length = bytes.len() as u64;
    assert_eq!(
        print,
        Print {
            length,
            hash: mixed(0, &bytes)
        }
    );
    let mut edited = bytes;
    edited[Print::CHUNK + 2] ^= 1;
    fs::write(&path, &edited).expect("written");
    assert_ne!(Print::of(&path).expect("read"), print);
}

/// A copy made with its ID file has no ID while the original keeps it this run, and is told which
/// keeps it from the next run, whose first scan gives it to the first by path: here the copy.
#[cfg(feature = "dev")]
#[test]
fn a_duplicate_is_told_which_asset_keeps_the_id_from_the_next_run() {
    let scratch = Scratch::new("duplicate-next-run");
    let root = scratch.0.as_path();
    fs::create_dir_all(root).expect("a directory");
    let x: AssetId = "5f3c9a1e07b2d846".parse().expect("an ID");
    fs::write(root.join("m.png"), "").expect("an asset");
    fs::write(root.join("m.png.meta"), Meta { id: x }.text()).expect("its ID file");
    let rescan = |catalog: &Catalog| scan(catalog, root, &mut Prints::default(), true);
    let catalog = rescan(&Catalog::default()).expect("found");
    for name in ["m.png", "m.png.meta"] {
        fs::copy(root.join(name), root.join(name.replacen('m', "a", 1))).expect("copied");
    }
    let catalog = rescan(&catalog).expect("changed");
    let (_, copy) = (catalog.listed().find(|&(path, _)| path == "a.png")).expect("listed");
    let problem = copy.problem.as_ref().expect("a problem");
    assert_eq!((copy.id, problem.code), (None, "asset.duplicate_id"));
    let told = "from the next run, a.png takes it";
    assert!(problem.message.contains(told), "{problem:?}");
    let next = rescan(&Catalog::default()).expect("found");
    let ids: Vec<_> = next
        .listed()
        .map(|(path, asset)| (path, asset.id))
        .collect();
    assert_eq!(ids, [("a.png", Some(x)), ("m.png", None)]);
}
