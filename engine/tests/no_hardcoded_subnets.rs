//! Guard: a subnet literal must never reach `src/`.
//!
//! PLAN.md section 3.7 forbids hardcoding network ranges, and phase 0 showed
//! why in one measurement: the USB tether came up on a prefix nowhere near the
//! one every tutorial attributes to Android USB tethering. Code looking for the
//! traditional range would have silently failed to find the phone, and
//! "silently failed to find a link" is the worst failure mode this project
//! has, because the download still succeeds, just slower, with no error.
//!
//! So the rule is enforced by a test rather than by review. The guard lands in
//! the same commit as the first types, before any code exists that could break
//! it, which is the only moment a rule like this is free to adopt.
//!
//! WHAT IS SCANNED, and why strings are deliberately kept
//!
//! Comments are blanked, and whole `#[cfg(test)]` blocks are blanked, because
//! prose and test fixtures may legitimately contain addresses. **String
//! literals in ordinary code are NOT blanked**, since that is precisely where
//! a subnet literal would appear: `"192.168.42.0/24".parse()`. Blanking them
//! would leave the guard passing forever while catching nothing.
//!
//! TWO SHAPES OF THE SAME MISTAKE
//!
//! A dotted literal, `"192.168.42.0"`, and a constructor fed numbers,
//! `Ipv4Addr::new(192, 168, 42, 1)`, which contains no dots at all. Both are
//! caught. For a constructor, any integer literal among its arguments counts,
//! not only an all-literal call, because `Ipv4Addr::new(10, 0, x, 1)` still
//! hardcodes a subnet.
//!
//! The addresses the standard library already names need no exception: write
//! `Ipv4Addr::UNSPECIFIED` or `Ipv6Addr::LOCALHOST`, which the guard ignores.
//! For anything else that is genuinely fixed, put a comment on the line
//! directly above, with a reason:
//!
//! ```text
//! // subnet-guard: allow <why this address cannot come from the OS>
//! ```
//!
//! A marker with no reason does not count. The point of the opt-out is that
//! every exception explains itself to the next reader.
//!
//! Known limits, so nobody trusts this further than it deserves:
//!   - Only `src/` is scanned. Integration tests may contain addresses.
//!   - Only calls spelt `Ipv4Addr::`, `Ipv6Addr::` or `IpAddr::` are seen.
//!     An aliased import (`use std::net::Ipv4Addr as V4`) or an array turned
//!     into an address with `.into()` slips through.
//!   - A constructor whose arguments contain an unrelated number, such as an
//!     index into a byte buffer, is reported anyway. That is what the opt-out
//!     is for.
//!   - Raw strings are handled approximately. The failure mode is a false
//!     positive, which is loud and instantly fixable, never a false negative.

use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn no_hardcoded_subnets_in_source() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let offenders = scan_src_for_subnet_literals(&src);

    assert!(
        offenders.is_empty(),
        "subnet literals found in src/, forbidden by PLAN.md section 3.7.\n  \
         Read the value from the OS at runtime instead of naming a range.\n  \
         For an address the standard library names, use its constant.\n  \
         For anything else genuinely fixed, put `{OPT_OUT} <reason>` on the line above.\n  {}",
        offenders.join("\n  ")
    );
}

/// Every offence in `src/`, each reading `path:line: token`.
///
/// Panics rather than skipping when something cannot be read. A guard that
/// quietly passes because it looked at nothing is the same failure as a guard
/// that looks and cannot see.
fn scan_src_for_subnet_literals(src_root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    collect_rs_files(src_root, &mut files);
    files.sort();
    assert!(
        files.iter().any(|f| f.ends_with("lib.rs")),
        "the guard found no lib.rs under {}, so it is scanning the wrong place",
        src_root.display()
    );

    let mut offenders = Vec::new();
    for path in files {
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("the guard cannot read {}: {e}", path.display()));
        let shown = path
            .strip_prefix(src_root.parent().unwrap_or(src_root))
            .unwrap_or(&path)
            .display()
            .to_string();

        for (line, what) in find_offences(&text) {
            offenders.push(format!("{shown}:{line}: {what}"));
        }
    }
    offenders
}

/// Every forbidden literal in one file's text, as `(line, offending text)`
/// with lines counted from 1 against the real file.
fn find_offences(text: &str) -> Vec<(usize, String)> {
    let cleaned = blank_test_modules(&blank_comments(text));

    let mut found = dotted_literals(&cleaned);
    found.extend(constructor_literals(&cleaned));
    found.sort();

    let raw: Vec<&str> = text.lines().collect();
    found.retain(|(line, _)| !opted_out(&raw, *line));
    found
}

const OPT_OUT: &str = "// subnet-guard: allow";

/// Whether the line directly above `line` is an opt-out comment that gives a
/// reason. Checked against the raw text, since comments are blanked from the
/// cleaned copy that everything else reads.
fn opted_out(raw: &[&str], line: usize) -> bool {
    let Some(above) = line.checked_sub(2).and_then(|i| raw.get(i)) else {
        return false;
    };
    above
        .trim_start()
        .strip_prefix(OPT_OUT)
        .is_some_and(|reason| reason.starts_with(char::is_whitespace) && !reason.trim().is_empty())
}

/// Types whose constructors turn numbers into an address.
///
/// `SocketAddr` and its relatives are deliberately absent. Their numeric
/// argument is a port, and binding to port 0 is how every probe will ask the
/// OS for any free port.
const ADDRESS_TYPES: &[&str] = &["Ipv4Addr", "Ipv6Addr", "IpAddr"];
const CONSTRUCTORS: &[&str] = &["new", "from", "from_bits", "from_octets", "from_segments"];

/// Constructor calls with an integer literal anywhere in their arguments,
/// reported at the line the call starts on.
fn constructor_literals(cleaned: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = cleaned.chars().collect();
    let mut found = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        let Some((args, end)) = constructor_call_at(&chars, i) else {
            i += 1;
            continue;
        };
        if has_integer_literal(&chars[args]) {
            let line = 1 + chars[..i].iter().filter(|&&c| c == '\n').count();
            let call: String = chars[i..end].iter().collect();
            found.push((line, call.split_whitespace().collect::<Vec<_>>().join(" ")));
        }
        // Skip past the whole call. A nested constructor's arguments are a
        // subset of this one's, so it was judged already, and reporting it
        // again would double-count one mistake.
        i = end;
    }
    found
}

/// If an address constructor call starts at `i`, the range of its argument
/// text and the index just past its closing parenthesis.
fn constructor_call_at(chars: &[char], i: usize) -> Option<(std::ops::Range<usize>, usize)> {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    if i > 0 && is_ident(chars[i - 1]) {
        return None;
    }

    let rest = |at: usize, s: &str| -> bool {
        let s: Vec<char> = s.chars().collect();
        chars.get(at..at + s.len()) == Some(&s[..])
    };
    let ty = ADDRESS_TYPES.iter().find(|t| rest(i, &format!("{t}::")))?;

    let name_start = i + ty.len() + 2;
    let mut j = name_start;
    while j < chars.len() && is_ident(chars[j]) {
        j += 1;
    }
    let name: String = chars[name_start..j].iter().collect();
    if !CONSTRUCTORS.contains(&name.as_str()) {
        return None;
    }

    while j < chars.len() && chars[j].is_whitespace() {
        j += 1;
    }
    if chars.get(j) != Some(&'(') {
        return None;
    }

    let open = j;
    let mut depth = 0i32;
    let mut in_string = false;
    while j < chars.len() {
        let c = chars[j];
        if in_string {
            if c == '\\' {
                j += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        } else if c == '(' {
            depth += 1;
        } else if c == ')' {
            depth -= 1;
            if depth == 0 {
                return Some((open + 1..j, j + 1));
            }
        }
        j += 1;
    }
    None
}

/// Whether any token in `args` is an integer literal: it starts with a digit
/// and is not a tuple field such as the `0` in `pair.0`.
fn has_integer_literal(args: &[char]) -> bool {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    (0..args.len()).any(|p| {
        args[p].is_ascii_digit() && (p == 0 || !(is_ident(args[p - 1]) || args[p - 1] == '.'))
    })
}

fn dotted_literals(cleaned: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (index, line) in cleaned.lines().enumerate() {
        for token in line.split(|c: char| !(c.is_ascii_digit() || c == '.')) {
            if looks_like_subnet(token) {
                found.push((index + 1, token.to_string()));
            }
        }
    }
    found
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Three or more dot-separated runs of one to three digits, no empty segment.
///
/// Matches `10.0.0.1` and a bare `192.168.155` prefix, while rejecting tuple
/// access like `.0.1` (empty leading segment) and floats like `1.0` (too few
/// segments).
fn looks_like_subnet(token: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    parts.len() >= 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit()))
}

/// Blank comments, preserving length and newlines so reported line numbers
/// still match the real file.
///
/// String literals are tracked but **copied through unchanged**. Tracking them
/// is what stops `"https://example.com"` being misread as starting a comment;
/// keeping their contents is what lets the guard see an address written as a
/// string, which is the normal way one would appear.
fn blank_comments(src: &str) -> String {
    enum State {
        Code,
        LineComment,
        BlockComment(u32),
        StringLit,
    }

    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(chars.len());
    let mut state = State::Code;
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied().unwrap_or('\0');

        match state {
            State::Code => {
                if c == '/' && next == '/' {
                    state = State::LineComment;
                    out.push_str("  ");
                    i += 2;
                } else if c == '/' && next == '*' {
                    state = State::BlockComment(1);
                    out.push_str("  ");
                    i += 2;
                } else {
                    if c == '"' {
                        state = State::StringLit;
                    }
                    out.push(c);
                    i += 1;
                }
            }
            State::LineComment => {
                if c == '\n' {
                    state = State::Code;
                    out.push('\n');
                } else {
                    out.push(' ');
                }
                i += 1;
            }
            State::BlockComment(depth) => {
                if c == '/' && next == '*' {
                    state = State::BlockComment(depth + 1);
                    out.push_str("  ");
                    i += 2;
                } else if c == '*' && next == '/' {
                    state = if depth == 1 {
                        State::Code
                    } else {
                        State::BlockComment(depth - 1)
                    };
                    out.push_str("  ");
                    i += 2;
                } else {
                    out.push(if c == '\n' { '\n' } else { ' ' });
                    i += 1;
                }
            }
            State::StringLit => {
                if c == '\\' {
                    out.push(c);
                    out.push(next);
                    i += 2;
                } else {
                    if c == '"' {
                        state = State::Code;
                    }
                    out.push(c);
                    i += 1;
                }
            }
        }
    }
    out
}

/// Blank every `#[cfg(test)]` block, attribute and body, by brace matching.
///
/// Brace matching skips string literals, so a `{` inside a string in a test
/// module cannot throw the match off and swallow the rest of the file.
fn blank_test_modules(src: &str) -> String {
    let needle: Vec<char> = "#[cfg(test)]".chars().collect();
    let mut chars: Vec<char> = src.chars().collect();
    let mut i = 0;

    while i + needle.len() <= chars.len() {
        if chars[i..i + needle.len()] != needle[..] {
            i += 1;
            continue;
        }

        let mut j = i + needle.len();
        while j < chars.len() && chars[j] != '{' {
            j += 1;
        }
        if j >= chars.len() {
            break;
        }

        let mut depth = 0i32;
        let mut in_string = false;
        while j < chars.len() {
            let c = chars[j];
            if in_string {
                if c == '\\' {
                    j += 2;
                    continue;
                }
                if c == '"' {
                    in_string = false;
                }
            } else if c == '"' {
                in_string = true;
            } else if c == '{' {
                depth += 1;
            } else if c == '}' {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            j += 1;
        }
        let end = j.min(chars.len().saturating_sub(1));

        for ch in chars[i..end + 1].iter_mut() {
            if *ch != '\n' {
                *ch = ' ';
            }
        }
        i = end + 1;
    }

    chars.into_iter().collect()
}

// The guard's own tests.
//
// An early version of this guard blanked string literals, which is exactly
// where an address gets written, so it passed while catching nothing. That
// was found only because a fixture was planted by hand. These tests replace
// the manual step: a guard that goes blind now fails on its own.

fn lines(found: &[(usize, String)]) -> Vec<usize> {
    found.iter().map(|(line, _)| *line).collect()
}

#[test]
fn guard_catches_a_dotted_address_in_a_string() {
    let found = find_offences(r#"let net: Net = "192.168.42.0/24".parse()?;"#);
    assert_eq!(lines(&found), [1], "{found:?}");
}

#[test]
fn guard_ignores_addresses_in_comments_and_test_modules() {
    let src = r#"
// The tether once came up on 192.168.155.0/24.
/* and a dock on 10.0.0.1 */
fn real() {}

#[cfg(test)]
mod tests {
    const FIXTURE: &str = "192.0.2.10";
    fn f() { let _ = Ipv4Addr::new(192, 0, 2, 10); }
}
"#;
    let found = find_offences(src);
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn guard_catches_a_constructor_with_literal_octets() {
    let found = find_offences("let gateway = Ipv4Addr::new(192, 168, 42, 1);");
    assert_eq!(lines(&found), [1], "{found:?}");
}

#[test]
fn guard_reports_a_multi_line_constructor_at_its_first_line() {
    let src = "fn f() {\n    let a = Ipv6Addr::new(\n        0x2001, 0xdb8, 0, 0,\n        0, 0, 0, 1,\n    );\n}\n";
    let found = find_offences(src);
    assert_eq!(lines(&found), [2], "{found:?}");
}

#[test]
fn guard_catches_array_integer_and_partly_literal_forms() {
    let src = "\
let a = Ipv4Addr::from([10, 0, 0, 1]);
let b = IpAddr::from([192, 168, 1, 1]);
let c = Ipv4Addr::from_bits(0xC0A8_0001);
let d = std::net::Ipv4Addr::new(10, 0, octet, 1);
";
    let found = find_offences(src);
    assert_eq!(lines(&found), [1, 2, 3, 4], "{found:?}");
}

#[test]
fn guard_allows_addresses_built_from_variables_or_named_constants() {
    let src = "\
let a = Ipv4Addr::new(o1, o2, o3, o4);
let b = Ipv4Addr::UNSPECIFIED;
let c = Ipv6Addr::LOCALHOST;
let d = Ipv4Addr::from(pair.0);
let e = SocketAddr::new(local, 0);
let f = Addr::from(ip);
let g = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 8080);
";
    let found = find_offences(src);
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn guard_honours_an_opt_out_only_when_it_gives_a_reason() {
    let src = "\
// subnet-guard: allow the resolver of last resort when DNS itself is down
let a = Ipv4Addr::new(9, 9, 9, 9);
// subnet-guard: allow
let b = Ipv4Addr::new(9, 9, 9, 9);
// subnet-guard: allow a documented fallback, see PLAN.md section 9
let c = \"203.0.113.7\";
";
    let found = find_offences(src);
    assert_eq!(lines(&found), [4], "{found:?}");
}
