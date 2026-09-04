<p align="center">
  <b>P R O T E I N V I E W</b>
</p>

<p align="center">
  <em>Explore molecular structures in your terminal</em>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="License: MIT"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Rust-1.85%2B-orange.svg" alt="Rust"></a>
  <img src="https://img.shields.io/badge/version-0.3.0-green.svg" alt="Version">
  <a href="https://github.com/001TMF/ProteinView/pulls"><img src="https://img.shields.io/badge/PRs-welcome-brightgreen.svg" alt="PRs Welcome"></a>
  <a href="https://www.linkedin.com/in/tristan-farmer-973b7a17a/"><img src="https://img.shields.io/badge/LinkedIn-Tristan%20Farmer-0A66C2?logo=linkedin" alt="LinkedIn"></a>
</p>

<p align="center">
  <img src="assets/hero-histone.png" alt="Nucleosome core particle with histone proteins and DNA rendered in FullHD mode" width="700">
</p>

<p align="center">
  <sub>Nucleosome core particle — histone octamer wrapped in DNA, rendered with Kitty graphics protocol</sub>
</p>

---

Terminal molecular structure viewer — load, rotate, and explore proteins, nucleic acids, and small molecules from PDB/CIF files right in your terminal. No browser, no GUI, no dependencies.

## Features

- **3-tier render modes** — Braille, HD, and FullHD (Sixel/Kitty) with automatic SSH detection
- **PNG-compressed Kitty protocol** — ~10-20x smaller than raw RGBA, making FullHD viable over SSH
- **Cartoon ribbon visualization** — Lambert-shaded ribbons with depth fog for helices, sheets, and coils
- **RNA/DNA support** — backbone, wireframe, and cartoon modes with base-type coloring
- **Small molecule rendering** — ligands as ball-and-stick, ions as spheres
- **Interface analysis** — inter-chain contacts, binding pockets, and interaction visualization (H-bonds, salt bridges, hydrophobic contacts)
- **Sequence panel** — scroll every chain's sequence, select residues, and show them as ball-and-stick in the 3D view
- **7 color schemes** — structure, chain, element (CPK), B-factor, rainbow, pLDDT (AlphaFold)
- **Interactive controls** — vim-style rotation, zoom, pan with auto-rotation
- **PDB & mmCIF** — both formats supported, with RCSB PDB fetch (`--fetch`)
- **Directory browser** — open a folder, choose structures from a persistent file panel, and keep FullHD interactive
- **Headless FullHD export** — render a pixel-perfect PNG for agents and scripts without starting a nested TUI
- **Single static binary** — zero runtime dependencies

## Render Modes

Three rendering tiers to match your terminal and connection:

<p align="center">
  <img src="assets/render-modes-grid.png" alt="Braille vs HD vs FullHD rendering comparison" width="700">
</p>

<p align="center">
  <sub>Left: Braille (works everywhere, including SSH/tmux) · Middle: HD (Lambert-shaded braille) · Right: FullHD (Kitty pixel graphics)</sub>
</p>

| Mode | Key | Quality | SSH Performance |
|------|-----|---------|-----------------|
| **Braille** | default | Text-based, monochrome per cell | Excellent |
| **HD** | `m` | Shaded braille with lighting + depth fog | Excellent |
| **FullHD** | `M` | Sixel/Kitty pixel graphics | Good (PNG compressed) |

`--hd` is SSH-aware: defaults to HD over SSH, FullHD locally. Use `--fullhd` to force pixel graphics.

## Visualization Modes

<p align="center">
  <img src="assets/viz-modes-grid.png" alt="Cartoon, Wireframe, and Backbone visualization modes" width="700">
</p>

<p align="center">
  <sub>Left: Cartoon (ribbon) · Middle: Wireframe (all-atom) · Right: Backbone (CA trace)</sub>
</p>

| Mode | Description |
|------|-------------|
| **Cartoon** | Smooth ribbon rendering — helices, beta-sheets, and coils with Lambert shading. Default. |
| **Wireframe** | All-atom bonds including inter-residue peptide and phosphodiester linkages. |
| **Backbone** | CA trace (proteins) / C4' trace (nucleic acids) with spheres and thick connecting lines. |

## Interface Analysis & Interactions

<p align="center">
  <img src="assets/interface-grid.png" alt="Interface analysis with interaction visualization" width="700">
</p>

<p align="center">
  <sub>Left: Interface residue coloring with sidebar panel · Right: Dashed interaction lines (H-bonds, salt bridges, hydrophobic contacts)</sub>
</p>

Press `f` to toggle interface mode — highlights contact residues across chain boundaries with a detailed sidebar. Press `I` to overlay interaction lines:

| Color | Interaction | Distance |
|-------|-------------|----------|
| Cyan | Hydrogen bond | &le; 3.5 &Aring; |
| Red | Salt bridge | &le; 4.0 &Aring; |
| Yellow | Hydrophobic contact | &le; 4.5 &Aring; |
| Gray | Other | &le; 4.5 &Aring; |

## Nucleic Acids

<p align="center">
  <img src="assets/dna-element.png" alt="B-DNA double helix with element (CPK) coloring" width="500">
</p>

<p align="center">
  <sub>B-DNA dodecamer in wireframe mode with CPK element coloring</sub>
</p>

Full support for DNA and RNA structures — backbone traces, wireframe bonds, and cartoon ribbons with nucleotide base-type coloring (A=red, U/T=blue, G=green, C=yellow).

## AlphaFold & pLDDT

<p align="center">
  <img src="assets/plddt-cartoon.png" alt="AlphaFold prediction with pLDDT confidence coloring" width="500">
</p>

<p align="center">
  <sub>AlphaFold prediction with pLDDT confidence coloring — blue (high confidence) to orange/yellow (low confidence)</sub>
</p>

Automatically detects AlphaFold structures and offers pLDDT confidence coloring. Cycle through color schemes with `c`.

## Installation

Requires [Rust 1.85+](https://www.rust-lang.org/tools/install). If you don't have Rust, install it with:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Then install proteinview:

```bash
git clone https://github.com/001TMF/ProteinView.git
cd ProteinView

# Basic install
cargo install --path .

# With RCSB PDB fetch support
cargo install --path . --features fetch

# Update an existing installation
cargo install --path . --force
```

## Quick Start

```bash
# View a local PDB file
proteinview examples/1AOI.pdb

# Browse every supported structure under a directory
proteinview examples --fullhd

# HD mode (fast text-based shading)
proteinview examples/4HHB.pdb --hd

# FullHD pixel mode (Kitty/Sixel terminals)
proteinview examples/4HHB.pdb --fullhd

# Headless FullHD pixel snapshot (no alternate screen or terminal probing)
proteinview examples/4HHB.pdb --snapshot 4HHB.png

# Fetch from RCSB and export a 1200x800 FullHD frame for inline display
proteinview --fetch 1UBQ --snapshot 1UBQ.png \
  --snapshot-width 1200 --snapshot-height 800

# Fetch from RCSB PDB
proteinview --fetch 1UBQ

# Choose color scheme and visualization
proteinview examples/1UBQ.pdb --color rainbow --mode wireframe

# FullHD biological interface view focused on chain A
proteinview examples/4HHB.pdb --snapshot interface.png \
  --snapshot-interface-chain A --snapshot-interactions

# Hide ligands and ions in a snapshot
proteinview examples/4HHB.pdb --snapshot polymer-only.png \
  --snapshot-hide-ligands

# Color exact residues (blank insertion code A:42 and insertion-coded A:42[A])
proteinview examples/4HHB.pdb --snapshot selected.png \
  --residue-color A:42=FF0000 \
  --residue-color 'A:42[A]=00FFFF'
```

`--snapshot` always uses the software pixel renderer behind **FullHD** and
writes a PNG before any raw-mode, alternate-screen, or graphics-protocol
setup. It is therefore safe to call from another terminal application.
This is distinct from **HD**, which is the shaded text-cell renderer.
Snapshot dimensions default to 1920×1080 and are capped at 4096 pixels per
side and 8,388,608 total pixels.
Snapshots can also focus one chain's interface, overlay classified
inter-chain interaction lines, retain or hide ligands, and use any regular
ProteinView color or visualization mode. These controls make the headless
renderer suitable for iterative, conversational analysis in an agent chat.
Interface highlighting uses its own green/orange focus/partner palette rather
than a regular color scheme. Exact residue colors identify polymer residues by
case-sensitive chain ID, signed sequence number, and optional insertion code.
Omitting the insertion code selects only the blank code, not every residue with
that number. Overrides use strict uppercase `RRGGBB` and take precedence over
regular, element, pLDDT, and interface colors.

For a live agent-owned panel, start the persistent headless server with one
private PNG path:

```bash
proteinview examples/1UBQ.pdb --panel-server \
  --output /tmp/proteinview-live.png \
  --panel-width 960 --panel-height 540
```

The server renders the initial frame, then writes a `ready` JSON object to
stdout. It accepts one NDJSON command per stdin line, for example
`{"id":1,"command":"rotate","axis":"y","delta":0.1}` or
`{"id":2,"command":"resize","width":1200,"height":800}`. Successful state
changes atomically replace the same PNG before their response is emitted.
Responses include the request ID, monotonic revision, camera and presentation
state, and exact frame path and dimensions. Use `get_state` without rendering
or `shutdown` to acknowledge and exit. Diagnostics remain on stderr, and the
server never emits terminal graphics escapes. Protocol requests and responses
are each capped at 64 KiB; display-only structure names are sanitized and
bounded, while structures whose required chain metadata cannot fit are rejected
before the initial frame is rendered.

An agent can replace all exact residue colors atomically with:

```json
{"id":3,"command":"set_residue_colors","residues":[{"chain":"A","residue_number":42,"color":"FF0000"},{"chain":"B","residue_number":101,"insertion_code":"A","color":"00FFFF"}]}
```

An empty `residues` array clears the overrides. Invalid or duplicate targets
leave the prior frame, state, and revision unchanged.

Named palettes are reachable too, so a panel is not stuck on whatever it started
with:

```json
{"id":4,"command":"set_palette","name":"aurora"}
{"id":5,"command":"cycle_palette"}
{"id":6,"command":"cycle_palette","direction":"prev"}
```

`get_state` reports the active `palette` and the `palettes` available, so an
agent can discover them rather than guessing. Naming one that does not exist is
an `invalid_params` error listing the real names, and leaves the frame
unchanged.

## Keybindings

Pass a directory instead of a file to start with the file browser open. It
searches that directory recursively for PDB, ENT, CIF, mmCIF, and XYZ files.
Use `j`/`k` or the arrow keys to choose a structure and `Enter` to load it;
focus then returns to the 3D viewer so its regular controls work immediately.
`Tab` moves focus between the browser and viewer, and `e` hides or reveals the
browser. Opening one file directly starts with the browser hidden, but `e`
reveals the other structures in that file's directory. The compact footer says
`EDITOR` when the file list owns input and `PROTEINVIEW` when the 3D view does;
press `?` in either mode for the complete keybinding reference.

| Key | Action |
|-----|--------|
| `h`/`l` | Rotate Y |
| `j`/`k` | Rotate X |
| `u`/`i` | Roll |
| `+`/`-` | Zoom |
| `w`/`a`/`s`/`d` | Pan |
| `r` | Reset view |
| `c` | Cycle color scheme |
| `p`/`P` | Cycle named palettes (see [Configuration](#configuration)) |
| `v` | Cycle viz mode |
| `m` | Braille / HD |
| `M` | HD / FullHD |
| `f` | Interface analysis |
| `I` | Interface interactions |
| `g` | Toggle ligands |
| `[`/`]` | Prev/next chain |
| `S` | Sequence panel |
| `b` | Ball-and-stick for the selection |
| `z` | Centre the view on the selection |
| `Space` | Auto-rotate |
| `e` | Show/hide the file browser |
| `Tab` | Focus file browser / 3D viewer |
| `?` | Help |
| `q` | Quit |

While the sequence panel is open it takes the arrow keys; `h`/`j`/`k`/`l` still
rotate the view, so you can turn the structure while picking residues.

| Key | Action in the sequence panel |
|-----|------------------------------|
| `←`/`→` | Move the cursor one residue (across chain ends) |
| `↑`/`↓` | Move one row |
| `Shift`+arrow | Extend the selection from the cursor |
| `PgUp`/`PgDn` | Move a screenful |
| `Home`/`End` | Start / end of the chain |
| `Enter` | Select or deselect the residue |
| `A` | Select or deselect the whole chain |
| `x` | Clear the selection |
| `[`/`]` | Jump to the previous / next chain |
| `<`/`>` | Shrink / grow the panel |
| `S` / `Esc` | Close the panel |

## Sequence Panel & Residue Selection

Press `S` to open a scrollable panel listing the sequence of every chain in
one-letter codes — amino acids and nucleotides alike, numbered in the gutter and
grouped in tens. Each chain gets a header with its type, length, and residue
range, so a 52-chain ribosome reads as one continuous list.

The cursor moves with the arrow keys; `Enter` picks a residue, `Shift`+arrow
extends a range, and `A` takes a whole chain. Picked residues are drawn in the
3D view as ball-and-stick over whatever mode is active, with the selection color
on carbons and CPK colors elsewhere, z-buffered so a side chain that really is
behind the structure stays behind it. `b` turns the ball-and-stick off, leaving a
marker sphere per residue; `z` centres the view on the selection, which is how
you find a handful of residues inside something the size of a ribosome. The
selection survives closing the panel, and the status bar keeps its count.

Letters are colored by the active color scheme, so the panel and the structure
read as one picture; the selection and cursor colors come from `[selection]` in
the config file.

## Color Schemes

| Scheme | Description |
|--------|-------------|
| **Structure** | Helix (red), sheet (yellow), coil (green). Default. |
| **Chain** | Distinct color per chain. |
| **Element** | CPK coloring (C, N, O, S, P, metals). |
| **B-factor** | Blue (rigid) to red (flexible). |
| **Rainbow** | N-terminus (blue) to C-terminus (red). |
| **pLDDT** | AlphaFold confidence (blue=high, orange=low). |

## Configuration

Colors, depth fog, and the modes ProteinView opens in all come from one TOML
file. ProteinView reads `~/.config/proteinview/config.toml` (or
`$XDG_CONFIG_HOME/proteinview/config.toml`) when it exists, and `--config <FILE>`
overrides that:

```bash
proteinview examples/1UBQ.pdb --config my-config.toml
```

Every key is optional — anything you leave out keeps its built-in default, so a
file this short is valid:

```toml
[structure]
helix = "#00FFFF"
```

Unknown keys are rejected rather than ignored, so a typo tells you rather than
silently doing nothing.

### Colors

Every fixed color ProteinView draws can be changed. Colors are six hex digits,
with or without a leading `#`, in either case. Element symbols merge onto the
built-in CPK table, so overriding carbon leaves the rest alone, while
`[chain] colors` replaces the chain cycle outright.

### The Rainbow ramp and the background

The Rainbow scheme runs a ramp along each chain, N-terminus to C-terminus.
`[rainbow] colors` replaces its built-in HSV sweep with stops of your own, spread
evenly and blended between. `[background] color` paints empty space, which
otherwise stays transparent so the terminal shows through — set it and snapshot
PNGs come out opaque, which is usually what a figure wants.

Both belong to a palette, so a named one can carry its own:

```toml
[[palette]]
name = "aurora"

[palette.background]
color = "#1a1b26"

[palette.rainbow]
colors = ["#A3E8C7", "#8EDBD8", "#8FCBF3", "#A5B4F5",
          "#C5A9F0", "#E5A3E0", "#F5A7C0", "#F7C9A0"]
```

```bash
proteinview examples/4HHB.pdb --palette-name aurora --color rainbow
```

Every chain gets the whole ramp, so in a multi-chain structure each one sweeps
end to end rather than taking a slice. A background pairs with `[fog]`: fog fades
distant material toward `fog.color`, so pick one near your background or the far
side of a structure floats instead of receding.

### Depth fog

Distant material fades toward a dark blue-gray, which is what gives a dense
structure its sense of depth. `strength` is the blend at the far side of a
structure no deeper than `reference_depth`; past that the ramp bends toward the
front and starts draining chroma, so a ribosome stays readable. If the fog is
heavier than you like, lower `strength` for small structures and `max_strength`
for large ones — or turn it off outright:

```toml
[fog]
strength = 0.2       # default 0.35
max_strength = 0.6   # default 0.85; the ceiling for deep structures
# strength = 0.0     # no fog at all
```

### Named palettes

A config file can define as many palettes as you like and cycle between them
with `p` (and `P` to go back) while ProteinView is running. Each `[[palette]]`
takes the same sections as the colors at the top of the file, under a
`palette.` prefix:

```toml
[[palette]]
name = "ocean"

[palette.structure]
helix = "0091EA"
sheet = "00BFA5"

[palette.chain]
colors = ["0091EA", "00BFA5", "7E57C2"]

[[palette]]
name = "print"

[palette.structure]
helix = "1A1A1A"
sheet = "5C5C5C"
```

The colors at the top of the file are always first in the cycle, under the name
`default`, so `p` goes `default` → `ocean` → `print` → `default`. The status bar
names the palette you are on whenever there is more than one to be in.

A named palette is a *whole* palette rather than a patch on the colors above it:
what it does not mention comes from the built-in defaults, so one block tells you
what it draws. Names must be unique, and `default` is taken.

`--palette-name <NAME>` opens on a given palette, which is how you get a snapshot
in one, since a snapshot has no keyboard:

```bash
proteinview examples/4HHB.pdb --palette-name print --snapshot figure.png
```

`[defaults] palette = "ocean"` sets which one to open on without a flag. Naming
a palette that does not exist is an error rather than a silent fall back to
`default` — a snapshot cannot tell you it was ignored.

### Startup defaults

`[defaults]` sets what ProteinView opens with. Each key is the default for the
flag or key of the same name, and passing the flag still wins:

```toml
[defaults]
render = "fullhd"     # braille | halfblock | hdplus | fullhd
mode = "cartoon"      # cartoon | backbone | wireframe
color = "chain"       # structure | chain | element | bfactor | rainbow | plddt
palette = "ocean"     # a [[palette]] name, or "default"
ball_and_stick = true
ligands = true
auto_rotate = false
```

Leaving a key out is not the same as writing its built-in value: an omitted key
lets the file-type heuristics run — an `.xyz` file opens as an element-colored
wireframe — while a key you wrote down stands.

See [`docs/config.example.toml`](docs/config.example.toml) for a fully commented
file listing every setting at its default value.

An older `palette.toml` in the same directory is still read when no
`config.toml` is there, and `--palette` still works as an alias for `--config`,
so a file written when this held only colors keeps working.

## Terminal Support

| Terminal | Braille | HD | FullHD |
|----------|---------|-----|--------|
| Any Unicode terminal | Yes | Yes | -- |
| Kitty | Yes | Yes | Yes (PNG) |
| WezTerm | Yes | Yes | Yes (Sixel) |
| iTerm2 | Yes | Yes | Yes |
| foot | Yes | Yes | Yes (Sixel) |
| tmux/screen | Yes | Yes | -- |

## Building

```bash
cargo build --release

# With RCSB fetch support
cargo build --release --features fetch
```

## Contributing

Contributions are welcome! Here's how to get started:

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/my-feature`)
3. Make your changes and add tests
4. Run `cargo test` to verify
5. Open a pull request against `develop`

Please open an issue first for major changes to discuss the approach.

## License

[MIT](LICENSE)
