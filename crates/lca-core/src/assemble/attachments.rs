//! Attachment staging and lookup (gh #113, FR-SESS-9): the content-addressed
//! store the `/attach` path writes and the assembler reads.
//!
//! Bytes are hashed first and written under their digest, so an identical file
//! is one blob; the create-race arm (already exists) is success, not an error,
//! in both the sync and the async writer.

use lca_protocol::Record;
use lca_session::{Session, SessionStore};

/// One resolved attachment: a media type and the bytes a provider carries.
#[derive(Debug, Clone)]
pub struct Attachment {
    /// IANA media type (`image/png`, ...), from magic-byte sniffing.
    pub media_type: String,
    /// Raw bytes.
    pub bytes: Vec<u8>,
}

/// Why an attachment could not be read, sniffed, or stored (gh #549).
///
/// `lca-core`'s attachment API is public, and the SRDD's coding standards ask
/// libraries for typed errors instead of bare `String`s. **The copy is the
/// contract**: the `read` tool's refusal reaches a model and `/attach`'s reaches
/// the user, so this type renders *exactly* the sentence each `format!`
/// produced: the migration replaced the carrier, not the words. `From<String>`
/// is what keeps that true - the call sites convert, they do not re-describe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentError(pub String);

impl AttachmentError {
    /// The refusal message, exactly as a caller reads it.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AttachmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AttachmentError {}

impl From<String> for AttachmentError {
    fn from(message: String) -> Self {
        AttachmentError(message)
    }
}

/// The result of staging one image file as a session attachment.
#[derive(Debug, Clone)]
pub struct StagedAttachment {
    /// The content hash (`sha256`), the record's attachment reference.
    pub hash: String,
    /// The text stub a provider (or a model without vision) can read.
    pub stub: String,
}

/// Read `path`, reject anything whose magic bytes are not a known image, and
/// write it into the session's content-addressed attachment store
/// (owner-only). Returns the hash and the model-visible stub text.
///
/// This is the `/attach`/`--attach` input path (ADR-0029). D8's rules hold:
/// the file name is the digest (no traversal), the media type is sniffed and
/// never taken from a user-controlled name, and the bytes are never
/// executable. A non-image is refused rather than attached as opaque text.
pub fn stage_image(
    session: &Session,
    path: &std::path::Path,
) -> Result<StagedAttachment, AttachmentError> {
    let bytes = std::fs::read(path)
        .map_err(|err| AttachmentError(format!("cannot read {}: {err}", path.display())))?;
    let Some(media_type) = lca_protocol::sniff_image_media_type(&bytes) else {
        return Err(AttachmentError(format!(
            "{} is not a recognized image (png, jpeg, gif, or webp)",
            path.display()
        )));
    };
    let hash = store_image_bytes(session, &bytes)?;
    let stub = format!(
        "[image attachment {}, {media_type}, {} bytes]",
        &hash[..8],
        bytes.len()
    );
    Ok(StagedAttachment { hash, stub })
}

/// Write validated image bytes into the session's content-addressed
/// attachment store (owner-only) and return the hash. The bytes must
/// already be a known image (the `read` tool and the clipboard path
/// both validate before calling); the media type is re-sniffed at
/// resolve time, so only bytes are stored (gh #113).
pub fn store_image_bytes(session: &Session, bytes: &[u8]) -> Result<String, AttachmentError> {
    let hash = lca_tools::sha256_hex(bytes);
    let dir = session.dir().join("attachments");
    std::fs::create_dir_all(&dir)
        .map_err(|err| AttachmentError(format!("cannot create {}: {err}", dir.display())))?;
    let target = dir.join(&hash);
    // No `exists()`-then-write: the `create_new` inside is the only check,
    // and it is atomic with the create (gh #547). A pre-existing file for
    // this hash already holds these bytes, so it is left exactly as found.
    write_attachment(&target, bytes)?;
    Ok(hash)
}

/// [`store_image_bytes`] for async callers (gh #153): the same write
/// through `tokio::fs`, so tool-result persistence never blocks a
/// runtime thread.
pub async fn store_image_bytes_async(
    session: &Session,
    bytes: &[u8],
) -> Result<String, AttachmentError> {
    use tokio::io::AsyncWriteExt;
    let hash = lca_tools::sha256_hex(bytes);
    let dir = session.dir().join("attachments");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|err| AttachmentError(format!("cannot create {}: {err}", dir.display())))?;
    let target = dir.join(&hash);
    if !tokio::fs::try_exists(&target)
        .await
        .map_err(|err| AttachmentError(format!("cannot stat {}: {err}", target.display())))?
    {
        // Owner-only (0600 on Unix), like the sync writer: the
        // `create_new` miss below races a parallel identical store,
        // which wrote the same content-addressed bytes - ignore it.
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        // Owner-only on Unix: `tokio::fs::OpenOptions` carries its
        // own `mode`, no trait import needed.
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&target).await {
            Ok(mut file) => {
                file.write_all(bytes).await.map_err(|err| {
                    AttachmentError(format!("cannot write {}: {err}", target.display()))
                })?;
                // Gh #547: **flush before returning the hash**, because on Unix
                // `tokio::fs` hands the write to a blocking thread and its
                // `Drop` only *schedules* the close. Measured on this box: 100
                // consecutive stores, 45-83 of them read back 0 bytes on the
                // line after `Ok` - and the sync twin then sees an existing
                // empty file and skips its own write, so the blank bytes can
                // outlive the call that reported success. A reader that ran
                // immediately (the assembly prefetch below does exactly that)
                // was reading whatever the scheduler had managed.
                file.flush().await.map_err(|err| {
                    AttachmentError(format!("cannot write {}: {err}", target.display()))
                })?;
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => {
                return Err(AttachmentError(format!(
                    "cannot create {}: {err}",
                    target.display()
                )));
            }
        }
    }
    Ok(hash)
}

/// Write one attachment file owner-only (0600 on Unix), never executable.
///
/// Gh #547: a `create_new` miss is a parallel identical store, not a failure.
/// The path is the content's SHA-256, so whatever else holds it wrote
/// byte-identical bytes - losing that race is the desired outcome, and
/// reporting it as an error failed one caller for the other's success. This
/// is the sync twin of `store_image_bytes_async`, which already ignored it;
/// the two raced because they disagreed (`exists()`-then-create here, a bare
/// `create_new` there).
fn write_attachment(path: &std::path::Path, bytes: &[u8]) -> Result<(), AttachmentError> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(err) => {
            return Err(AttachmentError(format!(
                "cannot create {}: {err}",
                path.display()
            )));
        }
    };
    file.write_all(bytes)
        .map_err(|err| format!("cannot write {}: {err}", path.display()))?;
    Ok(())
}

/// Every attachment hash the records reference (gh #153): user
/// attachments plus tool-result images, deduplicated in first-seen
/// order. The async turn pre-reads exactly these through `tokio::fs`
/// and hands assembly a map lookup, so no request assembly blocks a
/// runtime thread on the filesystem.
pub fn attachment_hashes(records: &[Record]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut hashes = Vec::new();
    for record in records {
        let list: &[String] = match record {
            Record::User { attachments, .. } => attachments,
            Record::ToolResult { images, .. } => images,
            _ => continue,
        };
        for hash in list {
            if seen.insert(hash.clone()) {
                hashes.push(hash.clone());
            }
        }
    }
    hashes
}

/// Read one stored attachment through `tokio::fs` (gh #153): the
/// async half of the prefetch pair. A missing file or non-image
/// resolves to `None`, exactly like the sync closure it replaces.
pub async fn read_attachment_async(
    store: &SessionStore,
    session: &Session,
    hash: &str,
) -> Option<Attachment> {
    let path = store.attachment_path(session, hash)?;
    let bytes = tokio::fs::read(path).await.ok()?;
    let media_type = lca_protocol::sniff_image_media_type(&bytes)?;
    Some(Attachment {
        media_type: media_type.to_string(),
        bytes,
    })
}
