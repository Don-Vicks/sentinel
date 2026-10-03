//! Keeps API keys out of anything shown to a user. Error text from the HTTP client includes the
//! full request URL, and Solami's RPC and Mirage URLs carry the key as `?api_key=...`.

use std::env;

/// Query-string style parameters whose values are masked wherever they appear.
const PARAMS: [&str; 5] = ["api_key", "apikey", "x-token", "token", "key"];

fn query_value(url: &str, name: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    q.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.to_string()))
}

/// Secrets configured on this instance, so a key is masked even where it isn't in a `name=value` form.
fn configured_secrets() -> Vec<String> {
    let mut found = Vec::new();
    for var in ["YELLOWSTONE_TOKEN", "BLUR_API_KEY"] {
        if let Ok(v) = env::var(var) {
            found.push(v);
        }
    }
    for var in ["SOLANA_RPC_URL", "MIRAGE_STREAM_URL", "YELLOWSTONE_ENDPOINT", "BEAM_API_URL", "BLUR_API_URL"] {
        if let Ok(u) = env::var(var) {
            for p in PARAMS {
                found.extend(query_value(&u, p));
            }
        }
    }
    found.retain(|s| s.len() >= 8);
    found
}

/// Replaces keys and key-like query values in `text` with `***`, and cuts every URL down to its
/// host: a Discord or Slack webhook URL carries its secret in the path, not in a parameter.
pub fn scrub(text: &str) -> String {
    let mut out = text.to_string();
    for secret in configured_secrets() {
        out = out.replace(&secret, "***");
    }
    out = reduce_urls(&out);
    for name in PARAMS {
        out = mask_param(&out, name);
    }
    out
}

/// `https://discord.com/api/webhooks/1/abc?x=y` becomes `https://discord.com/...`.
fn reduce_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = ["https://", "http://", "wss://", "ws://"].iter().filter_map(|p| rest.find(p)).min() {
        out.push_str(&rest[..start]);
        let url = &rest[start..];
        let end = url.find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\'' | '>' | ']' | ',')).unwrap_or(url.len());
        let (full, after) = url.split_at(end);
        let scheme_end = full.find("://").map(|n| n + 3).unwrap_or(0);
        match full[scheme_end..].find(['/', '?', '#']) {
            Some(n) => {
                out.push_str(&full[..scheme_end + n]);
                out.push_str("/...");
            }
            None => out.push_str(full),
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Masks the value after every `name=` (case-insensitive) up to the next delimiter.
fn mask_param(text: &str, name: &str) -> String {
    let needle = format!("{name}=");
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(pos) = lower[i..].find(&needle) {
        let at = i + pos;
        // Only a whole parameter name: not the tail of a longer word such as "monkey=".
        let boundary = at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric();
        let value_start = at + needle.len();
        if !boundary {
            out.push_str(&text[i..value_start]);
            i = value_start;
            continue;
        }
        out.push_str(&text[i..value_start]);
        let end = text[value_start..]
            .find(|c: char| matches!(c, '&' | ')' | ' ' | '"' | '\'' | '\n' | ',' | ';' | ']'))
            .map(|n| value_start + n)
            .unwrap_or(text.len());
        out.push_str("***");
        i = end;
    }
    out.push_str(&text[i..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_the_key_in_a_client_error_url() {
        let e = "error sending request for url (https://rpc.solami.dev/sol?api_key=rpc_FAKEKEYFORTESTS1234): error trying to connect";
        let s = scrub(e);
        assert!(!s.contains("rpc_FAKEKEYFORTESTS1234") && !s.contains("api_key"), "{s}");
        assert!(s.contains("rpc.solami.dev") && s.contains("error trying to connect"), "{s}");
    }

    #[test]
    fn webhook_secrets_in_the_path_are_cut_too() {
        let s = scrub("error sending request for url (https://discord.com/api/webhooks/123456/AbCdEf-secret_token): timed out");
        assert!(!s.contains("AbCdEf") && !s.contains("123456"), "{s}");
        assert!(s.contains("discord.com") && s.contains("timed out"), "{s}");
        assert_eq!(scrub("Not found at https://example.com"), "Not found at https://example.com");
    }

    #[test]
    fn masks_several_forms_and_leaves_ordinary_text() {
        let s = scrub("wss://ws.solami.dev/mirage/stream/x?api_key=SECRETKEY&Token=abc123def456 failed, x-token=zzzz");
        assert!(!s.contains("SECRETKEY") && !s.contains("abc123def456") && !s.contains("zzzz"), "{s}");
        let bare = scrub("rejected: Token=abc123def456 and key=SECRETKEY.");
        assert!(!bare.contains("abc123def456") && !bare.contains("SECRETKEY"), "{bare}");
        assert_eq!(scrub("Transaction not found"), "Transaction not found");
        assert_eq!(scrub("a monkey=banana"), "a monkey=banana", "only whole parameter names are masked");
    }

    #[test]
    fn masks_configured_secrets_wherever_they_appear() {
        std::env::set_var("YELLOWSTONE_TOKEN", "grpc_1234567890abcdef");
        let s = scrub("rejected token grpc_1234567890abcdef for this stream");
        assert!(!s.contains("grpc_1234567890abcdef"), "{s}");
    }
}
