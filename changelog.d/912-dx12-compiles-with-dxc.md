### DX12 compiles shaders with DXC, with FXC as the fallback (#912)
**What changed.** On DX12, Flint now compiles shaders with DXC, which is about
5x faster than FXC on the ASCII style. DXC needs `dxcompiler.dll` and
`dxil.dll` beside the executable, or in the folder `FLINT_DXC_DIR` names. A
copy anywhere else, `PATH` included, is never loaded. Without the two DLLs,
DX12 uses FXC as before and prints one warning. Every adapter line now names
the compiler: `flint render`'s new `GPU:` line, `flint goldens`' `adapter:`
line, F3, and `--perf`. `WGPU_DX12_COMPILER=fxc` forces FXC.
**What to do.** Run `flint dxc install` after a build. It copies the pinned
release's DLLs beside `flint.exe` and the test binaries. To ship DXC in your
game, run `flint dxc install --to <your build folder>`, which copies two files
plus their licence files. Read the licence terms in the book's GPU Selection
section first. Vulkan and the web are unchanged.
