//! What the SYSTEM side of Kairo's Windows TUN is made of, as plain functions
//! that run (and are tested) everywhere:
//!
//! * [`names`]: the one layout under `%ProgramData%\Kairo`, and the only
//!   arguments the host accepts;
//! * [`sanitize`]: the user's config reduced to what the core is allowed to run
//!   as SYSTEM;
//! * [`gate`]: which controller calls the host forwards to the core;
//! * [`winfs`] (Windows): directories created locked, reparse points refused.
//!
//! See `RESULT-windows-slice2.md` (slice 2b) for the threat model these rules
//! come from.

pub mod gate;
pub mod names;
pub mod sanitize;
#[cfg(windows)]
pub mod winfs;
