//! OSAIMA scheduler daemon.
//!
//! Every few seconds it measures how each process has been using the CPU (how
//! much, in what size bursts, how often it waits, how often it gives the CPU up
//! on its own), decides which kind of process it is, and sets its CPU priority to
//! match: interactive and media work is favoured over long batch jobs and idle
//! background tasks, so the desktop stays responsive while something heavy runs.
//!
//! - [`features`]: turns scheduler counters into the numbers the classifier reads
//! - [`model`]: the classifier (a small trained model, with a rule-based fallback)
//! - [`policy`]: what priority each kind of process gets
//! - [`collector`]: where the counters come from (`/proc`, or eBPF telemetry)
//! - [`actuator`]: how the priority is set, and put back
//! - [`engine`]: one measurement round, with the safeguards

pub mod actuator;
pub mod collector;
pub mod engine;
pub mod features;
pub mod model;
pub mod policy;
