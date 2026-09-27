//! One bounded, lock-protected request/reply transaction.

use std::io;
use std::time::{Duration, Instant};

use meltalarm_source_api::hidapi::HidDevice;

use crate::lock::PsuLock;
use crate::protocol::{FRAME, Frame, Reg, Reply, classify, read_request};

const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const REPLY_DEADLINE: Duration = Duration::from_millis(500);
const MAX_DRAIN: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub enum TxError {
    /// Another PSU reader held the lock too long; this read is skipped.
    LockTimeout,
    /// No matching reply within the deadline.
    Timeout,
    /// The PSU answered with its busy pattern.
    Busy,
    /// The echo matched but the reply was too short to decode.
    Short,
    Io(String),
}

/// Minimal device I/O, so transactions can be tested against a fake device.
pub(crate) trait Transport {
    fn send(&self, packet: &[u8; FRAME]) -> io::Result<()>;
    fn recv(&self, buf: &mut [u8; FRAME], timeout_ms: i32) -> io::Result<usize>;
}

impl Transport for HidDevice {
    fn send(&self, packet: &[u8; FRAME]) -> io::Result<()> {
        // THE single device-write call site of this crate (read-only contract,
        // docs/ARCHITECTURE.md §9). `packet` can only come from `read_request`.
        // The windows-native backend may return Ok(0) on success; the reply echo validates.
        #[allow(clippy::disallowed_methods)]
        self.write(packet).map(|_| ()).map_err(io::Error::other)
    }

    fn recv(&self, buf: &mut [u8; FRAME], timeout_ms: i32) -> io::Result<usize> {
        self.read_timeout(buf, timeout_ms).map_err(io::Error::other)
    }
}

/// lock → drain stale reports → request → read until our echo → unlock.
pub(crate) fn transact(dev: &impl Transport, lock: &PsuLock, reg: Reg) -> Result<Frame, TxError> {
    let _guard = lock.acquire(LOCK_TIMEOUT)?;
    let io = |e: io::Error| TxError::Io(e.to_string());

    // Other clients' replies are delivered to our handle too (F17): discard them first.
    let mut buf = [0u8; FRAME];
    for _ in 0..MAX_DRAIN {
        if dev.recv(&mut buf, 0).map_err(io)? == 0 {
            break;
        }
    }

    dev.send(&read_request(reg)).map_err(io)?;

    let deadline = Instant::now() + REPLY_DEADLINE;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(TxError::Timeout);
        }
        buf = [0u8; FRAME];
        let n = dev.recv(&mut buf, left.as_millis().clamp(1, 100) as i32).map_err(io)?;
        if n == 0 {
            continue;
        }
        match classify(reg, &buf[..n]) {
            Reply::Ours(frame) => return Ok(frame),
            Reply::Busy => return Err(TxError::Busy),
            Reply::Short => return Err(TxError::Short),
            Reply::Foreign => continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::tests::{C1_NORMAL, E0_IDLE, raw};
    use std::cell::RefCell;
    use std::collections::VecDeque;

    /// Scripted device: `stale` reports are queued before the request, `replies` after it.
    struct Fake {
        stale: RefCell<VecDeque<Vec<u8>>>,
        replies: RefCell<VecDeque<Vec<u8>>>,
        sent: RefCell<Vec<[u8; FRAME]>>,
    }

    impl Fake {
        fn new(stale: &[&str], replies: &[&str]) -> Self {
            Fake {
                stale: RefCell::new(stale.iter().map(|h| raw(h)).collect()),
                replies: RefCell::new(replies.iter().map(|h| raw(h)).collect()),
                sent: RefCell::new(vec![]),
            }
        }
    }

    impl Transport for Fake {
        fn send(&self, p: &[u8; FRAME]) -> io::Result<()> {
            self.sent.borrow_mut().push(*p);
            Ok(())
        }
        fn recv(&self, buf: &mut [u8; FRAME], _ms: i32) -> io::Result<usize> {
            let queue = if self.sent.borrow().is_empty() { &self.stale } else { &self.replies };
            match queue.borrow_mut().pop_front() {
                Some(r) => {
                    buf[..r.len()].copy_from_slice(&r);
                    Ok(r.len())
                }
                None => Ok(0),
            }
        }
    }

    #[test]
    fn drains_stale_same_register_reply_before_requesting() {
        // A stale E0 (another client's, wire 1 = 12 A) must not be taken as our answer.
        let stale_e0 = format!("{}C0E0{}", &E0_IDLE[..16], &E0_IDLE[20..]);
        let dev = Fake::new(&[&stale_e0, C1_NORMAL], &[E0_IDLE]);
        let f = transact(&dev, &PsuLock::None, Reg::Telemetry).unwrap();
        assert_eq!(f.wire_currents()[0], [0.125; 6]);
        assert!(dev.stale.borrow().is_empty());
        assert_eq!(dev.sent.borrow().len(), 1);
    }

    #[test]
    fn skips_foreign_replies_after_request() {
        let dev = Fake::new(&[], &[C1_NORMAL, C1_NORMAL, E0_IDLE]);
        assert!(transact(&dev, &PsuLock::None, Reg::Telemetry).is_ok());
    }

    #[test]
    fn busy_and_timeout_are_errors_not_zeros() {
        let dev = Fake::new(&[], &["51E0FE"]);
        assert_eq!(transact(&dev, &PsuLock::None, Reg::Telemetry), Err(TxError::Busy));
        let dev = Fake::new(&[], &[C1_NORMAL]);
        assert_eq!(transact(&dev, &PsuLock::None, Reg::Telemetry), Err(TxError::Timeout));
    }

    #[test]
    fn sends_only_the_read_request() {
        let dev = Fake::new(&[], &[E0_IDLE]);
        transact(&dev, &PsuLock::None, Reg::Telemetry).unwrap();
        assert_eq!(dev.sent.borrow()[0], read_request(Reg::Telemetry));
    }
}
