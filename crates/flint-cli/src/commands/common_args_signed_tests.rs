//! A coordinate or an angle may start with a minus (#1700).
//!
//! clap reads a value that starts with `-` as the next flag unless its
//! argument says otherwise, so `flint render x --target -60,2,120` was
//! refused (`unexpected argument '-6' found`) and only `--target=-60,2,120`
//! worked; `--yaw -45` and `--pitch -10` were refused the same way. A target
//! on a centred terrain is as often left of the origin as right of it.

use clap::Parser;
use std::any::TypeId;

/// Scalar flags whose value is as often negative as not: the camera's
/// angles, and `--orbit`'s turn the other way.
const SIGNED: &[&str] = &["yaw", "pitch", "orbit"];

/// The value types of a vector flag: `X,Y`, `X,Y,Z`, `R,G,B`, `X,Y,Z,W`
/// and flint-player's window `--position`.
fn takes_a_vector(arg: &clap::Arg) -> bool {
    let id = arg.get_value_parser().type_id();
    [
        TypeId::of::<[f32; 2]>(),
        TypeId::of::<[f32; 3]>(),
        TypeId::of::<[f32; 4]>(),
        TypeId::of::<(i32, i32)>(),
    ]
    .into_iter()
    .any(|t| id == t)
}

/// Every vector and signed flag under `cmd`, as `flint render --target`,
/// with whether a leading minus reaches its value. A vector's value is not
/// a number to clap (`-60,2,120`), so it needs `allow_hyphen_values`; a
/// signed scalar is one, so `allow_negative_numbers` will do.
fn walk(cmd: &clap::Command, path: &str, out: &mut Vec<(String, bool)>) {
    for arg in cmd.get_arguments() {
        let Some(long) = arg.get_long() else { continue };
        if !arg.get_action().takes_values() {
            continue;
        }
        let vector = takes_a_vector(arg);
        if !vector && !SIGNED.contains(&long) {
            continue;
        }
        let takes =
            arg.is_allow_hyphen_values_set() || (!vector && arg.is_allow_negative_numbers_set());
        out.push((format!("{path} --{long}"), takes));
    }
    for sub in cmd.get_subcommands() {
        walk(sub, &format!("{path} {}", sub.get_name()), out);
    }
}

/// On a thread with room: a debug clap parser needs more stack than a
/// test thread's 2 MB on Windows (root CLAUDE.md).
fn on_a_big_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
}

#[test]
fn every_vector_and_angle_flag_takes_a_leading_minus() {
    on_a_big_stack(|| {
        let mut found = Vec::new();
        walk(&crate::stock_command(), "flint", &mut found);
        let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
        // The walk sees the flags the issue named, or it proves nothing.
        for known in [
            "flint render --target",
            "flint render --sheen-color",
            "flint render --yaw",
            "flint render --pitch",
            "flint edit --target",
            "flint preview --target",
            "flint play --position",
        ] {
            assert!(names.contains(&known), "the walk missed {known}: {names:?}");
        }
        let refused: Vec<&str> = found
            .iter()
            .filter(|(_, takes)| !takes)
            .map(|(n, _)| n.as_str())
            .collect();
        assert!(
            refused.is_empty(),
            "these flags refuse a value that starts with a minus (#1700): {refused:?}. \
             Give a vector `allow_hyphen_values = true`, a signed number \
             `allow_negative_numbers = true`"
        );
    });
}

#[test]
fn render_takes_a_target_left_of_the_origin() {
    on_a_big_stack(|| {
        let cli = crate::Cli::try_parse_from([
            "flint",
            "render",
            "relief.scene.toml",
            "--target",
            "-60,2,120",
            "--yaw",
            "-45",
            "--pitch",
            "-10",
            "--sheen-color",
            "-0.5,0,1",
            "--no-grid",
        ])
        .unwrap_or_else(|e| panic!("{e}"));
        let crate::Commands::Render(args) = cli.command else {
            panic!("not a render");
        };
        assert_eq!(args.camera.target, Some([-60.0, 2.0, 120.0]));
        assert_eq!(args.camera.yaw, Some(-45.0));
        assert_eq!(args.camera.pitch, Some(-10.0));
        assert_eq!(args.shading.sheen_color, Some([-0.5, 0.0, 1.0]));
        // The flag after a negative value is still a flag.
        assert!(args.camera.no_grid);
    });
}

#[test]
fn the_editors_take_a_target_left_of_the_origin() {
    on_a_big_stack(|| {
        let line = |words: &[&str]| {
            crate::Cli::try_parse_from(["flint"].iter().chain(words))
                .unwrap_or_else(|e| panic!("{words:?}: {e}"))
                .command
        };
        let crate::Commands::Edit(edit) = line(&[
            "edit",
            "relief.scene.toml",
            "--target",
            "-60,2,120",
            "--yaw",
            "-45",
        ]) else {
            panic!("not an edit");
        };
        assert_eq!(edit.target, Some([-60.0, 2.0, 120.0]));
        assert_eq!(edit.yaw, Some(-45.0));
        let crate::Commands::Preview(preview) =
            line(&["preview", "m.glb", "--target", "-1,0,-1", "--pitch", "-5"])
        else {
            panic!("not a preview");
        };
        assert_eq!(preview.target, Some([-1.0, 0.0, -1.0]));
        assert_eq!(preview.pitch, Some(-5.0));
    });
}

/// A missing value still says so: `--target --no-grid` takes `--no-grid`
/// as its value now, and parse_vec3 names it as the flag it is.
#[test]
fn a_flag_where_the_target_should_be_is_still_an_error() {
    on_a_big_stack(|| {
        let e = match crate::Cli::try_parse_from([
            "flint",
            "render",
            "s.scene.toml",
            "--target",
            "--no-grid",
        ]) {
            Ok(_) => panic!("--target --no-grid parsed"),
            Err(e) => e.to_string(),
        };
        assert!(e.contains("--target"), "{e}");
        assert!(
            e.contains("a value is required, not the flag '--no-grid'"),
            "{e}"
        );
    });
}

#[test]
fn a_negative_coordinate_is_not_taken_for_a_flag() {
    assert_eq!(super::parse_vec3("-60,2,120"), Ok([-60.0, 2.0, 120.0]));
    assert_eq!(super::parse_vec3("-.5,0,0"), Ok([-0.5, 0.0, 0.0]));
    assert!(super::parse_vec3("-x,0,0")
        .unwrap_err()
        .contains("not the flag"));
    assert!(super::parse_vec4("--set")
        .unwrap_err()
        .contains("not the flag"));
}
