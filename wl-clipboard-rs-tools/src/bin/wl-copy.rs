use std::fs;
use std::fs::OpenOptions;
use std::io::{self, IsTerminal};
use std::path::Path;

use clap::Parser;
use libc::fork;
use rustix::stdio::{dup2_stdin, dup2_stdout};
use wl_clipboard_rs::copy::{self, clear, ClipboardType, MimeSource, MimeType, Seat, ServeRequests, Source};
use wl_clipboard_rs_tools::wl_copy::Options;

fn from_options(x: &Options) -> copy::Options {
    let mut opts = copy::Options::new();
    opts.serve_requests(if x.paste_once {
        ServeRequests::Only(1)
    } else {
        ServeRequests::Unlimited
    })
    .foreground(true) // We fork manually to support background mode.
    .clipboard(if x.primary {
        if x.regular {
            ClipboardType::Both
        } else {
            ClipboardType::Primary
        }
    } else {
        ClipboardType::Regular
    })
    .trim_newline(x.trim_newline)
    .seat(x.seat.clone().map(Seat::Specific).unwrap_or_default())
    .omit_additional_text_mime_types(x.no_text_fallback);
    opts
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
enum SourceError {
    /// File does not exist
    FileNotFound(String),
    /// Failed to read file
    FileReadError(String, io::Error),
    /// --stdin used with no stdin connected
    NoStdinConnected,
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
fn resolve_sources(options: &Options) -> Result<Vec<MimeSource>, SourceError> {
    let mut sources = Vec::new();

    // Process --file sources (pairs of [MIME, PATH])
    for chunk in options.file_sources.chunks(2) {
        if chunk.len() == 2 {
            let mime = parse_mime_type(&chunk[0]);
            let path = &chunk[1];

            if !Path::new(path).exists() {
                return Err(SourceError::FileNotFound(path.clone()));
            }

            let data = fs::read(path).map_err(|e| SourceError::FileReadError(path.clone(), e))?;

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
    // Only if no explicit sources were provided
    let has_explicit_sources = !options.file_sources.is_empty()
        || options.stdin_source.is_some()
        || !options.literal_sources.is_empty();

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
    } else if !has_explicit_sources {
        // No positional args and no explicit sources - use implicit stdin
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

fn main() -> Result<(), anyhow::Error> {
    // Parse command-line options.
    let options = Options::parse();

    stderrlog::new()
        .verbosity(usize::from(options.verbose) + 1)
        .init()
        .unwrap();

    if options.clear {
        let clipboard = if options.primary {
            ClipboardType::Primary
        } else {
            ClipboardType::Regular
        };
        clear(
            clipboard,
            options.seat.clone().map(Seat::Specific).unwrap_or_default(),
        )?;
        return Ok(());
    }

    // Resolve all sources
    let sources = resolve_sources(&options).map_err(|e| anyhow::anyhow!("{}", e))?;

    if sources.is_empty() {
        anyhow::bail!("no data to copy (specify text, use stdin, or use --file/--literal/--stdin)");
    }

    let foreground = options.foreground;
    let prepared_copy = from_options(&options).prepare_copy_multi(sources)?;

    if foreground {
        prepared_copy.serve()?;
    } else {
        // SAFETY: We don't spawn any threads, so doing things after forking is safe.
        // TODO: is there any way to verify that we don't spawn any threads?
        match unsafe { fork() } {
            -1 => panic!("error forking: {:?}", std::io::Error::last_os_error()),
            0 => {
                // Replace STDIN and STDOUT with /dev/null. We won't be using them, and keeping
                // them as is hangs a potential pipeline (i.e. wl-copy hello | cat). Also, simply
                // closing the file descriptors is a bad idea because then they get reused by
                // subsequent temp file opens, which breaks the dup2/close logic during data
                // copying.
                if let Ok(dev_null) = OpenOptions::new().read(true).write(true).open("/dev/null") {
                    let _ = dup2_stdin(&dev_null);
                    let _ = dup2_stdout(&dev_null);
                }

                drop(prepared_copy.serve());
            }
            _ => (),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_options() -> Options {
        Options {
            paste_once: false,
            foreground: false,
            clear: false,
            primary: false,
            regular: false,
            trim_newline: false,
            seat: None,
            mime_type: None,
            file_sources: vec![],
            stdin_source: None,
            literal_sources: vec![],
            no_text_fallback: false,
            text: vec![],
            verbose: 0,
        }
    }

    #[test]
    fn test_parse_mime_type_auto() {
        assert!(matches!(parse_mime_type("auto"), MimeType::Autodetect));
        assert!(matches!(parse_mime_type("AUTO"), MimeType::Autodetect));
        assert!(matches!(parse_mime_type("Auto"), MimeType::Autodetect));
    }

    #[test]
    fn test_parse_mime_type_specific() {
        match parse_mime_type("text/html") {
            MimeType::Specific(s) => assert_eq!(s, "text/html"),
            _ => panic!("expected Specific"),
        }
    }

    #[test]
    fn test_literal_source() {
        let mut opts = make_options();
        opts.literal_sources = vec!["text/plain".to_string(), "hello".to_string()];

        let sources = resolve_sources(&opts).unwrap();
        assert_eq!(sources.len(), 1);

        match &sources[0].source {
            Source::Bytes(data) => assert_eq!(&**data, b"hello"),
            _ => panic!("expected Bytes"),
        }
        match &sources[0].mime_type {
            MimeType::Specific(s) => assert_eq!(s, "text/plain"),
            _ => panic!("expected Specific"),
        }
    }

    #[test]
    fn test_multiple_literals() {
        let mut opts = make_options();
        opts.literal_sources = vec![
            "text/html".to_string(),
            "<b>hi</b>".to_string(),
            "text/plain".to_string(),
            "hi".to_string(),
        ];

        let sources = resolve_sources(&opts).unwrap();
        assert_eq!(sources.len(), 2);
    }

    #[test]
    fn test_positional_args() {
        let mut opts = make_options();
        opts.text = vec!["hello".into(), "world".into()];

        let sources = resolve_sources(&opts).unwrap();
        assert_eq!(sources.len(), 1);

        match &sources[0].source {
            Source::Bytes(data) => assert_eq!(&**data, b"hello world"),
            _ => panic!("expected Bytes"),
        }
    }

    #[test]
    fn test_positional_with_type() {
        let mut opts = make_options();
        opts.text = vec!["test".into()];
        opts.mime_type = Some("text/html".to_string());

        let sources = resolve_sources(&opts).unwrap();
        assert_eq!(sources.len(), 1);

        match &sources[0].mime_type {
            MimeType::Specific(s) => assert_eq!(s, "text/html"),
            _ => panic!("expected Specific"),
        }
    }

    #[test]
    fn test_file_not_found() {
        let mut opts = make_options();
        opts.file_sources = vec![
            "text/plain".to_string(),
            "/nonexistent/path/file.txt".to_string(),
        ];

        let result = resolve_sources(&opts);
        assert!(matches!(result, Err(SourceError::FileNotFound(_))));
    }

    #[test]
    fn test_literal_with_auto() {
        let mut opts = make_options();
        opts.literal_sources = vec!["auto".to_string(), "hello".to_string()];

        let sources = resolve_sources(&opts).unwrap();
        assert!(matches!(sources[0].mime_type, MimeType::Autodetect));
    }
}
