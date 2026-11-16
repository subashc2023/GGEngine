//! A UI's `.ui` file, a tree of nodes in RON, which [`read`] reads and checks. Each refusal has a
//! stable code and says where in the text it is. The format is [the module's](super).

use std::cell::RefCell;
use std::collections::HashSet;
use std::mem;
use std::path::Path;
use std::sync::Arc;

use gg_core::math::{Vec2, Vec4};
use gg_core::{Error, Result};
use ron::extensions::Extensions;
use serde::{Deserialize, Deserializer};

use super::{Document, Kind, Look, Named, Node, Size, Widget};
use crate::Align;
use crate::package::Files;

/// A UI file's most bytes: it's read whole.
const MOST: u64 = 16 << 20;

/// A node name's most bytes.
const MOST_NAME: usize = 64;

/// A UI's most nodes. A few bytes a node would otherwise take gigabytes as it's read and copied
/// per UI, or make a row of fills long to share out.
const MOST_NODES: usize = 10_000;

/// The most canvas units a size, margin, padding, gap, radius, slice width or text size may be,
/// and a fill's share. Far past any view's, so sums of thousands stay finite.
const MOST_UNITS: f32 = 1e6;

/// A drop-down's most options, and a text field's most characters.
const MOST_OPTIONS: usize = 256;
const MOST_TYPED: u32 = 4096;

/// A button's default looks: normal, focused, pressed, disabled, in linear RGBA, alpha not
/// premultiplied.
const BUTTON: [Look; 4] = [
    look([1.0, 1.0, 1.0, 0.08], [1.0, 1.0, 1.0, 0.85]),
    look([0.05, 0.27, 0.95, 1.0], [1.0, 1.0, 1.0, 1.0]),
    look([0.02, 0.12, 0.5, 1.0], [0.85, 0.9, 1.0, 1.0]),
    look([1.0, 1.0, 1.0, 0.03], [1.0, 1.0, 1.0, 0.3]),
];

/// The default looks of a slider, toggle, drop-down and text field, like a button's. The box
/// lightens when focused, so what's on in it shows over it in the accent.
const WIDGET: [Look; 4] = [
    look([1.0, 1.0, 1.0, 0.08], [1.0, 1.0, 1.0, 0.85]),
    look([1.0, 1.0, 1.0, 0.2], [1.0, 1.0, 1.0, 1.0]),
    look([1.0, 1.0, 1.0, 0.14], [0.85, 0.9, 1.0, 1.0]),
    look([1.0, 1.0, 1.0, 0.03], [1.0, 1.0, 1.0, 0.3]),
];

/// The default `accent`, a button's focused color, and a drop-down's default options backing.
const ACCENT: Vec4 = Vec4::new(0.05, 0.27, 0.95, 1.0);
const BACKING: Vec4 = Vec4::new(0.02, 0.025, 0.04, 0.96);

const fn look(color: [f32; 4], text: [f32; 4]) -> Look {
    Look {
        color: Vec4::from_array(color),
        text: Vec4::from_array(text),
        texture: None,
    }
}

/// Reads UI `name`'s file at `path` from `files` (the disk or the package) and [`parse`]s it.
///
/// Fails with `engine.bad_ui` if the file can't be read, isn't a regular file, is over 16 MiB or
/// isn't UTF-8; and as `parse` does. Every message names the UI and its file.
pub(crate) fn read(name: &str, path: &Path, files: &Files) -> Result<Document> {
    let _zone = gg_core::zone!();
    let error = |why: String| {
        let message = format!("can't read UI {name:?} from {}: {why}", path.display());
        Error::new("engine.bad_ui", message)
    };
    let bytes = files
        .read(path, MOST + 1)
        .map_err(|e| error(e.to_string()))?;
    if bytes.len() as u64 > MOST {
        return Err(error("it's more than 16 MiB".to_owned()));
    }
    let text = String::from_utf8(bytes).map_err(|e| error(e.to_string()))?;
    parse(&text).map_err(|refused| {
        let message = format!("UI {name:?} at {}: {refused}", path.display());
        refused.with_message(message)
    })
}

/// State of a parse, which serde's derives can't carry: names taken, and the code of a node
/// check's refusal (serde carries only its message). A parse stays on one thread.
#[derive(Default)]
struct Parsing {
    names: HashSet<Arc<str>>,
    nodes: usize,
    code: Option<&'static str>,
}

thread_local! {
    static PARSING: RefCell<Parsing> = RefCell::default();
}

/// The UI `text` holds, in RON ([the format](super)).
///
/// Fails at the line and column of the refusal with:
/// - `ui.syntax`: text that isn't RON of a UI, such as an unknown or mistyped field.
/// - `ui.bad_name`: a name that isn't 1 to 64 ASCII letters, digits, `_` and `-`.
/// - `ui.duplicate_name`: a name another node has.
/// - `ui.bad_field`: a field its kind doesn't take (a text's children), an image without its
///   texture or a drop-down without its options.
/// - `ui.unnamed_button`: a button without a name, which its presses are told by.
/// - `ui.unnamed_widget`: a slider, toggle, drop-down or text field without one, which its
///   changes are told by.
/// - `ui.bad_value`, at the number: a size, margin, padding, gap, radius, slice width, text size,
///   fill share or slider step outside 0 to [`MOST_UNITS`]; a slider bound or value outside
///   -[`MOST_UNITS`] to it; a text size or fill share of 0; a slice over half a texture; a
///   non-finite color; a slider whose `max` isn't above its `min` or whose `value` is outside
///   them; a drop-down with over [`MOST_OPTIONS`] options or a `choice` that isn't one; a text
///   field that holds no character or over [`MOST_TYPED`], or whose text is longer than it holds.
/// - `ui.too_many_nodes`: past [`MOST_NODES`].
/// - `ui.no_node`, after the rest: a `focus` no focusable node has as its name.
pub(crate) fn parse(text: &str) -> Result<Document> {
    let options = ron::Options::default()
        .with_default_extension(Extensions::IMPLICIT_SOME | Extensions::UNWRAP_VARIANT_NEWTYPES);
    PARSING.with_borrow_mut(|parsing| *parsing = Parsing::default());
    let parsed = options.from_str::<RawFile>(text);
    let Parsing { code, .. } = PARSING.with_borrow_mut(mem::take);
    let raw = parsed.map_err(|error| {
        let at = &error.span.end;
        let message = format!("{}:{}: {}", at.line, at.col, error.code);
        Error::new(code.unwrap_or("ui.syntax"), message)
    })?;
    let mut nodes = Vec::new();
    for root in raw.nodes {
        flatten(root.0, None, &mut nodes);
    }
    let focus = match raw.focus {
        None => None,
        Some(focus) => {
            let found = nodes.iter().position(|node| {
                node.kind.focuses() && node.name.as_deref() == Some(focus.as_str())
            });
            let found = found.ok_or_else(|| {
                let message = format!(
                    "`focus` names `{focus}`, but no button, slider, toggle, drop-down or text \
                     field has that name"
                );
                Error::new("ui.no_node", message)
            })?;
            Some(found as u32)
        }
    };
    Ok(Document { nodes, focus })
}

/// Names asset `name`, by ID or path; it loads after the parse.
fn named<T>(name: String) -> Option<Named<T>> {
    Some(Named {
        name: name.into(),
        handle: None,
    })
}

/// Appends `built` and its descendants to `nodes`, each after its parent.
fn flatten(built: Built, parent: Option<u32>, nodes: &mut Vec<Node>) {
    let at = nodes.len();
    let Built { mut node, children } = built;
    node.parent = parent;
    nodes.push(node);
    for child in children {
        flatten(child, Some(at as u32), nodes);
    }
    nodes[at].end = nodes.len() as u32;
}

/// A refusal of `code`. Serde carries the message; the parse takes the code back.
fn refuse<E: serde::de::Error>(code: &'static str, message: &str) -> E {
    PARSING.with_borrow_mut(|parsing| parsing.code = Some(code));
    E::custom(message)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    focus: Option<String>,
    nodes: Vec<Checked>,
}

#[derive(Deserialize)]
enum RawNode {
    Panel(Fields),
    Text(Fields),
    Image(Fields),
    Button(Fields),
    Slider(Fields),
    Toggle(Fields),
    Dropdown(Fields),
    Field(Fields),
    Row(Fields),
    Column(Fields),
    Stack(Fields),
    List(Fields),
}

/// Every field a node may have; [`Kind::takes`] says which kinds take which.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Fields {
    name: Option<NodeName>,
    visible: Option<bool>,
    width: Option<RawSize>,
    height: Option<RawSize>,
    margin: Option<(Units, Units)>,
    anchor: Option<(Side, Side)>,
    padding: Option<(Units, Units)>,
    gap: Option<Units>,
    align: Option<(Side, Side)>,
    children: Option<Vec<Checked>>,
    color: Option<(f32, f32, f32, f32)>,
    texture: Option<String>,
    radius: Option<Units>,
    slice: Option<(f32, Units)>,
    text: Option<String>,
    font: Option<String>,
    size: Option<Units>,
    disabled: Option<bool>,
    looks: Option<RawLooks>,
    accent: Option<(f32, f32, f32, f32)>,
    clip: Option<bool>,
    min: Option<Number>,
    max: Option<Number>,
    step: Option<Units>,
    value: Option<Number>,
    on: Option<bool>,
    options: Option<Vec<String>>,
    choice: Option<u32>,
    hint: Option<String>,
    most: Option<u32>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawLooks {
    normal: Option<RawLook>,
    focused: Option<RawLook>,
    pressed: Option<RawLook>,
    disabled: Option<RawLook>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawLook {
    color: Option<(f32, f32, f32, f32)>,
    text_color: Option<(f32, f32, f32, f32)>,
    texture: Option<String>,
}

#[derive(Clone, Copy, Deserialize)]
enum RawSize {
    Fixed(Units),
    Fit,
    Fill(Units),
}

/// Canvas units or a fill's share, 0 to [`MOST_UNITS`], checked as read so a refusal says where.
#[derive(Clone, Copy)]
struct Units(f32);

impl<'de> Deserialize<'de> for Units {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let units = f32::deserialize(deserializer)?;
        if !(0.0..=MOST_UNITS).contains(&units) {
            let message = format!(
                "{units} is out of range: a size, a margin, padding, a gap, a radius, a slice's \
                 width, a text's size and a fill's share are each from 0 to 1,000,000"
            );
            return Err(refuse("ui.bad_value", &message));
        }
        Ok(Self(units))
    }
}

/// A slider's bound or value, -[`MOST_UNITS`] to [`MOST_UNITS`], checked as read so a refusal says
/// where.
#[derive(Clone, Copy)]
struct Number(f32);

impl<'de> Deserialize<'de> for Number {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let number = f32::deserialize(deserializer)?;
        if !(-MOST_UNITS..=MOST_UNITS).contains(&number) {
            let message = format!(
                "{number} is out of range: a slider's bounds and value are each from -1,000,000 \
                 to 1,000,000"
            );
            return Err(refuse("ui.bad_value", &message));
        }
        Ok(Self(number))
    }
}

#[derive(Clone, Copy, Deserialize)]
enum Side {
    Start,
    Center,
    End,
}

impl From<Side> for Align {
    fn from(side: Side) -> Self {
        match side {
            Side::Start => Self::Start,
            Side::Center => Self::Center,
            Side::End => Self::End,
        }
    }
}

/// A node's name, checked as read so a refusal says where: 1 to [`MOST_NAME`] ASCII letters,
/// digits, `_` and `-` (plain for code and tools to write), unique within its UI.
struct NodeName(Arc<str>);

impl<'de> Deserialize<'de> for NodeName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        let plain = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
        if name.is_empty() || name.len() > MOST_NAME || !name.bytes().all(plain) {
            let message = format!(
                "{name:?} isn't a node's name, which is 1 to {MOST_NAME} ASCII letters, digits, \
                 `_` and `-`"
            );
            return Err(refuse("ui.bad_name", &message));
        }
        let name: Arc<str> = name.into();
        if !PARSING.with_borrow_mut(|parsing| parsing.names.insert(name.clone())) {
            let message = format!(
                "a node is named `{name}` already, and code and tools find a node by its name, \
                 which is its UI's alone"
            );
            return Err(refuse("ui.duplicate_name", &message));
        }
        Ok(Self(name))
    }
}

/// A node and its descendants, checked as read so a refusal says where.
struct Checked(Built);

/// A node and its children, before it takes its place among its UI's nodes.
struct Built {
    node: Node,
    children: Vec<Built>,
}

impl<'de> Deserialize<'de> for Checked {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = RawNode::deserialize(deserializer)?;
        let nodes = PARSING.with_borrow_mut(|parsing| {
            parsing.nodes += 1;
            parsing.nodes
        });
        if nodes > MOST_NODES {
            let message = format!("a UI has at most {MOST_NODES} nodes, and this is one more");
            return Err(refuse("ui.too_many_nodes", &message));
        }
        let (kind, fields) = match raw {
            RawNode::Panel(fields) => (Kind::Panel, fields),
            RawNode::Text(fields) => (Kind::Text, fields),
            RawNode::Image(fields) => (Kind::Image, fields),
            RawNode::Button(fields) => (Kind::Button, fields),
            RawNode::Slider(fields) => (Kind::Slider, fields),
            RawNode::Toggle(fields) => (Kind::Toggle, fields),
            RawNode::Dropdown(fields) => (Kind::Dropdown, fields),
            RawNode::Field(fields) => (Kind::Field, fields),
            RawNode::Row(fields) => (Kind::Row, fields),
            RawNode::Column(fields) => (Kind::Column, fields),
            RawNode::Stack(fields) => (Kind::Stack, fields),
            RawNode::List(fields) => (Kind::List, fields),
        };
        build(kind, fields)
            .map(Checked)
            .map_err(|(code, message)| refuse(code, &message))
    }
}

impl Kind {
    /// Whether this kind takes `field`. Every kind takes name, visibility, size, margin and
    /// anchor.
    fn takes(self, field: &str) -> bool {
        use Kind::{
            Button, Column, Dropdown, Field, Image, List, Panel, Row, Slider, Stack, Text, Toggle,
        };
        match field {
            "padding" => !matches!(self, Text | Image),
            "gap" => matches!(self, Row | Column | List),
            "align" => self != Image,
            "children" => matches!(self, Panel | Row | Column | Stack | List),
            "color" => matches!(self, Panel | Text | Image | Dropdown | List),
            "texture" => matches!(self, Panel | Image),
            "radius" => matches!(self, Panel | List) || self.focuses(),
            "slice" => matches!(self, Panel | Button),
            "text" => matches!(self, Text | Button | Toggle | Field),
            "font" | "size" => matches!(self, Text | Button | Toggle | Dropdown | Field),
            "disabled" | "looks" => self.focuses(),
            "accent" => matches!(self, Slider | Toggle | Dropdown | Field | List),
            "clip" => matches!(self, Panel | Row | Column | Stack),
            "min" | "max" | "step" | "value" => self == Slider,
            "on" => self == Toggle,
            "options" | "choice" => self == Dropdown,
            "hint" | "most" => self == Field,
            _ => true,
        }
    }
}

/// The node of `kind` that `fields` give, with its children and defaults; or the code and message
/// of its refusal, as [`parse`] says.
fn build(kind: Kind, fields: Fields) -> std::result::Result<Built, (&'static str, String)> {
    let Fields {
        name,
        visible,
        width,
        height,
        margin,
        anchor,
        padding,
        gap,
        align,
        children,
        color,
        texture,
        radius,
        slice,
        text,
        font,
        size: em,
        disabled,
        looks,
        accent,
        clip,
        min,
        max,
        step,
        value,
        on,
        options,
        choice,
        hint,
        most,
    } = fields;
    let name = name.map(|name| name.0);
    let called = match &name {
        Some(name) => format!("{} `{name}`", kind.called()),
        None => kind.called().to_owned(),
    };
    let given = [
        ("padding", padding.is_some()),
        ("gap", gap.is_some()),
        ("align", align.is_some()),
        ("children", children.is_some()),
        ("color", color.is_some()),
        ("texture", texture.is_some()),
        ("radius", radius.is_some()),
        ("slice", slice.is_some()),
        ("text", text.is_some()),
        ("font", font.is_some()),
        ("size", em.is_some()),
        ("disabled", disabled.is_some()),
        ("looks", looks.is_some()),
        ("accent", accent.is_some()),
        ("clip", clip.is_some()),
        ("min", min.is_some()),
        ("max", max.is_some()),
        ("step", step.is_some()),
        ("value", value.is_some()),
        ("on", on.is_some()),
        ("options", options.is_some()),
        ("choice", choice.is_some()),
        ("hint", hint.is_some()),
        ("most", most.is_some()),
    ];
    if let Some((field, _)) = given
        .iter()
        .find(|&&(field, given)| given && !kind.takes(field))
    {
        return Err(("ui.bad_field", format!("{called} takes no `{field}`")));
    }
    if kind == Kind::Button && name.is_none() {
        let message = format!("{called} has no `name`, which code is told its presses by");
        return Err(("ui.unnamed_button", message));
    }
    if kind.focuses() && name.is_none() {
        let message = format!("{called} has no `name`, which code is told its changes by");
        return Err(("ui.unnamed_widget", message));
    }
    if kind == Kind::Image && texture.is_none() {
        return Err(("ui.bad_field", format!("{called} names no `texture`")));
    }
    // A slider is as wide as a menu's buttons, as tall as its thumb.
    let fit = match kind {
        Kind::Slider => [RawSize::Fixed(Units(400.0)), RawSize::Fixed(Units(48.0))],
        _ => [RawSize::Fit; 2],
    };
    let size_of = |field: &str, given: Option<RawSize>, fit| match given.unwrap_or(fit) {
        RawSize::Fixed(Units(units)) => Ok(Size::Fixed(units)),
        RawSize::Fit => Ok(Size::Fit),
        RawSize::Fill(Units(share)) if share > 0.0 => Ok(Size::Fill(share)),
        RawSize::Fill(Units(share)) => {
            let why = "a fill's share is above 0";
            Err(bad(&called, field, format!("Fill({share})"), why))
        }
    };
    let size = [
        size_of("width", width, fit[0])?,
        size_of("height", height, fit[1])?,
    ];
    let pair = |given: Option<(Units, Units)>, default: Vec2| {
        given.map_or(default, |(across, down)| Vec2::new(across.0, down.0))
    };
    let margin = pair(margin, Vec2::ZERO);
    let padding = match kind {
        Kind::Button => pair(padding, Vec2::new(32.0, 14.0)),
        Kind::Toggle | Kind::Dropdown | Kind::Field => pair(padding, Vec2::new(24.0, 12.0)),
        Kind::Slider => pair(padding, Vec2::new(24.0, 0.0)),
        _ => pair(padding, Vec2::ZERO),
    };
    let gap = gap.map_or(0.0, |gap| gap.0);
    let radius = radius.map_or(if kind.focuses() { 10.0 } else { 0.0 }, |radius| radius.0);
    let text_size = em.map_or(if kind == Kind::Button { 34.0 } else { 32.0 }, |em| em.0);
    if text_size <= 0.0 {
        return Err(bad(
            &called,
            "size",
            text_size.to_string(),
            "a text's size is above 0",
        ));
    }
    let slice = match slice {
        Some((fraction, _)) if !(0.0..=0.5).contains(&fraction) => {
            let why = "a slice is a fraction of its texture, 0 to 0.5, and its width";
            return Err(bad(&called, "slice", fraction.to_string(), why));
        }
        slice => slice.map(|(fraction, units)| (fraction, units.0)),
    };
    let looks = if kind.focuses() {
        widget_looks(&called, kind, looks)?
    } else {
        plain_looks(&called, kind, color, texture)?
    };
    let accent = accent.map_or(Ok(ACCENT), |accent| colored(&called, "accent", accent))?;
    let text = text.unwrap_or_default();
    let widget = match kind {
        Kind::Slider => slider(&called, min, max, step, value)?,
        Kind::Toggle => Widget::Toggle {
            on: on.unwrap_or(false),
        },
        Kind::Dropdown => dropdown(&called, options, choice, color)?,
        Kind::Field => text_field(&called, &text, hint, most)?,
        _ => Widget::None,
    };
    let align = align.map_or(
        match kind {
            // Text from the left; a list's children from the top.
            Kind::Toggle | Kind::Dropdown | Kind::Field => [Align::Start, Align::Center],
            Kind::List => [Align::Center, Align::Start],
            _ => [Align::Center; 2],
        },
        |(across, down)| [across.into(), down.into()],
    );
    let node = Node {
        name,
        kind,
        end: 0,
        parent: None,
        visible: visible.unwrap_or(true),
        disabled: disabled.unwrap_or(false),
        size,
        margin,
        padding,
        gap,
        align,
        anchor: anchor.map(|(across, down)| [across.into(), down.into()]),
        text,
        font: font.and_then(named),
        text_size,
        radius,
        slice,
        looks,
        natural: None,
        widget,
        accent,
        clips: clip.unwrap_or(kind == Kind::List),
        scroll: 0.0,
    };
    let children = children.unwrap_or_default();
    Ok(Built {
        node,
        children: children.into_iter().map(|child| child.0).collect(),
    })
}

/// The `ui.bad_value` refusal of `called`'s `field` being `value`, saying `why`.
fn bad(called: &str, field: &str, value: String, why: &str) -> (&'static str, String) {
    let message = format!("{called}'s `{field}` is {value}, and {why}");
    ("ui.bad_value", message)
}

/// `given` as a color, refused unless finite.
fn colored(
    called: &str,
    field: &str,
    given: (f32, f32, f32, f32),
) -> std::result::Result<Vec4, (&'static str, String)> {
    let color = Vec4::from(given);
    color
        .is_finite()
        .then_some(color)
        .ok_or_else(|| bad(called, field, format!("{given:?}"), "a color is finite"))
}

/// A button's or widget's four looks: `looks` over its kind's defaults.
fn widget_looks(
    called: &str,
    kind: Kind,
    looks: Option<RawLooks>,
) -> std::result::Result<[Look; 4], (&'static str, String)> {
    let RawLooks {
        normal,
        focused,
        pressed,
        disabled,
    } = looks.unwrap_or_default();
    let mut built = if kind == Kind::Button { BUTTON } else { WIDGET };
    for (look, given) in built.iter_mut().zip([normal, focused, pressed, disabled]) {
        let RawLook {
            color,
            text_color,
            texture,
        } = given.unwrap_or_default();
        if let Some(color) = color {
            look.color = colored(called, "looks' color", color)?;
        }
        if let Some(text) = text_color {
            look.text = colored(called, "looks' text_color", text)?;
        }
        look.texture = texture.and_then(named);
    }
    // Each look takes the normal look's texture unless it gives its own.
    let normal = built[0].texture.clone();
    for look in &mut built[1..] {
        if look.texture.is_none() {
            look.texture.clone_from(&normal);
        }
    }
    Ok(built)
}

/// The one look, of `color` and `texture`, of a node that's no button or widget.
fn plain_looks(
    called: &str,
    kind: Kind,
    color: Option<(f32, f32, f32, f32)>,
    texture: Option<String>,
) -> std::result::Result<[Look; 4], (&'static str, String)> {
    // A list is clear unless its file gives a color.
    let unseen = if kind == Kind::List {
        Vec4::ZERO
    } else {
        Vec4::ONE
    };
    let color = color.map_or(Ok(unseen), |color| colored(called, "color", color))?;
    let look = Look {
        color,
        text: color,
        texture: texture.and_then(named),
    };
    Ok([look.clone(), look.clone(), look.clone(), look])
}

fn slider(
    called: &str,
    min: Option<Number>,
    max: Option<Number>,
    step: Option<Units>,
    value: Option<Number>,
) -> std::result::Result<Widget, (&'static str, String)> {
    let (min, max) = (min.map_or(0.0, |min| min.0), max.map_or(1.0, |max| max.0));
    if max <= min {
        let why = format!("a slider's `max` is above its `min`, {min}");
        return Err(bad(called, "max", max.to_string(), &why));
    }
    let value = value.map_or(min, |value| value.0);
    if !(min..=max).contains(&value) {
        let why = format!("a slider's `value` is from its `min`, {min}, to its `max`");
        return Err(bad(called, "value", value.to_string(), &why));
    }
    let step = step.map_or(0.0, |step| step.0);
    Ok(Widget::Slider {
        min,
        max,
        step,
        value,
    })
}

/// A drop-down of `options`, `choice` chosen, `color` behind them when open.
fn dropdown(
    called: &str,
    options: Option<Vec<String>>,
    choice: Option<u32>,
    color: Option<(f32, f32, f32, f32)>,
) -> std::result::Result<Widget, (&'static str, String)> {
    let Some(options) = options.filter(|options| !options.is_empty()) else {
        return Err(("ui.bad_field", format!("{called} names no `options`")));
    };
    if options.len() > MOST_OPTIONS {
        let why = format!("a drop-down has at most {MOST_OPTIONS} options");
        return Err(bad(
            called,
            "options",
            format!("{} long", options.len()),
            &why,
        ));
    }
    let choice = choice.unwrap_or(0);
    if choice as usize >= options.len() {
        let why = format!("a drop-down's `choice` is one of its {}", options.len());
        return Err(bad(called, "choice", choice.to_string(), &why));
    }
    let backing = color.map_or(Ok(BACKING), |color| colored(called, "color", color))?;
    Ok(Widget::Dropdown {
        options,
        choice: choice as usize,
        backing,
    })
}

/// A text field holding `text`, at most `most` characters, showing `hint` while empty.
fn text_field(
    called: &str,
    text: &str,
    hint: Option<String>,
    most: Option<u32>,
) -> std::result::Result<Widget, (&'static str, String)> {
    let most = most.unwrap_or(64);
    if !(1..=MOST_TYPED).contains(&most) {
        let why = format!("a text field holds from 1 to {MOST_TYPED} characters");
        return Err(bad(called, "most", most.to_string(), &why));
    }
    let typed = text.chars().count();
    if typed > most as usize || text.chars().any(char::is_control) {
        let why =
            format!("a text field's text is a line of at most the {most} characters it holds");
        return Err(bad(called, "text", format!("{typed} characters"), &why));
    }
    Ok(Widget::Field {
        hint: hint.unwrap_or_default(),
        most: most as usize,
    })
}
