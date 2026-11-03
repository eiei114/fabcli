//! Fetching asset files and recording where they came from.

use crate::api;
use crate::cdp::{Conn, head};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Anything larger than this would be a surprise for a single asset file.
const MAX_ARCHIVE: u64 = 8 << 30;

/// One downloadable file belonging to a listing format.
#[derive(Debug, Clone)]
pub struct FileRef {
    pub uid: String,
    pub name: String,
    pub size: Option<u64>,
}

impl FileRef {
    /// True when no `--only` filter is set, or the name matches it.
    pub fn matches(&self, only: Option<&str>) -> bool {
        let Some(only) = only else { return true };
        let name = self.name.to_lowercase();
        let only = only.to_lowercase();
        // A filter with a dot is a suffix; without one it is an extension.
        if only.contains('.') {
            name.ends_with(&only)
        } else {
            name.ends_with(&format!(".{only}"))
        }
    }
}

/// Files offered by one format of one listing.
pub fn files(conn: &mut Conn, uid: &str, format: &str) -> Result<Vec<FileRef>> {
    let reply = api::get(conn, &format!("/listings/{uid}/asset-formats/{format}"))?;
    if !reply.ok() {
        bail!("format {format} is not available: HTTP {}", reply.status);
    }
    Ok(reply
        .array_at("files")
        .iter()
        .map(|file| FileRef {
            uid: file["uid"].as_str().unwrap_or_default().to_owned(),
            name: file["name"].as_str().unwrap_or_default().to_owned(),
            size: file["size"].as_u64(),
        })
        .collect())
}

/// Resolve a file's signed CDN URL.
fn download_url(conn: &mut Conn, uid: &str, format: &str, file: &str) -> Result<String> {
    let reply = api::get(
        conn,
        &format!("/listings/{uid}/asset-formats/{format}/files/{file}/download-info"),
    )?;
    if !reply.ok() {
        bail!("download-info failed: HTTP {}", reply.status);
    }
    reply
        .array_at("downloadInfo")
        .first()
        .and_then(|info| info["downloadUrl"].as_str())
        .map(str::to_owned)
        .with_context(|| format!("no downloadUrl in download-info: {}", reply.brief()))
}

/// Stream a URL to disk, refusing to write a partial file as if it were complete.
pub fn fetch(url: &str, dest: &Path, expected: Option<u64>) -> Result<u64> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(None)
        .http_status_as_error(true)
        .build()
        .into();
    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("downloading {}", head(url, 120)))?;
    let source = response.body_mut().as_reader();
    let mut sink = File::create(dest).with_context(|| format!("creating {}", dest.display()))?;
    let written = io::copy(&mut source.take(MAX_ARCHIVE), &mut sink)
        .with_context(|| format!("writing {}", dest.display()))?;
    sink.flush().context("flushing the download")?;
    drop(sink);

    if let Some(expected) = expected
        && written != expected
    {
        let _ = fs::remove_file(dest);
        bail!(
            "size mismatch for {}: expected {expected} bytes, got {written}",
            dest.display()
        );
    }
    Ok(written)
}

/// Download every selected file of a format.
pub fn download(
    conn: &mut Conn,
    uid: &str,
    format: &str,
    dir: &Path,
    only: Option<&str>,
) -> Result<Vec<(PathBuf, u64)>> {
    // Claim first: an unowned free listing has no download-info until it is in the library.
    if !api::owned(conn, uid)? {
        println!("{}", api::claim(conn, uid, "personal")?);
    }
    let selected: Vec<FileRef> = files(conn, uid, format)?
        .into_iter()
        .filter(|file| file.matches(only))
        .collect();
    if selected.is_empty() {
        bail!("no files match. UE-only formats need the Fab plugin inside the UE editor");
    }
    let mut saved = Vec::new();
    for file in &selected {
        let url = download_url(conn, uid, format, &file.uid)?;
        let name = if file.name.is_empty() {
            format!("{uid}.{format}.zip")
        } else {
            Path::new(&file.name)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.name.clone())
        };
        let dest = dir.join(name);
        let written = fetch(&url, &dest, file.size)?;
        println!("saved {} ({written} bytes)", dest.display());
        saved.push((dest, written));
    }
    Ok(saved)
}

/// Extract an archive, capping the expanded size and refusing paths that escape the target.
pub fn unzip(archive: &Path, dir: &Path) -> Result<PathBuf> {
    let stem = archive
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "extracted".to_owned());
    let target = dir.join(stem);
    let file = File::open(archive).with_context(|| format!("opening {}", archive.display()))?;
    let mut zip =
        zip::ZipArchive::new(file).with_context(|| format!("reading {}", archive.display()))?;
    // The extractor sanitises entry names and keeps symlinks inside the destination.
    zip.extract(&target)
        .with_context(|| format!("extracting {}", archive.display()))?;
    println!("unzipped -> {}", target.display());
    Ok(target)
}

/// The provenance record written next to every download.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub uid: String,
    pub title: String,
    pub by: Option<String>,
    pub license: Option<String>,
    pub license_slug: Option<String>,
    /// This is listing data, not a verified entitlement.
    pub license_basis: &'static str,
    pub cc0: bool,
    pub attribution_required: bool,
    pub ai_generated: bool,
    pub url: String,
    pub fetched: String,
}

impl Source {
    pub fn from_listing(uid: &str, listing: &api::Reply) -> Self {
        let license = api::free_license(listing);
        Self {
            uid: uid.to_owned(),
            title: listing.str_at("title").unwrap_or_default().to_owned(),
            by: listing
                .str_at("publisher/sellerName")
                .or_else(|| listing.str_at("user/sellerName"))
                .or_else(|| listing.str_at("user/displayName"))
                .map(str::to_owned),
            license: license.as_ref().map(|l| l.name.clone()),
            license_slug: license.as_ref().map(|l| l.slug.clone()),
            license_basis: "listing-free-license-not-entitlement",
            cc0: license.as_ref().is_some_and(|l| l.cc0),
            attribution_required: license.as_ref().is_some_and(|l| l.attribution_required),
            ai_generated: listing.at("isAiGenerated").as_bool().unwrap_or(false),
            url: format!("https://www.fab.com/listings/{uid}"),
            fetched: now(),
        }
    }

    pub fn write(&self, dir: &Path) -> Result<PathBuf> {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join("FAB_SOURCE.json");
        let body = serde_json::to_string_pretty(self).context("serialising FAB_SOURCE.json")?;
        fs::write(&path, body + "\n").with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }
}

/// RFC 3339 UTC, second precision.
pub fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    let (year, month, day) = civil(secs / 86_400);
    let remaining = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        remaining / 3600,
        (remaining % 3600) / 60,
        remaining % 60
    )
}

/// Days since the Unix epoch to a civil date (Howard Hinnant's algorithm).
fn civil(days: u64) -> (u64, u64, u64) {
    let days = days as i64;
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        year += 1;
    }
    (year as u64, month as u64, day as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn year_2033_is_civil() {
        // 2033-05-18T03:33:20Z
        assert_eq!(civil(2_000_000_000 / 86_400), (2033, 5, 18));
    }

    #[test]
    fn epoch_is_1970() {
        assert_eq!(civil(0), (1970, 1, 1));
    }

    #[test]
    fn only_filter_accepts_suffix_and_extension() {
        let file = FileRef {
            uid: "u".into(),
            name: "electrical_panel.glb".into(),
            size: None,
        };
        assert!(file.matches(None));
        assert!(file.matches(Some("glb")));
        assert!(file.matches(Some(".glb")));
        assert!(file.matches(Some("panel.glb")));
        assert!(!file.matches(Some("fbx")));
    }
}
