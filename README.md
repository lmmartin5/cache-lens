# cache-lens

You run `curl -sI` against some URL, get back a wall of headers, and then
have to remember whether `Cache-Control: no-cache` actually means "don't
cache this" (it doesn't) or how `Age` factors into `max-age`, or whether
`Expires` still matters when `max-age` is also set (it doesn't). The rules
are all in RFC 7234, but nobody wants to re-read a spec to answer "will
this response get cached, and for how long."

cache-lens reads a set of response headers and gives you a straight
answer: is this cacheable, how many seconds of freshness are left, and
why. It's a small Rust library (`cache_lens`) with a thin CLI on top.

## build

```
cargo build --release
```

No external crates - the whole thing is standard library, including the
HTTP date parsing for `Expires`.

## usage

Pipe real response headers in:

```
$ curl -sI https://example.com | cache-lens
cacheable: true
fresh for: 3421s
notes:
  - freshness computed from max-age=3600
```

Or read them from a file:

```
$ cache-lens --file headers.txt
```

Add `--json` for machine-readable output - the shape is stable and meant
to be piped into other tools:

```
$ cache-lens --json < headers.txt
{"cacheable":true,"freshness_seconds":3421,"etag":null,"last_modified":null,"vary":[],"stale_while_revalidate":null,"stale_if_error":null,"not_modified":null,"notes":["freshness computed from max-age=3600"]}
```

`freshness_seconds` is `null` when there's no `max-age` or usable
`Expires`/`Date` pair to compute it from, and negative when the response
is already stale.

## stale-while-revalidate and stale-if-error

These two `Cache-Control` extensions (RFC 5861) don't change how long a
response is fresh - they say how much longer a cache may keep serving it
*after* it goes stale, either while fetching a fresh copy in the
background (`stale-while-revalidate`) or when that fetch fails
(`stale-if-error`). cache-lens surfaces both as separate fields rather
than folding them into `freshness_seconds`, since serving stale content
under either directive is a distinct, cache-implementation-dependent
behavior from being fresh:

```
$ cache-lens < headers.txt
cacheable: true
fresh for: 55s
stale-while-revalidate: 30s
notes:
  - freshness computed from max-age=60
  - stale-while-revalidate=30: once stale, a cache may keep serving this for up to 30s while it revalidates in the background
```

## Vary

A cached response is only reusable for a later request if that request
matches on every header listed in `Vary`. cache-lens flags the two ways
that tends to go wrong: `Vary: *`, which means the response depends on
something outside the request headers it can see and so a shared cache
can basically never reuse it, and `Vary` lists that are broad enough to
fragment the cache badly - `User-Agent` (near-unique per client) or any
list longer than three headers:

```
$ cache-lens < headers.txt
cacheable: true
fresh for: 3421s
vary: Accept-Encoding, User-Agent
notes:
  - freshness computed from max-age=3600
  - Vary includes User-Agent, whose value differs per client; this fragments the cache into roughly one entry per visitor
```

## revalidation

Once a response goes stale, a cache doesn't have to refetch the whole
thing - if the response carries an `ETag` or `Last-Modified`, the cache
can send a conditional request and the origin answers `304 Not Modified`
if nothing changed. Check what that conditional request would get back:

```
$ cache-lens --if-none-match '"abc123"' < headers.txt
cacheable: true
stale by: 12s
etag: "abc123"
revalidation: 304 Not Modified
```

`--if-modified-since <http-date>` works the same way against
`Last-Modified`. If both are given, `--if-none-match` wins, matching how
an origin server evaluates the two (RFC 7232 section 6). The JSON output
gains a `not_modified` boolean (`null` unless one of these flags is
passed).

## library

```rust
let headers = cache_lens::parse_headers(raw_header_text);
let analysis = cache_lens::analyze(&headers);
println!("{}", analysis.cacheable);
```

`parse_cache_control` and `parse_http_date` are also public if you just
need those two pieces, and `is_not_modified` / `if_none_match_satisfied` /
`etag_matches` implement the `If-None-Match` and `If-Modified-Since`
comparison rules from RFC 7232 on their own.

## input format

One header per line, `Name: Value`, the way `curl -sI` prints them.
Header names are matched case-insensitively. A leading `HTTP/1.1 200 OK`
status line, if present, is ignored.

## license

MIT, see LICENSE.
