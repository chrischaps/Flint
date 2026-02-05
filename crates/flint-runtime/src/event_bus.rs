//! Event bus for broadcasting game events

use crate::event::GameEvent;

/// A simple event queue that systems push to and consumers drain
pub struct EventBus {
    events: Vec<GameEvent>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    /// Push an event onto the bus
    pub fn push(&mut self, event: GameEvent) {
        self.events.push(event);
    }

    /// Drain all events from the bus, returning them
    pub fn drain(&mut self) -> Vec<GameEvent> {
        std::mem::take(&mut self.events)
    }

    /// Check if there are pending events
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Number of pending events
    pub fn len(&self) -> usize {
        self.events.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::EntityId;

    #[test]
    fn test_push_and_drain() {
        let mut bus = EventBus::new();
        assert!(bus.is_empty());

        bus.push(GameEvent::ActionPressed("jump".into()));
        bus.push(GameEvent::CollisionStarted {
            entity_a: EntityId::new(),
            entity_b: EntityId::new(),
        });

        assert_eq!(bus.len(), 2);
        assert!(!bus.is_empty());

        let events = bus.drain();
        assert_eq!(events.len(), 2);
        assert!(bus.is_empty());
    }

    #[test]
    fn test_drain_clears() {
        let mut bus = EventBus::new();
        bus.push(GameEvent::ActionPressed("test".into()));

        let _ = bus.drain();
        let events = bus.drain();
        assert!(events.is_empty());
    }
}
