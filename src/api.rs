//! Fab.com's web API, called from inside a logged-in page.
//!
//! The browser holds the session; this module only shapes requests and reads responses.

use crate::cdp::{Conn, head};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// All Fab API paths sit under this prefix on the site origin.
const PREFIX: &str = "/i";

/// One response, kept raw so each command can decide how strict to be.
pub struct Reply {
    pub status: u16,
    pub json: Value,
    pub text: String,
}

impl Reply {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// A JSON field by slash path, or `Value::Null`.
    pub fn at(&self, path: &str) -> &Value {
        let mut node = &self.json;
        for key in path.split('/').filter(|k| !k.is_empty()) {
            node = match node.get(key) {
                Some(next) => next,
                None => return &Value::Null,
            };
        }
        node
    }

    pub fn str_at(&self, path: &str) -> Option<&str> {
        self.at(path).as_str()
    }

    pub fn array_at(&self, path: &str) -> &[Value] {
        self.at(path).as_array().map(Vec::as_slice).unwrap_or(&[])
    }

    /// The first `limit` characters of the body, for an error message.
    pub fn brief(&self) -> String {
        head(self.text.trim(), 200)
    }
}

pub fn get(conn: &mut Conn, path: &str) -> Result<Reply> {
    call(conn, "GET", path, &[])
}

pub fn post(conn: &mut Conn, path: &str, form: &[(&str, &str)]) -> Result<Reply> {
    call(conn, "POST", path, form)
}

/// Issue one request from inside the page.
///
/// The CSRF token is only available to page JavaScript, so it is read there and echoed into the
/// request headers. `credentials: 'include'` attaches the session cookie.
fn call(conn: &mut Conn, method: &str, path: &str, form: &[(&str, &str)]) -> Result<Reply> {
    let mut expression = String::from(
        "(async()=>{const c=(document.cookie.match(/fab_csrftoken=([^;]+)/)||[])[1];\
         const o={credentials:'include',headers:{'accept':'application/json','x-csrftoken':c||''}};",
    );
    expression.push_str(&format!("o.method={};", json!(method)));
    if !form.is_empty() {
        expression.push_str("const f=new FormData();");
        for (key, value) in form {
            expression.push_str(&format!("f.append({},{},);", json!(key), json!(value)));
        }
        expression.push_str("o.body=f;");
    }
    expression.push_str(&format!(
        "const r=await fetch('{PREFIX}'+{},o);const t=await r.text();\
         return JSON.stringify({{status:r.status,body:t}})}})()",
        json!(path)
    ));

    let raw = conn
        .evaluate_str(&expression)
        .with_context(|| format!("calling {path}"))?;
    let envelope: Value = serde_json::from_str(&raw).context("parsing the page envelope")?;
    let status = envelope["status"]
        .as_u64()
        .context("the page returned no status")? as u16;
    let text = envelope["body"].as_str().unwrap_or_default().to_owned();
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok(Reply { status, json, text })
}

/// A free license, as the listing describes it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FreeLicense {
    pub slug: String,
    pub name: String,
    pub offer_id: String,
    pub cc0: bool,
    pub attribution_required: bool,
}

/// A price node is zero when it is numeric and equals 0.
///
/// Prices arrive as JSON numbers that may be integers or floats, so both forms are accepted.
pub fn is_zero(value: &Value) -> bool {
    match value {
        Value::Number(number) => number.as_f64().is_some_and(|n| n == 0.0),
        _ => false,
    }
}

/// The license whose price is 0, if the listing has one.
pub fn free_license(listing: &Reply) -> Option<FreeLicense> {
    let entry = listing
        .at("licenses")
        .as_array()?
        .iter()
        .find(|license| is_zero(license.pointer("/priceTier/price").unwrap_or(&Value::Null)))?;
    let slug = entry["slug"].as_str().unwrap_or_default().to_owned();
    Some(FreeLicense {
        // `isCc0` also fires for CC-BY listings, so the slug is the authority here.
        cc0: slug == "cc0",
        attribution_required: matches!(slug.as_str(), "cc-by" | "cc-by-sa"),
        name: entry["name"].as_str().unwrap_or_default().to_owned(),
        offer_id: entry["offerId"].as_str().unwrap_or_default().to_owned(),
        slug,
    })
}

/// Format codes a listing offers, e.g. `["glb", "fbx"]`.
pub fn formats(listing: &Reply) -> Vec<String> {
    listing
        .at("assetFormats")
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|format| {
                    format
                        .pointer("/assetFormatType/code")
                        .and_then(Value::as_str)
                })
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the signed-in user already owns the listing.
pub fn owned(conn: &mut Conn, uid: &str) -> Result<bool> {
    let reply = get(
        conn,
        &format!("/users/me/listings-states?listing_ids={uid}"),
    )?;
    if !reply.ok() {
        bail!("ownership check failed: HTTP {}", reply.status);
    }
    // The endpoint wraps the answer in an array, one entry per requested listing.
    let states = reply.json.as_array().cloned().unwrap_or_default();
    Ok(states
        .iter()
        .any(|state| state.get("acquired").and_then(Value::as_bool) == Some(true)))
}

/// Add a free license to the library. Refuses anything that is not free.
pub fn claim(conn: &mut Conn, uid: &str, slug: &str) -> Result<String> {
    let listing = get(conn, &format!("/listings/{uid}"))?;
    if !listing.ok() {
        bail!("listing {uid} is not readable: HTTP {}", listing.status);
    }
    let licenses = listing
        .at("licenses")
        .as_array()
        .cloned()
        .unwrap_or_default();
    let chosen = licenses
        .iter()
        .find(|license| license["slug"].as_str() == Some(slug))
        .or_else(|| licenses.first())
        .context("the listing has no license information")?;

    let price = chosen.pointer("/priceTier/price").unwrap_or(&Value::Null);
    if !is_zero(price) {
        let name = chosen["name"].as_str().unwrap_or("unknown");
        let amount = price
            .as_f64()
            .map(|n| n.to_string())
            .unwrap_or_else(|| "unknown".to_owned());
        bail!("not free ({name}: {amount}). Buy it on the web; this CLI never purchases");
    }
    let offer_id = chosen["offerId"]
        .as_str()
        .context("the license has no offerId")?;

    let reply = post(
        conn,
        &format!("/listings/{uid}/add-to-library"),
        &[("offer_id", offer_id)],
    )?;
    let title = listing.str_at("title").unwrap_or(uid);
    let license_name = chosen["name"].as_str().unwrap_or(slug);
    if reply.ok() {
        return Ok(format!("claimed: {title} ({license_name}, free)"));
    }
    if reply.text.to_lowercase().contains("already") || reply.text.to_lowercase().contains("own") {
        return Ok(format!("already owned: {title}"));
    }
    bail!("claim failed: HTTP {}: {}", reply.status, reply.brief())
}

#[cfg(test)]
mod price_tests {
    use super::*;

    fn reply(json: Value) -> Reply {
        Reply {
            status: 200,
            json,
            text: String::new(),
        }
    }

    #[test]
    fn integer_and_float_zero_are_both_free() {
        assert!(is_zero(&json!(0)));
        assert!(is_zero(&json!(0.0)));
        assert!(!is_zero(&json!(1)));
        assert!(!is_zero(&json!(0.01)));
        assert!(!is_zero(&Value::Null));
        assert!(!is_zero(&json!("0")));
    }

    #[test]
    fn free_license_picks_the_zero_priced_one() {
        // Price arrives as a float here, which is what the live API returns.
        let listing = reply(json!({
            "licenses": [
                { "slug": "professional", "name": "Professional", "offerId": "a",
                  "priceTier": { "price": 0.0 } },
                { "slug": "personal", "name": "Personal", "offerId": "b",
                  "priceTier": { "price": 0.0 } }
            ]
        }));
        let license = free_license(&listing).expect("a free license");
        assert_eq!(license.slug, "professional");
        assert!(!license.cc0);
        assert!(!license.attribution_required);
    }

    #[test]
    fn cc0_is_decided_by_slug_not_by_the_cc0_flag() {
        let listing = reply(json!({
            "licenses": [
                { "slug": "cc-by", "name": "CC-BY", "offerId": "x",
                  "isCc0": true, "priceTier": { "price": 0 } }
            ]
        }));
        let license = free_license(&listing).expect("a free license");
        assert!(!license.cc0, "CC-BY must not be reported as CC0");
        assert!(license.attribution_required);
    }

    #[test]
    fn paid_listing_has_no_free_license() {
        let listing = reply(json!({
            "licenses": [
                { "slug": "personal", "name": "Personal", "offerId": "a",
                  "priceTier": { "price": 19.99 } }
            ]
        }));
        assert!(free_license(&listing).is_none());
    }

    #[test]
    fn formats_are_read_from_the_nested_code() {
        let listing = reply(json!({
            "assetFormats": [
                { "assetFormatType": { "code": "glb" } },
                { "assetFormatType": { "code": "fbx" } },
                { "assetFormatType": {} }
            ]
        }));
        assert_eq!(formats(&listing), vec!["glb", "fbx"]);
    }
}
