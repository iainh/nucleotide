// ABOUTME: Connection-wide retained-byte accounting for remote protocol v5
// ABOUTME: Uses move-only reservations so abandoned streams release capacity automatically

use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone, Debug)]
pub(crate) struct V5ConnectionByteBudget {
    inner: Arc<V5ConnectionByteBudgetInner>,
}

#[derive(Debug)]
struct V5ConnectionByteBudgetInner {
    limit: usize,
    used: AtomicUsize,
}

#[derive(Debug)]
pub(crate) struct V5ByteReservation {
    budget: V5ConnectionByteBudget,
    bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct V5ByteBudgetExceeded {
    pub(crate) limit: usize,
    pub(crate) used: usize,
    pub(crate) requested: usize,
}

impl fmt::Display for V5ByteBudgetExceeded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "retained-byte budget {} exhausted with {} bytes in use and {} more requested",
            self.limit, self.used, self.requested
        )
    }
}

impl std::error::Error for V5ByteBudgetExceeded {}

impl V5ConnectionByteBudget {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            inner: Arc::new(V5ConnectionByteBudgetInner {
                limit,
                used: AtomicUsize::new(0),
            }),
        }
    }

    pub(crate) fn reservation(&self) -> V5ByteReservation {
        V5ByteReservation {
            budget: self.clone(),
            bytes: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn used(&self) -> usize {
        self.inner.used.load(Ordering::Acquire)
    }
}

impl V5ByteReservation {
    pub(crate) fn try_grow(&mut self, bytes: usize) -> Result<(), V5ByteBudgetExceeded> {
        if bytes == 0 {
            return Ok(());
        }

        let limit = self.budget.inner.limit;
        match self
            .budget
            .inner
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|next| *next <= limit)
            }) {
            Ok(previous_used) => {
                let Some(reserved_bytes) = self.bytes.checked_add(bytes) else {
                    self.budget.inner.used.fetch_sub(bytes, Ordering::AcqRel);
                    return Err(V5ByteBudgetExceeded {
                        limit,
                        used: previous_used,
                        requested: bytes,
                    });
                };
                self.bytes = reserved_bytes;
                Ok(())
            }
            Err(used) => Err(V5ByteBudgetExceeded {
                limit,
                used,
                requested: bytes,
            }),
        }
    }

    pub(crate) fn release_all(&mut self) {
        if self.bytes == 0 {
            return;
        }
        let previous = self
            .budget
            .inner
            .used
            .fetch_sub(self.bytes, Ordering::AcqRel);
        debug_assert!(previous >= self.bytes, "v5 byte budget underflow");
        self.bytes = 0;
    }
}

impl Drop for V5ByteReservation {
    fn drop(&mut self) {
        self.release_all();
    }
}

#[derive(Debug)]
pub(crate) struct V5Budgeted<T> {
    value: T,
    _reservation: V5ByteReservation,
}

impl<T> V5Budgeted<T> {
    pub(crate) fn new(value: T, reservation: V5ByteReservation) -> Self {
        Self {
            value,
            _reservation: reservation,
        }
    }

    pub(crate) fn into_inner(self) -> T {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservations_enforce_the_shared_limit_and_release_on_drop() {
        let budget = V5ConnectionByteBudget::new(10);
        let mut first = budget.reservation();
        let mut second = budget.reservation();

        first.try_grow(6).unwrap();
        let error = second.try_grow(5).unwrap_err();
        assert_eq!(
            error,
            V5ByteBudgetExceeded {
                limit: 10,
                used: 6,
                requested: 5,
            }
        );
        assert_eq!(budget.used(), 6);

        drop(first);
        second.try_grow(5).unwrap();
        assert_eq!(budget.used(), 5);
    }

    #[test]
    fn releasing_early_does_not_release_twice_on_drop() {
        let budget = V5ConnectionByteBudget::new(10);
        let mut reservation = budget.reservation();
        reservation.try_grow(10).unwrap();
        reservation.release_all();
        assert_eq!(budget.used(), 0);

        drop(reservation);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn budgeted_value_stays_charged_until_transport_ownership_ends() {
        let budget = V5ConnectionByteBudget::new(10);
        let mut reservation = budget.reservation();
        reservation.try_grow(7).unwrap();
        let value = V5Budgeted::new(vec![1, 2, 3], reservation);
        assert_eq!(budget.used(), 7);

        assert_eq!(value.into_inner(), vec![1, 2, 3]);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn queued_budgeted_delivery_stays_charged_until_received_value_is_taken() {
        let budget = V5ConnectionByteBudget::new(10);
        let mut reservation = budget.reservation();
        reservation.try_grow(7).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();

        sender
            .send(V5Budgeted::new(vec![1, 2, 3], reservation))
            .unwrap();
        assert_eq!(budget.used(), 7);

        let value = receiver.recv().unwrap();
        assert_eq!(budget.used(), 7);
        assert_eq!(value.into_inner(), vec![1, 2, 3]);
        assert_eq!(budget.used(), 0);
    }
}
