//! Typed, frame-bounded event queues for communication between ECS systems.

use std::{
    any::{Any, TypeId},
    collections::{HashMap, VecDeque},
};

/// Stores typed events and transfers published events at an explicit frame boundary.
///
/// Events sent during a frame are placed in the next-frame buffer. Calling
/// [`EventQueue::begin_frame`] makes that buffer visible to consumers, so an
/// event cannot be consumed twice and systems cannot accidentally observe an
/// event emitted later in the same frame.
#[derive(Default)]
pub struct EventQueue {
    current: HashMap<TypeId, VecDeque<Box<dyn Any + Send>>>,
    next: HashMap<TypeId, VecDeque<Box<dyn Any + Send>>>,
    frame: u64,
}

impl EventQueue {
    /// Creates an empty event queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Publishes an event for the next frame boundary.
    pub fn send<T: Any + Send>(&mut self, event: T) {
        self.next
            .entry(TypeId::of::<T>())
            .or_default()
            .push_back(Box::new(event));
    }

    /// Makes events published since the previous boundary available to consumers.
    ///
    /// Any unconsumed events from the previous frame are discarded deliberately;
    /// this gives the queue a clear one-frame ownership boundary instead of
    /// silently carrying stale events into later frames.
    pub fn begin_frame(&mut self) {
        self.current.clear();
        std::mem::swap(&mut self.current, &mut self.next);
        self.frame = self.frame.wrapping_add(1);
    }

    /// Returns the current event frame number.
    #[must_use]
    pub const fn frame(&self) -> u64 {
        self.frame
    }

    /// Returns the number of currently visible events of type `T`.
    #[must_use]
    pub fn len<T: Any>(&self) -> usize {
        self.current
            .get(&TypeId::of::<T>())
            .map_or(0, VecDeque::len)
    }

    /// Returns whether no currently visible events of type `T` remain.
    #[must_use]
    pub fn is_empty<T: Any>(&self) -> bool {
        self.len::<T>() == 0
    }

    /// Reads currently visible events without consuming them.
    #[must_use]
    pub fn read<T: Any>(&self) -> Vec<&T> {
        self.current
            .get(&TypeId::of::<T>())
            .into_iter()
            .flat_map(|events| events.iter())
            .filter_map(|event| event.downcast_ref::<T>())
            .collect()
    }

    /// Consumes and returns currently visible events in publication order.
    pub fn drain<T: Any>(&mut self) -> Vec<T> {
        self.current
            .remove(&TypeId::of::<T>())
            .into_iter()
            .flat_map(|events| events.into_iter())
            .map(|event| {
                *event
                    .downcast::<T>()
                    .expect("event type id must match its concrete value")
            })
            .collect()
    }

    /// Discards all currently visible events without touching next-frame events.
    pub fn clear_current(&mut self) {
        self.current.clear();
    }

    /// Returns the number of queued events that will become visible at the next boundary.
    #[must_use]
    pub fn pending_len<T: Any>(&self) -> usize {
        self.next.get(&TypeId::of::<T>()).map_or(0, VecDeque::len)
    }
}

#[cfg(test)]
mod tests {
    use super::EventQueue;

    #[derive(Debug, PartialEq)]
    struct Damage {
        amount: u32,
    }

    #[derive(Debug, PartialEq)]
    struct Spawned {
        id: u32,
    }

    #[test]
    fn preserves_type_and_publication_order() {
        let mut queue = EventQueue::new();
        queue.send(Damage { amount: 10 });
        queue.send(Damage { amount: 20 });
        queue.send(Spawned { id: 7 });
        assert_eq!(queue.pending_len::<Damage>(), 2);
        assert_eq!(queue.frame(), 0);

        queue.begin_frame();
        assert_eq!(queue.frame(), 1);
        assert_eq!(
            queue.read::<Damage>(),
            vec![&Damage { amount: 10 }, &Damage { amount: 20 }]
        );
        assert_eq!(
            queue.drain::<Damage>(),
            vec![Damage { amount: 10 }, Damage { amount: 20 }]
        );
        assert_eq!(queue.drain::<Spawned>(), vec![Spawned { id: 7 }]);
    }

    #[test]
    fn drained_events_cannot_be_consumed_twice() {
        let mut queue = EventQueue::new();
        queue.send(Damage { amount: 5 });
        queue.begin_frame();
        assert_eq!(queue.drain::<Damage>(), vec![Damage { amount: 5 }]);
        assert!(queue.is_empty::<Damage>());
        assert!(queue.drain::<Damage>().is_empty());
    }

    #[test]
    fn events_sent_during_a_frame_wait_for_the_next_boundary() {
        let mut queue = EventQueue::new();
        queue.send(Damage { amount: 1 });
        queue.begin_frame();
        queue.send(Damage { amount: 2 });
        assert_eq!(queue.read::<Damage>(), vec![&Damage { amount: 1 }]);
        assert_eq!(queue.pending_len::<Damage>(), 1);

        queue.begin_frame();
        assert_eq!(queue.drain::<Damage>(), vec![Damage { amount: 2 }]);
    }

    #[test]
    fn unconsumed_events_are_dropped_at_the_next_boundary() {
        let mut queue = EventQueue::new();
        queue.send(Damage { amount: 1 });
        queue.begin_frame();
        queue.send(Damage { amount: 2 });
        queue.begin_frame();
        assert_eq!(queue.drain::<Damage>(), vec![Damage { amount: 2 }]);
    }

    #[test]
    fn clear_current_does_not_drop_pending_events() {
        let mut queue = EventQueue::new();
        queue.send(Damage { amount: 1 });
        queue.begin_frame();
        queue.send(Damage { amount: 2 });
        queue.clear_current();
        assert!(queue.is_empty::<Damage>());
        queue.begin_frame();
        assert_eq!(queue.drain::<Damage>(), vec![Damage { amount: 2 }]);
    }
}
