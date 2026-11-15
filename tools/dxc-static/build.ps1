<#
.SYNOPSIS
  Build DXC as one static library from Microsoft's source, and pack it (#947).

.DESCRIPTION
  Flint links DX12's shader compiler into its binaries (wgpu's `static-dxc`,
  through vendor/static-dxc). Microsoft ships DXC only as a DLL, so this
  builds the pinned release from microsoft/DirectXShaderCompiler with one
  change, dxcompiler-static.patch (`SHARED` -> `STATIC` on the dxcompiler
  target), and merges the libraries it links into one `.lib`.

  The archive holds:
    flint_dxcompiler.lib   every static library dxcompiler links, merged
    LICENSE.TXT            DXC's licence (LLVM Release License, UIUC/NCSA)
    ThirdPartyNotices.txt  DXC's third-party notices
    BUILD-INFO.txt         source commit, patch hash, compiler, flags

  Release build, no debug info, the dynamic CRT (/MD, what Rust's MSVC
  target links by default). Needs Visual Studio's C++ tools (MSVC, CMake,
  Ninja, the DIA SDK), git and Python, all on GitHub's windows-latest.
  .github/workflows/dxc-static.yml runs it; it runs the same way locally.

.EXAMPLE
  powershell -File tools/dxc-static/build.ps1 -Work C:\dxc-work
#>
param(
    # Folder for the DXC checkout, its build and the archive
    [Parameter(Mandatory = $true)][string]$Work,
    # Parallel compile jobs (default: the CPU count)
    [int]$Jobs = 0
)

$ErrorActionPreference = 'Stop'

# The pin. A new DXC release changes these three and the archive's -<n>.
$DxcTag = 'v1.9.2609'
$DxcCommit = '01b62ad47db7dee3dd33f90e7339cf9e860b3f93'
$ArchiveName = "dxc-static-$DxcTag-1-x86_64-windows-msvc.tar.gz"

$Here = Split-Path -Parent $MyInvocation.MyCommand.Path
$Patch = Join-Path $Here 'dxcompiler-static.patch'
$Src = Join-Path $Work 'src'
$Build = Join-Path $Work 'build'
$Stage = Join-Path $Work 'stage'
$Out = Join-Path $Work 'out'
if ($Jobs -le 0) { $Jobs = [Environment]::ProcessorCount }

function Step($what) { Write-Host "== $what ($(Get-Date -Format HH:mm:ss))" }
# A native command, its stderr shown but not an error (Windows PowerShell
# turns redirected stderr lines into errors); its exit code decides
function Run($exe) {
    $ErrorActionPreference = 'Continue'
    & $exe @args 2>&1 | ForEach-Object { "$_" } | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "$exe $($args -join ' ') failed ($LASTEXITCODE)" }
}

# The x64 developer environment, from the newest Visual Studio with C++
Step 'Visual Studio x64 environment'
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw 'no Visual Studio with the C++ x64 tools' }
$vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat'
cmd /c "`"$vcvars`" >nul 2>&1 && set" | ForEach-Object {
    if ($_ -match '^([^=]+)=(.*)$') { Set-Item "env:$($Matches[1])" $Matches[2] }
}
New-Item -ItemType Directory -Force $Work | Out-Null
$probe = Join-Path $Work 'msc_ver.c'
Set-Content -Encoding ascii $probe '_MSC_VER _MSC_FULL_VER'
$msc = (cmd /c "cl /nologo /EP `"$probe`" 2>nul" | Where-Object { $_.Trim() } | Select-Object -Last 1).Trim()
Write-Host "cl: _MSC_VER _MSC_FULL_VER = $msc; tools $env:VCToolsVersion"

Step "DXC $DxcTag"
if (-not (Test-Path (Join-Path $Src '.git'))) {
    # LF line endings, so the patch applies the same on any machine
    Run git -c core.autocrlf=false clone --quiet --depth 1 --branch $DxcTag `
        --recurse-submodules --shallow-submodules `
        https://github.com/microsoft/DirectXShaderCompiler.git $Src
}
$head = (git -C $Src rev-parse HEAD).Trim()
if ($head -ne $DxcCommit) { throw "$DxcTag is $head, not the pinned $DxcCommit" }

Step 'patch'
cmd /c "git -C `"$Src`" apply --check --reverse `"$Patch`" 2>nul"
if ($LASTEXITCODE -ne 0) { Run git -C $Src apply $Patch } else { Write-Host 'already applied' }

Step 'configure'
# CMAKE_MSVC_DEBUG_INFORMATION_FORMAT set turns off DXC's own `/Zi` for
# Release (cmake/modules/HandleLLVMOptions.cmake); with DXC's CMake minimum
# (3.17) CMake doesn't act on the value, so nothing adds debug info.
# The check after the build makes sure.
# DXC_API_IMPORT empty: DXC's headers declare its API `dllimport` for every
# file but dxcapi.cpp, so its own calls go through `__imp_DxcCreateInstance`.
# Linked into an exe whose `windows` crate also imports dxcompiler.dll
# (wgpu-hal's), the linker takes that import and DXC's definition both
# (LNK2005). Empty, they are plain calls to the function linked in.
Run cmake -S $Src -B $Build -G Ninja `
    -C (Join-Path $Src 'cmake\caches\PredefinedParams.cmake') `
    -DCMAKE_BUILD_TYPE=Release `
    "-DCMAKE_CXX_FLAGS=/DDXC_API_IMPORT=" `
    -DCMAKE_MSVC_DEBUG_INFORMATION_FORMAT=none `
    -DLLVM_APPEND_VC_REV=OFF `
    -DHLSL_INCLUDE_TESTS=OFF -DSPIRV_BUILD_TESTS=OFF -DENABLE_SPIRV_CODEGEN=OFF `
    -DLLVM_INCLUDE_TESTS=OFF -DCLANG_INCLUDE_TESTS=OFF

$ninja = Get-Content (Join-Path $Build 'build.ninja') -Raw
if ($ninja -match '\s/Z[i7I]\s') { throw 'the build would add debug info (/Zi or /Z7)' }
if ($ninja -notmatch '\s-MD\s|\s/MD\s') { throw 'the build is not on the dynamic CRT (/MD)' }

Step "build ($Jobs jobs)"
# A static target doesn't build its link components first; these two
# are the ones nothing else asks for
$sw = [Diagnostics.Stopwatch]::StartNew()
Run ninja -C $Build -j $Jobs dxcompiler LLVMPassPrinters LLVMDxilCompression
Write-Host "build took $([int]$sw.Elapsed.TotalSeconds) s"

Step 'merge and pack'
if (Test-Path $Stage) { Remove-Item -Recurse -Force $Stage }
New-Item -ItemType Directory -Force $Stage, $Out | Out-Null
$libs = Get-ChildItem (Join-Path $Build 'lib') -Filter *.lib | Sort-Object Name
Run lib /nologo /ignore:4006 /ignore:4221 "/OUT:$(Join-Path $Stage 'flint_dxcompiler.lib')" @($libs.FullName)
Copy-Item (Join-Path $Src 'LICENSE.TXT'), (Join-Path $Src 'ThirdPartyNotices.txt') $Stage
$patchHash = (Get-FileHash $Patch -Algorithm SHA256).Hash.ToLower()
@(
    "DXC $DxcTag, built as one static library for Flint (#947)"
    "source: https://github.com/microsoft/DirectXShaderCompiler/tree/$DxcCommit"
    "patch: tools/dxc-static/dxcompiler-static.patch, sha256 $patchHash"
    "compiler: MSVC $env:VCToolsVersion (_MSC_VER _MSC_FULL_VER = $msc)"
    'build: CMake Release, Ninja, no debug info, /MD, SPIR-V codegen off'
    "libraries merged: $($libs.Count) ($(($libs.Name) -join ' '))"
    'licence: LICENSE.TXT (LLVM Release License, UIUC/NCSA) and ThirdPartyNotices.txt'
) | Set-Content -Encoding ascii (Join-Path $Stage 'BUILD-INFO.txt')

$archive = Join-Path $Out $ArchiveName
if (Test-Path $archive) { Remove-Item $archive }
# Windows' own bsdtar: Git's GNU tar reads C: as a remote host
Run "$env:SystemRoot\System32\tar.exe" -czf $archive -C $Stage flint_dxcompiler.lib LICENSE.TXT ThirdPartyNotices.txt BUILD-INFO.txt
$hash = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLower()
$size = (Get-Item $archive).Length
$libSize = (Get-Item (Join-Path $Stage 'flint_dxcompiler.lib')).Length
Write-Host "archive: $archive"
Write-Host "sha256: $hash"
Write-Host "size: $size bytes (flint_dxcompiler.lib $libSize bytes)"
"$hash  $ArchiveName" | Set-Content -Encoding ascii "$archive.sha256"
Copy-Item (Join-Path $Stage 'BUILD-INFO.txt') $Out
