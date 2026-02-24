# Remaining Planned Work (Pending)

This file tracks the **remaining** items from the approved audit plan that are **not implemented yet**.

## 1) P1 Reliability/Quality (Pending)

- [ ] Add app-layer test coverage for `flint-cli`, `flint-player`, and `flint-viewer`
- [ ] Add CLI integration tests for:
- [ ] `flint asset resolve` nested-reference behavior (`model.asset`, `material.texture`, `audio_source.file`, `sprite.texture`)
- [ ] startup failure paths in viewer/player (ensure graceful error and clean exit)
- [ ] Remove or fully integrate legacy duplicate serve implementation in `crates/flint-cli/src/commands/serve.rs`
- [ ] Make HTTP timeout/retry policy configurable in `.flint/config.toml` instead of provider-level constants
- [ ] Add provider-level test coverage for retry/timeout behavior and poll-timeout handling

## 2) P2 Feature Work (Pending)

- [ ] Projectile system
- [ ] Add runtime projectile component/schema and update loop integration
- [ ] Add collision/damage/splash hooks for script and gameplay usage
- [ ] Game state machine and level transitions
- [ ] Add `GameState` runtime gating (play/pause/death/menu)
- [ ] Add scene transition request flow and safe runtime teardown/reload
- [ ] Particle system
- [ ] Add emitter data model and update loop
- [ ] Add billboard-based particle rendering integration
- [ ] Enemy pathing baseline
- [ ] Add simple navigation/pathing suitable for Doom-style arena movement
- [ ] Settings persistence
- [ ] Add `settings.toml` load/save for mouse sensitivity, keybinds, and volume

## 3) Public Interface Follow-ups (Pending)

- [ ] Add explicit runtime transition interface (`load_scene` request/event path)
- [ ] Document provider retry/timeout config in user-facing docs
- [ ] Add docs for nested asset reference resolution behavior

## 4) Validation/Acceptance (Pending)

- [ ] Verify no regressions in asset generation + sidecar registration flows
- [ ] Add end-to-end test for batch generation producing immediately resolvable assets
- [ ] Run full workspace tests and clippy after pending items are implemented
