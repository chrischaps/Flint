# Flint documentation: build, preview and publish. All the logic is in
# docs/publish.sh (#764); this passes the switches on to it in Git Bash.
#
#   powershell -ExecutionPolicy Bypass -File docs/build.ps1                    # build and copy into the docs repo
#   powershell -ExecutionPolicy Bypass -File docs/build.ps1 -Publish           # ...then commit and push it
#   powershell -ExecutionPolicy Bypass -File docs/build.ps1 -SkipRustdoc       # no cargo doc (faster)
#   powershell -ExecutionPolicy Bypass -File docs/build.ps1 -SkipRustdoc -Serve  # preview at http://localhost:3000
#   ... -DocsRepo D:\docs.chaps.dev                                            # another checkout of the docs repo

param(
    [switch]$SkipRustdoc,
    [switch]$Publish,
    [switch]$Serve,
    [string]$DocsRepo
)

$flags = @()
if ($SkipRustdoc) { $flags += '--skip-rustdoc' }
if ($Publish) { $flags += '--publish' }
if ($Serve) { $flags += '--serve' }
if ($DocsRepo) { $flags += @('--docs-repo', $DocsRepo) }

& (Join-Path $PSScriptRoot '..\scripts\sh.cmd') (Join-Path $PSScriptRoot 'publish.sh') @flags
exit $LASTEXITCODE
