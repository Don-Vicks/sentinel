//! The integration boundary with Vortex. Sentinel depends on this trait, not
//! on how Vortex connects to Solana.

use std::sync::Arc;
use tokio::sync::broadcast;
use vortex::events::VortexTransaction;
use vortex::hub::{HubStats, VortexHub};

pub trait VortexSource: Send + Sync {
    /// Live stream of decoded transactions for the programs being watched.
    fn subscribe(&self) -> broadcast::Receiver<Arc<VortexTransaction>>;
    /// Replaces the set of programs Sentinel wants streamed.
    fn watch_programs(&self, programs: Vec<String>);
    fn health(&self) -> HubStats;
}

const CONSUMER: &str = "sentinel";

impl VortexSource for VortexHub {
    fn subscribe(&self) -> broadcast::Receiver<Arc<VortexTransaction>> {
        VortexHub::subscribe(self)
    }

    fn watch_programs(&self, programs: Vec<String>) {
        self.set_programs(CONSUMER, programs);
    }

    fn health(&self) -> HubStats {
        self.stats()
    }
}
