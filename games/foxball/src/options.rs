//! The options a player chose in the menu, kept for them between windowed runs in `options.ron`, as
//! [`Context::save_state`] keeps an app's settings: each bus's volume, how the view is antialiased,
//! and how well the bot plays. Headless runs keep nothing, so that tests, replays and recordings
//! start from the defaults.

use gg_engine::{Antialiasing, Bus, Context};

use crate::{ANTIALIASING, Difficulty, VOLUMES};

/// The options' file, as [`Context::read_state`] names it.
pub(crate) const FILE: &str = "options.ron";

/// The options, as the menu chose them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Options {
    /// The main, music and sound buses' volumes, from 0 to 1, in [`VOLUMES`]' order.
    pub(crate) volumes: [f32; 3],
    /// How the view is antialiased, or `None` for as the world's environment says.
    pub(crate) antialiasing: Option<Antialiasing>,
    /// How well the bot plays in a run alone.
    pub(crate) difficulty: Difficulty,
}

impl Default for Options {
    /// As a run starts without them: at full volume, antialiased as the arena's environment says,
    /// and the bot at Normal.
    fn default() -> Self {
        Self {
            volumes: [1.0; 3],
            antialiasing: None,
            difficulty: Difficulty::default(),
        }
    }
}

impl Options {
    /// The options as the run has them: its buses' volumes and antialiasing, as `cx` has them, and
    /// the bot's `difficulty`.
    pub(crate) fn of(cx: &Context, difficulty: Difficulty) -> Self {
        Self {
            volumes: VOLUMES.map(|(.., bus): (&str, &str, Bus)| cx.volume(bus)),
            antialiasing: cx.antialiasing(),
            difficulty,
        }
    }

    /// The file's text, valid RON, a field to a line, which [`parse`](Self::parse) reads.
    pub(crate) fn text(&self) -> String {
        let [main, music, sound] = self.volumes;
        let antialiasing = match self.antialiasing {
            Some(antialiasing) => format!("Some({})", antialiasing_name(antialiasing)),
            None => "None".to_owned(),
        };
        let difficulty = difficulty_name(self.difficulty);
        format!(
            "(\n    main_volume: {main:?},\n    music_volume: {music:?},\n    sound_volume: \
             {sound:?},\n    antialiasing: {antialiasing},\n    difficulty: {difficulty},\n)\n"
        )
    }

    /// The options `text` holds, as [`text`](Self::text) writes them, each field once in its
    /// order, whatever the spaces and line breaks about them; `None` for any other text, or a
    /// volume that isn't from 0 to 1.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let body = text.trim().strip_prefix('(')?.strip_suffix(')')?;
        let mut fields = (body.split(',').map(str::trim))
            .filter(|field| !field.is_empty())
            .map(|field| {
                field
                    .split_once(':')
                    .map(|(key, value)| (key.trim(), value.trim()))
            });
        let mut field = |name: &str| match fields.next() {
            Some(Some((key, value))) if key == name => Some(value),
            _ => None,
        };
        let mut volume = |name: &str| {
            let volume = field(name)?.parse::<f32>().ok()?;
            (0.0..=1.0).contains(&volume).then_some(volume)
        };
        let volumes = [
            volume("main_volume")?,
            volume("music_volume")?,
            volume("sound_volume")?,
        ];
        let antialiasing = match field("antialiasing")? {
            "None" => None,
            some => {
                let name = some.strip_prefix("Some(")?.strip_suffix(')')?;
                let found = ANTIALIASING
                    .into_iter()
                    .find(|&of| antialiasing_name(of) == name);
                Some(found?)
            }
        };
        let difficulty = field("difficulty")?;
        let difficulty =
            (Difficulty::ALL.into_iter()).find(|&of| difficulty_name(of) == difficulty)?;
        fields.next().is_none().then_some(Self {
            volumes,
            antialiasing,
            difficulty,
        })
    }

    /// The options the player kept, as `cx` reads them for the user: `None` if they kept none, as
    /// in headless runs; or else the defaults, warning (`foxball.options`), if they can't be read.
    pub(crate) fn read(cx: &Context) -> Option<Self> {
        let bytes = match cx.read_state(FILE) {
            Ok(bytes) => bytes?,
            Err(error) => {
                tracing::warn!(
                    code = "foxball.options",
                    file = FILE,
                    "{error:#}; the defaults stand"
                );
                return Some(Self::default());
            }
        };
        let read = std::str::from_utf8(&bytes).ok().and_then(Self::parse);
        if read.is_none() {
            tracing::warn!(
                code = "foxball.options",
                file = FILE,
                "the options kept aren't options as the game writes them; the defaults stand"
            );
        }
        Some(read.unwrap_or_default())
    }

    /// Keeps the options for the player, as `cx` keeps a file for the user, warning
    /// (`foxball.options`) if it can't.
    pub(crate) fn save(&self, cx: &Context) {
        if let Err(error) = cx.save_state(FILE, self.text().as_bytes()) {
            tracing::warn!(code = "foxball.options", file = FILE, "{error:#}");
        }
    }
}

/// `antialiasing`'s name in the file.
fn antialiasing_name(antialiasing: Antialiasing) -> &'static str {
    match antialiasing {
        Antialiasing::Taa => "Taa",
        Antialiasing::Smaa => "Smaa",
        Antialiasing::None => "Off",
    }
}

/// `difficulty`'s name in the file.
fn difficulty_name(difficulty: Difficulty) -> &'static str {
    match difficulty {
        Difficulty::Easy => "Easy",
        Difficulty::Normal => "Normal",
        Difficulty::Hard => "Hard",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_read_back_as_written_and_anything_else_is_refused() {
        let chosen = Options {
            volumes: [0.25, 1.0, 0.0],
            antialiasing: Some(Antialiasing::Smaa),
            difficulty: Difficulty::Hard,
        };
        let text = chosen.text();
        assert_eq!(
            text,
            "(\n    main_volume: 0.25,\n    music_volume: 1.0,\n    sound_volume: 0.0,\n    \
             antialiasing: Some(Smaa),\n    difficulty: Hard,\n)\n"
        );
        assert_eq!(Options::parse(&text), Some(chosen));
        for antialiasing in ANTIALIASING.map(Some).into_iter().chain([None]) {
            for difficulty in Difficulty::ALL {
                let options = Options {
                    antialiasing,
                    difficulty,
                    ..Options::default()
                };
                assert_eq!(Options::parse(&options.text()), Some(options));
            }
        }
        // On one line, as RON may be.
        let flat = "(main_volume: 0.5, music_volume: 0.5, sound_volume: 0.5, antialiasing: None, \
                    difficulty: Easy)";
        let read = Options::parse(flat).map(|options| options.difficulty);
        assert_eq!(read, Some(Difficulty::Easy));
        // Damaged, cut short, out of range, out of order, of another field or with one more.
        for damaged in [
            "",
            "not options",
            &text[..text.len() / 2],
            &text.replace("0.25", "1.5"),
            &text.replace("0.25", "NaN"),
            &text.replace("Smaa", "Fxaa"),
            &text.replace("Hard", "Brutal"),
            &text.replace("Some(Smaa)", "Smaa"),
            &text.replace("main_volume", "master_volume"),
            &text.replace(
                "    difficulty: Hard,\n",
                "    difficulty: Hard,\n    extra: 1,\n",
            ),
            &text.replace(
                "    main_volume: 0.25,\n    music_volume: 1.0,\n",
                "    music_volume: 1.0,\n    main_volume: 0.25,\n",
            ),
        ] {
            assert_eq!(Options::parse(damaged), None, "{damaged:?}");
        }
    }
}
