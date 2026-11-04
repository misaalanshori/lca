//! The `None`-protocol fallback: a clickable `file://` link when one is possible, the
//! same text as a plain label when it is not (gh #324 gap 5).
//!
//! Split out of `image.rs` for gate 11's 1,200-line ceiling. This is the path **most**
//! terminals get - tmux among them, so most users of this project - which is why the
//! label deserves to be more than a dead sentence.
//!
//! Three rules make it honest:
//!
//! * **one text builder, two wrappings** ([`fallback_text`], [`osc8_link`]) - a second
//!   sentence for the unlinked case would be a second code path, and the two would drift;
//! * **the URL is a real URL** ([`file_url`]) - `/` literal, everything else
//!   percent-encoded, so a path with a space or a non-ASCII byte still opens;
//! * **a link is a claim the file exists** - a pruned attachment falls back to the label
//!   rather than offering a link to something gone.

use super::*;

/// The image fallback's **visible text** (gh #324 gap 5): pi's `imageFallback` grammar,
/// `[Image: <path> [<media type>] <w>x<h>]`.
///
/// One builder for both wrappings, deliberately. The linked and unlinked forms differ by
/// an OSC 8 wrapper and by nothing else - a second sentence for the unlinked case would be
/// a second code path, and the two would drift until a user on `screen` read a different
/// label from a user on kitty.
///
/// The path is `~/`-shortened when it is under the home directory, because a home
/// directory name is PII and the shortcut is what every shell prompt already does. A path
/// outside `$HOME` is shown in full: there is no home to abbreviate, and inventing `~`
/// for it would be a lie about where the file is.
#[must_use]
pub fn fallback_text(info: &ImageInfo) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(path) = &info.path {
        parts.push(display_path(path));
    }
    parts.push(format!("[{}]", info.media_type));
    if let (Some(w), Some(h)) = (info.width, info.height) {
        parts.push(format!("{w}x{h}"));
    }
    format!("[Image: {}]", parts.join(" "))
}

/// A path as the user should read it: `~/…` under the home directory, else absolute.
///
/// Uses `$HOME` (the platform-data convention this project already relies on, and what a
/// test can set without touching `APPDATA`/XDG). A relative path is shown as given - it
/// has no home to be under.
fn display_path(path: &std::path::Path) -> String {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    if let Some(home) = home
        && let Ok(rest) = path.strip_prefix(&home)
        && !rest.as_os_str().is_empty()
    {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

/// A `file://` URL for an absolute path, percent-encoded the way `pathToFileURL` does.
///
/// `/` stays literal, every other byte outside the unreserved set becomes `%XX` with
/// **uppercase** hex. That is the form a terminal hands to the OS, so it must be a real
/// URL: a raw space makes the link unopenable, and a link that opens nothing is worse than
/// the label it replaced. Measured against node's `pathToFileURL`:
/// `/home/u/café.png` -> `file:///home/u/caf%C3%A9.png`.
///
/// Returns `None` for a relative path: `file://` addresses an absolute location, and
/// resolving a relative one against the CWD here would link to something the user did not
/// attach.
#[must_use]
pub fn file_url(path: &std::path::Path) -> Option<String> {
    if !path.is_absolute() {
        return None;
    }
    Some(encode_file_url(&path.to_string_lossy()))
}

/// [`file_url`]'s encoding rule, over a path **string** (gh #324 gap 5).
///
/// Split out so the rule is testable on any host: the Windows shape (`\` separators and a
/// drive colon) is one a Linux run cannot produce through `Path`, and CI is the only place
/// that shape appears for real. Testing the encoder directly pins the rule everywhere
/// rather than only where the platform happens to build such a path.
#[must_use]
pub fn encode_file_url(path: &str) -> String {
    // A URL has one separator and it is `/`. On Windows a path arrives with `\`, and the
    // drive colon is the one `:` that is **not** encoded: the canonical form is
    // `file:///C:/Users/...` - three slashes, because the authority is empty and the drive
    // is the first path segment.
    //
    // Both mistakes were caught by the row below rather than by CI, which is the point of
    // pinning the encoder directly: my first version produced `file://C%3A/...` because it
    // assumed a leading `/` that a Windows path does not have.
    let normalized = path.replace('\\', "/");
    let with_slash = if normalized.starts_with('/') {
        normalized
    } else {
        format!("/{normalized}")
    };
    // A drive colon is `/<letter>:` at the start of the path.
    let drive_colon = with_slash.as_bytes().get(2) == Some(&b':')
        && with_slash
            .as_bytes()
            .get(1)
            .is_some_and(u8::is_ascii_alphabetic);
    let mut out = String::from("file://");
    for (index, char) in with_slash.chars().enumerate() {
        let keep = match char {
            '/' | '-' | '_' | '.' | '~' => true,
            ':' => drive_colon && index == 2,
            _ => char.is_ascii_alphanumeric(),
        };
        if keep {
            out.push(char);
        } else {
            // One percent sign per **byte**, so non-ASCII is UTF-8-encoded as `file://`
            // requires.
            for byte in char.to_string().bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    out
}

/// Wrap visible text in an OSC 8 hyperlink, ST-terminated (gh #324 gap 5).
///
/// The same shape the markdown renderer emits (`LinkMode::Hyperlink`), because two
/// emitters with different terminators would be two things for a terminal to cope with.
fn osc8_link(text: &str, url: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

/// Whether the fallback may link, given the image and the terminal.
///
/// Three conditions, and each is load-bearing:
///
/// * **hyperlinks are available** (the caller passes
///   [`crate::engine::terminal::supports_hyperlinks`], the one ladder - never a second
///   detection);
/// * **there is an absolute path** to point at;
/// * **the file is still there.** A link is a claim the file exists; an attachment
///   `lca session gc` pruned must fall back to the label rather than offer a link to
///   something gone.
///
/// The existence check is a `metadata` call on a path the user chose, which is cheap and
/// already happens elsewhere on the attach path - and it is the only way to keep the claim
/// honest.
fn fallback_link(
    info: &ImageInfo,
    hyperlinks: bool,
    file_exists: impl Fn(&std::path::Path) -> bool,
) -> Option<(String, String)> {
    if !hyperlinks {
        return None;
    }
    let path = info.path.as_deref()?;
    if !file_exists(path) {
        return None;
    }
    let url = file_url(path)?;
    Some((fallback_text(info), url))
}

/// The fallback rows for the `None` protocol: the plain text when there is nothing to
/// link to, the same text wrapped in an OSC 8 link when there is.
///
/// `hyperlinks` comes from the caller so the one ladder stays in `terminal.rs`; production
/// passes [`crate::engine::terminal::supports_hyperlinks`].
#[must_use]
pub fn render_fallback(info: &ImageInfo, width: usize, hyperlinks: bool) -> Vec<String> {
    render_fallback_with(info, width, hyperlinks, |path| path.exists())
}

/// [`render_fallback`] with the existence check injected, so a row can pin the pruned
/// case without creating and deleting a real file.
#[must_use]
pub fn render_fallback_with(
    info: &ImageInfo,
    width: usize,
    hyperlinks: bool,
    file_exists: impl Fn(&std::path::Path) -> bool,
) -> Vec<String> {
    let text = match fallback_link(info, hyperlinks, file_exists) {
        Some((text, url)) => osc8_link(&text, &url),
        None => fallback_text(info),
    };
    let mut lines = vec![crate::engine::text::truncate_to_width(
        &text,
        width.max(1),
        "…",
        false,
    )];
    // The hint only appears when the header was unreadable, as before - it explains an
    // unusual state, not the normal fallback.
    if info.width.is_none() {
        lines.push(crate::engine::text::truncate_to_width(
            "  (terminal graphics unavailable; shown as a placeholder)",
            width.max(1),
            "…",
            false,
        ));
    }
    lines
}

#[cfg(test)]
mod fallback_link_tests {
    use super::*;

    /// Strip OSC 8 wrappers, leaving the visible text - what a terminal that ignores
    /// the link shows.
    fn strip_osc8(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find("\x1b]8;;") {
            out.push_str(&rest[..at]);
            let after = &rest[at + 5..];
            match after.find("\x1b\\") {
                Some(end) => rest = &after[end + 2..],
                None => return out,
            }
        }
        out.push_str(rest);
        out
    }

    /// A platform-absolute scratch root, so these rows run on Windows too.
    ///
    /// Three of them asserted Unix literals as if `/tmp/x.png` were absolute everywhere;
    /// on Windows it is a relative path, `file_url` correctly refused it, and CI caught
    /// what this box could not. A `PathBuf` built from the platform's own rules is the
    /// only version that means the same thing on both.
    fn root() -> std::path::PathBuf {
        if cfg!(windows) {
            std::path::PathBuf::from(r"C:\lca-test")
        } else {
            std::path::PathBuf::from("/tmp")
        }
    }

    /// A `None`-protocol `ImageInfo` with a path, which is what `/attach` produces.
    fn attached(path: &str) -> ImageInfo {
        let bytes = {
            let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
            png.extend_from_slice(&[0, 0, 0, 13]);
            png.extend_from_slice(b"IHDR");
            png.extend_from_slice(&800u32.to_be_bytes());
            png.extend_from_slice(&600u32.to_be_bytes());
            png.extend_from_slice(&[0; 12]);
            png
        };
        let mut info = ImageInfo::new("image/png", &bytes);
        info.path = Some(std::path::PathBuf::from(path));
        info
    }

    // Verifies: gh #324 gap 5 - **with hyperlinks available, the fallback is a link.**
    // The visible text is the `~/`-shortened path and the dimensions; the OSC 8 wrapper
    // points at the absolute `file://` URL. Without this the fallback is a dead label -
    // which is what the issue filed.
    #[test]
    fn with_hyperlinks_the_fallback_links_to_the_file() {
        // `$HOME` is read rather than assumed: the `~/` shortening is defined against it,
        // so a row that invented a home could assert a path the product would not produce.
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
            eprintln!("skip: HOME is unset, so there is no ~/ to shorten against");
            return;
        };
        let path = home.join("pics/chart.png");
        let info = attached(&path.to_string_lossy());
        // The file need not exist for this row: it asserts the *link shape*, and the
        // pruned case has its own row below.
        let joined = render_fallback_with(&info, 80, true, |_| true).join("\n");
        assert!(
            joined.contains("\x1b]8;;file://"),
            "the label is an OSC 8 link: {joined:?}"
        );
        assert!(
            joined.contains("~/pics/chart.png"),
            "and the visible text is the ~/-shortened path: {joined:?}"
        );
        assert!(
            joined.contains("800x600"),
            "with the dimensions: {joined:?}"
        );
        assert!(
            joined.contains("\x1b]8;;\x1b\\"),
            "and the link closes: {joined:?}"
        );
    }

    // Verifies: gh #324 gap 5 - **without hyperlinks, the same text and no OSC 8.** One
    // formatter, two wrappings: a second sentence for the unlinked case would be a second
    // code path that can drift from the linked one.
    #[test]
    fn without_hyperlinks_the_text_is_identical_and_unwrapped() {
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
            eprintln!("skip: HOME is unset");
            return;
        };
        let path = home.join("pics/chart.png");
        let info = attached(&path.to_string_lossy());
        let linked = render_fallback(&info, 80, true).join("\n");
        let plain = render_fallback(&info, 80, false).join("\n");
        assert!(
            !plain.contains("\x1b]8"),
            "no OSC 8 when unavailable: {plain:?}"
        );
        assert!(
            plain.contains("~/pics/chart.png") && plain.contains("800x600"),
            "the same visible text: {plain:?}"
        );
        // The plain form is exactly the linked form with the wrapper removed.
        let stripped = strip_osc8(&linked);
        assert_eq!(stripped, plain, "one formatter, two wrappings");
    }

    // Verifies: gh #324 gap 5 - **a path with a space is percent-encoded.** A URL that
    // contains a raw space is not a URL; a terminal that tries to open it fails, and a
    // link that opens nothing is worse than a label.
    #[test]
    fn a_path_with_a_space_is_percent_encoded() {
        let info = attached(&root().join("my picture file.png").to_string_lossy());
        let joined = render_fallback_with(&info, 120, true, |_| true).join("\n");
        assert!(
            joined.contains("my%20picture%20file.png"),
            "the spaces are %20: {joined:?}"
        );
        assert!(
            !joined.contains("%2F"),
            "and the separator is never encoded: {joined:?}"
        );
        // The raw space must not reach the *URL*; the display keeps it, which is the
        // line above. Asserting "no raw space anywhere" would fail a correct output.
        let url_start = joined.find("file://").expect("a url");
        let url_end = joined[url_start..].find("\x1b\\").expect("the terminator") + url_start;
        assert!(
            !joined[url_start..url_end].contains(' '),
            "and the URL itself has no raw space: {:?}",
            &joined[url_start..url_end]
        );
        // The *display* keeps the real space - only the URL is encoded.
        assert!(
            joined.contains("my picture file.png"),
            "the visible text is the real path: {joined:?}"
        );
    }

    // Verifies: gh #324 gap 5 - **the Windows shape is a real URL.** CI caught this and
    // this Linux box could not produce the input: a Windows path arrives with `\`
    // separators, and the drive colon must stay literal (`file:///C:/…`) rather than
    // become `%3A`. The encoder is pinned directly, so the rule holds on every host.
    #[test]
    fn a_windows_drive_path_encodes_to_a_file_url() {
        assert_eq!(
            encode_file_url(r"C:\Users\u\a b.png"),
            "file:///C:/Users/u/a%20b.png",
            "backslashes become slashes and the drive colon stays literal"
        );
        assert_eq!(
            encode_file_url(r"C:\Users\u\café.png"),
            "file:///C:/Users/u/caf%C3%A9.png",
            "and a non-ASCII byte is still UTF-8 percent-encoded"
        );
        // A plain Unix path keeps its single leading slash and gains no drive colon.
        assert_eq!(
            encode_file_url("/tmp/a b.png"),
            "file:///tmp/a%20b.png",
            "the Unix shape is unchanged"
        );
    }

    // Verifies: gh #324 gap 5 - **non-ASCII is UTF-8 percent-encoded**, one byte at a
    // time, which is what `file://` means. Matching `pathToFileURL` exactly, measured
    // against node: `/home/u/café.png` -> `file:///home/u/caf%C3%A9.png`.
    #[test]
    fn a_non_ascii_path_is_utf8_percent_encoded() {
        let info = attached(&root().join("café.png").to_string_lossy());
        let joined = render_fallback_with(&info, 120, true, |_| true).join("\n");
        // The encoded form of the *platform's* path: `caf%C3%A9` on both, whatever the
        // separators before it.
        assert!(
            joined.contains("caf%C3%A9.png"),
            "the UTF-8 bytes are encoded: {joined:?}"
        );
        // The display keeps the real character (it is a readable path); only the **URL**
        // must be pure ASCII, so the assertion is scoped to the URL - asserting on the
        // whole line would fail a correct output, which is the same mistake I made with
        // the space row.
        let url = file_url(&root().join("café.png")).expect("a url");
        assert!(
            url.is_ascii(),
            "the URL is pure ASCII even though the path is not: {url:?}"
        );
    }

    // Verifies: gh #324 gap 5 + the secrets rule - **a path outside `$HOME` is shown in
    // full and still linked.** The `~/` form is the default because a home directory name
    // is PII; a path that is not under `$HOME` has no home to abbreviate, and inventing
    // `~` for it would be a lie. It is linked, because it is still a real file.
    #[test]
    fn a_path_outside_home_is_shown_in_full() {
        let path = root().join("lca/shot.png");
        let info = attached(&path.to_string_lossy());
        let joined = render_fallback_with(&info, 120, true, |_| true).join("\n");
        assert!(
            joined.contains(&*path.to_string_lossy()),
            "the whole path is the label: {joined:?}"
        );
        assert!(!joined.contains('~'), "and no ~ was invented: {joined:?}");
        // The link is derived from the same path, so the assertion cannot drift from the
        // platform's separators.
        let url = file_url(&path).expect("an absolute path has a url");
        assert!(joined.contains(&url), "and it is still linked: {joined:?}");
    }

    // Verifies: gh #324 gap 5 - **a link is a claim the file exists.** An attachment that
    // `lca session gc` pruned (or one that never had a file) must not offer a link to
    // something gone: it renders as a label, with the path still named so the user can
    // see what it was.
    #[test]
    fn a_missing_file_is_a_label_not_a_link() {
        let info = attached(&root().join("lca-pruned-this-file.png").to_string_lossy());
        // The file is gone, which the injected check reports - that is the pruned case.
        let joined = render_fallback_with(&info, 120, true, |_| false).join("\n");
        assert!(
            !joined.contains("\x1b]8"),
            "no link to a file that is not there: {joined:?}"
        );
        assert!(
            joined.contains("lca-pruned-this-file.png"),
            "but the path is still named: {joined:?}"
        );
        assert!(joined.contains("800x600"), "and the dimensions: {joined:?}");
    }

    // Verifies: gh #324 gap 5 - **a clipboard image has no path at all**, so there is
    // nothing to link to. The label carries the media type and dimensions, which is all
    // there is to say; a `file://` link to a path that never existed would be a lie.
    #[test]
    fn an_image_without_a_path_is_a_label() {
        let bytes = {
            let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
            png.extend_from_slice(&[0, 0, 0, 13]);
            png.extend_from_slice(b"IHDR");
            png.extend_from_slice(&320u32.to_be_bytes());
            png.extend_from_slice(&200u32.to_be_bytes());
            png.extend_from_slice(&[0; 12]);
            png
        };
        let info = ImageInfo::new("image/png", &bytes);
        assert!(info.path.is_none());
        let joined = render_fallback_with(&info, 120, true, |_| true).join("\n");
        assert!(!joined.contains("\x1b]8"), "nothing to link to: {joined:?}");
        assert!(joined.contains("image/png"), "the media type: {joined:?}");
        assert!(joined.contains("320x200"), "the dimensions: {joined:?}");
    }

    // Verifies: #326 - **the placeholder grammar a user reads is unchanged.** The
    // `None` protocol is what most terminals get, so the text must not shift under them;
    // gap 5 adds a link wrapper and an optional path, never a reworded sentence.
    #[test]
    fn the_none_protocol_grammar_is_unchanged() {
        // #326's grammar is the **path-less** case: a clipboard paste, or an image whose
        // file is unknown. An image with a file gets gap 5's richer form instead.
        let mut info = attached(&root().join("x.png").to_string_lossy());
        info.path = None;
        let lines = render_image(&info, &[], ImageProtocol::None, 80);
        let joined = lines.join("\n");
        assert!(
            joined.contains("[image image/png · 800×600"),
            "the #326 grammar: {joined:?}"
        );
        // And no OSC 8 unless the caller asked for it.
        assert!(!joined.contains("\x1b]8"), "the protocol arm is unchanged");
    }
}
