/*!
 * Client Sessions
 *
 * The state a server keeps for each client connection, which connection
 * commands such as `HELLO` and `QUIT` change.
 */

use crate::protocol::Protocol;
use bytes::Bytes;
use std::sync::atomic::{AtomicU64, Ordering};

/// The id of the next connection's session; Redis numbers clients from 1
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Error for a client name that `CLIENT LIST` could not show
pub(crate) const INVALID_CLIENT_NAME: &str =
    "ERR Client names cannot contain spaces, newlines or special characters.";

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
    closing: bool,
}

impl Session {
    /// The session of a new connection, with the next client id
    pub fn new() -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            ..Self::default()
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
        if !name.iter().all(|b| (b'!'..=b'~').contains(b)) {
            return Err(INVALID_CLIENT_NAME);
        }
        self.name = (!name.is_empty()).then_some(name);
        Ok(())
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
}
