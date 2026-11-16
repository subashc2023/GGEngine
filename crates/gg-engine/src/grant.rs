//! Capabilities: what a mod's scripts and a tool API connection may do (ARCHITECTURE.md's Game
//! code and Agents and tools).

use std::fmt;
use std::str::FromStr;

use gg_core::{Error, Result};

/// What a mod's scripts or a tool API connection may do: `read` or `write` the whole world or one
/// type's components (`read:Shard`), or one of the tool API's own: `play`, `input`, `view`,
/// `scene`, `snapshot`, `debug`, `patch`, `files`, `note` or `quit`. A mod may ask for `read` and
/// `write` alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Capability {
    /// The components of the named type alone, or all.
    Read(Option<String>),
    Write(Option<String>),
    /// Plays, stops, pauses and steps.
    Play,
    /// Hands the layers input.
    Input,
    /// Captures, shares the view, picks, and sets the look.
    View,
    /// Opens, saves and empties the open scene.
    Scene,
    /// Saves the world as a snapshot.
    Snapshot,
    /// Debugs the scripts.
    Debug,
    /// Applies patches of the game's code, which run client-given code.
    Patch,
    /// Writes a capture or snapshot where a request names.
    Files,
    /// Logs a client's notes.
    Note,
    /// Ends the run.
    Quit,
}

/// The capabilities that aren't `read` or `write`, by their names.
const OWN: [(&str, Capability); 10] = [
    ("play", Capability::Play),
    ("input", Capability::Input),
    ("view", Capability::View),
    ("scene", Capability::Scene),
    ("snapshot", Capability::Snapshot),
    ("debug", Capability::Debug),
    ("patch", Capability::Patch),
    ("files", Capability::Files),
    ("note", Capability::Note),
    ("quit", Capability::Quit),
];

impl Capability {
    /// Whether `self` allows all `other` does: it is `other`, or whole-world `read` or `write`
    /// where `other` is one type's.
    pub fn covers(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Read(None), Self::Read(_)) | (Self::Write(None), Self::Write(_)) => true,
            _ => self == other,
        }
    }

    /// Whether it's `read` or `write`: all a mod may ask for.
    pub fn of_the_world(&self) -> bool {
        matches!(self, Self::Read(_) | Self::Write(_))
    }
}

impl FromStr for Capability {
    type Err = Error;

    /// Reads a capability as [`Display`](fmt::Display) writes it.
    ///
    /// Fails with `grant.unknown` for an unknown name or an empty type after `read:`/`write:`.
    fn from_str(name: &str) -> Result<Self> {
        let unknown = || {
            let own = OWN.map(|(name, _)| name).join(", ");
            Error::new(
                "grant.unknown",
                format!(
                    "there's no capability `{name}`: the capabilities are `read` and `write`, or \
                     of one component type's, as `read:Shard`, and {own}"
                ),
            )
        };
        let of = |ty: &str| (!ty.is_empty()).then(|| ty.to_owned()).ok_or_else(unknown);
        Ok(match name.split_once(':') {
            None if name == "read" => Self::Read(None),
            None if name == "write" => Self::Write(None),
            Some(("read", ty)) => Self::Read(Some(of(ty)?)),
            Some(("write", ty)) => Self::Write(Some(of(ty)?)),
            Some(_) => return Err(unknown()),
            None => (OWN.iter())
                .find_map(|(own, capability)| (*own == name).then(|| capability.clone()))
                .ok_or_else(unknown)?,
        })
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(None) => f.write_str("read"),
            Self::Write(None) => f.write_str("write"),
            Self::Read(Some(ty)) => write!(f, "read:{ty}"),
            Self::Write(Some(ty)) => write!(f, "write:{ty}"),
            own => {
                let name = OWN.iter().find(|(_, capability)| capability == own);
                f.write_str(name.expect("every other capability is named").0)
            }
        }
    }
}

/// Capabilities granted: asked by a mod's manifest, the most a game grants mods, what a tool API
/// connection is kept to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grant(Vec<Capability>);

impl Grant {
    /// Every capability: a tool API connection's unless its `hello` names fewer.
    pub fn all() -> Self {
        let own = OWN.map(|(_, capability)| capability);
        Self(
            [Capability::Read(None), Capability::Write(None)]
                .into_iter()
                .chain(own)
                .collect(),
        )
    }

    /// The capabilities `names` name, as a mod's manifest and the tool API's `hello` write them
    /// (`["read", "write:Sprite"]`).
    ///
    /// Fails with `grant.unknown`, as reading a capability does.
    pub fn parse<S: AsRef<str>>(names: impl IntoIterator<Item = S>) -> Result<Self> {
        let names = names.into_iter();
        names
            .map(|name| name.as_ref().parse())
            .collect::<Result<_>>()
            .map(Self)
    }

    /// Whether it allows all `capability` does.
    pub fn has(&self, capability: &Capability) -> bool {
        self.0.iter().any(|granted| granted.covers(capability))
    }

    /// Whether it allows reading `component`.
    pub fn reads(&self, component: &str) -> bool {
        self.has(&Capability::Read(Some(component.to_owned())))
    }

    /// Whether it allows writing `component`.
    pub fn writes(&self, component: &str) -> bool {
        self.has(&Capability::Write(Some(component.to_owned())))
    }

    /// Its capabilities that `limits` doesn't cover.
    pub fn beyond<'g>(&'g self, limits: &Self) -> impl Iterator<Item = &'g Capability> {
        self.0.iter().filter(|capability| !limits.has(capability))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Capability> {
        self.0.iter()
    }
}

impl fmt::Display for Grant {
    /// Comma-separated, as `gg --grant` takes them; `nothing` for none.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return f.write_str("nothing");
        }
        for (at, capability) in self.0.iter().enumerate() {
            if at > 0 {
                f.write_str(",")?;
            }
            write!(f, "{capability}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_read_as_they_write_and_the_whole_world_s_cover_a_type_s() {
        let names = ["read", "write:Sprite", "view", "read:Shard", "quit"];
        let grant = Grant::parse(names).expect("capabilities");
        assert_eq!(grant.to_string(), names.join(","));
        let again = Grant::parse(grant.to_string().split(',')).expect("capabilities");
        assert_eq!(again, grant);
        assert!(grant.reads("Match") && grant.writes("Sprite") && !grant.writes("Match"));
        assert!(grant.has(&Capability::View) && !grant.has(&Capability::Patch));
        assert!(!grant.has(&Capability::Write(None)));
        let asked = Grant::parse(["read:Match", "write:Match", "write"]).expect("capabilities");
        let beyond: Vec<String> = asked.beyond(&grant).map(ToString::to_string).collect();
        assert_eq!(beyond, ["write:Match", "write"]);
        assert!(Grant::all().beyond(&Grant::all()).next().is_none());
        for name in ["reed", "read:", "write:", "view:Shard", ""] {
            let refused = name.parse::<Capability>().expect_err("not a capability");
            assert_eq!(refused.code(), "grant.unknown", "{name}");
        }
    }
}
