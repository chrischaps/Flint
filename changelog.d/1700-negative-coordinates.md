### `--target -60,2,120` and `--yaw -45` parse: a coordinate or angle may start with a minus (#1700)
**Package.** engine
**What changed.** `flint render`, `flint edit` and the model previewer took a negative first coordinate or angle only written with `=` (`--target=-60,2,120`); with a space clap read the minus as a flag and stopped (`unexpected argument '-6' found`). `--target`, `--sheen-color`, `--yaw` and `--pitch` now take the value either way, as `--orbit` and the player's `--position` already did. A line that worked before means the same.
**What to do.** Nothing. Scripts that wrote `--target=-x,y,z` keep working.
