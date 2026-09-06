//! Parses HTTP response headers and works out what a cache is actually
//! allowed to do with the response: store it, how long it stays fresh,
//! and whether it needs revalidation. Rules follow RFC 7234 section 5.2
//! for Cache-Control and section 5.3 for Expires.

/// Parsed `Cache-Control` directives. Unknown directives are kept around
/// (as their bare name, lowercased) rather than dropped, so callers can
/// still see them even though this library doesn't interpret them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CacheControl {
    pub no_store: bool,
    pub no_cache: bool,
    pub private: bool,
    pub public: bool,
    pub must_revalidate: bool,
    pub proxy_revalidate: bool,
    pub immutable: bool,
    pub max_age: Option<i64>,
    pub s_maxage: Option<i64>,
    pub stale_while_revalidate: Option<i64>,
    pub stale_if_error: Option<i64>,
    pub unrecognized: Vec<String>,
}

/// Splits a `Cache-Control` header value into its directives.
///
/// Directives are comma-separated and may carry an `=value` argument,
/// which may itself be quoted (`max-age="3600"` shows up in the wild
/// even though the grammar doesn't require it).
pub fn parse_cache_control(value: &str) -> CacheControl {
    let mut cc = CacheControl::default();
    for part in value.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (name, arg) = match part.split_once('=') {
            Some((n, v)) => (n.trim(), Some(v.trim().trim_matches('"'))),
            None => (part, None),
        };
        match name.to_ascii_lowercase().as_str() {
            "no-store" => cc.no_store = true,
            "no-cache" => cc.no_cache = true,
            "private" => cc.private = true,
            "public" => cc.public = true,
            "must-revalidate" => cc.must_revalidate = true,
            "proxy-revalidate" => cc.proxy_revalidate = true,
            "immutable" => cc.immutable = true,
            "max-age" => cc.max_age = arg.and_then(|v| v.parse().ok()),
            "s-maxage" => cc.s_maxage = arg.and_then(|v| v.parse().ok()),
            "stale-while-revalidate" => {
                cc.stale_while_revalidate = arg.and_then(|v| v.parse().ok())
            }
            "stale-if-error" => cc.stale_if_error = arg.and_then(|v| v.parse().ok()),
            other => cc.unrecognized.push(other.to_string()),
        }
    }
    cc
}

/// Turns raw header text (one `Name: Value` pair per line, as produced by
/// `curl -sI`) into a list of (name, value) pairs. Status lines and blank
/// lines are skipped; header names keep their original case.
pub fn parse_headers(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("HTTP/") {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            out.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    out
}

/// Parses an RFC 1123 HTTP date (`Sun, 06 Nov 1994 08:49:37 GMT`) into a
/// Unix timestamp. This is the only date format `Expires` and `Date` are
/// allowed to use in current HTTP, so nothing else is supported.
pub fn parse_http_date(s: &str) -> Option<i64> {
    let fields: Vec<&str> = s.trim().split_whitespace().collect();
    if fields.len() != 6 || fields[5] != "GMT" {
        return None;
    }
    let day: u32 = fields[1].parse().ok()?;
    let month = month_from_name(fields[2])?;
    let year: i64 = fields[3].parse().ok()?;
    let time: Vec<&str> = fields[4].split(':').collect();
    if time.len() != 3 {
        return None;
    }
    let hour: i64 = time[0].parse().ok()?;
    let minute: i64 = time[1].parse().ok()?;
    let second: i64 = time[2].parse().ok()?;
    let days = days_from_civil(year, month, day);
    Some(days * 86400 + hour * 3600 + minute * 60 + second)
}

fn month_from_name(name: &str) -> Option<u32> {
    Some(match name {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    })
}

/// Days since 1970-01-01 for a given civil (Gregorian) date.
/// Howard Hinnant's `days_from_civil` algorithm - handles the full proleptic
/// Gregorian range correctly, including leap years, without pulling in a
/// date library just to convert `Expires` headers.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m as i64 - 3 } else { m as i64 + 9 }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// Result of evaluating a set of response headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Analysis {
    /// Whether any cache (shared or private) is allowed to store the response.
    pub cacheable: bool,
    /// Seconds until the stored copy goes stale, counting from now.
    /// Negative means it's already stale. `None` means there's no explicit
    /// freshness lifetime to compute one from.
    pub freshness_seconds: Option<i64>,
    /// `ETag` header value, unparsed (still carries its quotes and any
    /// `W/` weak indicator), if the response had one.
    pub etag: Option<String>,
    /// `Last-Modified` header, parsed to a Unix timestamp, if present and
    /// parseable.
    pub last_modified: Option<i64>,
    /// Plain-language explanations for the verdict above, in the order the
    /// rules were applied.
    pub notes: Vec<String>,
}

/// Evaluates cacheability and freshness from a list of response headers.
/// Header name matching is case-insensitive, as HTTP requires.
pub fn analyze(headers: &[(String, String)]) -> Analysis {
    let get = |name: &str| -> Option<&str> {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };

    let mut notes = Vec::new();
    let cc = get("cache-control").map(parse_cache_control).unwrap_or_default();
    let age: i64 = get("age").and_then(|v| v.trim().parse().ok()).unwrap_or(0);

    if cc.no_store {
        notes.push("no-store: response must not be stored by any cache".to_string());
        return Analysis {
            cacheable: false,
            freshness_seconds: None,
            etag: None,
            last_modified: None,
            notes,
        };
    }

    if get("set-cookie").is_some() && !cc.public {
        notes.push(
            "Set-Cookie is present without a public directive; shared caches typically won't store this".to_string(),
        );
    }

    if cc.no_cache {
        notes.push(
            "no-cache: a cache may store this but must revalidate with the origin before reusing it"
                .to_string(),
        );
    }

    let freshness_seconds = if let Some(max_age) = cc.max_age {
        notes.push(format!("freshness computed from max-age={}", max_age));
        Some(max_age - age)
    } else if let Some(expires) = get("expires") {
        match parse_http_date(expires) {
            Some(expires_ts) => match get("date").and_then(parse_http_date) {
                Some(date_ts) => {
                    notes.push("freshness computed from Expires minus Date".to_string());
                    Some(expires_ts - date_ts - age)
                }
                None => {
                    notes.push(
                        "Expires is present but there's no Date header to measure it against"
                            .to_string(),
                    );
                    None
                }
            },
            None => {
                notes.push(format!("couldn't parse Expires header: {}", expires));
                None
            }
        }
    } else {
        notes.push("no max-age or Expires; response has no explicit freshness lifetime".to_string());
        None
    };

    let etag = get("etag").map(|s| s.to_string());
    let last_modified = get("last-modified").and_then(parse_http_date);

    match (&etag, last_modified) {
        (Some(tag), _) => notes.push(format!(
            "has an ETag ({}); once stale it can be revalidated with If-None-Match instead of a full refetch",
            tag
        )),
        (None, Some(_)) => notes.push(
            "has a Last-Modified date; once stale it can be revalidated with If-Modified-Since instead of a full refetch"
                .to_string(),
        ),
        (None, None) => {}
    }

    Analysis {
        cacheable: true,
        freshness_seconds,
        etag,
        last_modified,
        notes,
    }
}

/// Strips the `W/` weak-validator prefix from an `ETag`, if present, leaving
/// the quoted opaque tag behind.
fn strip_weak(tag: &str) -> &str {
    tag.strip_prefix("W/").unwrap_or(tag)
}

/// Compares two `ETag` values using the *weak* comparison function (RFC 7232
/// section 2.3.2): opaque tags must match, but the weak indicator is
/// ignored. This is the comparison `If-None-Match` is defined to use.
pub fn etag_matches(a: &str, b: &str) -> bool {
    strip_weak(a.trim()) == strip_weak(b.trim())
}

/// Evaluates an `If-None-Match` request header against a response's `ETag`.
/// Returns `true` when the precondition is satisfied, meaning the cache's
/// stored copy is still good and the origin should answer `304 Not Modified`
/// rather than resending the body.
///
/// `if_none_match` may be `*` (matches any existing representation) or a
/// comma-separated list of entity tags, per RFC 7232 section 3.2.
pub fn if_none_match_satisfied(if_none_match: &str, etag: &str) -> bool {
    let if_none_match = if_none_match.trim();
    if if_none_match == "*" {
        return !etag.is_empty();
    }
    if etag.is_empty() {
        return false;
    }
    if_none_match
        .split(',')
        .any(|candidate| etag_matches(candidate.trim(), etag))
}

/// Works out whether a conditional request would come back `304 Not
/// Modified` against the response validators `analyze` extracted.
///
/// `If-None-Match` takes precedence over `If-Modified-Since` when both are
/// present, matching the origin server's evaluation order in RFC 7232
/// section 6. Pass `None` for whichever conditional header the request
/// didn't send.
pub fn is_not_modified(
    etag: Option<&str>,
    last_modified: Option<i64>,
    if_none_match: Option<&str>,
    if_modified_since: Option<i64>,
) -> bool {
    if let Some(inm) = if_none_match {
        return if_none_match_satisfied(inm, etag.unwrap_or(""));
    }
    match (last_modified, if_modified_since) {
        (Some(lm), Some(since)) => lm <= since,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_date_parses_to_zero() {
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
    }

    #[test]
    fn no_store_wins_over_everything_else() {
        let headers = vec![(
            "Cache-Control".to_string(),
            "no-store, max-age=3600".to_string(),
        )];
        let a = analyze(&headers);
        assert!(!a.cacheable);
        assert_eq!(a.freshness_seconds, None);
    }

    #[test]
    fn max_age_minus_age_gives_remaining_freshness() {
        let headers = vec![
            ("Cache-Control".to_string(), "max-age=100".to_string()),
            ("Age".to_string(), "40".to_string()),
        ];
        let a = analyze(&headers);
        assert!(a.cacheable);
        assert_eq!(a.freshness_seconds, Some(60));
    }

    #[test]
    fn analyze_picks_up_etag_and_last_modified() {
        let headers = vec![
            ("ETag".to_string(), "\"abc123\"".to_string()),
            (
                "Last-Modified".to_string(),
                "Sun, 06 Nov 1994 08:49:37 GMT".to_string(),
            ),
        ];
        let a = analyze(&headers);
        assert_eq!(a.etag.as_deref(), Some("\"abc123\""));
        assert!(a.last_modified.is_some());
    }

    #[test]
    fn etag_matches_ignores_weak_indicator() {
        assert!(etag_matches("W/\"abc\"", "\"abc\""));
        assert!(etag_matches("\"abc\"", "\"abc\""));
        assert!(!etag_matches("\"abc\"", "\"xyz\""));
    }

    #[test]
    fn if_none_match_star_matches_any_existing_etag() {
        assert!(if_none_match_satisfied("*", "\"abc\""));
        assert!(!if_none_match_satisfied("*", ""));
    }

    #[test]
    fn if_none_match_checks_list_of_candidates() {
        let list = "\"one\", \"two\", W/\"abc\"";
        assert!(if_none_match_satisfied(list, "\"abc\""));
        assert!(!if_none_match_satisfied(list, "\"three\""));
    }

    #[test]
    fn is_not_modified_prefers_etag_over_date() {
        // If-None-Match fails but If-Modified-Since would pass; the ETag
        // check wins and the answer is "modified".
        assert!(!is_not_modified(
            Some("\"abc\""),
            Some(1000),
            Some("\"different\""),
            Some(2000)
        ));
    }

    #[test]
    fn is_not_modified_falls_back_to_last_modified() {
        assert!(is_not_modified(None, Some(1000), None, Some(2000)));
        assert!(!is_not_modified(None, Some(3000), None, Some(2000)));
    }
}
