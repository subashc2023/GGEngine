//! Checks the workspace's AGENTS.md links: each leads to a file or folder that's there and, with
//! an anchor, to one of its headings.

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> &'static Path {
    let gg = Path::new(env!("CARGO_MANIFEST_DIR"));
    gg.ancestors()
        .nth(2)
        .expect("gg is at tools/gg in the workspace")
}

fn agents_files() -> Vec<PathBuf> {
    let mut files = vec![root().join("AGENTS.md")];
    for group in ["crates", "apps", "tools", "games"] {
        let members = fs::read_dir(root().join(group)).expect("the members' folder lists");
        for member in members {
            let file = member.expect("a member").path().join("AGENTS.md");
            if file.is_file() {
                files.push(file);
            }
        }
    }
    files
}

/// The targets of `[text](target)` links outside fenced code.
fn links(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        let mut rest = line;
        while !fenced && let Some(at) = rest.find("](") {
            let after = &rest[at + 2..];
            let Some(end) = after.find(')') else {
                break;
            };
            found.push(&after[..end]);
            rest = &after[end..];
        }
    }
    found
}

/// Heading anchors outside fenced code, as GitHub makes them: lowercase, `-` for a space, no
/// punctuation but `-` and `_`.
fn anchors(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if !fenced && line.starts_with('#') {
            let heading = line.trim_start_matches('#').trim().to_lowercase();
            let anchor = heading.chars().filter_map(|c| match c {
                ' ' => Some('-'),
                c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
                _ => None,
            });
            found.push(anchor.collect());
        }
    }
    found
}

#[test]
fn every_link_in_the_agents_files_leads_somewhere() {
    let files = agents_files();
    assert!(
        files.len() > 1,
        "the root's and a member's at least: {files:?}"
    );
    for file in files {
        let text = fs::read_to_string(&file).expect("AGENTS.md reads");
        let dir = file.parent().expect("in a folder");
        let links = links(&text);
        assert!(!links.is_empty() || file != root().join("AGENTS.md"));
        for link in links.into_iter().filter(|link| !link.contains("://")) {
            let (target, anchor) = match link.split_once('#') {
                Some((target, anchor)) => (target, Some(anchor)),
                None => (link, None),
            };
            let target = if target.is_empty() {
                file.clone()
            } else {
                dir.join(target)
            };
            assert!(target.exists(), "{}: {link} leads nowhere", file.display());
            if let Some(anchor) = anchor {
                let headed = fs::read_to_string(&target).expect("the linked file reads");
                assert!(
                    anchors(&headed).iter().any(|heading| heading == anchor),
                    "{}: {link} names no heading of it",
                    file.display()
                );
            }
        }
    }
}

#[test]
fn links_and_anchors_are_read_as_markdown_has_them() {
    let text =
        "See [a](a.md) and [b](b/c.md#two-words).\n```\n[not](x.md)\n# not\n```\n## Two words\n";
    assert_eq!(links(text), ["a.md", "b/c.md#two-words"]);
    assert_eq!(anchors(text), ["two-words"]);
    assert_eq!(
        anchors("# Agents and tools\n## Engine rules: v0.1\n"),
        ["agents-and-tools", "engine-rules-v01"]
    );
}
