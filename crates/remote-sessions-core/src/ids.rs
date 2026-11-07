//! Session id handling (from cloud-sessions, same authors).
use crate::Result;

/// `https://claude.ai/code/session_01Ab…`, `cse_01Ab…` or a bare id -> the canonical `session_<id>`.
pub fn parse_session_id(input: &str) -> Result<String> {
    let text = input.trim();
    let last = if text.contains('/') {
        let rest = text.strip_prefix("https://").ok_or("Use the session's https://claude.ai/code/... link, or its ID.")?;
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        if host != "claude.ai" { return Err("Only claude.ai session links are accepted.".into()); }
        path.split(['?', '#']).next().unwrap_or("").trim_end_matches('/').rsplit('/').next().unwrap_or("")
    } else { text };
    if !((8..=100).contains(&last.len()) && last.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')) {
        return Err("That is not a valid session link or ID.".into());
    }
    Ok(if let Some(rest) = last.strip_prefix("cse_") {
        format!("session_{rest}")
    } else if last.starts_with("session_") {
        last.to_string()
    } else if last.starts_with("01") && !last.contains('_') {
        format!("session_{last}")
    } else {
        last.to_string()
    })
}

/// The API form of a session id: `session_<x>` on claude.ai is `cse_<x>` in the API.
pub fn cse_id(input: &str) -> Result<String> {
    let id = parse_session_id(input)?;
    Ok(match id.strip_prefix("session_") {
        Some(rest) => format!("cse_{rest}"),
        None => id,
    })
}

pub fn session_url(id: &str) -> String {
    format!("https://claude.ai/code/{id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_form_of_one_session_maps_to_the_same_id() {
        let want = "session_01ExampleBBBBBBBBBBBBBBB";
        for form in [
            "session_01ExampleBBBBBBBBBBBBBBB",
            "01ExampleBBBBBBBBBBBBBBB",
            "cse_01ExampleBBBBBBBBBBBBBBB",
            "https://claude.ai/code/session_01ExampleBBBBBBBBBBBBBBB",
            "https://claude.ai/code/session_01ExampleBBBBBBBBBBBBBBB?from=cli&m=0",
            "  https://claude.ai/code/session_01ExampleBBBBBBBBBBBBBBB/  ",
        ] {
            assert_eq!(parse_session_id(form).unwrap(), want, "form: {form}");
        }
    }
    #[test]
    fn session_and_cse_map_both_ways() {
        assert_eq!(cse_id("session_01AbCdEfGh").unwrap(), "cse_01AbCdEfGh");
        assert_eq!(cse_id("cse_01AbCdEfGh").unwrap(), "cse_01AbCdEfGh");
        assert_eq!(parse_session_id("cse_01AbCdEfGh").unwrap(), "session_01AbCdEfGh");
    }
    #[test]
    fn bad_ids_and_hosts_are_refused() {
        for bad in ["https://evil.example/code/session_01AbCdEf", "http://claude.ai/code/session_01AbCdEf", "../../etc", "abc", ""] {
            assert!(parse_session_id(bad).is_err(), "{bad}");
        }
    }
}
