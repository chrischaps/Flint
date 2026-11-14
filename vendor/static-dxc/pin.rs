// The archive this crate links (#947), shared by build.rs and src/lib.rs.
// tools/dxc-static/build.ps1 builds it and .github/workflows/dxc-static.yml
// publishes it as a release asset of chrischaps/Flint. A new DXC release
// changes all of these together.

/// The DXC release the archive is built from.
pub const DXC_VERSION: &str = "v1.9.2609";
/// Its commit in microsoft/DirectXShaderCompiler.
pub const DXC_COMMIT: &str = "01b62ad47db7dee3dd33f90e7339cf9e860b3f93";
/// The chrischaps/Flint release that carries the archive.
pub const RELEASE_TAG: &str = "dxc-static-v1.9.2609-1";
/// The archive's file name in that release.
pub const ARCHIVE_NAME: &str = "dxc-static-v1.9.2609-1-x86_64-windows-msvc.tar.gz";
/// The archive's SHA-256, checked whichever way it arrives.
pub const ARCHIVE_SHA256: &str = "a410cc8140cfa24e4d8e45f5523aba6c27c7b7e96aa557d19452f2b3f3fdd4a2";
/// `_MSC_VER` of the MSVC that built it: a link needs this one or newer.
pub const BUILT_WITH_MSC_VER: u32 = 1944;
