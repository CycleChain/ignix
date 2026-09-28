/*!
 * Client Sessions
 *
 * The state a server keeps for each client connection, which connection
 * commands such as `HELLO` and `QUIT` change.
 */

use crate::protocol::{is_printable_ascii, ClientInfo, Protocol};
use crate::stats::{LocalCounter, Stats};
use bytes::Bytes;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// The id of the next connection's session; Redis numbers clients from 1
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Error for a client name that `CLIENT LIST` could not show
pub(crate) const INVALID_CLIENT_NAME: &str =
    "ERR Client names cannot contain spaces, newlines or special characters.";

/// Reply to a command a client runs before authenticating
pub(crate) const NOAUTH: &str = "NOAUTH Authentication required.";

/// Reply to HELLO from a client that has not authenticated, and did not
/// with HELLO's AUTH option
pub(crate) const HELLO_NOAUTH: &str = "NOAUTH HELLO must be called with the client already \
    authenticated, otherwise the HELLO AUTH <user> <pass> option can be used to authenticate \
    the client and select the RESP protocol version at the same time";

/// Reply to a wrong user or password
const WRONGPASS: &str = "WRONGPASS invalid username-password pair or user is disabled.";

/// Reply to AUTH with a password alone when the server has no password
const AUTH_WITHOUT_PASSWORD: &str = "ERR AUTH <password> called without any password \
    configured for the default user. Are you sure your configuration is correct?";

/// The password clients must give before running commands; its `Debug`
/// output does not show it
#[derive(Clone)]
pub(crate) struct Password(Arc<[u8]>);

impl Password {
    pub(crate) fn new(password: &[u8]) -> Self {
        Self(password.into())
    }

    /// Whether `given` is the password, compared in a time that does not
    /// depend on where the two differ
    fn matches(&self, given: &[u8]) -> bool {
        let expected = &self.0[..];
        let mut differ = given.len() ^ expected.len();
        for (i, &byte) in expected.iter().enumerate() {
            differ |= usize::from(byte ^ given.get(i).copied().unwrap_or(0));
        }
        differ == 0
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(..)")
    }
}

/// State of one client connection
///
/// A server keeps one per connection and runs its commands with
/// [`Shard::exec_session`](crate::Shard::exec_session).
/// `Session::default()` has id 0 and is not counted; [`Shard::exec`](crate::Shard::exec)
/// uses one for each command.
#[derive(Debug, Default)]
pub struct Session {
    id: u64,
    protocol: Protocol,
    name: Option<Bytes>,
    lib_name: Option<Bytes>,
    lib_ver: Option<Bytes>,
    closing: bool,
    /// The password to give with AUTH or HELLO AUTH, if one is needed
    password: Option<Password>,
    /// Set until the client gives the password: meanwhile it may only run
    /// AUTH, HELLO and QUIT
    needs_auth: bool,
    /// Set for a connection of the server, which counts it in its statistics
    client: Option<Client>,
}

/// The place of a server connection in the server's statistics: counted
/// as connected until it is dropped
#[derive(Debug)]
struct Client {
    stats: Arc<Stats>,
    /// The commands counter of the worker thread serving the connection
    commands: Arc<LocalCounter>,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stats.client_disconnected();
    }
}

impl Session {
    /// The session of a new connection, with the next client id
    pub fn new() -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            ..Self::default()
        }
    }

    /// The session of a new connection that must authenticate with
    /// `password` (AUTH or HELLO's AUTH option) before running other
    /// commands, like a Redis server with `requirepass`
    pub fn with_password(password: &[u8]) -> Self {
        Self::new().require(Some(&Password::new(password)))
    }

    /// The session of a new server connection, counted in `stats` while it
    /// lives; its commands are counted in `commands`, the counter of the
    /// worker thread that serves it
    pub(crate) fn connected(
        stats: &Arc<Stats>,
        commands: &Arc<LocalCounter>,
        password: Option<&Password>,
    ) -> Self {
        stats.client_connected();
        Self {
            client: Some(Client {
                stats: stats.clone(),
                commands: commands.clone(),
            }),
            ..Self::new()
        }
        .require(password)
    }

    fn require(self, password: Option<&Password>) -> Self {
        Self {
            needs_auth: password.is_some(),
            password: password.cloned(),
            ..self
        }
    }

    /// Count an executed command in the server's statistics
    pub(crate) fn count_command(&self) {
        if let Some(client) = &self.client {
            client.commands.increment();
        }
    }

    /// The client id, unique among the sessions made with [`Session::new`]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The RESP version replies are written in
    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// The name the client gave itself, if any
    pub fn name(&self) -> Option<&Bytes> {
        self.name.as_ref()
    }

    /// The client library's name, from `CLIENT SETINFO LIB-NAME`
    pub fn lib_name(&self) -> Option<&Bytes> {
        self.lib_name.as_ref()
    }

    /// The client library's version, from `CLIENT SETINFO LIB-VER`
    pub fn lib_ver(&self) -> Option<&Bytes> {
        self.lib_ver.as_ref()
    }

    /// Whether the client may run any command: it has given the password, or
    /// none is needed
    pub fn is_authenticated(&self) -> bool {
        !self.needs_auth
    }

    /// Authenticate as `username` (`default` when `None`) with `password`,
    /// like Redis AUTH: `default` is the only user, and without a password
    /// it accepts any, but AUTH with a password alone is then an error. A
    /// failure leaves the session as it was.
    pub(crate) fn authenticate(
        &mut self,
        username: Option<&[u8]>,
        password: &[u8],
    ) -> Result<(), &'static str> {
        let accepted = match (&self.password, username) {
            (_, Some(user)) if user != b"default" => false,
            (None, None) => return Err(AUTH_WITHOUT_PASSWORD),
            (None, Some(_)) => true,
            (Some(expected), _) => expected.matches(password),
        };
        if !accepted {
            return Err(WRONGPASS);
        }
        self.needs_auth = false;
        Ok(())
    }

    /// Whether the client has asked to close the connection (`QUIT`): the
    /// replies written so far must be sent, then the connection closed
    /// without running any later request.
    pub fn is_closing(&self) -> bool {
        self.closing
    }

    pub(crate) fn set_protocol(&mut self, protocol: Protocol) {
        self.protocol = protocol;
    }

    /// Set the client name; an empty name removes it. Like Redis, a name
    /// may only contain printable ASCII characters other than space.
    pub(crate) fn set_name(&mut self, name: Bytes) -> Result<(), &'static str> {
        if !is_printable_ascii(&name) {
            return Err(INVALID_CLIENT_NAME);
        }
        self.name = (!name.is_empty()).then_some(name);
        Ok(())
    }

    /// Record a client attribute; the value must already be checked with
    /// `is_printable_ascii`. An empty value removes it.
    pub(crate) fn set_info(&mut self, info: ClientInfo, value: Bytes) {
        let value = (!value.is_empty()).then_some(value);
        match info {
            ClientInfo::LibName => self.lib_name = value,
            ClientInfo::LibVer => self.lib_ver = value,
        }
    }

    pub(crate) fn close(&mut self) {
        self.closing = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sessions_get_increasing_ids() {
        let (a, b) = (Session::new(), Session::new());
        assert!(a.id() >= 1 && b.id() > a.id());
        assert_eq!(Session::default().id(), 0);
    }

    #[test]
    fn client_names_are_printable_ascii_without_spaces() {
        let mut session = Session::new();
        assert_eq!(session.set_name(Bytes::from_static(b"worker-1")), Ok(()));
        assert_eq!(session.name().map(|n| &n[..]), Some(&b"worker-1"[..]));
        for bad in [&b"a b"[..], b"a\nb", b"caf\xc3\xa9", b"\x7f"] {
            let bad = Bytes::copy_from_slice(bad);
            assert_eq!(session.set_name(bad), Err(INVALID_CLIENT_NAME));
        }
        // A rejected name leaves the old one
        assert_eq!(session.name().map(|n| &n[..]), Some(&b"worker-1"[..]));
        assert_eq!(session.set_name(Bytes::new()), Ok(()));
        assert_eq!(session.name(), None);
    }

    #[test]
    fn passwords_match_only_themselves() {
        let password = Password::new(b"secret");
        assert!(password.matches(b"secret"));
        for wrong in [
            &b""[..],
            b"secre",
            b"secret!",
            b"Secret",
            b"secreu",
            b"xsecret",
        ] {
            assert!(!password.matches(wrong), "{wrong:?}");
        }
        assert!(Password::new(b"").matches(b""));
        assert!(!format!("{:?}", Session::with_password(b"secret")).contains("secret"));
    }

    #[test]
    fn authentication_follows_redis() {
        let mut session = Session::with_password(b"secret");
        assert!(!session.is_authenticated());
        assert_eq!(session.authenticate(None, b"wrong"), Err(WRONGPASS));
        assert_eq!(
            session.authenticate(Some(b"other"), b"secret"),
            Err(WRONGPASS)
        );
        assert!(!session.is_authenticated());
        assert_eq!(session.authenticate(None, b"secret"), Ok(()));
        assert!(session.is_authenticated());
        // A failure afterwards does not undo it
        assert_eq!(
            session.authenticate(Some(b"default"), b"wrong"),
            Err(WRONGPASS)
        );
        assert!(session.is_authenticated());
        assert_eq!(session.authenticate(Some(b"default"), b"secret"), Ok(()));

        // Without a password, `default` accepts any, but not AUTH <password>
        let mut session = Session::new();
        assert!(session.is_authenticated());
        assert_eq!(session.authenticate(None, b"x"), Err(AUTH_WITHOUT_PASSWORD));
        assert_eq!(session.authenticate(Some(b"default"), b"x"), Ok(()));
        assert_eq!(session.authenticate(Some(b"other"), b"x"), Err(WRONGPASS));
    }
}
