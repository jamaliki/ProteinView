use std::process::{Command, Stdio};

use image::GenericImageView;
use tempfile::tempdir;

#[test]
fn snapshot_cli_is_terminal_independent_and_writes_exact_dimensions() {
    let output_dir = tempdir().unwrap();
    let output_path = output_dir.path().join("1ubq-fullhd.png");
    let fixture = format!("{}/examples/1UBQ.pdb", env!("CARGO_MANIFEST_DIR"));

    let result = Command::new(env!("CARGO_BIN_EXE_proteinview"))
        .args([
            &fixture,
            "--snapshot",
            output_path.to_str().unwrap(),
            "--snapshot-width",
            "320",
            "--snapshot-height",
            "180",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert!(
        result.status.success(),
        "snapshot failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!result.stdout.contains(&0x1b));
    assert!(!result.stderr.contains(&0x1b));

    let image = image::open(&output_path).unwrap();
    assert_eq!(image.dimensions(), (320, 180));
    assert!(image.to_rgba8().pixels().any(|pixel| pixel[3] > 0));
}

#[test]
fn snapshot_cli_accepts_repeatable_exact_residue_colors() {
    let output_dir = tempdir().unwrap();
    let output_path = output_dir.path().join("1ubq-residues.png");
    let fixture = format!("{}/examples/1UBQ.pdb", env!("CARGO_MANIFEST_DIR"));

    let result = Command::new(env!("CARGO_BIN_EXE_proteinview"))
        .args([
            &fixture,
            "--snapshot",
            output_path.to_str().unwrap(),
            "--snapshot-width",
            "320",
            "--snapshot-height",
            "180",
            "--mode",
            "backbone",
            "--residue-color",
            "A:1=FF0000",
            "--residue-color",
            "A:2=00FF00",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert!(
        result.status.success(),
        "snapshot failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(image::open(output_path).unwrap().dimensions(), (320, 180));
}

#[test]
fn snapshot_cli_rejects_unknown_residue_before_writing_output() {
    let output_dir = tempdir().unwrap();
    let output_path = output_dir.path().join("unknown.png");
    let fixture = format!("{}/examples/1UBQ.pdb", env!("CARGO_MANIFEST_DIR"));

    let result = Command::new(env!("CARGO_BIN_EXE_proteinview"))
        .args([
            &fixture,
            "--snapshot",
            output_path.to_str().unwrap(),
            "--residue-color",
            "Z:999=FF0000",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert!(!output_path.exists());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("available chains"),
        "unexpected error: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

/// Render `1UBQ` with the given config file and extra flags, returning the
/// frame's pixels.
fn render_with(config: &str, extra: &[&str]) -> image::RgbaImage {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, config).unwrap();
    let output_path = dir.path().join("out.png");
    let fixture = format!("{}/examples/1UBQ.pdb", env!("CARGO_MANIFEST_DIR"));

    let mut args = vec![
        fixture.as_str(),
        "--config",
        config_path.to_str().unwrap(),
        "--snapshot",
        output_path.to_str().unwrap(),
        "--snapshot-width",
        "240",
        "--snapshot-height",
        "180",
    ];
    args.extend_from_slice(extra);

    let result = Command::new(env!("CARGO_BIN_EXE_proteinview"))
        .args(&args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "render failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    image::open(&output_path).unwrap().to_rgba8()
}

#[test]
fn a_config_file_sets_the_startup_mode_and_a_flag_still_beats_it() {
    // Same structure three ways: the built-in default, the file's choice, and
    // the file overruled by the flag.  The file has to change the picture, and
    // the flag has to change it back.
    let cartoon = render_with("", &[]);
    let from_config = render_with("[defaults]\nmode = \"wireframe\"\n", &[]);
    let flag_wins = render_with("[defaults]\nmode = \"wireframe\"\n", &["--mode", "cartoon"]);

    assert!(
        from_config != cartoon,
        "`defaults.mode` in the config should change what is drawn"
    );
    assert_eq!(
        flag_wins, cartoon,
        "--mode should overrule the config file, not merely coexist with it"
    );
}

#[test]
fn the_config_file_tunes_the_depth_fog() {
    let default_fog = render_with("", &[]);
    let no_fog = render_with("[fog]\nstrength = 0.0\n", &[]);
    assert!(
        no_fog != default_fog,
        "`fog.strength = 0.0` should visibly change the frame"
    );
}

#[test]
fn a_bad_config_stops_the_run_rather_than_being_ignored() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, "[fog]\nstrength = 4.0\n").unwrap();
    let fixture = format!("{}/examples/1UBQ.pdb", env!("CARGO_MANIFEST_DIR"));

    let result = Command::new(env!("CARGO_BIN_EXE_proteinview"))
        .args([
            &fixture,
            "--config",
            config_path.to_str().unwrap(),
            "--snapshot",
            dir.path().join("out.png").to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert!(!result.status.success(), "a bad config should not render");
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("fog.strength"),
        "the error should name the offending key: {stderr}"
    );
}

#[test]
fn the_old_palette_flag_still_works() {
    // `--palette` was the name before the file grew past colors.
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("palette.toml");
    std::fs::write(&config_path, "[structure]\nhelix = \"00FFFF\"\n").unwrap();
    let output_path = dir.path().join("out.png");
    let fixture = format!("{}/examples/1UBQ.pdb", env!("CARGO_MANIFEST_DIR"));

    let result = Command::new(env!("CARGO_BIN_EXE_proteinview"))
        .args([
            &fixture,
            "--palette",
            config_path.to_str().unwrap(),
            "--snapshot",
            output_path.to_str().unwrap(),
            "--snapshot-width",
            "240",
            "--snapshot-height",
            "180",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert!(
        result.status.success(),
        "--palette should still be accepted: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output_path.is_file());
}
