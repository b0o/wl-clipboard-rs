use std::ffi::OsString;
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::Path;

use clap::Parser;
use wl_clipboard_rs::copy::{MimeSource, MimeType, Source};

#[derive(Parser)]
#[command(
    name = "wl-copy",
    version,
    about = "Copy clipboard contents on Wayland."
)]
pub struct Options {
    /// Serve only a single paste request and then exit
    ///
    /// This option effectively clears the clipboard after the first paste. It can be used when
    /// copying e.g. sensitive data, like passwords. Note however that certain apps may have issues
    /// pasting when this option is used, in particular XWayland clients are known to suffer from
    /// this.
    #[arg(long, short = 'o', conflicts_with = "clear")]
    pub paste_once: bool,

    /// Stay in the foreground instead of forking
    #[arg(long, short, conflicts_with = "clear")]
    pub foreground: bool,

    /// Clear the clipboard instead of copying
    #[arg(long, short)]
    pub clear: bool,

    /// Use the "primary" clipboard
    ///
    /// Copying to the "primary" clipboard requires the compositor to support the data-control
    /// protocol of version 2 or above.
    #[arg(long, short)]
    pub primary: bool,

    /// Use the regular clipboard
    ///
    /// Set this flag together with --primary to operate on both clipboards at once. Has no effect
    /// otherwise (since the regular clipboard is the default clipboard).
    #[arg(long, short)]
    pub regular: bool,

    /// Trim the trailing newline character before copying
    ///
    /// This flag is only applied for text MIME types.
    #[arg(long, short = 'n', conflicts_with = "clear")]
    pub trim_newline: bool,

    /// Pick the seat to work with
    ///
    /// By default wl-copy operates on all seats at once.
    #[arg(long, short)]
    pub seat: Option<String>,

    /// Override the inferred MIME type for the content
    #[arg(
        name = "MIME/TYPE",
        long = "type",
        short = 't',
        conflicts_with = "clear"
    )]
    pub mime_type: Option<String>,

    /// Add a file as a MIME source
    ///
    /// Takes two arguments: MIME type (or "auto" for auto-detection) and file path.
    /// Can be specified multiple times.
    #[arg(
        short = 'F',
        long = "file",
        num_args = 2,
        value_names = ["MIME", "PATH"],
        conflicts_with = "clear",
        action = clap::ArgAction::Append
    )]
    pub file_sources: Vec<String>,

    /// Add stdin as a MIME source
    ///
    /// Takes one argument: MIME type (or "auto" for auto-detection).
    /// Can only be used once.
    #[arg(long = "stdin", value_name = "MIME", conflicts_with = "clear")]
    pub stdin_source: Option<String>,

    /// Add a literal string as a MIME source
    ///
    /// Takes two arguments: MIME type and literal string data.
    /// Can be specified multiple times.
    #[arg(
        short = 'L',
        long = "literal",
        num_args = 2,
        value_names = ["MIME", "DATA"],
        conflicts_with = "clear",
        action = clap::ArgAction::Append
    )]
    pub literal_sources: Vec<String>,

    /// Disable automatic text MIME type fallbacks
    ///
    /// By default, when copying text, additional MIME types like text/plain, UTF8_STRING,
    /// STRING, and TEXT are automatically offered. This flag disables that behavior.
    #[arg(long = "no-text-fallback", conflicts_with = "clear")]
    pub no_text_fallback: bool,

    /// Text to copy
    ///
    /// If not specified, wl-copy will use data from the standard input.
    #[arg(name = "TEXT TO COPY", conflicts_with = "clear")]
    pub text: Vec<OsString>,

    /// Enable verbose logging
    #[arg(long, short, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

/// Parse a MIME type string, treating "auto" as auto-detection.
fn parse_mime_type(mime: &str) -> MimeType {
    if mime.eq_ignore_ascii_case("auto") {
        MimeType::Autodetect
    } else {
        MimeType::Specific(mime.to_string())
    }
}

/// Errors that can occur during source resolution.
#[derive(Debug)]
pub enum SourceError {
    /// File does not exist
    FileNotFound(String),
    /// Failed to read file
    FileReadError(String, io::Error),
    /// --stdin used with no stdin connected
    NoStdinConnected,
    /// Multiple conflicting source specifications
    ConflictingSourceType(String),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceError::FileNotFound(path) => write!(f, "file not found: {}", path),
            SourceError::FileReadError(path, err) => {
                write!(f, "failed to read file '{}': {}", path, err)
            }
            SourceError::NoStdinConnected => {
                write!(f, "--stdin requires stdin to be connected (not a TTY)")
            }
            SourceError::ConflictingSourceType(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for SourceError {}

/// Resolve all source specifications into MimeSource entries.
///
/// Source resolution order:
/// 1. Process --file sources
/// 2. Process --stdin source (if specified)
/// 3. Process --literal sources
/// 4. Process default input (positional args or implicit stdin)
///
/// Note: Same MIME type from multiple sources = last wins (handled by library).
pub fn resolve_sources(options: &Options) -> Result<Vec<MimeSource>, SourceError> {
    let mut sources = Vec::new();

    // Process --file sources (pairs of [MIME, PATH])
    for chunk in options.file_sources.chunks(2) {
        if chunk.len() == 2 {
            let mime = parse_mime_type(&chunk[0]);
            let path = &chunk[1];

            if !Path::new(path).exists() {
                return Err(SourceError::FileNotFound(path.clone()));
            }

            let data = fs::read(path)
                .map_err(|e| SourceError::FileReadError(path.clone(), e))?;

            sources.push(MimeSource {
                source: Source::Bytes(data.into()),
                mime_type: mime,
            });
        }
    }

    // Process --stdin source
    if let Some(ref mime_str) = options.stdin_source {
        // Check if stdin is connected (not a TTY)
        if io::stdin().is_terminal() {
            return Err(SourceError::NoStdinConnected);
        }

        sources.push(MimeSource {
            source: Source::StdIn,
            mime_type: parse_mime_type(mime_str),
        });
    }

    // Process --literal sources (pairs of [MIME, DATA])
    for chunk in options.literal_sources.chunks(2) {
        if chunk.len() == 2 {
            let mime = parse_mime_type(&chunk[0]);
            let data = chunk[1].clone().into_bytes();

            sources.push(MimeSource {
                source: Source::Bytes(data.into()),
                mime_type: mime,
            });
        }
    }

    // Process default input (positional args or implicit stdin)
    // Only if no explicit --stdin was given
    if !options.text.is_empty() {
        // Positional args present - join them with spaces
        let mut iter = options.text.iter();
        let mut data = iter.next().unwrap().clone();
        for arg in iter {
            data.push(" ");
            data.push(arg);
        }

        let mime = options
            .mime_type
            .as_ref()
            .map(|m| parse_mime_type(m))
            .unwrap_or(MimeType::Autodetect);

        sources.push(MimeSource {
            source: Source::Bytes(data.into_encoded_bytes().into()),
            mime_type: mime,
        });
    } else if options.stdin_source.is_none() {
        // No positional args and no explicit --stdin - use implicit stdin
        // Only if stdin is actually connected
        if !io::stdin().is_terminal() {
            let mime = options
                .mime_type
                .as_ref()
                .map(|m| parse_mime_type(m))
                .unwrap_or(MimeType::Autodetect);

            sources.push(MimeSource {
                source: Source::StdIn,
                mime_type: mime,
            });
        }
    }

    Ok(sources)
}
