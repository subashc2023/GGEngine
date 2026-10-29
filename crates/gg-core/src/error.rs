//! The error type the engine's crates return.

use std::fmt;

type Source = Box<dyn std::error::Error + Send + Sync + 'static>;

/// An engine error: a stable code, a message that names what failed, and what caused it.
///
/// Tools and agents match on the code, so a code names one kind of failure and keeps its
/// meaning. Codes are dotted names, such as `gpu.device_lost`.
pub struct Error {
    code: &'static str,
    message: String,
    source: Option<Source>,
}

/// What the engine's crates return: `T`, or an [`Error`] unless it names another error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// An error of `code`, saying `message`.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            source: None,
        }
    }

    /// Records the lower-level error that caused this one.
    #[must_use]
    pub fn with_source(mut self, source: impl Into<Source>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// This error with `message` in place of its own, keeping its code and what caused it: for an
    /// error that says more of where it happened as it's passed up.
    #[must_use]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    /// Its code, which tools and agents match on.
    pub fn code(&self) -> &'static str {
        self.code
    }
}

/// `{}` writes the message; `{:#}` follows it with each cause in turn, after `: `.
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        if f.alternate() {
            let mut cause = std::error::Error::source(self);
            while let Some(error) = cause {
                write!(f, ": {error}")?;
                cause = error.source();
            }
        }
        Ok(())
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {self:#}", self.code)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_deref().map(|source| source as _)
    }
}
