//! The library's typed error.
//!
//! Everything under `registry/` and `vcs/` returns [`Error`]. The command layer
//! (`commands/`) wraps these in `anyhow::Result` and attaches human context, so
//! this enum stays focused on *what went wrong*, not *what the user was doing*.

use std::path::PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no repo registered under alias `{0}`\nrun `grit show` to see what is registered")]
    UnknownAlias(String),

    #[error("no repos tagged `{0}`\nrun `grit show` to see the available tags")]
    UnknownTag(String),

    #[error("`{0}` is a grit command, so it can't be used as an alias")]
    ReservedAlias(String),

    #[error("alias `{alias}` is already registered to {}\npass --force to overwrite it", existing.display())]
    DuplicateAlias { alias: String, existing: PathBuf },

    #[error(
        "invalid alias `{alias}`: {reason}\naliases may contain letters, digits, `-`, `_` and `.`"
    )]
    InvalidAlias { alias: String, reason: String },

    #[error("{} is not a {kinds} repository", path.display())]
    NotARepo { path: PathBuf, kinds: String },

    #[error("{} does not exist", .0.display())]
    PathNotFound(PathBuf),

    #[error("could not work out where to store grit's config: {0}")]
    NoConfigHome(String),

    #[error("config file {} is not valid TOML", path.display())]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error(
        "config file {} was written by a newer grit (format v{found}, this build understands v{supported})",
        path.display()
    )]
    ConfigVersion {
        path: PathBuf,
        found: u32,
        supported: u32,
    },

    #[error("{context} ({})", path.display())]
    Io {
        context: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not run `{program}`: {source}\nis it installed and on your PATH?")]
    Spawn {
        program: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "could not make sense of `{program}`'s output: {source}\nthis usually means the installed {program} is a different version than grit expects"
    )]
    BadOutput {
        program: &'static str,
        #[source]
        source: serde_json::Error,
    },

    #[error("`{program} {args}` failed with {status}: {stderr}")]
    CommandFailed {
        program: &'static str,
        /// A short, single-line rendering of the invocation. Long arguments — a
        /// multi-line SQL query, say — must be summarised rather than pasted:
        /// `grit status` shows only an error's first line, so a newline in here
        /// hides everything after it, the backend's own explanation included.
        args: String,
        status: String,
        /// The backend's explanation, on the same line for that same reason.
        /// Flatten it with [`crate::vcs::one_line`].
        stderr: String,
    },
}

impl Error {
    /// Attach a path and a short human phrase to an [`std::io::Error`].
    ///
    /// `Error::io("could not read the config file", path)` reads as a closure
    /// you can hand straight to `map_err`.
    pub fn io(
        context: &'static str,
        path: impl Into<PathBuf>,
    ) -> impl FnOnce(std::io::Error) -> Self {
        let path = path.into();
        move |source| Error::Io {
            context,
            path,
            source,
        }
    }
}
