//! Launch a URL in the platform's browser with no shell in the path (R3).
//!
//! One implementation for both callers: the OAuth flow's `oauth.open`
//! host import (`Capabilities::oauth_open`) and the interface's OSC-8
//! link opener (`lca-cli`'s `open_url`).
//!
//! The Windows trap this exists to close: `cmd /C start "" <url>` puts
//! `cmd`'s own command-line parser between the caller and the browser, and
//! `cmd` splits at the first `&`. An OAuth authorize URL is exactly a
//! `&`-separated query string, so the browser opened with only the part
//! before the first `&` — Google's "Required parameter is missing:
//! response_type". `rundll32 url.dll,FileProtocolHandler` is a direct exec
//! parsed by `CommandLineToArgvW`, where `&` is not special. macOS `open`
//! and Linux `xdg-open` are already direct execs.
//!
//! The same trap is not Windows-only in principle: any URL with shell
//! metacharacters routed through a shell launcher has it. The rule here is
//! structural — the URL is a single argument to a directly-executed
//! program, and no candidate takes a shell.

use std::process::{Command, Stdio};

/// One URL launcher: the program and its fixed arguments. The URL is
/// appended as the final argument by [`open_url`], never interpolated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UrlLauncher {
    /// The executable.
    pub program: &'static str,
    /// Arguments that precede the URL.
    pub args: &'static [&'static str],
}

/// The Windows launcher plan, kept platform-independent so any host's test
/// can assert the `&`-safety of the chosen invocation (R3).
pub fn windows_url_launcher() -> UrlLauncher {
    UrlLauncher {
        program: "rundll32",
        args: &["url.dll,FileProtocolHandler"],
    }
}

/// The current platform's URL-launcher candidates, in order.
pub fn url_launchers() -> Vec<UrlLauncher> {
    #[cfg(target_os = "macos")]
    {
        vec![UrlLauncher {
            program: "open",
            args: &[],
        }]
    }
    #[cfg(target_os = "windows")]
    {
        vec![windows_url_launcher()]
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        vec![
            UrlLauncher {
                program: "xdg-open",
                args: &[],
            },
            UrlLauncher {
                program: "x-www-browser",
                args: &[],
            },
        ]
    }
}

/// Open `url` in the platform browser. Fire-and-forget: `Ok` once a
/// launcher process has spawned, `Err` with the reason when no candidate
/// could start.
pub fn open_url(url: &str) -> Result<(), String> {
    let mut tried: Vec<String> = Vec::new();
    for launcher in url_launchers() {
        let mut command = Command::new(launcher.program);
        command
            .args(launcher.args)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if command.spawn().is_ok() {
            return Ok(());
        }
        tried.push(launcher.program.to_string());
    }
    Err(format!("no URL opener found (tried {})", tried.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The OAuth authorize URL shape with everything a shell would mangle:
    /// `&`, `?`, a space, quotes, and `%`-encoding.
    const TRICKY: &str = "https://accounts.example.com/o/oauth2/v2/auth?client_id=abc\
&response_type=code&redirect_uri=http://127.0.0.1:9/callback&state=x\"y&scope=a%20b#frag";

    // Verifies: R3 - the URL reaches the launcher as one unmodified
    // argument. Direct exec means no shell parser can split it at `&`.
    #[test]
    fn a_url_with_metacharacters_round_trips_as_one_argument() {
        for launcher in url_launchers() {
            let mut command = Command::new(launcher.program);
            command.args(launcher.args).arg(TRICKY);
            let args: Vec<String> = command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            assert_eq!(
                args.last().map(String::as_str),
                Some(TRICKY),
                "the URL is the final argument, unchanged"
            );
            assert_eq!(
                args.len(),
                launcher.args.len() + 1,
                "the URL is one argument, not split"
            );
        }
    }

    // Verifies: R3 - the Windows candidate is a shell-free direct exec with
    // the URL appended whole, on every host the test runs on.
    #[test]
    fn the_windows_launcher_is_shell_free() {
        let launcher = windows_url_launcher();
        assert_eq!(launcher.program, "rundll32");
        assert_eq!(launcher.args, &["url.dll,FileProtocolHandler"]);
        assert!(
            !launcher.program.contains("cmd"),
            "cmd's parser must not sit between the URL and the browser"
        );
        let mut command = Command::new(launcher.program);
        command.args(launcher.args).arg(TRICKY);
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args.last().map(String::as_str), Some(TRICKY));
    }
}
