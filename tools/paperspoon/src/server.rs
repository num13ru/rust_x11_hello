//! Shared ownership of the current PaperPad TCP connection.

use std::io::{self, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use paper_protocol::v3;
use paper_protocol::{V2PixelFormat, V2PixelFormats};

#[derive(Clone, Default)]
pub(crate) struct CurrentConnection {
    inner: Arc<Mutex<Option<ActiveConnection>>>,
}

struct ActiveConnection {
    token: Arc<()>,
    stream: TcpStream,
    pixel_formats: V2PixelFormats,
    viewport: Option<(u16, u16)>,
    encodings: Option<v3::Encodings>,
}

#[derive(Clone)]
pub(crate) struct ConnectionToken(Arc<()>);

impl ConnectionToken {
    pub(crate) fn matches(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone)]
pub(crate) struct FrameTarget {
    token: ConnectionToken,
    viewport: (u16, u16),
}

impl FrameTarget {
    pub(crate) fn viewport(&self) -> (u16, u16) {
        self.viewport
    }

    pub(crate) fn is_same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.token.0, &other.token.0) && self.viewport == other.viewport
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrameTargetError {
    NoConnection,
    UnsupportedPixelFormat,
    ViewportUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrameForward {
    Sent,
    NoConnection,
    UnsupportedPixelFormat,
    ConnectionChanged,
    ViewportChanged,
}

impl CurrentConnection {
    pub(crate) fn is_active(&self) -> bool {
        self.inner
            .lock()
            .expect("current connection lock")
            .is_some()
    }

    #[cfg(test)]
    pub(crate) fn install(&self, stream: &TcpStream) -> io::Result<ConnectionToken> {
        self.install_inner(stream, V2PixelFormats::MONO1, None, None)
    }

    #[cfg(test)]
    pub(crate) fn install_with_pixel_formats(
        &self,
        stream: &TcpStream,
        pixel_formats: V2PixelFormats,
    ) -> io::Result<ConnectionToken> {
        self.install_inner(stream, pixel_formats, None, None)
    }

    #[cfg(test)]
    pub(crate) fn install_for_hello(
        &self,
        stream: &TcpStream,
        pixel_formats: V2PixelFormats,
        viewport: (u16, u16),
    ) -> io::Result<ConnectionToken> {
        self.install_negotiated(stream, pixel_formats, viewport, None)
    }

    pub(crate) fn install_negotiated(
        &self,
        stream: &TcpStream,
        pixel_formats: V2PixelFormats,
        viewport: (u16, u16),
        encodings: Option<v3::Encodings>,
    ) -> io::Result<ConnectionToken> {
        self.install_inner(stream, pixel_formats, Some(viewport), encodings)
    }

    fn install_inner(
        &self,
        stream: &TcpStream,
        pixel_formats: V2PixelFormats,
        viewport: Option<(u16, u16)>,
        encodings: Option<v3::Encodings>,
    ) -> io::Result<ConnectionToken> {
        let token = Arc::new(());
        let active = ActiveConnection {
            token: Arc::clone(&token),
            stream: stream.try_clone()?,
            pixel_formats,
            viewport,
            encodings,
        };
        *self.inner.lock().expect("current connection lock") = Some(active);
        Ok(ConnectionToken(token))
    }

    pub(crate) fn transport(&self) -> Option<(ConnectionToken, Option<v3::Encodings>)> {
        let guard = self.inner.lock().expect("current connection lock");
        guard
            .as_ref()
            .map(|active| (ConnectionToken(Arc::clone(&active.token)), active.encodings))
    }

    pub(crate) fn frame_target(
        &self,
        pixel_format: V2PixelFormat,
    ) -> Result<FrameTarget, FrameTargetError> {
        let guard = self.inner.lock().expect("current connection lock");
        let Some(active) = guard.as_ref() else {
            return Err(FrameTargetError::NoConnection);
        };
        if !active.pixel_formats.supports(pixel_format) {
            return Err(FrameTargetError::UnsupportedPixelFormat);
        }
        let viewport = active
            .viewport
            .ok_or(FrameTargetError::ViewportUnavailable)?;
        Ok(FrameTarget {
            token: ConnectionToken(Arc::clone(&active.token)),
            viewport,
        })
    }

    /// Write a framebuffer only when the active peer advertised its format.
    #[cfg(test)]
    pub(crate) fn forward_frame(
        &self,
        bytes: &[u8],
        pixel_format: V2PixelFormat,
    ) -> io::Result<FrameForward> {
        self.forward_frame_inner(bytes, pixel_format, None, None)
    }

    #[cfg(test)]
    pub(crate) fn forward_frame_to(
        &self,
        bytes: &[u8],
        pixel_format: V2PixelFormat,
        target: &FrameTarget,
    ) -> io::Result<FrameForward> {
        self.forward_frame_inner(bytes, pixel_format, Some(target), None)
    }

    pub(crate) fn forward_negotiated(
        &self,
        bytes: &[u8],
        pixel_format: V2PixelFormat,
        target: Option<&FrameTarget>,
        connection: &ConnectionToken,
    ) -> io::Result<FrameForward> {
        self.forward_frame_inner(bytes, pixel_format, target, Some(connection))
    }

    fn forward_frame_inner(
        &self,
        bytes: &[u8],
        pixel_format: V2PixelFormat,
        target: Option<&FrameTarget>,
        connection: Option<&ConnectionToken>,
    ) -> io::Result<FrameForward> {
        let mut guard = self.inner.lock().expect("current connection lock");
        let Some(active) = guard.as_mut() else {
            return Ok(FrameForward::NoConnection);
        };
        if connection.is_some_and(|candidate| !Arc::ptr_eq(&active.token, &candidate.0)) {
            return Ok(FrameForward::ConnectionChanged);
        }
        if let Some(target) = target {
            if !Arc::ptr_eq(&active.token, &target.token.0) {
                return Ok(FrameForward::ConnectionChanged);
            }
            if active.viewport != Some(target.viewport) {
                return Ok(FrameForward::ViewportChanged);
            }
        }
        if !active.pixel_formats.supports(pixel_format) {
            return Ok(FrameForward::UnsupportedPixelFormat);
        }
        let write_result = if target.is_some() {
            write_targeted_frame(&mut active.stream, bytes)
        } else {
            active.stream.write_all(bytes)
        };
        if let Err(error) = write_result {
            let _ = active.stream.shutdown(Shutdown::Both);
            guard.take();
            return Err(error);
        }
        Ok(FrameForward::Sent)
    }

    pub(crate) fn update_viewport_if_current(
        &self,
        candidate: &ConnectionToken,
        viewport: (u16, u16),
    ) -> bool {
        let mut guard = self.inner.lock().expect("current connection lock");
        let Some(active) = guard.as_mut() else {
            return false;
        };
        if !Arc::ptr_eq(&active.token, &candidate.0) {
            return false;
        }
        active.viewport = Some(viewport);
        true
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

    /// Write exact binary bytes to the current connection.
    ///
    /// Returns `Ok(false)` when no PaperPad is connected. A failed write
    /// closes and clears the connection generation used for the write.
    /// Holding the connection lock across `write_all` prevents concurrent
    /// frame producers from interleaving protocol bytes on cloned sockets.
    #[cfg(test)]
    pub(crate) fn forward_bytes(&self, bytes: &[u8]) -> io::Result<bool> {
        let mut guard = self.inner.lock().expect("current connection lock");
        let Some(active) = guard.as_mut() else {
            return Ok(false);
        };

        if let Err(error) = active.stream.write_all(bytes) {
            let _ = active.stream.shutdown(Shutdown::Both);
            guard.take();
            return Err(error);
        }
        Ok(true)
    }
}

// A client that keeps reading small amounts still has a finite frame deadline.
fn write_targeted_frame(stream: &mut TcpStream, mut bytes: &[u8]) -> io::Result<()> {
    let original = stream.write_timeout()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while !bytes.is_empty() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "frame write deadline",
            ));
        }
        stream.set_write_timeout(Some(remaining.max(Duration::from_millis(1))))?;
        match stream.write(bytes) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "writing frame")),
            Ok(written) => bytes = &bytes[written..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    stream.set_write_timeout(original)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
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
        assert!(!current.forward_bytes(b"nobody").expect("no target"));

        let (first, mut first_peer) = tcp_pair();
        first_peer
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set first timeout");
        let first_token = current.install(&first).expect("install first");
        assert!(current.forward_bytes(&[1, 2]).expect("forward first"));
        let mut first_bytes = [0; 2];
        first_peer
            .read_exact(&mut first_bytes)
            .expect("read first bytes");
        assert_eq!(first_bytes, [1, 2]);

        let (second, mut second_peer) = tcp_pair();
        second_peer
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set second timeout");
        let second_token = current.install(&second).expect("install second");
        assert!(!current.clear_if_current(&first_token));
        assert!(current.forward_bytes(&[3, 4]).expect("forward second"));
        let mut second_bytes = [0; 2];
        second_peer
            .read_exact(&mut second_bytes)
            .expect("read second bytes");
        assert_eq!(second_bytes, [3, 4]);

        assert!(current.clear_if_current(&second_token));
        assert!(!current.forward_bytes(b"nobody").expect("cleared target"));
    }

    #[test]
    fn frame_forwarding_enforces_active_hello_capabilities() {
        let current = CurrentConnection::default();
        let (stream, mut peer) = tcp_pair();
        current
            .install_with_pixel_formats(&stream, V2PixelFormats::GRAY8)
            .expect("install Gray8 connection");

        assert_eq!(
            current
                .forward_frame(b"mono", V2PixelFormat::Mono1)
                .expect("reject unsupported format"),
            FrameForward::UnsupportedPixelFormat
        );
        assert_eq!(
            current
                .forward_frame(b"gray", V2PixelFormat::Gray8)
                .expect("forward Gray8"),
            FrameForward::Sent
        );
        let mut received = [0; 4];
        peer.read_exact(&mut received).expect("read Gray8 bytes");
        assert_eq!(&received, b"gray");
    }

    #[test]
    fn frame_target_rejects_viewport_and_connection_races() {
        let current = CurrentConnection::default();
        let (stream, mut peer) = tcp_pair();
        let token = current
            .install_for_hello(&stream, V2PixelFormats::GRAY8, (5, 7))
            .expect("install Gray8 connection");
        assert_eq!(
            current.frame_target(V2PixelFormat::Mono1).err(),
            Some(FrameTargetError::UnsupportedPixelFormat)
        );

        let target = current
            .frame_target(V2PixelFormat::Gray8)
            .expect("Gray8 frame target");
        assert_eq!(target.viewport(), (5, 7));
        assert_eq!(
            current
                .forward_frame_to(b"first", V2PixelFormat::Gray8, &target)
                .expect("forward matching target"),
            FrameForward::Sent
        );
        let mut received = [0; 5];
        peer.read_exact(&mut received).expect("read targeted bytes");
        assert_eq!(&received, b"first");

        assert!(current.update_viewport_if_current(&token, (6, 8)));
        assert_eq!(
            current
                .forward_frame_to(b"stale", V2PixelFormat::Gray8, &target)
                .expect("reject stale viewport"),
            FrameForward::ViewportChanged
        );
        let updated = current
            .frame_target(V2PixelFormat::Gray8)
            .expect("updated frame target");
        assert_eq!(updated.viewport(), (6, 8));

        let (replacement, _replacement_peer) = tcp_pair();
        current
            .install_for_hello(&replacement, V2PixelFormats::GRAY8, (6, 8))
            .expect("install replacement");
        assert_eq!(
            current
                .forward_frame_to(b"stale", V2PixelFormat::Gray8, &updated)
                .expect("reject replaced connection"),
            FrameForward::ConnectionChanged
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
            .forward_bytes(b"failure")
            .expect_err("closed stream write must fail");
        assert!(!current.forward_bytes(b"nobody").expect("cleared target"));
    }

    #[test]
    fn binary_forwarding_preserves_exact_bytes_without_newline() {
        let current = CurrentConnection::default();
        let (stream, mut peer) = tcp_pair();
        peer.set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set timeout");
        current.install(&stream).expect("install");

        let expected = [b'P', b'P', b'F', b'B', 0, 2, 0xff];
        assert!(current.forward_bytes(&expected).expect("forward bytes"));
        let mut actual = [0; 7];
        peer.read_exact(&mut actual).expect("read exact bytes");
        assert_eq!(actual, expected);
    }

    #[test]
    fn concurrent_forwarders_preserve_whole_message_boundaries() {
        let current = CurrentConnection::default();
        let (stream, mut peer) = tcp_pair();
        current.install(&stream).expect("install connection");
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let message_len = 128 * 1024;

        let first = {
            let current = current.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                current
                    .forward_bytes(&vec![0x11; message_len])
                    .expect("forward first")
            })
        };
        let second = {
            let current = current.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                current
                    .forward_bytes(&vec![0x22; message_len])
                    .expect("forward second")
            })
        };

        barrier.wait();
        let mut received = vec![0; message_len * 2];
        peer.read_exact(&mut received).expect("read both messages");
        assert!(first.join().expect("join first"));
        assert!(second.join().expect("join second"));
        assert!(
            received == [vec![0x11; message_len], vec![0x22; message_len]].concat()
                || received == [vec![0x22; message_len], vec![0x11; message_len]].concat()
        );
    }
}
