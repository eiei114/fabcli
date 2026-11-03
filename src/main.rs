//! fab — operate Fab.com from a terminal.
//!
//! Borrows the session from an already logged-in Chrome over the DevTools protocol and calls
//! Fab's own web API from inside the page, so no API key or credential is stored here.

mod api;
mod cdp;
mod download;
mod session;

use anyhow::{Context, Result, bail};
use cdp::{Browser, Conn};
use clap::{Parser, Subcommand};
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;

const FAB_ORIGIN: &str = "https://www.fab.com";

#[derive(Parser)]
#[command(
    name = "fab",
    version,
    about = "Operate Fab.com using a logged-in Chrome session",
    long_about = "Borrows a logged-in Chrome over CDP (default http://127.0.0.1:9223, override with \
                  FAB_CDP) and calls Fab's web API from inside the page.\n\n\
                  This CLI never purchases anything: claim and download only ever select licenses \
                  whose price is 0."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Open the Fab login page in the CDP browser
    Login,
    /// Copy Fab/Epic cookies into this browser from another CDP browser
    ImportSession {
        /// Source CDP port, e.g. 9222
        port: String,
        /// Destination CDP port
        #[arg(long, default_value = "9223")]
        to: String,
    },
    /// Confirm the session is logged in
    Status,
    /// List owned assets
    Library {
        #[arg(long, default_value_t = 200)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Search listings
    Search {
        /// Words to search for
        words: Vec<String>,
        /// Only free listings (unreliable; confirm with `show`)
        #[arg(long)]
        free: bool,
        /// Keep only listings offering this format, e.g. fbx
        #[arg(long)]
        format: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Show formats, licenses, prices and ownership for one listing
    Show {
        /// Listing UID or URL
        uid: String,
    },
    /// Add a free license to the library
    Claim {
        /// Listing UID or URL
        uid: String,
        #[arg(long, default_value = "personal")]
        license: String,
    },
    /// List the files each format offers
    Formats {
        /// Listing UID or URL
        uid: String,
    },
    /// Download a listing's files, claiming it first if needed
    Download {
        /// Listing UID or URL
        uid: String,
        /// Format to fetch, e.g. fbx, glb, gltf, obj, blender, additional-files
        #[arg(long, default_value = "fbx")]
        format: String,
        /// Destination directory
        #[arg(long, default_value = ".")]
        out: PathBuf,
        /// Extract any downloaded archive
        #[arg(long)]
        unzip: bool,
        /// Keep only files whose name matches this suffix or extension
        #[arg(long)]
        only: Option<String>,
    },
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fab: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let endpoint = std::env::var("FAB_CDP").unwrap_or_else(|_| cdp::DEFAULT_ENDPOINT.to_owned());
    let browser = Browser::new(&endpoint);
    let cli = Cli::parse();

    match cli.command {
        Command::ImportSession { port, to } => session::run(&port, &to),
        Command::Login => {
            browser.open(&format!("{FAB_ORIGIN}/login"))?;
            println!("opened the login page; run `fab status` when done");
            Ok(())
        }
        command => {
            let mut page = open_page(&browser)?;
            match command {
                Command::Status => status(&mut page),
                Command::Library { limit, json } => library(&mut page, limit, json),
                Command::Search {
                    words,
                    free,
                    format,
                    limit,
                    json,
                } => search(
                    &mut page,
                    &words.join(" "),
                    free,
                    format.as_deref(),
                    limit,
                    json,
                ),
                Command::Show { uid } => show(&mut page, &uid),
                Command::Claim { uid, license } => {
                    let uid = uid_of(&uid);
                    println!("{}", api::claim(&mut page, &uid, &license)?);
                    Ok(())
                }
                Command::Formats { uid } => formats(&mut page, &uid),
                Command::Download {
                    uid,
                    format,
                    out,
                    unzip,
                    only,
                } => download(&mut page, &uid, &format, &out, unzip, only.as_deref()),
                Command::Login | Command::ImportSession { .. } => unreachable!(),
            }
        }
    }
}

/// Reuse the Fab tab, or open one and give it a moment to load.
fn open_page(browser: &Browser) -> Result<Conn> {
    if let Some(target) = browser.find_page(FAB_ORIGIN)? {
        return Conn::page(&target);
    }
    let target = browser
        .open(&format!("{FAB_ORIGIN}/"))
        .context("opening a Fab tab")?;
    std::thread::sleep(std::time::Duration::from_secs(5));
    Conn::page(&target)
}

fn status(page: &mut Conn) -> Result<()> {
    let reply = api::get(page, "/users/me")?;
    if reply.ok() {
        let name = reply.str_at("displayName").unwrap_or("(no display name)");
        println!("ok: {name} (signed in)");
        return Ok(());
    }
    println!("not signed in (HTTP {}). Run `fab login`", reply.status);
    std::process::exit(1);
}

#[derive(Serialize)]
struct LibraryRow {
    uid: String,
    library_uid: String,
    title: String,
    kind: Option<String>,
    formats: Vec<String>,
    by: Option<String>,
    licenses: Option<Value>,
}

fn library(page: &mut Conn, limit: usize, json: bool) -> Result<()> {
    let limit = limit.clamp(1, 1000);
    let mut cursor: Option<String> = None;
    let mut rows: Vec<LibraryRow> = Vec::new();
    loop {
        let mut path = format!(
            "/library/search?count=100&sort_by=-createdAt&source=acquired&cursor={}",
            cursor.as_deref().unwrap_or("")
        );
        if cursor.is_none() {
            path = "/library/search?count=100&sort_by=-createdAt&source=acquired".to_owned();
        }
        let reply = api::get(page, &path)?;
        if !reply.ok() {
            bail!("library request failed: HTTP {}", reply.status);
        }
        for item in reply.array_at("results") {
            rows.push(LibraryRow {
                uid: item
                    .pointer("/listing/uid")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                library_uid: item["uid"].as_str().unwrap_or_default().to_owned(),
                title: item
                    .pointer("/listing/title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                kind: item
                    .pointer("/listing/listingType")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                formats: item
                    .pointer("/listing/assetFormats")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|f| {
                                f.pointer("/assetFormatType/code").and_then(Value::as_str)
                            })
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                by: item
                    .pointer("/listing/publisher/sellerName")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                licenses: item
                    .pointer("/entitlement/licenses")
                    .cloned()
                    .filter(|v| !v.is_null()),
            });
        }
        if rows.len() >= limit {
            break;
        }
        let next = reply.str_at("cursors/next").filter(|next| !next.is_empty());
        match next {
            Some(next) if Some(next) != cursor.as_deref() => cursor = Some(next.to_owned()),
            _ => break,
        }
    }
    rows.truncate(limit);

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    for row in &rows {
        let kind = row.kind.as_deref().unwrap_or("-");
        println!(
            "{}  [{}{}]  {}",
            row.uid,
            kind,
            if row.formats.is_empty() {
                String::new()
            } else {
                format!(": {}", row.formats.join(","))
            },
            row.title
        );
    }
    Ok(())
}

#[derive(Serialize)]
struct SearchRow {
    uid: String,
    title: String,
    kind: Option<String>,
    formats: Vec<String>,
    free: bool,
    by: Option<String>,
}

fn search(
    page: &mut Conn,
    query: &str,
    free: bool,
    format: Option<&str>,
    limit: usize,
    json: bool,
) -> Result<()> {
    let mut path = format!("/listings/search?sort_by=-relevance&q={}", encode(query));
    if free {
        path.push_str("&is_free=1");
    }
    let reply = api::get(page, &path)?;
    if !reply.ok() {
        bail!("search failed: HTTP {}: {}", reply.status, reply.brief());
    }
    let mut rows: Vec<SearchRow> = reply
        .array_at("results")
        .iter()
        .filter(|listing| match format {
            None => true,
            Some(format) => listing
                .pointer("/assetFormats")
                .and_then(Value::as_array)
                .is_some_and(|formats| {
                    formats.iter().any(|f| {
                        f.pointer("/assetFormatType/code").and_then(Value::as_str) == Some(format)
                    })
                }),
        })
        .map(|listing| SearchRow {
            uid: listing["uid"].as_str().unwrap_or_default().to_owned(),
            title: listing["title"].as_str().unwrap_or_default().to_owned(),
            kind: listing["listingType"].as_str().map(str::to_owned),
            formats: listing
                .pointer("/assetFormats")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|f| f.pointer("/assetFormatType/code").and_then(Value::as_str))
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            // The badge is optimistic; `show` is the authority.
            free: listing["isFree"].as_bool().unwrap_or(false)
                || api::is_zero(
                    listing
                        .pointer("/startingPrice/price")
                        .unwrap_or(&Value::Null),
                ),
            by: listing
                .pointer("/user/sellerName")
                .or_else(|| listing.pointer("/user/displayName"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
        .take(limit)
        .collect();
    rows.shrink_to_fit();

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    for row in &rows {
        println!(
            "{}  {}  [{}]  {}",
            row.uid,
            if row.free { "FREE" } else { "    " },
            row.formats.join(","),
            row.title
        );
    }
    Ok(())
}

fn show(page: &mut Conn, uid: &str) -> Result<()> {
    let uid = uid_of(uid);
    let listing = api::get(page, &format!("/listings/{uid}"))?;
    if !listing.ok() {
        bail!("listing {uid}: HTTP {}", listing.status);
    }
    let licenses: Vec<Value> = listing
        .array_at("licenses")
        .iter()
        .map(|license| {
            serde_json::json!({
                "slug": license["slug"],
                "name": license["name"],
                "price": license.pointer("/priceTier/price"),
                "offerId": license["offerId"],
                "cc0": license["slug"].as_str() == Some("cc0"),
            })
        })
        .collect();
    let report = serde_json::json!({
        "uid": uid,
        "title": listing.at("title"),
        "by": listing
            .str_at("publisher/sellerName")
            .or_else(|| listing.str_at("user/sellerName")),
        "type": listing.at("listingType"),
        "free": listing.at("isFree"),
        "formats": api::formats(&listing),
        "aiGenerated": listing.at("isAiGenerated"),
        "licenses": licenses,
        "owned": api::owned(page, &uid)?,
        "url": format!("{FAB_ORIGIN}/listings/{uid}"),
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("# NOTE: is_free and FREE badges are unreliable. Confirm licenses[].price above.");
    Ok(())
}

fn formats(page: &mut Conn, uid: &str) -> Result<()> {
    let uid = uid_of(uid);
    let listing = api::get(page, &format!("/listings/{uid}"))?;
    if !listing.ok() {
        bail!("listing {uid}: HTTP {}", listing.status);
    }
    for format in api::formats(&listing) {
        let files = download::files(page, &uid, &format)?;
        let described: Vec<String> = files
            .iter()
            .map(|file| match file.size {
                Some(size) => format!("{} {:.1}MB", file.name, size as f64 / 1e6),
                None => file.name.clone(),
            })
            .collect();
        println!("{format}: {}", described.join(", "));
    }
    Ok(())
}

fn download(
    page: &mut Conn,
    uid: &str,
    format: &str,
    out: &PathBuf,
    unzip: bool,
    only: Option<&str>,
) -> Result<()> {
    let uid = uid_of(uid);
    let dir = std::path::absolute(out).unwrap_or_else(|_| out.clone());
    let saved = download::download(page, &uid, format, &dir, only)?;

    if unzip {
        for (path, _) in &saved {
            if path.extension().and_then(|e| e.to_str()) == Some("zip") {
                download::unzip(path, &dir)?;
            }
        }
    }

    let listing = api::get(page, &format!("/listings/{uid}"))?;
    let source = download::Source::from_listing(&uid, &listing);
    let record = source.write(&dir)?;
    println!("record {}", record.display());
    if source.attribution_required {
        println!(
            "ATTRIBUTION REQUIRED: \"{}\" by {} ({})",
            source.title,
            source.by.as_deref().unwrap_or("unknown"),
            source.url
        );
    }
    Ok(())
}

/// Pull the listing UID out of a URL or accept a bare UID.
fn uid_of(input: &str) -> String {
    let mut rest = input;
    while let Some(start) = rest.find(|c: char| c.is_ascii_hexdigit()) {
        let candidate: String = rest[start..]
            .chars()
            .take_while(|c| c.is_ascii_hexdigit() || *c == '-')
            .collect();
        if is_uid(&candidate) {
            return candidate;
        }
        rest = &rest[start + candidate.len().max(1)..];
    }
    input.to_owned()
}

/// A UUID: 8-4-4-4-12 hex digits.
fn is_uid(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    parts.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&parts)
            .all(|(len, part)| part.len() == *len && part.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Percent-encode a query value, keeping only the unreserved set.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_is_recognised() {
        assert!(is_uid("f65dd22a-1d17-4502-ba13-aca1948d9405"));
        assert!(!is_uid("f65dd22a-1d17-4502-ba13"));
        assert!(!is_uid("not-a-uid"));
    }

    #[test]
    fn uid_comes_out_of_a_url() {
        let url = "https://www.fab.com/listings/f65dd22a-1d17-4502-ba13-aca1948d9405";
        assert_eq!(uid_of(url), "f65dd22a-1d17-4502-ba13-aca1948d9405");
    }

    #[test]
    fn bare_uid_is_unchanged() {
        let uid = "f65dd22a-1d17-4502-ba13-aca1948d9405";
        assert_eq!(uid_of(uid), uid);
    }

    #[test]
    fn non_uid_input_passes_through() {
        assert_eq!(uid_of("hello"), "hello");
    }

    #[test]
    fn query_is_encoded() {
        assert_eq!(encode("wooden crate"), "wooden+crate");
        assert_eq!(encode("a&b"), "a%26b");
        assert_eq!(encode("ü"), "%C3%BC");
    }
}
