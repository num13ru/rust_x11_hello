//! Owned TCP connection state and socket lifecycle transitions.

use anyhow::{Context, Result, anyhow};
use std::net::{Shutdown, TcpStream};
use std::sync::Arc;

pub(super) enum ConnectionState {
    Disconnected,
    Connected { token: Arc<()>, stream: TcpStream },
}

#[derive(Debug)]
pub(super) struct ConnectionStream {
    token: ConnectionToken,
    stream: TcpStream,
}

#[derive(Clone, Debug)]
pub(super) struct ConnectionToken(Arc<()>);

impl ConnectionStream {
    pub(super) fn stream_mut(&mut self) -> &mut TcpStream {
        &mut self.stream
    }

    pub(super) fn into_stream(self) -> TcpStream {
        self.stream
    }
}

impl ConnectionState {
    pub(super) fn disconnected() -> Self {
        Self::Disconnected
    }

    pub(super) fn connected(stream: TcpStream) -> Self {
        Self::Connected {
            token: Arc::new(()),
            stream,
        }
    }

    pub(super) fn current_token(&self) -> Result<ConnectionToken> {
        let Self::Connected { token, .. } = self else {
            return Err(anyhow!("PaperSpoon not connected"));
        };
        Ok(ConnectionToken(Arc::clone(token)))
    }

    pub(super) fn clone_stream(&self) -> Result<ConnectionStream> {
        let Self::Connected { token, stream } = self else {
            return Err(anyhow!("PaperSpoon not connected"));
        };
        Ok(ConnectionStream {
            token: ConnectionToken(Arc::clone(token)),
            stream: stream
                .try_clone()
                .context("failed to clone PaperSpoon stream")?,
        })
    }

    pub(super) fn clone_stream_for(&self, candidate: &ConnectionToken) -> Result<ConnectionStream> {
        let Self::Connected { token, stream } = self else {
            return Err(anyhow!("PaperSpoon not connected"));
        };
        if !Arc::ptr_eq(token, &candidate.0) {
            return Err(anyhow!("PaperSpoon connection changed before action write"));
        }
        Ok(ConnectionStream {
            token: candidate.clone(),
            stream: stream
                .try_clone()
                .context("failed to clone PaperSpoon stream")?,
        })
    }

    pub(super) fn reconnect(&mut self, stream: TcpStream) {
        self.disconnect();
        *self = Self::connected(stream);
    }

    pub(super) fn disconnect_if_current(&mut self, candidate: &ConnectionStream) -> bool {
        let Self::Connected { token, .. } = self else {
            return false;
        };
        if !Arc::ptr_eq(token, &candidate.token.0) {
            return false;
        }
        self.disconnect();
        true
    }

    pub(super) fn disconnect(&mut self) {
        let previous = std::mem::replace(self, Self::Disconnected);
        if let Self::Connected { stream, .. } = previous {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    fn tcp_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let client = TcpStream::connect(listener.local_addr().expect("listener address"))
            .expect("connect client");
        let (peer, _) = listener.accept().expect("accept client");
        (client, peer)
    }

    #[test]
    fn disconnected_state_rejects_stream_clone() {
        let state = ConnectionState::disconnected();
        let error = state.clone_stream().expect_err("must be disconnected");
        assert!(error.to_string().contains("PaperSpoon not connected"));
        state.current_token().expect_err("must have no token");
    }

    #[test]
    fn reconnect_closes_previous_socket_and_installs_replacement() {
        let (first, mut first_peer) = tcp_pair();
        let (replacement, mut replacement_peer) = tcp_pair();
        first_peer
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set first peer timeout");
        replacement_peer
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set replacement peer timeout");

        let mut state = ConnectionState::connected(first);
        state.reconnect(replacement);

        let mut byte = [0];
        assert_eq!(first_peer.read(&mut byte).expect("read first EOF"), 0);
        state
            .clone_stream()
            .expect("clone replacement")
            .stream_mut()
            .write_all(b"x")
            .expect("write replacement");
        replacement_peer
            .read_exact(&mut byte)
            .expect("read replacement");
        assert_eq!(byte, [b'x']);
    }

    #[test]
    fn disconnect_shuts_down_cloned_socket_and_is_idempotent() {
        let (client, mut peer) = tcp_pair();
        peer.set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set peer timeout");
        let mut state = ConnectionState::connected(client);
        let _clone = state.clone_stream().expect("clone connected stream");

        state.disconnect();
        state.disconnect();

        let mut byte = [0];
        assert_eq!(peer.read(&mut byte).expect("read EOF"), 0);
        state.clone_stream().expect_err("must remain disconnected");
    }

    #[test]
    fn stale_stream_cannot_disconnect_replacement() {
        let (first, _first_peer) = tcp_pair();
        let (replacement, mut replacement_peer) = tcp_pair();
        replacement_peer
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set replacement peer timeout");

        let mut state = ConnectionState::connected(first);
        let stale_token = state.current_token().expect("first token");
        let stale = state.clone_stream().expect("clone first stream");
        state.reconnect(replacement);

        assert!(
            state
                .clone_stream_for(&stale_token)
                .expect_err("stale token must not clone replacement")
                .to_string()
                .contains("connection changed")
        );
        assert!(!state.disconnect_if_current(&stale));
        let mut current = state.clone_stream().expect("clone replacement");
        current
            .stream_mut()
            .write_all(b"x")
            .expect("write replacement");
        let mut byte = [0];
        replacement_peer
            .read_exact(&mut byte)
            .expect("read replacement");
        assert_eq!(byte, [b'x']);

        assert!(state.disconnect_if_current(&current));
        assert_eq!(
            replacement_peer
                .read(&mut byte)
                .expect("read replacement EOF"),
            0
        );
    }
}
