//! Integration tests against canned header sets shaped like what
//! `curl -sI` actually returns from real sites, rather than the minimal
//! two- or three-header fixtures the unit tests use. The point is to catch
//! interactions between directives that only show up once a response has
//! all the headers a real server sends at once.

const STATIC_ASSET: &str = "\
HTTP/2 200
content-type: application/javascript; charset=utf-8
content-length: 84213
cache-control: public, max-age=31536000, immutable
etag: \"7f3a9c-84213\"
last-modified: Tue, 14 Jan 2025 10:03:00 GMT
vary: Accept-Encoding
";

const HTML_PAGE: &str = "\
HTTP/1.1 200 OK
content-type: text/html; charset=UTF-8
cache-control: no-cache, must-revalidate
etag: W/\"a1b2c3d4\"
vary: Accept-Encoding, Accept-Language
date: Mon, 20 Jan 2025 12:00:00 GMT
";

const PRIVATE_API_RESPONSE: &str = "\
HTTP/1.1 200 OK
content-type: application/json
cache-control: private, no-store
set-cookie: session=abc123; Secure; HttpOnly
";

const LEGACY_EXPIRES_ONLY: &str = "\
HTTP/1.1 200 OK
content-type: image/png
date: Wed, 21 Oct 2015 07:28:00 GMT
expires: Wed, 21 Oct 2015 08:28:00 GMT
last-modified: Fri, 01 Aug 2014 00:00:00 GMT
";

const CDN_WITH_STALE_WHILE_REVALIDATE: &str = "\
HTTP/2 200
content-type: text/html; charset=utf-8
cache-control: public, max-age=120, stale-while-revalidate=600, stale-if-error=86400
age: 45
server: cloudflare
";

const OVERFRAGMENTED_VARY: &str = "\
HTTP/1.1 200 OK
content-type: text/html
cache-control: public, max-age=60
vary: Accept, Accept-Encoding, Accept-Language, Cookie, User-Agent
";

const REDIRECT_CHAIN: &str = "\
HTTP/1.1 301 Moved Permanently
location: https://example.com/new-path
cache-control: max-age=3600

HTTP/1.1 200 OK
content-type: text/html; charset=UTF-8
cache-control: public, max-age=600
etag: \"final-page-v3\"
";

#[test]
fn static_asset_is_cacheable_for_a_year_and_flags_no_fragmentation() {
    let headers = cache_lens::parse_headers(STATIC_ASSET);
    let a = cache_lens::analyze(&headers);
    assert!(a.cacheable);
    assert_eq!(a.freshness_seconds, Some(31536000));
    assert_eq!(a.etag.as_deref(), Some("\"7f3a9c-84213\""));
    assert!(a.last_modified.is_some());
    assert!(!a.notes.iter().any(|n| n.contains("fragment")));
}

#[test]
fn html_page_with_no_cache_must_revalidate_before_reuse() {
    let headers = cache_lens::parse_headers(HTML_PAGE);
    let a = cache_lens::analyze(&headers);
    assert!(a.cacheable);
    assert_eq!(a.freshness_seconds, None);
    assert!(a.notes.iter().any(|n| n.contains("must revalidate")));
    assert_eq!(a.vary, vec!["Accept-Encoding".to_string(), "Accept-Language".to_string()]);
}

#[test]
fn private_api_response_with_cookie_is_not_cacheable() {
    let headers = cache_lens::parse_headers(PRIVATE_API_RESPONSE);
    let a = cache_lens::analyze(&headers);
    assert!(!a.cacheable);
    assert_eq!(a.freshness_seconds, None);
}

#[test]
fn legacy_expires_only_response_computes_freshness_from_date_pair() {
    let headers = cache_lens::parse_headers(LEGACY_EXPIRES_ONLY);
    let a = cache_lens::analyze(&headers);
    assert!(a.cacheable);
    // Expires is exactly one hour after Date in this fixture.
    assert_eq!(a.freshness_seconds, Some(3600));
    assert!(a.last_modified.is_some());
}

#[test]
fn cdn_response_surfaces_age_adjusted_freshness_and_both_stale_windows() {
    let headers = cache_lens::parse_headers(CDN_WITH_STALE_WHILE_REVALIDATE);
    let a = cache_lens::analyze(&headers);
    assert!(a.cacheable);
    assert_eq!(a.freshness_seconds, Some(120 - 45));
    assert_eq!(a.stale_while_revalidate, Some(600));
    assert_eq!(a.stale_if_error, Some(86400));
}

#[test]
fn overfragmented_vary_list_is_flagged() {
    let headers = cache_lens::parse_headers(OVERFRAGMENTED_VARY);
    let a = cache_lens::analyze(&headers);
    assert!(a.cacheable);
    assert_eq!(a.vary.len(), 5);
    assert!(a.notes.iter().any(|n| n.contains("lists 5 headers")));
}

#[test]
fn redirect_chain_is_split_into_two_analyzable_responses() {
    let blocks = cache_lens::parse_response_blocks(REDIRECT_CHAIN);
    assert_eq!(blocks.len(), 2);

    let redirect = cache_lens::analyze(&blocks[0]);
    assert!(redirect.cacheable);
    assert_eq!(redirect.freshness_seconds, Some(3600));

    let final_page = cache_lens::analyze(&blocks[1]);
    assert!(final_page.cacheable);
    assert_eq!(final_page.freshness_seconds, Some(600));
    assert_eq!(final_page.etag.as_deref(), Some("\"final-page-v3\""));
}

#[test]
fn conditional_request_against_static_asset_etag_is_satisfied() {
    let headers = cache_lens::parse_headers(STATIC_ASSET);
    let a = cache_lens::analyze(&headers);
    let not_modified = cache_lens::is_not_modified(
        a.etag.as_deref(),
        a.last_modified,
        Some("\"7f3a9c-84213\""),
        None,
    );
    assert!(not_modified);
}

#[test]
fn conditional_request_against_html_page_weak_etag_is_satisfied() {
    let headers = cache_lens::parse_headers(HTML_PAGE);
    let a = cache_lens::analyze(&headers);
    // The client sends back a strong comparison candidate; weak comparison
    // still matches because If-None-Match ignores the W/ prefix.
    let not_modified = cache_lens::is_not_modified(
        a.etag.as_deref(),
        a.last_modified,
        Some("\"a1b2c3d4\""),
        None,
    );
    assert!(not_modified);
}
