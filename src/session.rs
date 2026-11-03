//! Copy Fab/Epic cookies from one CDP browser into another.
//!
//! Useful when a login happened in a throwaway browser and the resident profile needs it. Cookie
//! values are moved between browsers and never printed.

use crate::cdp::Conn;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// Domains whose cookies are worth moving for Fab.
fn wanted(domain: &str) -> bool {
    let host = domain.trim_start_matches('.');
    host == "fab.com"
        || host.ends_with(".fab.com")
        || host == "epicgames.com"
        || host.ends_with(".epicgames.com")
}

pub fn run(from_port: &str, to_port: &str) -> Result<()> {
    let from = format!("http://127.0.0.1:{from_port}");
    let to = format!("http://127.0.0.1:{to_port}");
    let mut source = browser(&from)?;
    let mut dest = browser(&to)?;

    let cookies = cookies_of(&mut source)?;
    let moved: Vec<Value> = cookies
        .into_iter()
        .filter(|cookie| wanted(cookie["domain"].as_str().unwrap_or_default()))
        .map(strip_session_fields)
        .collect();
    if moved.is_empty() {
        bail!("no Fab/Epic cookies in {from}; log in there first");
    }

    let count = moved.len();
    let names: Vec<&str> = moved
        .iter()
        .filter_map(|c| c["name"].as_str())
        .filter(|name| {
            let lower = name.to_lowercase();
            lower.contains("session")
                || lower.contains("sid")
                || lower.contains("epic")
                || lower.contains("token")
        })
        .collect();

    let reply = dest
        .call("Storage.setCookies", json!({ "cookies": moved }))
        .context("writing cookies into the destination browser")?;
    if reply.get("error").is_some() {
        bail!("cookie copy failed: {reply}");
    }
    println!(
        "copied {count} cookies ({})",
        if names.is_empty() {
            "no session-like names".to_owned()
        } else {
            names.join(", ")
        }
    );
    Ok(())
}

fn browser(endpoint: &str) -> Result<Conn> {
    let http = crate::cdp::Browser::new(endpoint);
    let ws = http
        .devtools_url()
        .with_context(|| format!("no CDP browser at {endpoint}"))?;
    Conn::connect(&ws)
}

/// Every cookie the source browser holds for every domain.
fn cookies_of(conn: &mut Conn) -> Result<Vec<Value>> {
    let result = conn.call("Storage.getCookies", json!({}))?;
    let cookies = result["cookies"]
        .as_array()
        .cloned()
        .context("Storage.getCookies returned no cookies array")?;
    Ok(cookies)
}

/// Drop the fields a receiving browser must not be given.
fn strip_session_fields(cookie: Value) -> Value {
    let mut map = cookie.as_object().cloned().unwrap_or_default();
    for key in [
        "size",
        "session",
        "priority",
        "sourceScheme",
        "sourcePort",
        "partitionKey",
    ] {
        map.remove(key);
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fab_and_epic_domains_are_wanted() {
        assert!(wanted(".fab.com"));
        assert!(wanted("www.fab.com"));
        assert!(wanted(".epicgames.com"));
        assert!(!wanted(".splice.com"));
        assert!(!wanted("notfab.com"));
    }

    #[test]
    fn session_fields_are_removed() {
        let cookie = json!({
            "name": "fab_csrftoken",
            "domain": ".fab.com",
            "size": 42,
            "session": false,
            "priority": "Medium",
            "sourceScheme": "Secure",
            "sourcePort": 443,
        });
        let trimmed = strip_session_fields(cookie);
        assert_eq!(trimmed["name"], "fab_csrftoken");
        for key in ["size", "session", "priority", "sourceScheme", "sourcePort"] {
            assert!(trimmed.get(key).is_none(), "{key} should be gone");
        }
    }
}
