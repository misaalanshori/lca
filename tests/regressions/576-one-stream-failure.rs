//! Gh #576 - `StreamFailure` was defined twice, with two `From<CapabilityError>`
//! impls, and grew no constructors, `Display` or `Error`.
//!
//! The two definitions had the same three fields and **different derives** (the
//! OpenAI kit derived `Clone, PartialEq, Eq`; the Anthropic kit derived only
//! `Debug`), plus one `From<CapabilityError>` each. The two `From` impls agreed
//! *today*, which is luck: nothing pinned them equal, so a policy edit in one kit
//! would diverge from the other silently. `lca-subscription` then had to depend on
//! `lca-wire-openai` to name a type it only needed the name of.
//!
//! The type now lives once, in `lca-protocol` beside `CapabilityError`, with one
//! `From` impl (the policy's single home), constructors for new code, `Display`
//! that renders the message verbatim, and `std::error::Error` so a host can box it.
//! The fields stay **public**: 45 struct-literal sites across `crates/`,
//! `extensions/` and `tests/` build one, and those are deliberately not swept in
//! this cycle - a mechanical 45-site rewrite would bury the guard fix that landed
//! beside it (gh #577).
//!
//! Verifies: gh #576.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

use lca_protocol::{CapabilityError, StreamFailure};

// Verifies: gh #576 - **the `CapabilityError` classification table is pinned**,
// one row per variant.
//
// This is the row that makes the two impls' accidental agreement permanent. The
// policy: a refusal is a configuration problem (`invalid`, never retried); an I/O
// failure or a timeout may heal (`transport`, retryable). Every variant is named
// with its expected pair, so adding a variant to `CapabilityError` without a
// decision here is a compile error (the match in `From` is exhaustive), and
// flipping a pair fails this row by name.
#[test]
fn the_capability_error_classification_table_is_pinned() {
    let cases: [(CapabilityError, (&str, bool)); 6] = [
        (
            CapabilityError::Permission("scope escape".to_string()),
            ("invalid", false),
        ),
        (
            CapabilityError::NotGranted("net".to_string()),
            ("invalid", false),
        ),
        (
            CapabilityError::NotFound("/x".to_string()),
            ("invalid", false),
        ),
        (
            CapabilityError::Invalid("bad handle".to_string()),
            ("invalid", false),
        ),
        (
            CapabilityError::Io("connection reset".to_string()),
            ("transport", true),
        ),
        (
            CapabilityError::Timeout("callback window".to_string()),
            ("transport", true),
        ),
    ];
    for (err, (class, retryable)) in cases {
        let label = format!("{err:?}");
        let failure = StreamFailure::from(err);
        assert_eq!(
            (failure.class, failure.retryable),
            (class, retryable),
            "`{label}` classifies as ({class}, {retryable})"
        );
        // The message is the capability error's own text, not a re-write: a
        // caller reads the same sentence the capability produced.
        assert!(
            failure.message.contains("error:")
                || failure.message.contains("not found")
                || failure.message.contains("timed out")
                || failure.message.contains("permission")
                || failure.message.contains("invalid")
                || failure.message.contains("not granted"),
            "the message carries the capability error's text: {:?}",
            failure.message
        );
    }
}

// Verifies: gh #576 - **the constructors produce the pairs their names claim.**
//
// `invalid`, `auth` and `transport` are the three classes the headless envelope
// distinguishes; each constructor sets the class and the retryability a caller
// would infer from the name, so new code does not have to spell a struct literal
// and get the tuple wrong. `new` takes all three for the cases a constructor
// cannot name (a 429 with a `retry-after`, say).
#[test]
fn the_constructors_produce_the_pairs_their_names_claim() {
    let cases: [(StreamFailure, (&str, bool)); 4] = [
        (StreamFailure::invalid("bad request"), ("invalid", false)),
        (StreamFailure::auth("the key was revoked"), ("auth", false)),
        (
            StreamFailure::transport("connection reset"),
            ("transport", true),
        ),
        (
            StreamFailure::new("rate limited", "transport", true),
            ("transport", true),
        ),
    ];
    for (failure, (class, retryable)) in cases {
        assert_eq!(failure.class, class, "{failure:?}");
        assert_eq!(failure.retryable, retryable, "{failure:?}");
        assert!(
            !failure.message.is_empty(),
            "every constructor carries its message: {failure:?}"
        );
    }
}

// Verifies: gh #576 - **`Display` is the message verbatim: no prefix, no class.**
//
// The headless envelope carries `class` as its own field (`docs/headless.md`),
// so putting it in the text would make the same fact appear twice - and let a
// caller's `class` disagree with the message it prints.
#[test]
fn display_renders_the_message_verbatim() {
    let failure = StreamFailure::auth("the codex session has expired or was revoked");
    assert_eq!(
        failure.to_string(),
        "the codex session has expired or was revoked",
        "no `auth: ` prefix, no class in the text"
    );
    assert!(
        !failure.to_string().contains("auth"),
        "the class is not smuggled into the message"
    );
    let dynamic = format!("{}", StreamFailure::transport("connection reset"));
    assert_eq!(dynamic, "connection reset");
}

// Verifies: gh #576 - **`std::error::Error` is implemented.**
//
// The trait bound a host relies on: it can box a `StreamFailure` and print it, and
// `?` into a `Box<dyn Error>` keeps the user-facing message.
#[test]
fn a_stream_failure_can_be_boxed_as_an_error() {
    let failure = StreamFailure::transport("the connection was reset");
    let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(failure.clone());
    assert_eq!(
        boxed.to_string(),
        "the connection was reset",
        "the Display survives boxing"
    );
    // And `From` is what lets `?` do the same without a manual map.
    fn returns_a_stream_failure() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Err(StreamFailure::invalid("bad request"))?
    }
    let err = returns_a_stream_failure().expect_err("must fail");
    assert_eq!(err.to_string(), "bad request");
}

// Verifies: gh #576 - **one definition, re-exported by both kits.**
//
// Every existing path keeps working: `lca_wire_openai::StreamFailure` and
// `lca_wire_openai::error::StreamFailure`, likewise for the Anthropic kit, and all
// of them *are* `lca_protocol::StreamFailure` rather than a look-alike. A
// structural row rather than a compile check alone, because the point is that two
// definitions became one - `16ae2353`'s sibling class, where a second copy is how
// the two `From` impls came to drift.
#[test]
fn both_wire_kits_re_export_the_one_definition() {
    // A value built through each kit's path is the protocol type.
    let via_openai: lca_wire_openai::StreamFailure = StreamFailure::auth("x");
    let via_openai_error: lca_wire_openai::error::StreamFailure = StreamFailure::auth("x");
    let via_anthropic: lca_wire_anthropic::StreamFailure = StreamFailure::auth("x");
    // Comparing them proves they are the same type with the same derives (`PartialEq`
    // came from the OpenAI definition only, before the move).
    assert_eq!(via_openai, via_openai_error);
    assert_eq!(via_openai, via_anthropic);
    assert_eq!(via_openai, StreamFailure::auth("x"));

    // And no kit defines its own struct any more: a second `pub struct
    // StreamFailure` is the defect returning.
    // `responses.rs` is a directory now (the gh #576 cohesion split), so the
    // module walk reads all three of its files - a filename-shaped guard here
    // would have gone vacuous, which is gh #574/#543's lesson.
    // **The module, not a filename** (gh #574/#543's rule): both kits became
    // directories in this cycle's cohesion split, so a guard keyed on
    // `messages.rs`/`responses.rs` would have gone vacuous the moment they moved.
    // `module_text` walks the `mod` declarations, so it reads every sibling.
    for module in [
        "crates/lca-wire-openai/src/lib.rs",
        "crates/lca-wire-anthropic/src/lib.rs",
        // The kits' own files, reached through their barrels as well:
        "crates/lca-wire-openai/src/error.rs",
        "crates/lca-wire-anthropic/src/messages/mod.rs",
    ] {
        let source = crate::module_text(module);
        assert!(
            !source.contains("pub struct StreamFailure"),
            "{module} must re-export `lca_protocol::StreamFailure`, not define one"
        );
    }
    let protocol = crate::module_text("crates/lca-protocol/src/lib.rs");
    assert!(
        protocol.contains("pub use stream_failure::StreamFailure"),
        "the one definition is exported from `lca-protocol`"
    );
}

// Verifies: gh #576 - **`lca-subscription` names the type from the protocol crate,
// not from a wire kit.**
//
// The cross-crate smell the issue names: the kit depended on `lca-wire-openai` only
// to spell `StreamFailure`, so an OAuth-only consumer pulled the whole OpenAI wire
// engine. After the move that naming goes to `lca-protocol`.
//
// **The edge itself stays**, and this row says why rather than claiming a removal
// that did not happen: `lca_subscription` still uses `json_error_message` (the vendor
// envelope reader), `build_responses_body` and `ResponseStreamDriver` in its
// `wasm` module - those are wire-kit things, not the shared type, so the dependency
// is right. A removed dependency would have been the receipt the brief wanted; the
// honest answer is that only the *type* moved.
#[test]
fn the_subscription_kit_takes_the_type_from_the_protocol_crate() {
    let lib = crate::module_text("crates/lca-subscription/src/lib.rs");
    assert!(
        lib.contains("lca_protocol::{OauthCap, ProviderCap, StreamFailure}")
            || lib.contains("StreamFailure") && lib.contains("use lca_protocol::"),
        "`lca-subscription` imports StreamFailure from `lca-protocol`: {lib}"
    );
    assert!(
        !lib.contains("lca_wire_openai::StreamFailure"),
        "and no longer from the wire kit"
    );
    // Walk the whole module: the `wasm` sibling builds one with a struct literal.
    let wasm = crate::module_text("crates/lca-subscription/src/wasm.rs");
    assert!(
        !wasm.contains("lca_wire_openai::StreamFailure"),
        "no file in the kit names the type through the wire kit: {wasm}"
    );
    // And the dependency that remains is for real, wire-kit work.
    let manifest = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/lca-subscription/Cargo.toml"),
    )
    .expect("read the kit manifest");
    assert!(
        manifest.contains("lca-wire-openai"),
        "the edge stays for `json_error_message`/`build_responses_body`/`ResponseStreamDriver`"
    );
}
