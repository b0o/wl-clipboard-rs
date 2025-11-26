use std::ffi::OsString;

use clap::Parser;

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
