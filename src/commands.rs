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
    Echo,
    Quit,
    Select,
    Hello,
    Client,
    DbSize,
    Type,
    Unlink,
    FlushDb,
    FlushAll,
    Keys,
    Scan,
    Info,
    Config,
    Expire,
    PExpire,
    ExpireAt,
    PExpireAt,
    Ttl,
    PTtl,
    ExpireTime,
    PExpireTime,
    Persist,
    SetEx,
    PSetEx,
    SetNx,
    GetSet,
    GetDel,
    GetEx,
    MSetNx,
    Auth,
    Command,
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
    /// Flags, as COMMAND INFO reports them (Redis 7.0's)
    pub(crate) flags: &'static [&'static str],
    /// Position of the first key argument, of the last one (negative:
    /// counted from the end) and the step between them; 0 for none
    pub(crate) keys: (i32, i32, i32),
    /// ACL categories, as COMMAND INFO reports them (Redis 7.0's)
    pub(crate) acl: &'static [&'static str],
    /// Hints for clients, as COMMAND INFO reports them (Redis 7.0's)
    pub(crate) tips: &'static [&'static str],
}

impl CommandSpec {
    /// Whether `argc` arguments, including the command name, match the arity
    pub(crate) fn arity_matches(&self, argc: usize) -> bool {
        arity_matches(self.arity, argc)
    }
}

/// Whether `argc` arguments match `arity`, counted like Redis
fn arity_matches(arity: i32, argc: usize) -> bool {
    let min = arity.unsigned_abs() as usize;
    if arity < 0 {
        argc >= min
    } else {
        argc == min
    }
}

/// The static description of a subcommand of CLIENT, CONFIG or COMMAND
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SubcommandSpec {
    /// Name as Redis reports it: `command|subcommand`, lowercase
    pub(crate) name: &'static str,
    /// Number of arguments including the command and subcommand names,
    /// counted like Redis
    pub(crate) arity: i32,
    pub(crate) flags: &'static [&'static str],
    pub(crate) acl: &'static [&'static str],
    pub(crate) tips: &'static [&'static str],
}

impl SubcommandSpec {
    /// The name after the `|`
    pub(crate) fn short_name(&self) -> &'static str {
        self.name
            .split_once('|')
            .map_or(self.name, |(_, name)| name)
    }

    /// Whether `argc` arguments, including the command and subcommand
    /// names, match the arity
    pub(crate) fn arity_matches(&self, argc: usize) -> bool {
        arity_matches(self.arity, argc)
    }
}

const fn subcommand(
    name: &'static str,
    arity: i32,
    flags: &'static [&'static str],
    acl: &'static [&'static str],
    tips: &'static [&'static str],
) -> SubcommandSpec {
    SubcommandSpec {
        name,
        arity,
        flags,
        acl,
        tips,
    }
}

/// Flags and ACL categories of most subcommands of these commands
const CONNECTION: &[&str] = &["noscript", "loading", "stale"];
const LOADING_STALE: &[&str] = &["loading", "stale"];
const SLOW_CONNECTION: &[&str] = &["@slow", "@connection"];

// Redis 7.0's descriptions; CLIENT SETINFO is Redis 7.2's
static CLIENT_SUBCOMMANDS: &[SubcommandSpec] = &[
    subcommand("client|id", 2, CONNECTION, SLOW_CONNECTION, &[]),
    subcommand("client|getname", 2, CONNECTION, SLOW_CONNECTION, &[]),
    subcommand("client|setname", 3, CONNECTION, SLOW_CONNECTION, &[]),
    subcommand("client|setinfo", 4, CONNECTION, SLOW_CONNECTION, &[]),
    subcommand("client|help", 2, LOADING_STALE, SLOW_CONNECTION, &[]),
];

static CONFIG_SUBCOMMANDS: &[SubcommandSpec] = &[
    subcommand(
        "config|get",
        -3,
        &["admin", "noscript", "loading", "stale"],
        &["@admin", "@slow", "@dangerous"],
        &[],
    ),
    subcommand("config|help", 2, LOADING_STALE, &["@slow"], &[]),
];

static COMMAND_SUBCOMMANDS: &[SubcommandSpec] = &[
    subcommand("command|count", 2, LOADING_STALE, SLOW_CONNECTION, &[]),
    subcommand(
        "command|info",
        -2,
        LOADING_STALE,
        SLOW_CONNECTION,
        &["nondeterministic_output_order"],
    ),
    subcommand(
        "command|list",
        -2,
        LOADING_STALE,
        SLOW_CONNECTION,
        &["nondeterministic_output_order"],
    ),
    subcommand("command|getkeys", -4, LOADING_STALE, SLOW_CONNECTION, &[]),
    subcommand("command|help", 2, LOADING_STALE, SLOW_CONNECTION, &[]),
];

/// The subcommands of a command; only CLIENT, CONFIG and COMMAND have some
pub(crate) fn subcommands(kind: Kind) -> &'static [SubcommandSpec] {
    match kind {
        Kind::Client => CLIENT_SUBCOMMANDS,
        Kind::Config => CONFIG_SUBCOMMANDS,
        Kind::Command => COMMAND_SUBCOMMANDS,
        _ => &[],
    }
}

/// Find a subcommand of the command `kind` by name, ignoring ASCII case
pub(crate) fn find_subcommand(kind: Kind, name: &[u8]) -> Option<&'static SubcommandSpec> {
    subcommands(kind)
        .iter()
        .find(|spec| spec.short_name().as_bytes().eq_ignore_ascii_case(name))
}

/// Longest command name that `lookup` can find
const MAX_NAME_LEN: usize = 16;

/// Pack a name of up to `MAX_NAME_LEN` bytes into an integer (first byte
/// lowest, zero-padded) with bit 5 of every byte set, so names can be
/// matched as numbers. Setting bit 5 lowercases ASCII letters and turns no
/// other byte into a letter, so a name made of letters (as every command
/// name is) matches in any case and nothing else.
///
/// Runs for every request, so it reads the name with a few loads that may
/// overlap rather than byte by byte.
#[inline(always)]
const fn pack(name: &[u8]) -> u128 {
    /// Bit 5 of eight bytes
    const CASE: u64 = 0x2020_2020_2020_2020;
    let len = name.len();
    // At most eight bytes, from two loads that may overlap
    let low = match len {
        0 => return 0,
        1 => name[0] as u64,
        2..=3 => le16(name, 0) | le16(name, len - 2) << ((len - 2) * 8),
        4..=8 => le32(name, 0) | le32(name, len - 4) << ((len - 4) * 8),
        _ => {
            let high = (le64(name, len - 8) | CASE) >> ((MAX_NAME_LEN - len) * 8);
            return (high as u128) << 64 | (le64(name, 0) | CASE) as u128;
        }
    };
    (low | CASE >> ((8 - len) * 8)) as u128
}

/// Bytes `at..at + 2` of `name`, first byte lowest
const fn le16(name: &[u8], at: usize) -> u64 {
    u16::from_le_bytes([name[at], name[at + 1]]) as u64
}

/// Bytes `at..at + 4` of `name`, first byte lowest
const fn le32(name: &[u8], at: usize) -> u64 {
    u32::from_le_bytes([name[at], name[at + 1], name[at + 2], name[at + 3]]) as u64
}

/// Bytes `at..at + 8` of `name`, first byte lowest
const fn le64(name: &[u8], at: usize) -> u64 {
    le32(name, at) | le32(name, at + 4) << 32
}

/// Slots of the table `lookup` finds commands in: a power of two with room
/// for a perfect hash of every name
const SLOTS: usize = 256;

/// No command in this slot
const EMPTY: u8 = u8::MAX;

/// The slot of a packed name for the multiplier `k` (a multiplicative hash
/// of the name's two halves, keeping the top bits)
const fn slot(packed: u128, k: u64) -> usize {
    let folded = (packed as u64) ^ ((packed >> 64) as u64).rotate_left(31);
    (folded.wrapping_mul(k) >> (64 - SLOTS.trailing_zeros())) as usize
}

/// The first multiplier that gives each of `names` a slot of its own,
/// searched for at compile time
const fn perfect_multiplier(names: &[u128]) -> u64 {
    let mut k: u64 = 0x9e37_79b9_7f4a_7c15;
    'search: loop {
        let mut used = [false; SLOTS];
        let mut i = 0;
        while i < names.len() {
            let s = slot(names[i], k);
            if used[s] {
                k = k.wrapping_add(0x6a09_e667_f3bc_c908);
                continue 'search;
            }
            used[s] = true;
            i += 1;
        }
        return k;
    }
}

/// The index in `names` of the name in each slot, `EMPTY` for none
const fn slot_table(names: &[u128], k: u64) -> [u8; SLOTS] {
    let mut table = [EMPTY; SLOTS];
    let mut i = 0;
    while i < names.len() {
        table[slot(names[i], k)] = i as u8;
        i += 1;
    }
    table
}

/// Define the command table: one constant per command, `COMMANDS` and
/// `lookup`, from a single list
macro_rules! commands {
    ($(
        $spec:ident = $name:literal, $arity:expr, $kind:ident,
            [$($flag:literal),*], $keys:expr, [$($acl:literal),*], [$($tip:literal),*];
    )*) => {
        $(
            const $spec: CommandSpec = CommandSpec {
                name: $name,
                arity: $arity,
                kind: Kind::$kind,
                flags: &[$($flag),*],
                keys: $keys,
                acl: &[$($acl),*],
                tips: &[$($tip),*],
            };
        )*

        /// Every command
        pub(crate) static COMMANDS: &[&CommandSpec] = &[$(&$spec),*];

        /// Each command's packed name, in the order of `COMMANDS`
        const PACKED: &[u128] = &[$(pack($spec.name.as_bytes())),*];

        // Indexes are stored as `u8`, with `EMPTY` for no command
        const _: () = assert!(PACKED.len() < EMPTY as usize);

        /// The multiplier of `slot` that gives every command its own slot
        const MULTIPLIER: u64 = perfect_multiplier(PACKED);

        /// The index in `COMMANDS` of the command in each slot
        static SLOT_TABLE: [u8; SLOTS] = slot_table(PACKED, MULTIPLIER);

        /// Find a command by name, ignoring ASCII case: the only command
        /// the name can be is the one in its slot
        ///
        /// Always inlined: the parser calls it for every request.
        #[inline(always)]
        pub(crate) fn lookup(name: &[u8]) -> Option<&'static CommandSpec> {
            if name.len() > MAX_NAME_LEN {
                return None;
            }
            let packed = pack(name);
            let index = usize::from(SLOT_TABLE[slot(packed, MULTIPLIER)]);
            match PACKED.get(index) {
                Some(&candidate) if candidate == packed => Some(COMMANDS[index]),
                _ => None,
            }
        }
    };
}

commands! {
    GET = "get", 2, Get,
        ["readonly", "fast"], (1, 1, 1), ["@read", "@string", "@fast"], [];
    SET = "set", -3, Set,
        ["write", "denyoom"], (1, 1, 1), ["@write", "@string", "@slow"], [];
    PING = "ping", -1, Ping,
        ["fast"], (0, 0, 0), ["@fast", "@connection"],
        ["request_policy:all_shards", "response_policy:all_succeeded"];
    DEL = "del", -2, Del,
        ["write"], (1, -1, 1), ["@keyspace", "@write", "@slow"],
        ["request_policy:multi_shard", "response_policy:agg_sum"];
    RENAME = "rename", 3, Rename,
        ["write"], (1, 2, 1), ["@keyspace", "@write", "@slow"], [];
    EXISTS = "exists", -2, Exists,
        ["readonly", "fast"], (1, -1, 1), ["@keyspace", "@read", "@fast"],
        ["request_policy:multi_shard", "response_policy:agg_sum"];
    INCR = "incr", 2, Incr,
        ["write", "denyoom", "fast"], (1, 1, 1), ["@write", "@string", "@fast"], [];
    INCRBY = "incrby", 3, IncrBy,
        ["write", "denyoom", "fast"], (1, 1, 1), ["@write", "@string", "@fast"], [];
    DECR = "decr", 2, Decr,
        ["write", "denyoom", "fast"], (1, 1, 1), ["@write", "@string", "@fast"], [];
    DECRBY = "decrby", 3, DecrBy,
        ["write", "denyoom", "fast"], (1, 1, 1), ["@write", "@string", "@fast"], [];
    MGET = "mget", -2, MGet,
        ["readonly", "fast"], (1, -1, 1), ["@read", "@string", "@fast"],
        ["request_policy:multi_shard"];
    MSET = "mset", -3, MSet,
        ["write", "denyoom"], (1, -1, 2), ["@write", "@string", "@slow"],
        ["request_policy:multi_shard", "response_policy:all_succeeded"];
    ECHO = "echo", 2, Echo,
        ["loading", "stale", "fast"], (0, 0, 0), ["@fast", "@connection"], [];
    QUIT = "quit", -1, Quit,
        ["noscript", "loading", "stale", "fast", "no_auth", "allow_busy"],
        (0, 0, 0), ["@fast", "@connection"], [];
    SELECT = "select", 2, Select,
        ["loading", "stale", "fast"], (0, 0, 0), ["@fast", "@connection"], [];
    HELLO = "hello", -1, Hello,
        ["noscript", "loading", "stale", "fast", "no_auth", "allow_busy"],
        (0, 0, 0), ["@fast", "@connection"], [];
    CLIENT = "client", -2, Client,
        [], (0, 0, 0), ["@slow"], [];
    DBSIZE = "dbsize", 1, DbSize,
        ["readonly", "fast"], (0, 0, 0), ["@keyspace", "@read", "@fast"],
        ["request_policy:all_shards", "response_policy:agg_sum"];
    TYPE = "type", 2, Type,
        ["readonly", "fast"], (1, 1, 1), ["@keyspace", "@read", "@fast"], [];
    UNLINK = "unlink", -2, Unlink,
        ["write", "fast"], (1, -1, 1), ["@keyspace", "@write", "@fast"],
        ["request_policy:multi_shard", "response_policy:agg_sum"];
    FLUSHDB = "flushdb", -1, FlushDb,
        ["write"], (0, 0, 0), ["@keyspace", "@write", "@slow", "@dangerous"],
        ["request_policy:all_shards", "response_policy:all_succeeded"];
    FLUSHALL = "flushall", -1, FlushAll,
        ["write"], (0, 0, 0), ["@keyspace", "@write", "@slow", "@dangerous"],
        ["request_policy:all_shards", "response_policy:all_succeeded"];
    KEYS = "keys", 2, Keys,
        ["readonly"], (0, 0, 0), ["@keyspace", "@read", "@slow", "@dangerous"],
        ["request_policy:all_shards", "nondeterministic_output_order"];
    SCAN = "scan", -2, Scan,
        ["readonly"], (0, 0, 0), ["@keyspace", "@read", "@slow"],
        ["nondeterministic_output", "request_policy:special"];
    INFO = "info", -1, Info,
        ["loading", "stale"], (0, 0, 0), ["@slow", "@dangerous"],
        ["nondeterministic_output", "request_policy:all_shards", "response_policy:special"];
    CONFIG = "config", -2, Config,
        [], (0, 0, 0), ["@slow"], [];
    EXPIRE = "expire", -3, Expire,
        ["write", "fast"], (1, 1, 1), ["@keyspace", "@write", "@fast"], [];
    PEXPIRE = "pexpire", -3, PExpire,
        ["write", "fast"], (1, 1, 1), ["@keyspace", "@write", "@fast"], [];
    EXPIREAT = "expireat", -3, ExpireAt,
        ["write", "fast"], (1, 1, 1), ["@keyspace", "@write", "@fast"], [];
    PEXPIREAT = "pexpireat", -3, PExpireAt,
        ["write", "fast"], (1, 1, 1), ["@keyspace", "@write", "@fast"], [];
    TTL = "ttl", 2, Ttl,
        ["readonly", "fast"], (1, 1, 1), ["@keyspace", "@read", "@fast"],
        ["nondeterministic_output"];
    PTTL = "pttl", 2, PTtl,
        ["readonly", "fast"], (1, 1, 1), ["@keyspace", "@read", "@fast"],
        ["nondeterministic_output"];
    EXPIRETIME = "expiretime", 2, ExpireTime,
        ["readonly", "fast"], (1, 1, 1), ["@keyspace", "@read", "@fast"], [];
    PEXPIRETIME = "pexpiretime", 2, PExpireTime,
        ["readonly", "fast"], (1, 1, 1), ["@keyspace", "@read", "@fast"], [];
    PERSIST = "persist", 2, Persist,
        ["write", "fast"], (1, 1, 1), ["@keyspace", "@write", "@fast"], [];
    SETEX = "setex", 4, SetEx,
        ["write", "denyoom"], (1, 1, 1), ["@write", "@string", "@slow"], [];
    PSETEX = "psetex", 4, PSetEx,
        ["write", "denyoom"], (1, 1, 1), ["@write", "@string", "@slow"], [];
    SETNX = "setnx", 3, SetNx,
        ["write", "denyoom", "fast"], (1, 1, 1), ["@write", "@string", "@fast"], [];
    GETSET = "getset", 3, GetSet,
        ["write", "denyoom", "fast"], (1, 1, 1), ["@write", "@string", "@fast"], [];
    GETDEL = "getdel", 2, GetDel,
        ["write", "fast"], (1, 1, 1), ["@write", "@string", "@fast"], [];
    GETEX = "getex", -2, GetEx,
        ["write", "fast"], (1, 1, 1), ["@write", "@string", "@fast"], [];
    MSETNX = "msetnx", -3, MSetNx,
        ["write", "denyoom"], (1, -1, 2), ["@write", "@string", "@slow"],
        ["request_policy:multi_shard", "response_policy:agg_min"];
    AUTH = "auth", -2, Auth,
        ["noscript", "loading", "stale", "fast", "no_auth", "allow_busy"],
        (0, 0, 0), ["@fast", "@connection"], [];
    COMMAND = "command", -1, Command,
        ["loading", "stale"], (0, 0, 0), ["@slow", "@connection"],
        ["nondeterministic_output_order"];
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
    fn pack_reads_every_byte_of_every_length() {
        // Byte by byte, as its documentation describes it
        fn by_byte(name: &[u8]) -> u128 {
            let mut bytes = [0u8; MAX_NAME_LEN];
            for (packed, &byte) in bytes.iter_mut().zip(name) {
                *packed = byte | 0x20;
            }
            u128::from_le_bytes(bytes)
        }
        // Varied bytes, zeros and high bytes included
        let bytes: Vec<u8> = (0u32..600)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .chain(0..=255)
            .collect();
        assert_eq!(pack(b""), 0);
        for len in 1..=MAX_NAME_LEN {
            for name in bytes.windows(len) {
                assert_eq!(pack(name), by_byte(name), "{name:?}");
            }
        }
    }

    #[test]
    fn no_auth_commands_are_auth_hello_and_quit() {
        // `Cmd::allowed_before_auth` and the parser's errors for commands
        // allowed before authenticating assume these three
        let mut no_auth: Vec<&str> = COMMANDS
            .iter()
            .filter(|spec| spec.flags.contains(&"no_auth"))
            .map(|spec| spec.name)
            .collect();
        no_auth.sort_unstable();
        assert_eq!(no_auth, ["auth", "hello", "quit"]);
    }

    #[test]
    fn subcommands_are_found_in_any_case() {
        let setname = find_subcommand(Kind::Client, b"SetName").unwrap();
        assert_eq!(
            (setname.name, setname.short_name()),
            ("client|setname", "setname")
        );
        assert!(setname.arity_matches(3) && !setname.arity_matches(2));
        assert_eq!(find_subcommand(Kind::Client, b"kill"), None);
        assert_eq!(find_subcommand(Kind::Get, b"id"), None);
        for kind in [Kind::Client, Kind::Config, Kind::Command] {
            for spec in subcommands(kind) {
                assert_eq!(
                    find_subcommand(kind, spec.short_name().as_bytes()),
                    Some(spec)
                );
            }
        }
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
