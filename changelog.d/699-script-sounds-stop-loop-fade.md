### `stop_sound` works; `play_sound` returns a handle, loops and fades (#699)
**What changed.** `stop_sound` used to do nothing. Now `play_sound` and
`play_sound_at` return a handle. `stop_sound(handle)` stops that sound, and
`stop_sound(name)` stops every sound a script started under that name. Both take
an optional fade in seconds. `set_sound_volume(handle, volume[, fade])` changes a
sound's volume, with an optional fade. The opts map takes `looping: true` and
`bus: "music"` (`loop` is a Rhai keyword, so the key is `looping`), and
`play_sound(name, opts)` is new. A scene change stops every loop a script
started. In Rust, `ScriptCommand::PlaySound` and `PlaySoundAt` now carry a
`handle` and a `play: SoundPlay`. `StopSound` carries a `SoundRef` and a fade,
and `SetSoundVolume` is new. So do the `ScriptHost` sound methods.
**What to do.** A script that called `stop_sound` and heard nothing will now
stop the sound. Check that it stops what you meant. A game binary that matches
these variants in `PlayerExtension::script_command`, or implements their
`ScriptHost` methods, needs the new fields.
