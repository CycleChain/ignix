/*!
 * Client Sessions
 *
 * The state a server keeps for each client connection, which connection
 * commands such as `QUIT` change.
 */

/// State of one client connection
///
/// A server keeps one per connection and runs its commands with
/// [`Shard::exec_session`](crate::Shard::exec_session).
#[derive(Debug, Default)]
pub struct Session {
    closing: bool,
}

impl Session {
    /// The session of a new connection
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the client has asked to close the connection (`QUIT`): the
    /// replies written so far must be sent, then the connection closed
    /// without running any later request.
    pub fn is_closing(&self) -> bool {
        self.closing
    }

    pub(crate) fn close(&mut self) {
        self.closing = true;
    }
}
