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
//! Known limits, so nobody trusts this further than it deserves:
//!   - Only `src/` is scanned. Integration tests may contain addresses.
//!   - `Ipv4Addr::new(192, 168, 42, 1)` has no dots and is NOT caught. Closing
//!     that gap means matching constructor calls with literal arguments, which
//!     is a separate piece of work; see the task 3 note in the phase 1 plan.
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
         Read the value from the OS at runtime instead of naming a range:\n  {}",
        offenders.join("\n  ")
    );
}

/// Every dotted-numeric literal left in `src/` once comments and
/// `#[cfg(test)]` blocks are removed. Each entry reads `path:line: token`.
fn scan_src_for_subnet_literals(src_root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    collect_rs_files(src_root, &mut files);
    files.sort();

    let mut offenders = Vec::new();
    for path in files {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let cleaned = blank_test_modules(&blank_comments(&text));

        let shown = path
            .strip_prefix(src_root.parent().unwrap_or(src_root))
            .unwrap_or(&path)
            .display()
            .to_string();

        for (index, line) in cleaned.lines().enumerate() {
            for token in line.split(|c: char| !(c.is_ascii_digit() || c == '.')) {
                if looks_like_subnet(token) {
                    offenders.push(format!("{shown}:{}: {token}", index + 1));
                }
            }
        }
    }
    offenders
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
