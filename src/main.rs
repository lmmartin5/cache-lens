use std::io::Read;

fn main() {
    let mut json = false;
    let mut file: Option<String> = None;
    let mut if_none_match: Option<String> = None;
    let mut if_modified_since: Option<String> = None;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--file" => {
                i += 1;
                match args.get(i) {
                    Some(path) => file = Some(path.clone()),
                    None => {
                        eprintln!("--file needs a path");
                        std::process::exit(2);
                    }
                }
            }
            "--if-none-match" => {
                i += 1;
                match args.get(i) {
                    Some(v) => if_none_match = Some(v.clone()),
                    None => {
                        eprintln!("--if-none-match needs a value");
                        std::process::exit(2);
                    }
                }
            }
            "--if-modified-since" => {
                i += 1;
                match args.get(i) {
                    Some(v) => if_modified_since = Some(v.clone()),
                    None => {
                        eprintln!("--if-modified-since needs an HTTP date");
                        std::process::exit(2);
                    }
                }
            }
            "-h" | "--help" => {
                print_help();
                return;
            }
            other => {
                eprintln!("unrecognized argument: {}", other);
                print_help();
                std::process::exit(2);
            }
        }
        i += 1;
    }

    let if_modified_since_ts = match if_modified_since.as_deref() {
        Some(raw) => match cache_lens::parse_http_date(raw) {
            Some(ts) => Some(ts),
            None => {
                eprintln!("couldn't parse --if-modified-since date: {}", raw);
                std::process::exit(2);
            }
        },
        None => None,
    };

    let input = match file {
        Some(path) => std::fs::read_to_string(&path).unwrap_or_else(|e| {
            eprintln!("failed to read {}: {}", path, e);
            std::process::exit(1);
        }),
        None => {
            let mut buf = String::new();
            if std::io::stdin().read_to_string(&mut buf).is_err() {
                eprintln!("failed to read stdin");
                std::process::exit(1);
            }
            buf
        }
    };

    let headers = cache_lens::parse_headers(&input);
    let analysis = cache_lens::analyze(&headers);

    let not_modified = if if_none_match.is_some() || if_modified_since_ts.is_some() {
        Some(cache_lens::is_not_modified(
            analysis.etag.as_deref(),
            analysis.last_modified,
            if_none_match.as_deref(),
            if_modified_since_ts,
        ))
    } else {
        None
    };

    if json {
        println!("{}", to_json(&analysis, not_modified));
    } else {
        print_human(&analysis, not_modified);
    }
}

fn print_human(a: &cache_lens::Analysis, not_modified: Option<bool>) {
    println!("cacheable: {}", a.cacheable);
    match a.freshness_seconds {
        Some(s) if s >= 0 => println!("fresh for: {}s", s),
        Some(s) => println!("stale by: {}s", -s),
        None => println!("freshness: unknown"),
    }
    if let Some(etag) = &a.etag {
        println!("etag: {}", etag);
    }
    if !a.vary.is_empty() {
        println!("vary: {}", a.vary.join(", "));
    }
    if let Some(swr) = a.stale_while_revalidate {
        println!("stale-while-revalidate: {}s", swr);
    }
    if let Some(sie) = a.stale_if_error {
        println!("stale-if-error: {}s", sie);
    }
    if let Some(not_modified) = not_modified {
        println!(
            "revalidation: {}",
            if not_modified {
                "304 Not Modified"
            } else {
                "200 (send full response)"
            }
        );
    }
    if !a.notes.is_empty() {
        println!("notes:");
        for note in &a.notes {
            println!("  - {}", note);
        }
    }
}

fn to_json(a: &cache_lens::Analysis, not_modified: Option<bool>) -> String {
    let mut out = String::from("{");
    out.push_str("\"cacheable\":");
    out.push_str(if a.cacheable { "true" } else { "false" });

    out.push_str(",\"freshness_seconds\":");
    match a.freshness_seconds {
        Some(v) => out.push_str(&v.to_string()),
        None => out.push_str("null"),
    }

    out.push_str(",\"etag\":");
    match &a.etag {
        Some(v) => out.push_str(&json_escape(v)),
        None => out.push_str("null"),
    }

    out.push_str(",\"last_modified\":");
    match a.last_modified {
        Some(v) => out.push_str(&v.to_string()),
        None => out.push_str("null"),
    }

    out.push_str(",\"vary\":[");
    for (idx, name) in a.vary.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push_str(&json_escape(name));
    }
    out.push(']');

    out.push_str(",\"stale_while_revalidate\":");
    match a.stale_while_revalidate {
        Some(v) => out.push_str(&v.to_string()),
        None => out.push_str("null"),
    }

    out.push_str(",\"stale_if_error\":");
    match a.stale_if_error {
        Some(v) => out.push_str(&v.to_string()),
        None => out.push_str("null"),
    }

    out.push_str(",\"not_modified\":");
    match not_modified {
        Some(v) => out.push_str(if v { "true" } else { "false" }),
        None => out.push_str("null"),
    }

    out.push_str(",\"notes\":[");
    for (idx, note) in a.notes.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push_str(&json_escape(note));
    }
    out.push_str("]}");
    out
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn print_help() {
    println!("cache-lens - inspect HTTP cache headers");
    println!();
    println!("usage:");
    println!("  curl -sI https://example.com | cache-lens");
    println!("  cache-lens --file headers.txt");
    println!("  cache-lens --json < headers.txt");
    println!("  cache-lens --if-none-match '\"abc123\"' < headers.txt");
    println!();
    println!("reads response headers (one \"Name: Value\" pair per line) from");
    println!("stdin or --file, and reports whether the response is cacheable");
    println!("and how long it stays fresh.");
    println!();
    println!("--if-none-match <value>       check the response's ETag against a");
    println!("                              client-supplied If-None-Match value");
    println!("--if-modified-since <date>    check Last-Modified against a client-");
    println!("                              supplied If-Modified-Since HTTP date");
    println!("                              (If-None-Match wins when both are given)");
}
