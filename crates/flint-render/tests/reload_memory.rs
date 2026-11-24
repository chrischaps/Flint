//! A hot reload frees what the install before it held (#742): live heap
//! bytes while one user style's `.shader.toml` is saved and reloaded 500
//! times stop growing after the first 100. Before #742 each reload
//! leaked the descriptor, its lists and every string in them, and the
//! bind-cache names: 437 KB after 500 reloads of this style, about 850 B
//! each (ADR 0129, Measured). Its own binary,
//! since the counting allocator is the process's.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicI64, Ordering};

struct Counting;
static LIVE: AtomicI64 = AtomicI64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        LIVE.fetch_add(l.size() as i64, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size() as i64, Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static A: Counting = Counting;

#[test]
fn reload_memory() {
    let dir = std::env::temp_dir().join(format!("flint-reload-mem-{}", std::process::id()));
    let styles = dir.join("styles");
    std::fs::create_dir_all(&styles).unwrap();
    let toml = styles.join("memstyle.shader.toml");
    let write = |save: u32| {
        std::fs::write(
            &toml,
            format!(
                "version = 1\nname = \"memstyle\"\nlabel = \"Memory\"\nshader = \"mem.wgsl\"\n\
                 [params.amount]\ndefault = {}\nmin = 0\nmax = 1\nlabel = \"Amount\"\nhelp = \"How much\"\n\
                 [options.mode]\nkind = \"choice\"\ndefault = \"a\"\nchoices = [\"a\", \"b\", \"c\"]\n",
                (save % 2) as f32 * 0.5
            ),
        )
        .unwrap();
        let f = std::fs::File::options().write(true).open(&toml).unwrap();
        f.set_modified(
            std::time::SystemTime::now() + std::time::Duration::from_secs(u64::from(save) + 1),
        )
        .unwrap();
    };
    std::fs::write(
        styles.join("mem.wgsl"),
        "fn style(p: StylePixel) -> vec4<f32> {\n    return p.color * params.amount;\n}\n",
    )
    .unwrap();
    write(0);
    reload_loop(&dir, write);
    let _ = std::fs::remove_dir_all(&dir);
}

#[allow(unused)]
fn live() -> i64 {
    LIVE.load(Ordering::Relaxed)
}

// The registry is this test's own value
fn reload_loop(dir: &std::path::Path, write: impl Fn(u32)) {
    let mut styles = flint_render::stylize::StyleRegistry::new();
    assert!(styles.load_dir(dir).is_empty());
    // The first reload makes what every later one reuses (the interned
    // names); measure from there
    write(1);
    assert_eq!(styles.reload_changed(dir).replaced.len(), 1);
    let base = live();
    println!("reloads,live_bytes_over_first_reload");
    let (mut at_100, mut last) = (0, 0);
    for save in 2..=501u32 {
        write(save);
        let r = styles.reload_changed(dir);
        assert_eq!(r.replaced.len(), 1, "save {save}: {r:?}");
        if (save - 1) % 25 == 0 {
            last = live() - base;
            println!("{},{last}", save - 1);
        }
        if save - 1 == 100 {
            at_100 = live() - base;
        }
    }
    // A reload costs nothing that stays: until #1271 a replaced install's
    // bind names went into a process-wide log (#427), four a reload, which
    // grew to its cap of 256 names by the 64th; a renderer now asks its own
    // registry which installs are gone, and there is no log
    assert!(
        at_100 < 32 * 1024,
        "the first 100 reloads left {at_100} bytes live"
    );
    assert!(
        (last - at_100).abs() < 1024,
        "reloads 100 to 500 left {} more bytes live",
        last - at_100
    );
}
