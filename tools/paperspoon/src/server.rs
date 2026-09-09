//! Shared ownership of the current PaperPad TCP connection.

use std::io::{self, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub(crate) struct CurrentConnection {
    inner: Arc<Mutex<Option<ActiveConnection>>>,
}

struct ActiveConnection {
    token: Arc<()>,
    stream: TcpStream,
}

pub(crate) struct ConnectionToken(Arc<()>);

impl CurrentConnection {
    pub(crate) fn install(&self, stream: &TcpStream) -> io::Result<ConnectionToken> {
        let token = Arc::new(());
        let active = ActiveConnection {
            token: Arc::clone(&token),
            stream: stream.try_clone()?,
        };
        *self.inner.lock().expect("current connection lock") = Some(active);
        Ok(ConnectionToken(token))
    }

    pub(crate) fn clear_if_current(&self, candidate: &ConnectionToken) -> bool {
        let mut guard = self.inner.lock().expect("current connection lock");
        let Some(active) = guard.as_ref() else {
            return false;
        };
        if !Arc::ptr_eq(&active.token, &candidate.0) {
            return false;
        }
        guard.take();
        true
    }

    /// Write one newline-terminated control line to the current connection.
    ///
    /// Returns `Ok(false)` when no PaperPad is connected. A failed write
    /// closes and clears only the generation used for that write.
    pub(crate) fn forward_line(&self, line: &str) -> io::Result<bool> {
        let (token, mut stream) = {
            let guard = self.inner.lock().expect("current connection lock");
            let Some(active) = guard.as_ref() else {
                return Ok(false);
            };
            (
                ConnectionToken(Arc::clone(&active.token)),
                active.stream.try_clone()?,
            )
        };

        if let Err(error) = writeln!(stream, "{line}") {
            let _ = stream.shutdown(Shutdown::Both);
            self.clear_if_current(&token);
            return Err(error);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::time::Duration;

    fn tcp_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let client =
            TcpStream::connect(listener.local_addr().expect("listener address")).expect("connect");
        let (server, _) = listener.accept().expect("accept");
        (client, server)
    }

    #[test]
    fn forwarding_follows_connection_replacement() {
        let current = CurrentConnection::default();
        assert!(!current.forward_line("display nobody").expect("no target"));

        let (first, first_peer) = tcp_pair();
        first_peer
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set first timeout");
        let first_token = current.install(&first).expect("install first");
        assert!(
            current
                .forward_line("display first")
                .expect("forward first")
        );
        let mut first_line = String::new();
        BufReader::new(first_peer)
            .read_line(&mut first_line)
            .expect("read first line");
        assert_eq!(first_line, "display first\n");

        let (second, second_peer) = tcp_pair();
        second_peer
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set second timeout");
        let second_token = current.install(&second).expect("install second");
        assert!(!current.clear_if_current(&first_token));
        assert!(
            current
                .forward_line("display second")
                .expect("forward second")
        );
        let mut second_line = String::new();
        BufReader::new(second_peer)
            .read_line(&mut second_line)
            .expect("read second line");
        assert_eq!(second_line, "display second\n");

        assert!(current.clear_if_current(&second_token));
        assert!(
            !current
                .forward_line("display nobody")
                .expect("cleared target")
        );
    }

    #[test]
    fn failed_write_clears_current_generation() {
        let current = CurrentConnection::default();
        let (stream, peer) = tcp_pair();
        current.install(&stream).expect("install");
        stream.shutdown(Shutdown::Both).expect("shutdown stream");
        drop(peer);

        current
            .forward_line("display failure")
            .expect_err("closed stream write must fail");
        assert!(
            !current
                .forward_line("display nobody")
                .expect("cleared target")
        );
    }
}
