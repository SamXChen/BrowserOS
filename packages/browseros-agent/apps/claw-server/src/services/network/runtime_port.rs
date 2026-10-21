//! The seam between the network service and the CDP runtime.
//!
//! `NetworkRuntimeHandle` is what production uses; tests inject a fake, so rule
//! lifecycle behaviour can be asserted without a browser.

use browseros_core::network::{ApplyReport, NetworkRuntimeHandle, RuntimeRuleSet, RuntimeStats};
use futures_util::future::BoxFuture;
use std::sync::{Arc, RwLock};

pub trait RuntimePort: Send + Sync {
    fn set_capture(&self, running: bool);
    fn apply_rules(&self, rules: Arc<RuntimeRuleSet>) -> BoxFuture<'_, ApplyReport>;
    fn stats(&self) -> RuntimeStats;
    fn shutdown(&self);
}

impl RuntimePort for NetworkRuntimeHandle {
    fn set_capture(&self, running: bool) {
        NetworkRuntimeHandle::set_capture(self, running);
    }

    fn apply_rules(&self, rules: Arc<RuntimeRuleSet>) -> BoxFuture<'_, ApplyReport> {
        Box::pin(NetworkRuntimeHandle::apply_rules(self, rules))
    }

    fn stats(&self) -> RuntimeStats {
        NetworkRuntimeHandle::stats(self)
    }

    fn shutdown(&self) {
        NetworkRuntimeHandle::shutdown(self);
    }
}

/// Holds the current runtime, if the browser is connected. Cloning the handle
/// out keeps callers from holding the lock across a CDP round trip.
pub struct RuntimeSlot(RwLock<Option<Arc<dyn RuntimePort>>>);

impl RuntimeSlot {
    pub fn empty() -> Self {
        Self(RwLock::new(None))
    }

    pub fn current(&self) -> Option<Arc<dyn RuntimePort>> {
        self.0
            .read()
            .ok()
            .and_then(|slot| slot.as_ref().map(Arc::clone))
    }

    pub fn replace(&self, next: Option<Arc<dyn RuntimePort>>) -> Option<Arc<dyn RuntimePort>> {
        let mut slot = self.0.write().ok()?;
        let previous = slot.take();
        *slot = next;
        previous
    }
}
