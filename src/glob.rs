/*!
 * Glob Patterns
 *
 * Patterns for KEYS, SCAN MATCH and CONFIG GET. A pattern matches exactly
 * the strings Redis's `stringmatchlen` matches, quirks included (see the
 * tests), but without recursion: a `*` only ever backtracks to the last
 * star, so matching takes at most pattern length × string length steps.
 */

/// Redis gives up on a pattern whose match recurses through more stars
const MAX_STARS: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// `*` (consecutive stars count as one): any run of bytes
    Star,
    /// `?`: any byte
    Any,
    /// A byte, possibly escaped with `\`
    Byte(u8),
    /// `[...]` or `[^...]`: a byte that is (or is not) in the set
    Class {
        negated: bool,
        items: Vec<ClassItem>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClassItem {
    Byte(u8),
    /// A byte escaped with `\`, which Redis compares case-sensitively even
    /// when case is ignored
    Escaped(u8),
    /// `a-z`: bytes compared as C `char`s, which are signed (so `\xff` is
    /// -1), with reversed bounds swapped, as Redis does
    Range(i32, i32),
}

/// A compiled glob pattern
#[derive(Debug, Clone)]
pub(crate) struct Pattern {
    tokens: Vec<Token>,
    nocase: bool,
}

/// A byte as a signed C `char`
fn signed(byte: u8) -> i32 {
    byte as i8 as i32
}

/// C `tolower` in the C locale, on a signed `char`
fn lower(c: i32) -> i32 {
    if (i32::from(b'A')..=i32::from(b'Z')).contains(&c) {
        c + 32
    } else {
        c
    }
}

impl Pattern {
    /// Compile `pattern`; with `nocase`, ASCII letters match in either case
    pub(crate) fn new(pattern: &[u8], nocase: bool) -> Self {
        let mut tokens = Vec::new();
        let mut i = 0;
        while i < pattern.len() {
            match pattern[i] {
                b'*' => {
                    if tokens.last() != Some(&Token::Star) {
                        tokens.push(Token::Star);
                    }
                    i += 1;
                }
                b'?' => {
                    tokens.push(Token::Any);
                    i += 1;
                }
                b'[' => {
                    let (token, next) = class(pattern, i + 1);
                    tokens.push(token);
                    i = next;
                }
                // A trailing backslash is a plain byte
                b'\\' if i + 1 < pattern.len() => {
                    tokens.push(Token::Byte(pattern[i + 1]));
                    i += 2;
                }
                byte => {
                    tokens.push(Token::Byte(byte));
                    i += 1;
                }
            }
        }
        Self { tokens, nocase }
    }

    fn token_matches(&self, token: &Token, c: u8) -> bool {
        match token {
            Token::Star | Token::Any => true,
            Token::Byte(b) => self.same(*b, c),
            Token::Class { negated, items } => {
                let found = items.iter().any(|item| match *item {
                    ClassItem::Byte(b) => self.same(b, c),
                    ClassItem::Escaped(b) => b == c,
                    ClassItem::Range(start, end) => {
                        let (mut start, mut end, mut c) = (start, end, signed(c));
                        if self.nocase {
                            (start, end, c) = (lower(start), lower(end), lower(c));
                        }
                        (start..=end).contains(&c)
                    }
                });
                found != *negated
            }
        }
    }

    fn same(&self, a: u8, b: u8) -> bool {
        if self.nocase {
            a.eq_ignore_ascii_case(&b)
        } else {
            a == b
        }
    }

    /// Whether the whole of `s` matches
    pub(crate) fn matches(&self, s: &[u8]) -> bool {
        // Redis only matches the empty string with the empty pattern (KEYS
        // and SCAN treat the pattern `*` specially)
        if s.is_empty() {
            return self.tokens.is_empty();
        }
        let tokens = &self.tokens;
        let (mut t, mut i) = (0, 0);
        // The token after the last star, and where in `s` its match starts
        let mut resume: Option<(usize, usize)> = None;
        let mut stars = 0;
        while i < s.len() {
            match tokens.get(t) {
                Some(Token::Star) => {
                    if t + 1 == tokens.len() {
                        return true;
                    }
                    stars += 1;
                    if stars > MAX_STARS {
                        return false;
                    }
                    resume = Some((t + 1, i));
                    t += 1;
                    continue;
                }
                Some(token) if self.token_matches(token, s[i]) => {
                    t += 1;
                    i += 1;
                    continue;
                }
                _ => {}
            }
            // Mismatch: let the last star take one more byte
            let Some((after_star, start)) = resume else {
                return false;
            };
            resume = Some((after_star, start + 1));
            t = after_star;
            i = start + 1;
        }
        tokens[t..].iter().all(|token| *token == Token::Star)
    }
}

/// Parse the class whose content (after `[`) starts at `i`, the way Redis
/// reads it: `\` escapes the next byte, `x-y` is a range when two more
/// bytes follow, and a class without `]` runs to the end of the pattern.
/// Returns the token and the index after the class.
fn class(pattern: &[u8], mut i: usize) -> (Token, usize) {
    let negated = pattern.get(i) == Some(&b'^');
    if negated {
        i += 1;
    }
    let mut items = Vec::new();
    loop {
        let rest = pattern.len() - i;
        if rest >= 2 && pattern[i] == b'\\' {
            items.push(ClassItem::Escaped(pattern[i + 1]));
            i += 2;
        } else if rest == 0 {
            break;
        } else if pattern[i] == b']' {
            i += 1;
            break;
        } else if rest >= 3 && pattern[i + 1] == b'-' {
            let (start, end) = (signed(pattern[i]), signed(pattern[i + 2]));
            items.push(ClassItem::Range(start.min(end), start.max(end)));
            i += 3;
        } else {
            items.push(ClassItem::Byte(pattern[i]));
            i += 1;
        }
    }
    (Token::Class { negated, items }, i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// The keys a KEYS <pattern> returned on Redis 7.0.15, from `keys`
    fn check(pattern: &[u8], keys: &[&[u8]], expected: &[&[u8]]) {
        let compiled = Pattern::new(pattern, false);
        let matched: Vec<&[u8]> = keys
            .iter()
            .copied()
            .filter(|k| compiled.matches(k))
            .collect();
        assert_eq!(
            matched,
            expected,
            "pattern {:?}",
            String::from_utf8_lossy(pattern)
        );
    }

    const KEYS: [&[u8]; 20] = [
        b"", b"a", b"b", b"ab", b"]", b"[", b"\\", b"-", b"^", b"_", b"`", b"x", b"Z", b"z", b"A",
        b"*", b"\xe9", b"\xff", b"\x7f", b"a-",
    ];

    #[test]
    fn stars_question_marks_and_escapes() {
        check(b"**", &KEYS, &KEYS[1..]);
        check(
            b"?",
            &KEYS,
            &[
                b"a", b"b", b"]", b"[", b"\\", b"-", b"^", b"_", b"`", b"x", b"Z", b"z", b"A",
                b"*", b"\xe9", b"\xff", b"\x7f",
            ],
        );
        check(b"\\*", &KEYS, &[b"*"]);
        // A trailing backslash matches itself
        check(b"\\", &KEYS, &[b"\\"]);
        check(b"a\\", &KEYS, &[]);
        check(b"a*", &KEYS, &[b"a", b"ab", b"a-"]);
        check(b"*b", &KEYS, &[b"b", b"ab"]);
    }

    #[test]
    fn classes_ranges_and_negation() {
        check(b"[z-a]", &KEYS, &[b"a", b"b", b"x", b"z"]);
        check(
            b"[Z-a]",
            &KEYS,
            &[b"a", b"]", b"[", b"\\", b"^", b"_", b"`", b"Z"],
        );
        check(b"[\\]]", &KEYS, &[b"]"]);
        check(
            b"[^a]",
            &KEYS,
            &[
                b"b", b"]", b"[", b"\\", b"-", b"^", b"_", b"`", b"x", b"Z", b"z", b"A", b"*",
                b"\xe9", b"\xff", b"\x7f",
            ],
        );
        // No `!` negation, and `]` right after `[` closes an empty class
        check(b"[!a]", &KEYS, &[b"a"]);
        check(b"[]a]", &KEYS, &[]);
    }

    #[test]
    fn unclosed_classes_run_to_the_end_of_the_pattern() {
        check(b"[", &KEYS, &[]);
        check(b"[a", &KEYS, &[b"a"]);
        check(b"[\\", &KEYS, &[b"\\"]);
        check(b"[a-", &KEYS, &[b"a", b"-"]);
        // An empty negated class matches any byte
        let single_bytes: Vec<&[u8]> = KEYS.iter().copied().filter(|k| k.len() == 1).collect();
        check(b"[^", &KEYS, &single_bytes);
        // `a-]` is a range, so the class goes on and takes `x` too
        check(b"[a-]x", &KEYS, &[b"a", b"]", b"^", b"_", b"`", b"x"]);
    }

    #[test]
    fn range_bounds_compare_as_signed_chars() {
        // \xe9 is -23, so the range is -23..=97: \xe9 to \xff, then \0 to `a`
        let expected: &[&[u8]] = &[
            b"a", b"]", b"[", b"\\", b"-", b"^", b"_", b"`", b"Z", b"A", b"*", b"\xe9", b"\xff",
        ];
        check(b"[a-\xe9]", &KEYS, expected);
        check(b"[\xe9-a]", &KEYS, expected);
    }

    #[test]
    fn nocase_ignores_ascii_case_except_for_escapes_and_mixed_ranges() {
        let matches = |pattern: &[u8], s: &[u8]| Pattern::new(pattern, true).matches(s);
        assert!(matches(b"MAXMEMORY*", b"maxmemory-policy"));
        assert!(matches(b"h[A-Z]llo", b"hello"));
        assert!(matches(b"[e]", b"E"));
        // Escaped bytes in a class stay case-sensitive
        assert!(!matches(b"[\\e]", b"E"));
        // Redis lowercases the bounds after ordering them: Z-a becomes z..a
        assert!(!matches(b"[Z-a]", b"a") && !matches(b"[Z-a]", b"z"));
    }

    #[test]
    fn only_the_empty_pattern_matches_the_empty_string() {
        assert!(Pattern::new(b"", false).matches(b""));
        assert!(!Pattern::new(b"*", false).matches(b""));
        assert!(!Pattern::new(b"", false).matches(b"a"));
    }

    #[test]
    fn matching_gives_up_after_a_thousand_stars_like_redis() {
        let s = vec![b'a'; 2000];
        let pattern = |stars: usize| {
            let mut p = b"a*".repeat(stars);
            p.push(b'a');
            p
        };
        assert!(Pattern::new(&pattern(1000), false).matches(&s));
        assert!(!Pattern::new(&pattern(1001), false).matches(&s));
    }

    #[test]
    fn malicious_patterns_are_fast() {
        let long_a = vec![b'a'; 4096];
        let patterns = [
            b"a*".repeat(32 * 1024),
            [b"*".as_slice(), &b"?".repeat(64 * 1024 - 1)].concat(),
            [b"*a".repeat(32 * 1024 - 1).as_slice(), b"b"].concat(),
            b"[a-".repeat(22 * 1024),
        ];
        let started = Instant::now();
        for pattern in &patterns {
            assert!(pattern.len() >= 64 * 1024 - 1);
            let compiled = Pattern::new(pattern, false);
            compiled.matches(&long_a);
            compiled.matches(b"user:1000:session");
        }
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "{:?}",
            started.elapsed()
        );
    }
}
