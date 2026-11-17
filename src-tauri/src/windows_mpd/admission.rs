//! Bounded command admission; the worker retains each lease through its ACK.
//! Separate slots reserve the timer STOP and a single shutdown message.
use super::protocol::Result;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc,
};

const LIMIT: usize = 32;
const CLOSED: usize = 1 << 8;
const SHUTDOWN: usize = 1 << 9;
const STOP: usize = 1 << 10;
const COUNT: usize = CLOSED - 1;

#[derive(Default)]
struct Gate(AtomicUsize);
impl Gate {
    fn admit(self: &Arc<Self>) -> Result<Permit> {
        let mut state = self.0.load(Ordering::Acquire);
        loop {
            if state & CLOSED != 0 {
                return Err("shutting_down");
            }
            if (state & COUNT) >= LIMIT {
                return Err("engine_busy");
            }
            match self.0.compare_exchange_weak(
                state,
                state + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Ok(Permit {
                        gate: self.clone(),
                        amount: 1,
                    })
                }
                Err(current) => state = current,
            }
        }
    }
    fn admit_stop(self: &Arc<Self>) -> Result<Permit> {
        let mut state = self.0.load(Ordering::Acquire);
        loop {
            if state & CLOSED != 0 {
                return Err("shutting_down");
            }
            if state & STOP != 0 {
                return Err("engine_busy");
            }
            match self.0.compare_exchange_weak(
                state,
                state | STOP,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Ok(Permit {
                        gate: self.clone(),
                        amount: STOP,
                    })
                }
                Err(current) => state = current,
            }
        }
    }
    fn close(&self) {
        self.0.fetch_or(CLOSED, Ordering::AcqRel);
    }
    fn shutdown_once(&self) -> Result<()> {
        let previous = self.0.fetch_or(CLOSED | SHUTDOWN, Ordering::AcqRel);
        if previous & SHUTDOWN != 0 {
            Err("shutting_down")
        } else {
            Ok(())
        }
    }
}
struct Permit {
    gate: Arc<Gate>,
    amount: usize,
}
impl Drop for Permit {
    fn drop(&mut self) {
        self.gate.0.fetch_sub(self.amount, Ordering::AcqRel);
    }
}

// Keep the lease even if the reply receiver disappears or the payload is moved.
pub(super) struct Lease {
    _permit: Option<Permit>,
}
pub(super) struct Envelope<T> {
    value: T,
    lease: Lease,
}
impl<T> Envelope<T> {
    pub(super) fn into_parts(self) -> (Lease, T) {
        (self.lease, self.value)
    }
}
pub(super) struct Sender<T> {
    sender: mpsc::SyncSender<Envelope<T>>,
    gate: Arc<Gate>,
}
impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            gate: self.gate.clone(),
        }
    }
}
pub(super) fn channel<T>() -> (Sender<T>, mpsc::Receiver<Envelope<T>>) {
    let (sender, receiver) = mpsc::sync_channel(LIMIT + 2);
    (
        Sender {
            sender,
            gate: Arc::new(Gate::default()),
        },
        receiver,
    )
}
impl<T> Sender<T> {
    pub(super) fn send(&self, value: T) -> Result<()> {
        let permit = self.gate.admit()?;
        self.enqueue(Envelope {
            value,
            lease: Lease {
                _permit: Some(permit),
            },
        })
    }
    // A saturated ordinary queue must never refuse the timer's only STOP.
    pub(super) fn send_stop(&self, value: T) -> Result<()> {
        let permit = self.gate.admit_stop()?;
        self.enqueue(Envelope {
            value,
            lease: Lease {
                _permit: Some(permit),
            },
        })
    }
    pub(super) fn close(&self) {
        self.gate.close();
    }
    pub(super) fn send_shutdown(&self, value: T) -> Result<()> {
        self.gate.shutdown_once()?;
        self.enqueue(Envelope {
            value,
            lease: Lease { _permit: None },
        })
    }
    fn enqueue(&self, envelope: Envelope<T>) -> Result<()> {
        self.sender.try_send(envelope).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => "engine_busy",
            mpsc::TrySendError::Disconnected(_) => "worker_failed",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    fn active<T>(sender: &Sender<T>) -> usize {
        sender.gate.0.load(Ordering::Acquire) & COUNT
    }

    #[test]
    fn saturation_refuses_before_enqueue_and_preserves_order_and_shutdown_slot() {
        let (sender, receiver) = channel();
        for i in 0..LIMIT {
            assert_eq!(sender.send(i), Ok(()));
        }
        assert_eq!(sender.send(999), Err("engine_busy"));
        assert_eq!(sender.send_stop(LIMIT), Ok(()));
        assert_eq!(sender.send_stop(999), Err("engine_busy"));
        assert_eq!(sender.send_shutdown(LIMIT + 1), Ok(()));
        assert_eq!(sender.send_shutdown(999), Err("shutting_down"));
        assert_eq!(sender.send(999), Err("shutting_down"));
        for i in 0..=LIMIT + 1 {
            let (_lease, value) = receiver.recv().unwrap().into_parts();
            assert_eq!(value, i);
        }
        assert!(receiver.try_recv().is_err());
        assert_eq!(active(&sender), 0);
    }
    #[test]
    fn in_flight_message_keeps_lease_after_payload_move_and_caller_exit() {
        let (sender, receiver) = channel();
        let (reply, caller) = mpsc::channel::<()>();
        sender.send(reply).unwrap();
        drop(caller);
        let (lease, reply) = receiver.recv().unwrap().into_parts();
        assert!(reply.send(()).is_err());
        for _ in 1..LIMIT {
            sender.send(mpsc::channel().0).unwrap();
        }
        assert_eq!(sender.send(mpsc::channel().0), Err("engine_busy"));
        drop(lease);
        assert_eq!(sender.send(mpsc::channel().0), Ok(()));
    }
    #[test]
    fn receiver_disconnect_releases_queued_and_failed_send_leases() {
        let (sender, receiver) = channel();
        for i in 0..LIMIT {
            sender.send(i).unwrap();
        }
        drop(receiver);
        assert_eq!(active(&sender), 0);
        assert_eq!(sender.send(99), Err("worker_failed"));
        assert_eq!(active(&sender), 0);
        assert_eq!(sender.send_shutdown(100), Err("worker_failed"));
        assert_eq!(sender.send_shutdown(101), Err("shutting_down"));
    }
    #[test]
    fn explicit_close_rejects_normal_calls_but_allows_shutdown_once() {
        let (sender, receiver) = channel();
        sender.send(1).unwrap();
        sender.close();
        sender.close();
        assert_eq!(sender.clone().send(2), Err("shutting_down"));
        sender.clone().send_shutdown(3).unwrap();
        for expected in [1, 3] {
            assert_eq!(receiver.recv().unwrap().into_parts().1, expected);
        }
        assert_eq!(active(&sender), 0);
    }
    #[test]
    fn preadmitted_delayed_send_cannot_consume_shutdown_reservation() {
        let (sender, receiver) = channel();
        let mut delayed = Vec::new();
        for value in 0..LIMIT {
            delayed.push(Envelope {
                value,
                lease: Lease {
                    _permit: Some(sender.gate.admit().unwrap()),
                },
            });
        }
        sender.send_shutdown(99).unwrap();
        for envelope in delayed {
            sender.enqueue(envelope).unwrap();
        }
        assert_eq!(receiver.recv().unwrap().into_parts().1, 99);
        // A real worker exits at shutdown and drops its receiver, rejecting all
        // delayed normal commands rather than executing them after shutdown.
        drop(receiver);
        assert_eq!(active(&sender), 0);
        assert_eq!(sender.send(100), Err("shutting_down"));
    }
    #[test]
    fn stop_reservation_releases_on_ack_disconnect_and_closed_admission() {
        let (sender, receiver) = channel();
        sender.send_stop(1).unwrap();
        let (lease, value) = receiver.recv().unwrap().into_parts();
        assert_eq!(value, 1);
        assert_eq!(sender.send_stop(2), Err("engine_busy"));
        drop(lease);
        sender.send_stop(3).unwrap();
        drop(receiver);
        assert_eq!(sender.gate.0.load(Ordering::Acquire), 0);
        assert_eq!(sender.send_stop(4), Err("worker_failed"));
        assert_eq!(sender.gate.0.load(Ordering::Acquire), 0);
        sender.close();
        assert_eq!(sender.send_stop(5), Err("shutting_down"));
    }
    #[test]
    fn close_racing_admission_preserves_stop_and_ordinary_counts_until_drop() {
        let gate = Arc::new(Gate::default());
        let mut leases = Vec::new();
        for _ in 0..LIMIT {
            leases.push(gate.admit().unwrap());
        }
        let stop = gate.admit_stop().unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let thread = {
            let gate = gate.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                gate.shutdown_once()
            })
        };
        let dropper = {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                drop(leases);
                drop(stop);
            })
        };
        barrier.wait();
        assert_eq!(thread.join().unwrap(), Ok(()));
        dropper.join().unwrap();
        assert_eq!(gate.0.load(Ordering::Acquire), CLOSED | SHUTDOWN);
        assert!(matches!(gate.admit(), Err("shutting_down")));
        assert!(matches!(gate.admit_stop(), Err("shutting_down")));
    }
    #[test]
    fn defensive_full_channel_error_releases_both_classes_of_lease() {
        // Production capacity is LIMIT+2. Force Full here to cover the error
        // cleanup as well as Disconnected without weakening that invariant.
        let (raw, _receiver) = mpsc::sync_channel(0);
        let sender = Sender {
            sender: raw,
            gate: Arc::new(Gate::default()),
        };
        assert_eq!(sender.send(1), Err("engine_busy"));
        assert_eq!(sender.gate.0.load(Ordering::Acquire), 0);
        assert_eq!(sender.send_stop(2), Err("engine_busy"));
        assert_eq!(sender.gate.0.load(Ordering::Acquire), 0);
    }
    #[test]
    fn concurrent_producers_never_exceed_limit_and_all_permits_are_reusable() {
        let gate = Arc::new(Gate::default());
        let admitted = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(65));
        let mut threads = Vec::new();
        for _ in 0..64 {
            let (gate, admitted, barrier) = (gate.clone(), admitted.clone(), barrier.clone());
            threads.push(std::thread::spawn(move || {
                let permit = gate.admit();
                match &permit {
                    Ok(_) => {
                        admitted.fetch_add(1, Ordering::AcqRel);
                    }
                    Err(code) => assert_eq!(*code, "engine_busy"),
                }
                barrier.wait();
                barrier.wait();
                drop(permit);
            }));
        }
        barrier.wait();
        assert_eq!(admitted.load(Ordering::Acquire), LIMIT);
        assert_eq!(gate.0.load(Ordering::Acquire) & COUNT, LIMIT);
        barrier.wait();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(gate.0.load(Ordering::Acquire), 0);
        drop(gate.admit().unwrap());
    }
    #[test]
    fn competing_shutdown_claims_admit_exactly_one_and_seal_all_clones() {
        let (sender, receiver) = channel();
        let mut threads = Vec::new();
        for i in 0..16 {
            let sender = sender.clone();
            threads.push(std::thread::spawn(move || sender.send_shutdown(i)));
        }
        let results: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(results
            .iter()
            .all(|result| result.is_ok() || *result == Err("shutting_down")));
        drop(receiver.recv().unwrap());
        assert!(receiver.try_recv().is_err());
        assert_eq!(sender.send(0), Err("shutting_down"));
    }
}
