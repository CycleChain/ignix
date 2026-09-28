/*!
 * Command Table
 *
 * Every command Ignix knows, with the name Redis reports for it and its
 * arity. The table is the one place that says which commands exist and how
 * many arguments they take; the parser looks commands up here.
 */

/// What a command does; the parser builds a `Cmd` from it
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Ping,
    Get,
    Set,
    Del,
    Rename,
    Exists,
    Incr,
    IncrBy,
    Decr,
    DecrBy,
    MGet,
    MSet,
}

/// The static description of a command
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CommandSpec {
    /// Lowercase name, as Redis reports it (e.g. in arity errors)
    pub(crate) name: &'static str,
    /// Number of arguments including the command name, counted like Redis:
    /// `n` means exactly `n`, `-n` means at least `n`. Some commands check
    /// more themselves (`PING` takes at most one argument, `MSET` pairs).
    pub(crate) arity: i32,
    pub(crate) kind: Kind,
}

impl CommandSpec {
    /// Whether `argc` arguments, including the command name, match the arity
    pub(crate) fn arity_matches(&self, argc: usize) -> bool {
        let arity = self.arity.unsigned_abs() as usize;
        if self.arity < 0 {
            argc >= arity
        } else {
            argc == arity
        }
    }
}

/// Longest command name that `lookup` can find
const MAX_NAME_LEN: usize = 16;

/// Pack a name of up to `MAX_NAME_LEN` bytes into an integer (first byte
/// lowest, zero-padded) with bit 5 of every byte set, so names can be
/// matched as numbers. Setting bit 5 lowercases ASCII letters and turns no
/// other byte into a letter, so a name made of letters (as every command
/// name is) matches in any case and nothing else.
const fn pack(name: &[u8]) -> u128 {
    let mut bytes = [0u8; MAX_NAME_LEN];
    let mut i = 0;
    while i < name.len() && i < MAX_NAME_LEN {
        bytes[i] = name[i] | 0x20;
        i += 1;
    }
    u128::from_le_bytes(bytes)
}

/// Define the command table: one constant per command, `COMMANDS` and
/// `lookup`, from a single list
macro_rules! commands {
    ($($spec:ident = $name:literal, $arity:expr, $kind:ident;)*) => {
        $(
            const $spec: CommandSpec = CommandSpec {
                name: $name,
                arity: $arity,
                kind: Kind::$kind,
            };
        )*

        /// Every command
        #[cfg_attr(not(test), allow(dead_code))]
        pub(crate) static COMMANDS: &[&CommandSpec] = &[$(&$spec),*];

        /// Each command's packed name
        mod packed {
            $(pub(super) const $spec: u128 = super::pack(super::$spec.name.as_bytes());)*
        }

        /// Find a command by name, ignoring ASCII case
        pub(crate) fn lookup(name: &[u8]) -> Option<&'static CommandSpec> {
            if name.len() > MAX_NAME_LEN {
                return None;
            }
            match pack(name) {
                $(packed::$spec => Some(&$spec),)*
                _ => None,
            }
        }
    };
}

commands! {
    GET = "get", 2, Get;
    SET = "set", -3, Set;
    PING = "ping", -1, Ping;
    DEL = "del", -2, Del;
    RENAME = "rename", 3, Rename;
    EXISTS = "exists", -2, Exists;
    INCR = "incr", 2, Incr;
    INCRBY = "incrby", 3, IncrBy;
    DECR = "decr", 2, Decr;
    DECRBY = "decrby", 3, DecrBy;
    MGET = "mget", -2, MGet;
    MSET = "mset", -3, MSet;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_is_found_by_its_name_in_any_case() {
        for &spec in COMMANDS {
            assert_eq!(lookup(spec.name.as_bytes()), Some(spec));
            let upper = spec.name.to_ascii_uppercase();
            assert_eq!(lookup(upper.as_bytes()), Some(spec));
            assert!(spec.name.len() <= MAX_NAME_LEN);
            // `pack` relies on names made of lowercase letters
            assert!(spec.name.bytes().all(|b| b.is_ascii_lowercase()));
        }
    }

    #[test]
    fn unknown_and_overlong_names_are_not_found() {
        assert_eq!(lookup(b"nosuchcommand"), None);
        assert_eq!(lookup(b""), None);
        assert_eq!(lookup(b"getgetgetgetgetgetget"), None);
        assert_eq!(lookup(b"ge"), None);
        assert_eq!(lookup(b"gett"), None);
        // Zero bytes must not match the padding
        assert_eq!(lookup(b"get\0"), None);
        assert_eq!(lookup(b"\0get"), None);
        // Setting bit 5 must not turn other bytes into letters
        assert_eq!(lookup(b"get "), None);
        assert_eq!(lookup(b"ge\x14"), None);
        assert_eq!(lookup(b"G\xc5T"), None);
        assert_eq!(lookup("gét".as_bytes()), None);
    }

    #[test]
    fn arity_is_exact_or_a_minimum() {
        assert!(GET.arity_matches(2));
        assert!(!GET.arity_matches(1) && !GET.arity_matches(3));
        assert!(SET.arity_matches(3) && SET.arity_matches(5));
        assert!(!SET.arity_matches(2));
        assert!(PING.arity_matches(1) && PING.arity_matches(2));
    }
}
