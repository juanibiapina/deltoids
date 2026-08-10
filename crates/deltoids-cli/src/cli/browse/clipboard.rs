//! The one side-effecting clipboard adapter for the TUI.
//!
//! [`copy`] tries a platform clipboard helper (`pbcopy` on macOS,
//! `wl-copy` / `xclip` / `xsel` on Linux, `clip` on Windows) and also
//! emits an OSC 52 escape sequence, which can reach the reviewer's local
//! clipboard over SSH and through terminal multiplexers. Copying is never
//! fatal: the caller reports the failure and keeps browsing.
//!
//! The shell owns this call, since it owns terminal output; modes ask for
//! a copy through [`super::mode::AppCommand::CopyToClipboard`].

use std::io::{self, Write};
use std::process::{Command, Stdio};

/// Copy `text` to the system clipboard.
///
/// Returns `Err` only when neither a platform helper nor OSC 52 could be
/// delivered, so callers can report a failed copy
/// instead of claiming success.
pub(super) fn copy(text: &str) -> Result<(), String> {
    copy_with(text, copy_via_command, write_osc52)
}

fn copy_with(
    text: &str,
    copy_native: impl FnOnce(&str) -> bool,
    copy_terminal: impl FnOnce(&str) -> io::Result<()>,
) -> Result<(), String> {
    let native_copied = copy_native(text);
    match copy_terminal(text) {
        Ok(()) => Ok(()),
        Err(_) if native_copied => Ok(()),
        Err(err) => Err(format!("clipboard write failed: {err}")),
    }
}

/// Platform clipboard commands to try, in order, each as `(program,
/// args)`. The first one that exists and accepts the text on stdin wins.
fn clipboard_commands() -> &'static [(&'static str, &'static [&'static str])] {
    #[cfg(target_os = "macos")]
    {
        &[("pbcopy", &[])]
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    }
    #[cfg(windows)]
    {
        &[("clip", &[])]
    }
    #[cfg(not(any(unix, windows)))]
    {
        &[]
    }
}

/// Pipe `text` to the first available native clipboard command. Returns
/// `true` when one succeeded.
fn copy_via_command(text: &str) -> bool {
    clipboard_commands()
        .iter()
        .any(|(program, args)| pipe_to_command(program, args, text))
}

fn pipe_to_command(program: &str, args: &[&str], text: &str) -> bool {
    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    {
        let Some(mut stdin) = child.stdin.take() else {
            return false;
        };
        if stdin.write_all(text.as_bytes()).is_err() {
            return false;
        }
        // `stdin` drops here, closing the pipe so the command can finish.
    }
    matches!(child.wait(), Ok(status) if status.success())
}

fn write_osc52(text: &str) -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.write_all(osc52_frame(text).as_bytes())?;
    stdout.flush()
}

/// Build the OSC 52 clipboard-set escape sequence for `text`.
fn osc52_frame(text: &str) -> String {
    format!("\u{1b}]52;c;{}\u{7}", base64_encode(text.as_bytes()))
}

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 encoder (with `=` padding). Local, to avoid a crate
/// dependency for the few bytes OSC 52 needs.
fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(BASE64_ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(BASE64_ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(BASE64_ALPHABET[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(BASE64_ALPHABET[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_uses_native_and_terminal_clipboards_when_both_are_available() {
        let native_calls = std::cell::Cell::new(0);
        let terminal_calls = std::cell::Cell::new(0);

        let result = copy_with(
            "review prompt",
            |text| {
                assert_eq!(text, "review prompt");
                native_calls.set(native_calls.get() + 1);
                true
            },
            |text| {
                assert_eq!(text, "review prompt");
                terminal_calls.set(terminal_calls.get() + 1);
                Ok(())
            },
        );

        assert_eq!(result, Ok(()));
        assert_eq!(native_calls.get(), 1);
        assert_eq!(terminal_calls.get(), 1);
    }

    #[test]
    fn copy_succeeds_when_either_clipboard_path_works() {
        let terminal_only = copy_with("text", |_| false, |_| Ok(()));
        let native_only = copy_with("text", |_| true, |_| Err(io::Error::other("blocked")));
        let neither = copy_with("text", |_| false, |_| Err(io::Error::other("blocked")));

        assert_eq!(terminal_only, Ok(()));
        assert_eq!(native_only, Ok(()));
        assert_eq!(neither, Err("clipboard write failed: blocked".to_string()));
    }

    #[test]
    fn base64_matches_known_vectors_including_padding() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_round_trips_unicode_and_multiline_text() {
        // Decoding is not needed at runtime, but the encoder must handle
        // multi-byte characters and newlines the prompt contains.
        assert_eq!(base64_encode("é\n".as_bytes()), "w6kK");
        assert_eq!(base64_encode(b"a\nb"), "YQpi");
    }

    #[test]
    fn osc52_frame_wraps_the_payload_in_the_clipboard_sequence() {
        assert_eq!(osc52_frame("foo"), "\u{1b}]52;c;Zm9v\u{7}");
        assert_eq!(osc52_frame(""), "\u{1b}]52;c;\u{7}");
    }
}
