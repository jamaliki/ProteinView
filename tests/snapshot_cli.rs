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

const TWO_PALETTES: &str = r#"
[structure]
helix = "FF0000"

[[palette]]
name = "ocean"
[palette.structure]
helix = "0000FF"

[[palette]]
name = "amber"
[palette.structure]
helix = "FFAA00"
"#;

#[test]
fn a_named_palette_can_be_chosen_from_the_command_line() {
    // Three palettes in one file, three different pictures.  Snapshots cannot
    // press `p`, so this is how a named palette reaches a rendered frame.
    let base = render_with(TWO_PALETTES, &[]);
    let ocean = render_with(TWO_PALETTES, &["--palette-name", "ocean"]);
    let amber = render_with(TWO_PALETTES, &["--palette-name", "amber"]);

    assert!(
        base != ocean,
        "`--palette-name ocean` should change the colors"
    );
    assert!(
        ocean != amber,
        "each named palette should render differently"
    );
}

#[test]
fn the_config_can_pick_which_palette_to_start_on() {
    let starting_on_ocean = format!("{TWO_PALETTES}\n[defaults]\npalette = \"ocean\"\n");
    assert_eq!(
        render_with(&starting_on_ocean, &[]),
        render_with(TWO_PALETTES, &["--palette-name", "ocean"]),
        "`defaults.palette` should select the same palette the flag does"
    );
    assert!(
        render_with(&starting_on_ocean, &["--palette-name", "amber"])
            != render_with(&starting_on_ocean, &[]),
        "--palette-name should overrule `defaults.palette`"
    );
}

#[test]
fn an_unknown_palette_name_stops_the_run_and_lists_the_real_ones() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, TWO_PALETTES).unwrap();
    let fixture = format!("{}/examples/1UBQ.pdb", env!("CARGO_MANIFEST_DIR"));

    let result = Command::new(env!("CARGO_BIN_EXE_proteinview"))
        .args([
            &fixture,
            "--config",
            config_path.to_str().unwrap(),
            "--palette-name",
            "oecan",
            "--snapshot",
            dir.path().join("out.png").to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert!(
        !result.status.success(),
        "an unknown palette should not render"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("oecan") && stderr.contains("ocean"),
        "the error should quote the typo and list what exists: {stderr}"
    );
}

#[test]
fn the_example_config_is_exactly_the_built_in_defaults() {
    // docs/config.example.toml tells the reader that every value in it is the
    // built-in default, so copying it wholesale changes nothing.  That claim is
    // the reason the file is safe to start from, and it rots silently -- one
    // retuned constant and the file is quietly lying.
    let dir = tempdir().unwrap();
    let fixture = format!("{}/examples/4HHB.pdb", env!("CARGO_MANIFEST_DIR"));
    let example = format!("{}/docs/config.example.toml", env!("CARGO_MANIFEST_DIR"));

    let render = |args: &[&str]| {
        let out = dir.path().join(format!("{}.png", args.len()));
        let mut full = vec![
            fixture.as_str(),
            "--snapshot",
            out.to_str().unwrap(),
            "--snapshot-width",
            "240",
            "--snapshot-height",
            "180",
        ];
        full.extend_from_slice(args);
        let result = Command::new(env!("CARGO_BIN_EXE_proteinview"))
            .args(&full)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "render failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        image::open(&out).unwrap().to_rgba8()
    };

    assert_eq!(
        render(&["--config", example.as_str()]),
        render(&[]),
        "docs/config.example.toml should render identically to no config at all"
    );
}

#[test]
fn a_configured_background_paints_empty_space_opaque() {
    // Without one, empty space stays transparent so the terminal shows through
    // -- which leaves a snapshot PNG with an alpha channel that a figure rarely
    // wants.  With one, it is painted and opaque.
    let transparent = render_with("", &[]);
    let painted = render_with("[background]\ncolor = \"1a1b26\"\n", &[]);

    let corner = transparent.get_pixel(0, 0);
    assert_eq!(
        corner[3], 0,
        "empty space should stay transparent by default"
    );

    let corner = painted.get_pixel(0, 0);
    assert_eq!(
        [corner[0], corner[1], corner[2], corner[3]],
        [0x1A, 0x1B, 0x26, 255],
        "a configured background should paint empty space at full alpha"
    );

    // The structure itself must still be drawn, not flooded over.
    assert!(
        painted
            .pixels()
            .any(|p| [p[0], p[1], p[2]] != [0x1A, 0x1B, 0x26]),
        "the structure should still be visible against its background"
    );
}

#[test]
fn a_rainbow_ramp_replaces_the_built_in_sweep() {
    let stock = render_with("", &["--color", "rainbow"]);
    let ramped = render_with(
        r#"
[rainbow]
colors = ["A3E8C7", "8EDBD8", "8FCBF3", "A5B4F5", "C5A9F0", "E5A3E0", "F5A7C0", "F7C9A0"]
"#,
        &["--color", "rainbow"],
    );
    assert!(
        stock != ramped,
        "`rainbow.colors` should change what is drawn"
    );

    // And it only touches Rainbow: the other schemes are unaffected.
    assert_eq!(
        render_with(
            "[rainbow]\ncolors = [\"A3E8C7\", \"F7C9A0\"]\n",
            &["--color", "chain"]
        ),
        render_with("", &["--color", "chain"]),
        "a rainbow ramp should not change the Chain scheme"
    );
}
