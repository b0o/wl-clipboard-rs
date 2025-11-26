use std::fs::OpenOptions;

use clap::Parser;
use libc::fork;
use rustix::stdio::{dup2_stdin, dup2_stdout};
use wl_clipboard_rs::copy::{self, clear, ClipboardType, Seat, ServeRequests};
use wl_clipboard_rs_tools::wl_copy::{resolve_sources, Options};

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
