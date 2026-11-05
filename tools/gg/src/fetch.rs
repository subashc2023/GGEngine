//! `gg fetch`: the files a project's fetch list names, `fetch.ron` beside its manifest, as gg-api's
//! `Fetch` reads it, fetched with curl, each checked against its SHA-256 before it takes its place,
//! with the ID file of each texture, model and font under the asset root: holding the ID the list
//! gives it, so that every checkout gives it the ID its scenes name, or else written, as a
//! development run would write it, if it has none.

use std::fs;
use std::path::Path;
use std::process::Command;

use gg_api::asset::{AssetId, AssetKind, Meta};
use gg_api::{Failure, Fetch, Project};
use serde_json::json;

use crate::Scope;

/// Fetches what the fetch list of `scope`'s project names, printing a line of JSON for each file,
/// `fetched` with its bytes or `kept`, with the `id` written beside it if one was, then one for
/// the list.
///
/// Fails as gg-api's `Fetch::read` and `Project::read` do, and as [`fetch_with`] does.
pub(crate) fn fetch(scope: &Scope) -> Result<(), Failure> {
    let dir = scope.dir();
    let project = Project::read(dir)?;
    let list = Fetch::read(dir)?;
    let root = project.assets_dir(dir);
    let (fetched, kept) = fetch_with(&list, dir, &root, curl, crate::print)?;
    let path = dir.join(Fetch::FILE);
    crate::print(&json!({"fetch": path, "fetched": fetched, "kept": kept}).to_string());
    Ok(())
}

/// Fetches each file `list` names into directory `dir`, by `download`, which writes what's at a URL
/// to a file, but those there already whose SHA-256 is the list's, each written first beside where
/// it goes, with `.part` after its name, and put in its place once its SHA-256 is the list's; then
/// writes the ID file of each that's a texture, a model or a font under the asset root, `root`:
/// with the ID the list gives it, unless it holds that, or else a fresh one, if it has none. Hands
/// `print` a line of JSON for each, and returns how many were fetched and how many kept.
///
/// Fails with `gg.fetch` for a file that can't be read or written, a download that fails, or one
/// whose SHA-256 isn't the list's, which it removes, keeping the files fetched before it.
fn fetch_with(
    list: &Fetch,
    dir: &Path,
    root: &Path,
    download: impl Fn(&str, &Path) -> Result<(), Failure>,
    print: impl Fn(&str),
) -> Result<(usize, usize), Failure> {
    let failed = |message: String| Failure::new("gg.fetch", message);
    let (mut fetched, mut kept) = (0, 0);
    for file in list.files() {
        let at = file.at(dir);
        let have = fs::read(&at)
            .ok()
            .filter(|data| sha256(data) == file.sha256);
        let mut line = match have {
            Some(_) => {
                kept += 1;
                json!({"kept": file.path})
            }
            None => {
                let part = at.with_added_extension("part");
                let parent = at.parent().unwrap_or(dir);
                fs::create_dir_all(parent).map_err(|error| {
                    failed(format!(
                        "can't make the folder {}: {error}",
                        parent.display()
                    ))
                })?;
                download(&file.url, &part)?;
                let data = fs::read(&part)
                    .map_err(|error| failed(format!("can't read {}: {error}", part.display())))?;
                let got = sha256(&data);
                if got != file.sha256 {
                    let _ = fs::remove_file(&part);
                    return Err(failed(format!(
                        "{} came with the SHA-256 {got}, not the list's {}: the list's `from` \
                         names a folder pinned to what it holds, as a repository's at a commit, so \
                         what's there changed, or the download was cut short",
                        file.url, file.sha256
                    )));
                }
                fs::rename(&part, &at).map_err(|error| {
                    failed(format!("can't put {} in its place: {error}", at.display()))
                })?;
                fetched += 1;
                json!({"fetched": file.path, "bytes": data.len()})
            }
        };
        let meta = Meta::path(&at);
        let id = match file.id {
            Some(given) => {
                (Meta::read(&meta).ok().map(|meta| meta.id) != Some(given)).then_some(given)
            }
            None => (!meta.exists()).then(AssetId::draw),
        };
        if let Some(id) = id.filter(|_| at.starts_with(root) && AssetKind::of(&at).is_some()) {
            fs::write(&meta, Meta { id }.text())
                .map_err(|error| failed(format!("can't write {}: {error}", meta.display())))?;
            line["id"] = json!(id.to_string());
        }
        print(&line.to_string());
    }
    Ok((fetched, kept))
}

/// Writes what's at `url` to the file at `to`, with curl, following redirects.
///
/// Fails with `gg.no_curl` if curl doesn't start, and with `gg.fetch` if it fails, with what it
/// says.
fn curl(url: &str, to: &Path) -> Result<(), Failure> {
    let output = Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--output",
        ])
        .arg(to)
        .arg(url)
        .output()
        .map_err(|error| {
            let message = format!("curl, which fetches the files, doesn't start: {error}");
            Failure::new("gg.no_curl", message)
        })?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr);
    let message = format!("curl couldn't fetch {url}: {}", said.trim());
    Err(Failure::new("gg.fetch", message))
}

/// The SHA-256 of `data`, as FIPS 180-4 defines it, in lowercase hex.
fn sha256(data: &[u8]) -> String {
    /// The first 32 bits of the fractional parts of the cube roots of the first 64 primes.
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    // The first 32 bits of the fractional parts of the square roots of the first 8 primes.
    let mut hash: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    // The whole blocks, then what's left, padded: a 1 bit, 0s, and the message's length in bits,
    // 64 of them.
    let (blocks, rest) = data.as_chunks::<64>();
    let mut last = rest.to_vec();
    last.push(0x80);
    while last.len() % 64 != 56 {
        last.push(0);
    }
    let bits = u64::try_from(data.len()).expect("fewer than 2^64 bytes") * 8;
    last.extend(bits.to_be_bytes());
    for block in blocks.iter().chain(last.as_chunks::<64>().0) {
        let mut w = [0u32; 64];
        for (word, bytes) in w.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*bytes);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = (w[i - 16].wrapping_add(s0))
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = hash;
        for (k, w) in K.iter().zip(w) {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ (!e & g);
            let t1 = (h.wrapping_add(s1).wrapping_add(choice))
                .wrapping_add(*k)
                .wrapping_add(w);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (word, add) in hash.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *word = word.wrapping_add(add);
        }
    }
    hash.iter().map(|word| format!("{word:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    /// FIPS 180-4's examples, from NIST's, and messages whose padding spills into a block of its
    /// own, or just doesn't.
    #[test]
    fn sha256_is_fips_180_4s() {
        let cases = [
            (
                &b""[..],
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                b"abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
            (
                b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklm\
                  nopqrlmnopqrsmnopqrstnopqrstu",
                "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1",
            ),
        ];
        for (data, hash) in cases {
            assert_eq!(sha256(data), hash, "{}", String::from_utf8_lossy(data));
        }
        let million = vec![b'a'; 1_000_000];
        assert_eq!(
            sha256(&million),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    /// Files not there, or there but other than the list's, are fetched into their places; those
    /// there as the list has them are kept, fetching nothing; textures, models and fonts under the
    /// asset root get ID files, the ID the list gives one, put back if its ID file holds another,
    /// and a fresh one for the rest, kept once they have one; and a download that isn't the list's
    /// is refused, leaving nothing in its place.
    #[test]
    fn fetching_puts_the_lists_files_in_place_and_refuses_others() {
        let dir = std::env::temp_dir().join(format!("gg-fetch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (model, image, buffer) = (&b"{}"[..], &b"png"[..], &b"\0\x01"[..]);
        let given = "5f3c9a1e07b2d846";
        let text = format!(
            "[\n    (\n        from: \"https://example.com/m\",\n        into: \"models/m\",\n        \
             files: {{\n            \"m.bin\": \"{}\",\n            \"m.gltf\": \"{}\",\n            \
             \"m.png\": \"{}\",\n        }},\n        ids: {{\n            \"m.gltf\": \
             \"{given}\",\n        }},\n    ),\n]\n",
            sha256(buffer),
            sha256(model),
            sha256(image)
        );
        let list = Fetch::parse(&text).expect("a list");
        let downloads = Cell::new(0);
        let serve = |url: &str, to: &Path| {
            downloads.set(downloads.get() + 1);
            let extension = url.rsplit('.').next();
            let data = match extension {
                Some("gltf") => model,
                Some("png") => image,
                _ => buffer,
            };
            fs::write(to, data).map_err(|error| Failure::new("gg.fetch", error.to_string()))
        };
        let at = |file: &str| dir.join("models").join("m").join(file);
        let id_of = |file: &str| Meta::read(&Meta::path(&at(file))).map(|meta| meta.id.to_string());
        let fetched = fetch_with(&list, &dir, &dir, serve, |_| {});
        let ids = [id_of("m.gltf"), id_of("m.png")];
        let unmeta = Meta::path(&at("m.bin")).exists();
        // Kept as they are, but one whose bytes changed, and an ID file holding another ID.
        fs::write(at("m.bin"), b"changed").expect("written");
        let other = Meta {
            id: "0a1b2c3d4e5f6071".parse().expect("an ID"),
        };
        fs::write(Meta::path(&at("m.gltf")), other.text()).expect("written");
        let again = fetch_with(&list, &dir, &dir, serve, |_| {});
        let kept_ids = [id_of("m.gltf"), id_of("m.png")];
        let restored = fs::read(at("m.bin"));
        // A download that isn't the list's.
        fs::remove_file(at("m.gltf")).expect("removed");
        let wrong = |_: &str, to: &Path| {
            fs::write(to, b"other").map_err(|error| Failure::new("gg.fetch", error.to_string()))
        };
        let refused = fetch_with(&list, &dir, &dir, wrong, |_| {}).map_err(|failure| failure.code);
        let left = [at("m.gltf"), at("m.gltf.part")].map(|path| path.exists());
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(fetched, Ok((3, 0)));
        assert_eq!(ids[0].as_deref(), Ok(given));
        assert!(ids[1].is_ok(), "{ids:?}");
        assert!(!unmeta, "a model's buffer is no asset");
        assert_eq!(again, Ok((1, 2)));
        assert_eq!(downloads.get(), 4);
        assert_eq!(kept_ids, ids);
        assert_eq!(restored.as_deref().ok(), Some(buffer));
        assert_eq!(refused, Err("gg.fetch".to_owned()));
        assert_eq!(left, [false, false]);
    }
}
