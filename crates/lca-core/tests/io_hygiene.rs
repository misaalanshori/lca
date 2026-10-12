//! IO hygiene (gh #153): the named async paths never block on the
//! filesystem. A source-text pin: `std::fs::` must not appear inside
//! `assemble_request` (the turn's per-round assembly) or any `async
//! fn` in `assemble.rs` (attachment staging and tool-image
//! persistence). Attachment bytes arrive via `tokio::fs` prefetch and
//! skill scans through the cached path instead.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

fn crate_src() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn read_source(name: &str) -> String {
    std::fs::read_to_string(crate_src().join(name)).expect("read own source")
}

/// The body of the named `async fn`, skipping strings, chars, and
/// comments so unbalanced braces in text cannot fool the balance.
fn async_fn_body(source: &str, name: &str) -> Option<String> {
    let marker = format!("async fn {name}");
    let mut rest = source.split(&marker).nth(1)?;
    rest = rest.split('{').nth(1)?;
    let mut depth = 1usize;
    let mut body = String::new();
    let bytes = rest.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'"' {
            // Ordinary and raw strings: consume through the closer.
            let mut hashes = 0;
            if index + 1 < bytes.len() && bytes[index + 1] == b'#' {
                let mut j = index + 1;
                while j < bytes.len() && bytes[j] == b'#' {
                    hashes += 1;
                    j += 1;
                }
            }
            let start = index + if hashes > 0 { hashes + 1 } else { 1 };
            let closer: Vec<u8> = if hashes > 0 {
                let mut c = vec![b'"'];
                c.extend(std::iter::repeat(b'#').take(hashes));
                c
            } else {
                vec![b'"']
            };
            let mut j = start;
            while j < bytes.len() {
                if bytes[j] == b'\\' {
                    j += 2;
                    continue;
                }
                if bytes[j..].starts_with(&closer) {
                    j += closer.len();
                    break;
                }
                j += 1;
            }
            body.push_str(&rest[index..j.min(bytes.len())]);
            index = j.min(bytes.len());
            continue;
        }
        if byte == b'\'' {
            // Lifetimes and char literals: consume through the closer.
            let mut j = index + 1;
            if j < bytes.len() && bytes[j] == b'\\' {
                j += 2;
            } else {
                j += 1;
            }
            while j < bytes.len() && bytes[j] != b'\'' {
                j += 1;
            }
            j += 1;
            body.push_str(&rest[index..j.min(bytes.len())]);
            index = j.min(bytes.len());
            continue;
        }
        if byte == b'/' && index + 1 < bytes.len() && bytes[index + 1] == b'/' {
            let end = rest[index..]
                .find('\n')
                .map_or(bytes.len(), |offset| index + offset);
            body.push_str(&rest[index..end]);
            index = end;
            continue;
        }
        if byte == b'/' && index + 1 < bytes.len() && bytes[index + 1] == b'*' {
            let end = rest[index..]
                .find("*/")
                .map_or(bytes.len(), |offset| index + offset + 2);
            body.push_str(&rest[index..end]);
            index = end;
            continue;
        }
        if byte == b'{' {
            depth += 1;
        }
        if byte == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some(body);
            }
        }
        body.push(byte as char);
        index += 1;
    }
    None
}

// Verifies: gh #153 - no blocking-filesystem call hides in the named
// async paths. `tokio::fs` (async) is fine; only `std::fs::`
// (blocking) fails the pin.
#[test]
fn no_blocking_fs_in_the_named_async_paths() {
    let turn = read_source("turn/mod.rs");
    let span = async_fn_body(&turn, "assemble_request").expect("assemble_request exists");
    assert!(
        !span.contains("std::fs::"),
        "assemble_request blocks on the filesystem"
    );
    let assemble = read_source("assemble.rs");
    for name in ["store_image_bytes_async"] {
        let span = async_fn_body(&assemble, name).expect("async fn exists");
        assert!(
            !span.contains("std::fs::"),
            "async fn {name} blocks on the filesystem"
        );
    }
}
