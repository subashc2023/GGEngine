//! The console format: one line per event, colored so the message stands out and the context
//! around it recedes.
//!
//! ```text
//!    0.505  INFO  gpu::device     GPU selected  name="NVIDIA GeForce RTX 4090" kind=discrete
//!    0.552 ERROR  gpu::vulkan     [gpu.validation] vkCreateImageView(): ...  vuid=VUID-...
//! ```
//!
//! A line holds the seconds since logging started, the level, where the event came from (an
//! engine crate without its `gg_` prefix, and its module), the event's error code if it has one,
//! the message and the other fields, and the spans the event happened in. Warnings and errors
//! end with the source line that logged them. Field names recede and their values are colored by
//! kind, as a syntax highlighter colors code: text, numbers and booleans each in their own color.

use std::fmt::{self, Write as _};
use std::time::Instant;

use nu_ansi_term::{Color, Style};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, FormattedFields};
use tracing_subscriber::registry::LookupSpan;

/// The width the source column is padded to, so that messages line up.
const SOURCE_WIDTH: usize = 15;

/// Where messages start: after the time, the level and the source. Lines after the first in a
/// multi-line message or value start here too.
const MESSAGE_COLUMN: usize = 8 + 8 + SOURCE_WIDTH + 1;

pub(super) struct Console {
    start: Instant,
    colors: bool,
}

impl Console {
    pub(super) fn new(start: Instant, colors: bool) -> Self {
        Self { start, colors }
    }
}

impl<S, N> FormatEvent<S, N> for Console
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut w: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let meta = event.metadata();
        let level = *meta.level();
        let s = Styles::new(level, self.colors);

        let time = self.start.elapsed();
        write!(
            w,
            "{}{:>4}.{:03}{}",
            s.dim.prefix(),
            time.as_secs(),
            time.subsec_millis(),
            s.dim.suffix()
        )?;
        write!(
            w,
            "{} {:>5} {} ",
            s.level.prefix(),
            level.as_str(),
            s.level.suffix()
        )?;
        write_source(&mut w, meta.target(), &s)?;

        let mut fields = Fields {
            w: &mut w,
            s: &s,
            pass: Pass::Code,
            first: true,
            result: Ok(()),
        };
        for pass in [Pass::Code, Pass::Message, Pass::Rest] {
            fields.pass = pass;
            event.record(&mut fields);
        }
        fields.result?;

        if let Some(scope) = ctx.event_scope() {
            write!(w, "{}", s.dim.prefix())?;
            let mut separator = "  in ";
            for span in scope.from_root() {
                write!(w, "{separator}{}", span.name())?;
                separator = " > ";
                let extensions = span.extensions();
                if let Some(fields) = extensions.get::<FormattedFields<N>>()
                    && !fields.is_empty()
                {
                    write!(w, "{{{fields}}}")?;
                }
            }
            write!(w, "{}", s.dim.suffix())?;
        }
        if level <= Level::WARN
            && let (Some(file), Some(line)) = (meta.file(), meta.line())
        {
            write!(w, "  {}{file}:{line}{}", s.dim.prefix(), s.dim.suffix())?;
        }
        writeln!(w)
    }
}

/// Writes where an event came from, padded: target `gg_gpu::device::pick` becomes
/// `gpu::device`, with the engine's crates in one color and every other crate in another.
fn write_source(w: &mut Writer<'_>, target: &str, s: &Styles) -> fmt::Result {
    let mut path = target.split("::");
    let krate = path.next().unwrap_or_default();
    let (krate, style) = match krate.strip_prefix("gg_") {
        Some(engine) => (engine, s.engine),
        None => (krate, s.foreign),
    };
    write!(w, "{}{krate}{}", style.prefix(), style.suffix())?;
    let mut width = krate.len();
    if let Some(module) = path.next() {
        write!(w, "{}::{module}{}", s.dim.prefix(), s.dim.suffix())?;
        width += 2 + module.len();
    }
    write!(w, "{:pad$} ", "", pad = SOURCE_WIDTH.saturating_sub(width))
}

/// The colors of one event's line, all plain when the console doesn't take colors.
struct Styles {
    /// Context that recedes: the time, modules, field names, spans and source lines.
    dim: Style,
    level: Style,
    code: Style,
    message: Style,
    engine: Style,
    foreign: Style,
    /// Field values, by kind.
    text: Style,
    number: Style,
    boolean: Style,
}

impl Styles {
    fn new(level: Level, colors: bool) -> Self {
        let plain = Style::new();
        if !colors {
            return Self {
                dim: plain,
                level: plain,
                code: plain,
                message: plain,
                engine: plain,
                foreign: plain,
                text: plain,
                number: plain,
                boolean: plain,
            };
        }
        let dim = Color::DarkGray.normal();
        // Errors and warnings get a badge, and their message keeps the badge's color.
        let (badge, message) = match level {
            Level::ERROR => (Color::White.on(Color::Red).bold(), Color::LightRed.bold()),
            Level::WARN => (
                Color::Black.on(Color::Yellow).bold(),
                Color::LightYellow.bold(),
            ),
            Level::INFO => (Color::LightGreen.bold(), plain.bold()),
            Level::DEBUG => (Color::LightBlue.bold(), plain),
            _ => (Color::Purple.normal(), dim),
        };
        // Trace events are the least wanted, so their values recede with their message.
        let value = |color: Color| {
            if level == Level::TRACE {
                dim
            } else {
                color.normal()
            }
        };
        Self {
            dim,
            level: badge,
            code: if level <= Level::WARN { message } else { dim },
            message,
            engine: Color::Cyan.normal(),
            foreign: Color::Purple.normal(),
            text: value(Color::Green),
            number: value(Color::LightCyan),
            boolean: value(Color::Yellow),
        }
    }
}

#[derive(Clone, Copy)]
enum Pass {
    Code,
    Message,
    Rest,
}

/// Writes an event's fields over three passes: its code, its message, then the others.
struct Fields<'a, 'w> {
    w: &'a mut Writer<'w>,
    s: &'a Styles,
    pass: Pass,
    first: bool,
    result: fmt::Result,
}

impl Fields<'_, '_> {
    /// Writes a field if this pass is for it; `style` is the value's color among the others.
    fn write(&mut self, field: &Field, value: Value<'_>, style: Style) {
        if self.result.is_err() {
            return;
        }
        let s = self.s;
        self.result = match (self.pass, field.name()) {
            (Pass::Code, "code") => {
                write!(self.w, "{}[{value}]{} ", s.code.prefix(), s.code.suffix())
            }
            (Pass::Message, "message") => {
                write!(
                    self.w,
                    "{}{value}{}",
                    s.message.prefix(),
                    s.message.suffix()
                )
            }
            // `log.*` fields carry the metadata of events from the `log` crate.
            (Pass::Rest, name)
                if !matches!(name, "code" | "message") && !name.starts_with("log.") =>
            {
                let separator = if std::mem::take(&mut self.first) {
                    "  "
                } else {
                    " "
                };
                let name = name.strip_prefix("r#").unwrap_or(name);
                write!(
                    self.w,
                    "{separator}{}{name}={}{}{}{}",
                    s.dim.prefix(),
                    s.dim.suffix(),
                    style.prefix(),
                    Quoted(value),
                    style.suffix()
                )
            }
            _ => Ok(()),
        };
    }
}

// Values logged with `%` or `?` arrive through `record_debug`, so they count as text whatever
// their type.
impl Visit for Fields<'_, '_> {
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.write(field, Value::Debug(&value), self.s.number);
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.write(field, Value::Debug(&value), self.s.number);
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.write(field, Value::Debug(&value), self.s.number);
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        self.write(field, Value::Debug(&value), self.s.number);
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        self.write(field, Value::Debug(&value), self.s.number);
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.write(field, Value::Debug(&value), self.s.boolean);
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.write(field, Value::Str(value), self.s.text);
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.write(field, Value::Debug(value), self.s.text);
    }
}

/// A field's value, written with control characters escaped, so a logged string can't move the
/// cursor or change the console's colors.
#[derive(Clone, Copy)]
enum Value<'a> {
    Str(&'a str),
    Debug(&'a dyn fmt::Debug),
}

impl fmt::Display for Value<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut f = Escape(f);
        match self {
            Value::Str(text) => f.write_str(text),
            Value::Debug(value) => write!(f, "{value:?}"),
        }
    }
}

/// A value among the fields after the message: an empty string, or one with spaces, is quoted so
/// it's clear where it ends.
struct Quoted<'a>(Value<'a>);

impl fmt::Display for Quoted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Value::Str(text) if text.is_empty() || text.contains(char::is_whitespace) => {
                write!(f, "{text:?}")
            }
            value => value.fmt(f),
        }
    }
}

/// Passes text through, escaping control characters other than tab, and starting the lines after
/// a newline at the message column.
struct Escape<'a, 'b>(&'a mut fmt::Formatter<'b>);

impl fmt::Write for Escape<'_, '_> {
    fn write_str(&mut self, mut text: &str) -> fmt::Result {
        while let Some((i, c)) = text
            .char_indices()
            .find(|&(_, c)| c.is_control() && c != '\t')
        {
            self.0.write_str(&text[..i])?;
            if c == '\n' {
                write!(self.0, "\n{:width$}", "", width = MESSAGE_COLUMN)?;
            } else {
                write!(self.0, "{}", c.escape_unicode())?;
            }
            text = &text[i + c.len_utf8()..];
        }
        self.0.write_str(text)
    }
}
