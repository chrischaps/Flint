//! The living package's tests that drive its subsystem through the player
//! (#1558): the living code is `flint_living::player` (the package's crate,
//! `packages/living/crates/flint-living`), and these stay here because they
//! bring a scene up in a `PlayerApp`, enter it and step its frames, which
//! the package's crate can't (it never depends on flint-player). They name
//! the package's types through flint-player's dev-dependency on
//! flint-living, and run where the run's launcher lists the package, as the
//! stock one does in a build with `living`.

mod api;
mod glue;
mod props;
mod ramps;
mod tuning;
mod wooze_read;
