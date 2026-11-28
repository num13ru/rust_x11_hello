//! Linux readiness wait used by the staged event-driven native runtime.

#![cfg_attr(not(test), allow(dead_code))]

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::sync::Arc;
use std::time::{Duration, Instant};

const EVENTFD_VALUE_SIZE: usize = std::mem::size_of::<u64>();
const NANOS_PER_MILLI: u128 = 1_000_000;

/// Readiness reported for the borrowed evdev descriptor.
///
/// `poll(2)` can report readable data together with a terminal condition, so
/// these flags deliberately are not represented as a mutually exclusive enum.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct InputReadiness {
    pub(crate) readable: bool,
    pub(crate) error: bool,
    pub(crate) hung_up: bool,
    pub(crate) invalid: bool,
}

impl InputReadiness {
    fn from_revents(revents: libc::c_short) -> Self {
        Self {
            readable: revents & libc::POLLIN != 0,
            error: revents & libc::POLLERR != 0,
            hung_up: revents & libc::POLLHUP != 0,
            invalid: revents & libc::POLLNVAL != 0,
        }
    }

    fn any(self) -> bool {
        self.readable || self.error || self.hung_up || self.invalid
    }
}

/// All readiness observed in one bounded `poll(2)` pass.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RuntimeReady {
    pub(crate) input: InputReadiness,
    pub(crate) notified: bool,
    pub(crate) deadline_elapsed: bool,
    pub(crate) power: Option<InputReadiness>,
}

/// Single-consumer readiness wait over evdev and a coalescing eventfd.
#[derive(Debug)]
pub(crate) struct RuntimeWait {
    input_fd: RawFd,
    notification_fd: Arc<OwnedFd>,
}

/// Cloneable producer handle for waking [`RuntimeWait`].
///
/// Notifications carry no payload. Producers must publish shared state before
/// calling `notify`; the consumer re-reads that authoritative state after wake.
#[derive(Clone, Debug)]
pub(crate) struct RuntimeNotifier {
    notification_fd: Arc<OwnedFd>,
}

impl RuntimeWait {
    /// Create a waiter borrowing `input_fd` for the lifetime of the waiter.
    ///
    /// The caller retains ownership and must keep the descriptor open. Closing
    /// it is reported as `InputReadiness::invalid` on a later wait.
    pub(crate) fn new(input_fd: RawFd) -> io::Result<(Self, RuntimeNotifier)> {
        let notification_fd = Arc::new(create_eventfd()?);
        validate_fd(input_fd)?;

        let notifier = RuntimeNotifier {
            notification_fd: Arc::clone(&notification_fd),
        };
        Ok((
            Self {
                input_fd,
                notification_fd,
            },
            notifier,
        ))
    }

    /// Block until input, notification, or the optional monotonic deadline.
    ///
    /// `None` waits indefinitely. Interruptions retry after recomputing the
    /// remaining timeout; an interrupted wait is not itself a shutdown event.
    pub(crate) fn wait(&self, deadline: Option<Instant>) -> io::Result<RuntimeReady> {
        self.wait_with_poll(deadline, None, system_poll)
    }

    /// Give Kindle power readiness priority over input, frames and deadlines.
    pub(crate) fn wait_for_session(
        &self,
        power: BorrowedFd<'_>,
        deadline: Option<Instant>,
    ) -> io::Result<RuntimeReady> {
        self.wait_with_poll(deadline, Some(power), system_poll)
    }

    fn wait_with_poll<P>(
        &self,
        deadline: Option<Instant>,
        power: Option<BorrowedFd<'_>>,
        mut poll: P,
    ) -> io::Result<RuntimeReady>
    where
        P: FnMut(&mut [libc::pollfd; 3], libc::c_int) -> io::Result<libc::c_int>,
    {
        loop {
            let mut descriptors = [
                libc::pollfd {
                    fd: self.input_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: self.notification_fd.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: power.map_or(-1, |fd| fd.as_raw_fd()),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];

            let timeout = poll_timeout(deadline, Instant::now());
            let ready_count = match poll(&mut descriptors, timeout) {
                Ok(ready_count) => ready_count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };

            if ready_count == 0 {
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    return Ok(RuntimeReady {
                        deadline_elapsed: true,
                        ..RuntimeReady::default()
                    });
                }

                // A clamped very-long timeout or an unexpectedly early timeout
                // is not a deadline event. Recompute rather than busy-dispatch.
                continue;
            }

            let power = InputReadiness::from_revents(descriptors[2].revents);
            if power.any() {
                return Ok(RuntimeReady {
                    power: Some(power),
                    ..RuntimeReady::default()
                });
            }

            let input = InputReadiness::from_revents(descriptors[0].revents);
            let notification_revents = descriptors[1].revents;
            check_notification_errors(notification_revents)?;
            let notified = notification_revents & libc::POLLIN != 0;
            if notified {
                consume_notification(self.notification_fd.as_raw_fd())?;
            }

            let deadline_elapsed = deadline.is_some_and(|deadline| Instant::now() >= deadline);
            if input.any() || notified || deadline_elapsed {
                return Ok(RuntimeReady {
                    input,
                    notified,
                    deadline_elapsed,
                    power: None,
                });
            }
        }
    }
}

impl RuntimeNotifier {
    /// Wake the waiter without blocking or allocating on the success path.
    ///
    /// A saturated eventfd already represents pending work, so `EAGAIN` is a
    /// successful coalesced notification rather than an error or retry loop.
    pub(crate) fn notify(&self) -> io::Result<()> {
        let value = 1_u64;
        loop {
            // SAFETY: eventfd requires one aligned native-endian u64. The
            // descriptor remains open through the shared `OwnedFd`.
            let written = unsafe {
                libc::write(
                    self.notification_fd.as_raw_fd(),
                    (&value as *const u64).cast::<libc::c_void>(),
                    EVENTFD_VALUE_SIZE,
                )
            };
            if written == EVENTFD_VALUE_SIZE as libc::ssize_t {
                return Ok(());
            }
            if written == -1 {
                let error = io::Error::last_os_error();
                match error.kind() {
                    io::ErrorKind::Interrupted => continue,
                    io::ErrorKind::WouldBlock => return Ok(()),
                    _ => return Err(error),
                }
            }
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                format!("short eventfd notification write: {written} bytes"),
            ));
        }
    }

    /// Borrow the notification descriptor for a future signal-safe writer.
    ///
    /// The raw descriptor is valid while any waiter or notifier clone remains
    /// alive. The caller must not close it.
    pub(crate) fn raw_fd(&self) -> RawFd {
        self.notification_fd.as_raw_fd()
    }
}

fn create_eventfd() -> io::Result<OwnedFd> {
    // SAFETY: no pointers are involved. Flags request nonblocking, close-on-exec
    // ownership from the moment the returned descriptor is wrapped below.
    let raw_fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
    if raw_fd == -1 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: a successful `eventfd` returns a new owned descriptor exactly
    // once; `OwnedFd` closes it on every later initialization failure.
    Ok(unsafe { OwnedFd::from_raw_fd(raw_fd) })
}

fn validate_fd(fd: RawFd) -> io::Result<()> {
    if fd < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "runtime input descriptor must be nonnegative",
        ));
    }

    loop {
        // SAFETY: `F_GETFD` reads descriptor flags and takes no third argument.
        let result = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if result >= 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn system_poll(
    descriptors: &mut [libc::pollfd; 3],
    timeout: libc::c_int,
) -> io::Result<libc::c_int> {
    // SAFETY: both pollfd entries remain live and writable for this call.
    let result = unsafe {
        libc::poll(
            descriptors.as_mut_ptr(),
            descriptors.len() as libc::nfds_t,
            timeout,
        )
    };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

fn poll_timeout(deadline: Option<Instant>, now: Instant) -> libc::c_int {
    let Some(deadline) = deadline else {
        return -1;
    };
    if deadline <= now {
        return 0;
    }
    duration_to_poll_timeout(deadline.duration_since(now))
}

fn duration_to_poll_timeout(remaining: Duration) -> libc::c_int {
    let nanos = remaining.as_nanos();
    let millis = nanos / NANOS_PER_MILLI + u128::from(nanos % NANOS_PER_MILLI != 0);
    millis.min(libc::c_int::MAX as u128) as libc::c_int
}

fn check_notification_errors(revents: libc::c_short) -> io::Result<()> {
    if revents & libc::POLLNVAL != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "runtime notification descriptor became invalid",
        ));
    }
    if revents & libc::POLLHUP != 0 {
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "runtime notification descriptor hung up",
        ));
    }
    if revents & libc::POLLERR != 0 {
        return Err(io::Error::other(
            "runtime notification descriptor reported an error",
        ));
    }
    Ok(())
}

fn consume_notification(fd: RawFd) -> io::Result<()> {
    let mut value = 0_u64;
    loop {
        // SAFETY: eventfd returns one native-endian u64 into this live buffer.
        let read = unsafe {
            libc::read(
                fd,
                (&mut value as *mut u64).cast::<libc::c_void>(),
                EVENTFD_VALUE_SIZE,
            )
        };
        if read == EVENTFD_VALUE_SIZE as libc::ssize_t {
            return Ok(());
        }
        if read == -1 {
            let error = io::Error::last_os_error();
            return match error.kind() {
                io::ErrorKind::Interrupted => continue,
                // A notification racing just after this read remains readable
                // for the next wait; a spurious readiness is harmless.
                io::ErrorKind::WouldBlock => Ok(()),
                _ => Err(error),
            };
        }
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "runtime notification descriptor reached EOF",
            ));
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("short eventfd notification read: {read} bytes"),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{AsFd, AsRawFd};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::thread;

    fn pipe_pair() -> (OwnedFd, OwnedFd) {
        let mut fds = [-1; 2];
        // SAFETY: `fds` provides two writable descriptor slots.
        let result = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
        assert_eq!(result, 0, "pipe2 failed: {}", io::Error::last_os_error());
        // SAFETY: successful pipe2 returned two distinct owned descriptors.
        unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) }
    }

    fn write_byte(fd: RawFd) {
        let value = [1_u8];
        // SAFETY: the one-byte buffer is live and the test owns `fd`.
        let written = unsafe { libc::write(fd, value.as_ptr().cast(), value.len()) };
        assert_eq!(
            written,
            1,
            "pipe write failed: {}",
            io::Error::last_os_error()
        );
    }

    fn read_byte(fd: RawFd) {
        let mut value = [0_u8];
        // SAFETY: the one-byte buffer is live and the test owns `fd`.
        let read = unsafe { libc::read(fd, value.as_mut_ptr().cast(), value.len()) };
        assert_eq!(read, 1, "pipe read failed: {}", io::Error::last_os_error());
    }

    fn write_eventfd_value(fd: RawFd, value: u64) {
        // SAFETY: eventfd accepts one aligned native-endian u64 and the test
        // keeps the shared descriptor alive.
        let written = unsafe {
            libc::write(
                fd,
                (&value as *const u64).cast::<libc::c_void>(),
                EVENTFD_VALUE_SIZE,
            )
        };
        assert_eq!(
            written,
            EVENTFD_VALUE_SIZE as libc::ssize_t,
            "eventfd write failed: {}",
            io::Error::last_os_error()
        );
    }

    #[test]
    fn idle_wait_blocks_until_notified() {
        let (input, _input_writer) = pipe_pair();
        let (waiter, notifier) = RuntimeWait::new(input.as_raw_fd()).expect("create waiter");
        let (result_tx, result_rx) = mpsc::channel();
        let worker = thread::spawn(move || result_tx.send(waiter.wait(None)).unwrap());

        assert!(
            matches!(
                result_rx.recv_timeout(Duration::from_millis(30)),
                Err(RecvTimeoutError::Timeout)
            ),
            "idle waiter returned without a source"
        );
        notifier.notify().expect("notify waiter");
        let ready = result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("waiter did not wake")
            .expect("wait failed");
        assert!(ready.notified);
        assert_eq!(ready.input, InputReadiness::default());
        assert!(!ready.deadline_elapsed);
        worker.join().expect("wait worker panicked");
    }

    #[test]
    fn multiple_notifications_coalesce() {
        let (input, _input_writer) = pipe_pair();
        let (waiter, notifier) = RuntimeWait::new(input.as_raw_fd()).expect("create waiter");
        notifier.notify().expect("first notification");
        notifier.notify().expect("second notification");
        notifier.notify().expect("third notification");

        let ready = waiter
            .wait(Some(Instant::now() + Duration::from_secs(1)))
            .expect("wait for notifications");
        assert!(ready.notified);

        let after_drain = waiter
            .wait(Some(Instant::now() + Duration::from_millis(10)))
            .expect("wait after notification drain");
        assert!(!after_drain.notified);
        assert!(after_drain.deadline_elapsed);
    }

    #[test]
    fn deadline_fires_without_readiness() {
        let (input, _input_writer) = pipe_pair();
        let (waiter, _notifier) = RuntimeWait::new(input.as_raw_fd()).expect("create waiter");
        let started = Instant::now();
        let ready = waiter
            .wait(Some(started + Duration::from_millis(10)))
            .expect("deadline wait");

        assert!(ready.deadline_elapsed);
        assert!(!ready.notified);
        assert_eq!(ready.input, InputReadiness::default());
        assert!(started.elapsed() >= Duration::from_millis(10));
    }

    #[test]
    fn input_and_notification_are_reported_together() {
        let (input, input_writer) = pipe_pair();
        let (waiter, notifier) = RuntimeWait::new(input.as_raw_fd()).expect("create waiter");
        write_byte(input_writer.as_raw_fd());
        notifier.notify().expect("notify waiter");

        let ready = waiter
            .wait(Some(Instant::now() + Duration::from_secs(1)))
            .expect("wait for simultaneous readiness");
        assert!(ready.input.readable);
        assert!(!ready.input.error);
        assert!(ready.notified);
        assert!(!ready.deadline_elapsed);
    }

    #[test]
    fn power_readiness_takes_priority_over_other_work() {
        let (input, input_writer) = pipe_pair();
        let (power, power_writer) = pipe_pair();
        let (waiter, notifier) = RuntimeWait::new(input.as_raw_fd()).expect("create waiter");
        write_byte(input_writer.as_raw_fd());
        write_byte(power_writer.as_raw_fd());
        notifier.notify().expect("notify waiter");

        let ready = waiter
            .wait_for_session(power.as_fd(), Some(Instant::now()))
            .expect("check simultaneous readiness");
        assert_eq!(
            ready,
            RuntimeReady {
                power: Some(InputReadiness {
                    readable: true,
                    ..InputReadiness::default()
                }),
                ..RuntimeReady::default()
            }
        );
    }

    #[test]
    fn a_lost_power_listener_wakes_the_session() {
        let (input, _input_writer) = pipe_pair();
        let (power, power_writer) = pipe_pair();
        let (waiter, _notifier) = RuntimeWait::new(input.as_raw_fd()).expect("create waiter");
        drop(power_writer);

        let ready = waiter
            .wait_for_session(power.as_fd(), Some(Instant::now()))
            .expect("check power listener hangup");
        assert!(ready.power.expect("power readiness").hung_up);
    }

    #[test]
    fn input_remains_ready_until_source_is_drained() {
        let (input, input_writer) = pipe_pair();
        let (waiter, _notifier) = RuntimeWait::new(input.as_raw_fd()).expect("create waiter");
        write_byte(input_writer.as_raw_fd());
        write_byte(input_writer.as_raw_fd());

        let first = waiter
            .wait(Some(Instant::now() + Duration::from_secs(1)))
            .expect("wait for first input readiness");
        assert!(first.input.readable);
        read_byte(input.as_raw_fd());

        let second = waiter
            .wait(Some(Instant::now() + Duration::from_secs(1)))
            .expect("wait for remaining input readiness");
        assert!(second.input.readable);
    }

    #[test]
    fn simultaneous_input_error_flags_are_preserved() {
        let (input, _input_writer) = pipe_pair();
        let (waiter, _notifier) = RuntimeWait::new(input.as_raw_fd()).expect("create waiter");

        let ready = waiter
            .wait_with_poll(
                Some(Instant::now() + Duration::from_secs(1)),
                None,
                |descriptors, _timeout| {
                    descriptors[0].revents = libc::POLLIN | libc::POLLERR | libc::POLLHUP;
                    Ok(1)
                },
            )
            .expect("report injected input readiness");

        assert!(ready.input.readable);
        assert!(ready.input.error);
        assert!(ready.input.hung_up);
        assert!(!ready.input.invalid);
    }

    #[test]
    fn notification_descriptor_error_is_fatal() {
        let (input, _input_writer) = pipe_pair();
        let (waiter, _notifier) = RuntimeWait::new(input.as_raw_fd()).expect("create waiter");

        let error = waiter
            .wait_with_poll(
                Some(Instant::now() + Duration::from_secs(1)),
                None,
                |descriptors, _timeout| {
                    descriptors[1].revents = libc::POLLERR;
                    Ok(1)
                },
            )
            .expect_err("notification error must fail the wait");

        assert_eq!(error.kind(), io::ErrorKind::Other);
    }

    #[test]
    fn saturated_notification_coalesces_without_retrying() {
        let (input, _input_writer) = pipe_pair();
        let (waiter, notifier) = RuntimeWait::new(input.as_raw_fd()).expect("create waiter");
        write_eventfd_value(notifier.raw_fd(), u64::MAX - 1);

        notifier
            .notify()
            .expect("saturated eventfd means a wake is already pending");
        let ready = waiter
            .wait(Some(Instant::now() + Duration::from_secs(1)))
            .expect("consume saturated notification");
        assert!(ready.notified);
    }

    #[test]
    fn input_hangup_is_observable() {
        let (input, input_writer) = pipe_pair();
        let (waiter, _notifier) = RuntimeWait::new(input.as_raw_fd()).expect("create waiter");
        drop(input_writer);

        let ready = waiter
            .wait(Some(Instant::now() + Duration::from_secs(1)))
            .expect("wait for hangup");
        assert!(ready.input.hung_up);
    }

    #[test]
    fn closed_input_descriptor_is_observable() {
        let (input, _input_writer) = pipe_pair();
        let (waiter, _notifier) = RuntimeWait::new(input.as_raw_fd()).expect("create waiter");
        drop(input);

        let ready = waiter
            .wait(Some(Instant::now() + Duration::from_secs(1)))
            .expect("wait for invalid descriptor");
        assert!(ready.input.invalid);
    }

    #[test]
    fn interrupted_poll_retries_and_recomputes() {
        let (input, input_writer) = pipe_pair();
        let (waiter, _notifier) = RuntimeWait::new(input.as_raw_fd()).expect("create waiter");
        let mut calls = 0;

        let ready = waiter
            .wait_with_poll(
                Some(Instant::now() + Duration::from_secs(1)),
                None,
                |descriptors, timeout| {
                    calls += 1;
                    if calls == 1 {
                        return Err(io::Error::from(io::ErrorKind::Interrupted));
                    }
                    write_byte(input_writer.as_raw_fd());
                    system_poll(descriptors, timeout)
                },
            )
            .expect("wait after interruption");

        assert_eq!(calls, 2);
        assert!(ready.input.readable);
    }

    #[test]
    fn sub_millisecond_deadlines_round_up_and_large_timeouts_clamp() {
        assert_eq!(duration_to_poll_timeout(Duration::from_nanos(1)), 1);
        assert_eq!(duration_to_poll_timeout(Duration::from_millis(1)), 1);
        assert_eq!(duration_to_poll_timeout(Duration::from_micros(1_001)), 2);
        assert_eq!(
            duration_to_poll_timeout(Duration::from_secs(u64::MAX)),
            libc::c_int::MAX
        );
    }

    #[test]
    fn shared_descriptor_lifetime_is_independent() {
        let (input, _input_writer) = pipe_pair();
        let (waiter, notifier) = RuntimeWait::new(input.as_raw_fd()).expect("create waiter");
        let notifier_clone = notifier.clone();
        let raw_fd = notifier.raw_fd();
        assert!(raw_fd >= 0);

        drop(waiter);
        drop(notifier);
        notifier_clone
            .notify()
            .expect("notifier should keep eventfd alive");
        drop(notifier_clone);

        let (waiter, notifier) = RuntimeWait::new(input.as_raw_fd()).expect("recreate waiter");
        drop(notifier);
        let ready = waiter
            .wait(Some(Instant::now() + Duration::from_millis(1)))
            .expect("waiter should keep eventfd alive");
        assert!(ready.deadline_elapsed);
    }

    #[test]
    fn invalid_input_is_rejected_after_eventfd_creation() {
        let error = RuntimeWait::new(-1).expect_err("negative descriptor must fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
