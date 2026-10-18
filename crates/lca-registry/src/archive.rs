//! The archive format (ADR-0010's zip: `extension.toml`, one component,
//! and an optional `resources/` bag) and its hostile-input ceilings
//! (gh #350). Split from `lib.rs` for the workspace file ceiling (gate 11).

use std::io::Read as _;

use crate::{
    Archive, Error, MAX_ARCHIVE_ENTRIES, MAX_COMPONENT_BYTES, MAX_MANIFEST_BYTES,
    RESOURCE_FILE_MAX_BYTES, RESOURCE_PACKAGE_MAX_BYTES, valid_archive_path,
};

// ---------------------------------------------------------------------------
// Archives (ADR-0010's zip: extension.toml + the component, nothing else)
// ---------------------------------------------------------------------------

/// Pack the files ADR-0010 names into a zip (authors and tests).
pub fn pack_archive(manifest: &str, component: &[u8]) -> Result<Vec<u8>, Error> {
    pack_archive_with_resources(manifest, component, &[])
}

/// Pack a manifest, a component, and an optional `resources/` bag
/// (ADR-0030) into the archive `read_archive` accepts.
pub fn pack_archive_with_resources(
    manifest: &str,
    component: &[u8],
    resources: &[(String, Vec<u8>)],
) -> Result<Vec<u8>, Error> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let deflated = zip::write::SimpleFileOptions::default();
        writer
            .start_file("extension.toml", deflated)
            .map_err(|err| Error::Invalid(err.to_string()))?;
        std::io::Write::write_all(&mut writer, manifest.as_bytes())?;
        writer
            .start_file("component.wasm", stored)
            .map_err(|err| Error::Invalid(err.to_string()))?;
        std::io::Write::write_all(&mut writer, component)?;
        for (rel, bytes) in resources {
            if !valid_archive_path(rel) {
                return Err(Error::Invalid(format!(
                    "resource path `{rel}` is not a safe relative path"
                )));
            }
            writer
                .start_file(format!("resources/{rel}"), deflated)
                .map_err(|err| Error::Invalid(err.to_string()))?;
            std::io::Write::write_all(&mut writer, bytes)?;
        }
        writer
            .finish()
            .map_err(|err| Error::Invalid(err.to_string()))?;
    }
    Ok(cursor.into_inner())
}

/// Unpack an archive to the pair everything downstream expects
/// (ADR-0010: exactly these two files, verified digest downstream).
pub fn read_archive(bytes: &[u8]) -> Result<Archive, Error> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|err| Error::Invalid(format!("not a zip archive: {err}")))?;
    // Gh #350: bound the member count before listing names. A bomb made of
    // many tiny members would otherwise multiply through the name vector
    // and the per-entry open cost; the cap is far above any real package
    // (`extension.toml`, one component, and a `resources/` bag).
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(Error::Invalid(format!(
            "the archive carries {} entries, over the {MAX_ARCHIVE_ENTRIES} entry cap - repackage with fewer files",
            archive.len()
        )));
    }
    // Names first (owned), so the per-file borrow of the archive ends
    // before the next iteration starts.
    let names: Vec<String> = (0..archive.len())
        .filter_map(|index| {
            archive
                .by_index(index)
                .ok()
                .map(|file| file.name().to_string())
        })
        .collect();
    let mut manifest = None;
    let mut component = None;
    let mut resources: Vec<(String, Vec<u8>)> = Vec::new();
    let mut bag_bytes = 0u64;
    for name in names {
        let mut file = archive
            .by_name(&name)
            .map_err(|err| Error::Invalid(err.to_string()))?;
        if name == "extension.toml" {
            if manifest.is_some() {
                return Err(Error::Invalid(
                    "the archive carries more than one extension.toml".to_string(),
                ));
            }
            let bytes = read_member(&mut file, "extension.toml", MAX_MANIFEST_BYTES)?;
            let text = String::from_utf8(bytes)
                .map_err(|err| Error::Invalid(format!("extension.toml is not UTF-8: {err}")))?;
            manifest = Some(text);
        } else if name.ends_with(".wasm") && !name.contains('/') {
            if component.is_some() {
                return Err(Error::Invalid(
                    "the archive carries more than one component".to_string(),
                ));
            }
            let buffer = read_member(&mut file, &name, MAX_COMPONENT_BYTES)?;
            component = Some(buffer);
        } else if let Some(rel) = name.strip_prefix("resources/") {
            // ADR-0030: the bag is the extension's own data, but a
            // hostile archive is a hostile archive. Reject traversal,
            // absolute paths, backslashes, and duplicates here, before
            // anything is written.
            if !valid_archive_path(rel) {
                return Err(Error::Invalid(format!(
                    "resource path `{name}` leaves the package"
                )));
            }
            if file.is_dir() {
                continue;
            }
            let buffer = read_member(&mut file, &name, RESOURCE_FILE_MAX_BYTES)?;
            bag_bytes += buffer.len() as u64;
            if bag_bytes > RESOURCE_PACKAGE_MAX_BYTES {
                return Err(Error::Invalid(format!(
                    "the resource bag exceeds the {RESOURCE_PACKAGE_MAX_BYTES} byte package cap"
                )));
            }
            if resources.iter().any(|(path, _)| path == rel) {
                return Err(Error::Invalid(format!("duplicate resource `{name}`")));
            }
            resources.push((rel.to_string(), buffer));
        } else {
            // ADR-0010 + ADR-0030: extension.toml, the component, and
            // `resources/**`, nothing else.
            return Err(Error::Invalid(format!(
                "unexpected archive entry `{name}` (the archive allows extension.toml, the component, and resources/**)"
            )));
        }
    }
    match (manifest, component) {
        (Some(manifest), Some(component)) => Ok(Archive {
            manifest,
            component,
            resources,
        }),
        // A data-only package (ADR-0030) declares `resources` and carries
        // no component; the archive is valid with the bag alone.
        (Some(manifest), None) if manifest_declares_resources(&manifest) => Ok(Archive {
            manifest,
            component: Vec::new(),
            resources,
        }),
        _ => Err(Error::Invalid(
            "the archive must carry extension.toml and the component".to_string(),
        )),
    }
}

/// Read one archive member with a ceiling on its **uncompressed** size
/// (gh #350).
///
/// `read_to_end` decompresses everything the member claims before anyone
/// can look at the result, so a 50 KiB archive declaring gigabytes of
/// content allocated gigabytes: the size check that followed was already
/// too late. `take(limit + 1)` bounds the allocation, and the `+ 1` is what
/// distinguishes "exactly at the limit" (fine) from "over it" (refused)
/// without trusting the declared size.
///
/// The error names the member and the limit, and says what to do: the
/// message reaches `lca ext install`'s output, where "repackage it smaller"
/// is the only actionable advice.
fn read_member(file: &mut impl std::io::Read, name: &str, limit: u64) -> Result<Vec<u8>, Error> {
    let mut buffer = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut buffer)?;
    if buffer.len() as u64 > limit {
        return Err(Error::Invalid(format!(
            "`{name}` expands past the {limit} byte limit - repackage it so no member exceeds that size"
        )));
    }
    Ok(buffer)
}

/// Whether a manifest declares a non-empty `resources` bag (the data-only
/// extension marker, ADR-0030).
pub(crate) fn manifest_declares_resources(manifest: &str) -> bool {
    manifest
        .parse::<toml::Value>()
        .ok()
        .and_then(|value| {
            value
                .get("resources")
                .and_then(|resources| resources.as_array())
                .map(|kinds| !kinds.is_empty())
        })
        .unwrap_or(false)
}
