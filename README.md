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
{"cacheable":true,"freshness_seconds":3421,"etag":null,"last_modified":null,"not_modified":null,"notes":["freshness computed from max-age=3600"]}
```

`freshness_seconds` is `null` when there's no `max-age` or usable
`Expires`/`Date` pair to compute it from, and negative when the response
is already stale.

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
