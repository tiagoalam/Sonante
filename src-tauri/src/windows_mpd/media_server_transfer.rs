//! The actual container-byte pump. A reader can return after retirement; its
//! late payload must be discarded before the socket publisher is called.
use std::io;
pub(super) fn write_current(
    mut bytes: &[u8],
    mut current: impl FnMut() -> bool,
    mut write: impl FnMut(&[u8]) -> io::Result<usize>,
) -> io::Result<()> {
    while !bytes.is_empty() {
        if !current() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "media_retired",
            ));
        }
        match write(bytes) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "media_peer_closed",
                ))
            }
            Ok(n) if n <= bytes.len() => bytes = &bytes[n..],
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "media_write_invalid",
                ))
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                std::thread::yield_now()
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
pub(super) fn copy(
    mut offset: u64,
    mut left: u64,
    mut current: impl FnMut() -> bool,
    mut unchanged: impl FnMut() -> bool,
    mut read: impl FnMut(&mut [u8], u64) -> io::Result<usize>,
    mut write: impl FnMut(&[u8]) -> io::Result<()>,
    mut failed: impl FnMut(),
) -> io::Result<()> {
    let mut buffer = [0; 64 * 1024];
    while left > 0 {
        if !current() {
            return Ok(());
        }
        if !unchanged() {
            failed();
            return Ok(());
        }
        if !current() {
            return Ok(());
        }
        let wanted = left.min(buffer.len() as u64) as usize;
        let n = match read(&mut buffer[..wanted], offset) {
            Ok(n) if n > 0 && n <= wanted => n,
            _ => {
                failed();
                return Ok(());
            }
        };
        if !current() {
            return Ok(());
        }
        write(&buffer[..n])?;
        offset += n as u64;
        left -= n as u64;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    #[test]
    fn retirement_during_read_never_reaches_the_socket_publisher() {
        let live = Cell::new(true);
        let writes = Cell::new(0);
        copy(
            0,
            4,
            || live.get(),
            || true,
            |b, _| {
                b.copy_from_slice(b"late");
                live.set(false);
                Ok(4)
            },
            |_| {
                writes.set(writes.get() + 1);
                Ok(())
            },
            || panic!("normal retirement is not a read error"),
        )
        .unwrap();
        assert_eq!(writes.get(), 0);
    }
    #[test]
    fn retirement_during_metadata_prevents_the_next_read() {
        let live = Cell::new(true);
        copy(
            0,
            4,
            || live.get(),
            || {
                live.set(false);
                true
            },
            |_, _| panic!("metadata returned after retirement"),
            |_| panic!("late publish"),
            || panic!("normal retirement"),
        )
        .unwrap();
    }
    #[test]
    fn short_reads_preserve_exact_range_without_loss_or_repetition() {
        let bytes: Vec<u8> = (0..300).map(|v| (v % 251) as u8).collect();
        let mut sent: Vec<u8> = Vec::new();
        copy(
            5,
            257,
            || true,
            || true,
            |b, offset| {
                let n = b.len().min(7);
                b[..n].copy_from_slice(&bytes[offset as usize..offset as usize + n]);
                Ok(n)
            },
            |b| {
                sent.extend(b);
                Ok(())
            },
            || panic!("own bytes are available"),
        )
        .unwrap();
        assert_eq!(sent, bytes[5..262]);
    }
    #[test]
    fn eof_and_read_failure_mark_the_source_without_publishing_bytes() {
        for eof in [true, false] {
            let failed = Cell::new(false);
            copy(
                0,
                1,
                || true,
                || true,
                |_, _| {
                    if eof {
                        Ok(0)
                    } else {
                        Err(io::Error::other("synthetic fault"))
                    }
                },
                |_| panic!("no payload"),
                || failed.set(true),
            )
            .unwrap();
            assert!(failed.get());
        }
    }
    #[test]
    fn client_write_abort_is_preserved_without_faking_a_source_read_failure() {
        let failed = Cell::new(false);
        let result = copy(
            0,
            1,
            || true,
            || true,
            |b, _| {
                b[0] = 1;
                Ok(1)
            },
            |_| {
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "own client cancelled",
                ))
            },
            || failed.set(true),
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        assert!(!failed.get());
    }
    #[test]
    fn consumer_backpressure_preserves_partial_write_position_without_replay() {
        let bytes = b"own-container-bytes";
        let mut sent = Vec::new();
        let mut calls = 0;
        write_current(
            bytes,
            || true,
            |b| {
                calls += 1;
                if calls == 2 {
                    return Err(io::Error::from(io::ErrorKind::TimedOut));
                }
                let n = b.len().min(3);
                sent.extend_from_slice(&b[..n]);
                Ok(n)
            },
        )
        .unwrap();
        assert_eq!(sent, bytes);
        assert!(calls > 2);
    }
    #[test]
    fn a_waiting_writer_can_retire_without_a_source_fault_or_late_payload() {
        let calls = Cell::new(0);
        let result = write_current(
            b"pending",
            || calls.get() < 3,
            |_| {
                calls.set(calls.get() + 1);
                Err(io::Error::from(io::ErrorKind::WouldBlock))
            },
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
        assert_eq!(calls.get(), 3);
    }
}
