//! User-configurable settings.
//!
//! One TOML file holds everything the user can tune without recompiling: every
//! fixed color ProteinView draws ([`Palette`]), the depth fog ([`Fog`]), and the
//! modes it opens in ([`Defaults`]).  The built-in values reproduce the
//! previously hardcoded ones exactly, so a user with no config file sees no
//! change, and a file may override any subset: anything omitted keeps its
//! default.
//!
//! The config is resolved once at startup and read-only thereafter, so it lives
//! in a process-wide [`OnceLock`] rather than being threaded through every
//! renderer.  Call [`init`] once from `main`; everything else reads [`config`]
//! or, for the common case, [`palette`].
//!
//! Everything ProteinView draws in a fixed color is reachable from here,
//! including the Rainbow scheme's ramp and the background behind the structure.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer};

use crate::app::{RenderMode, VizMode};
use crate::render::color::ColorSchemeType;

/// An RGB triple, deserialized from a hex string such as `"FF0080"` or `"#ff0080"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub [u8; 3]);

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self([r, g, b])
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let raw = String::deserialize(deserializer)?;
        let hex = raw.strip_prefix('#').unwrap_or(&raw);
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(D::Error::custom(format!(
                "expected six hexadecimal digits such as \"FF0080\", got {raw:?}"
            )));
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(D::Error::custom);
        Ok(Rgb([byte(0)?, byte(2)?, byte(4)?]))
    }
}

// ---------------------------------------------------------------------------
// Sections
// ---------------------------------------------------------------------------

/// Secondary-structure colors for the Structure scheme.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StructurePalette {
    pub helix: Rgb,
    pub sheet: Rgb,
    pub turn: Rgb,
    pub coil: Rgb,
}

impl Default for StructurePalette {
    fn default() -> Self {
        Self {
            helix: Rgb::new(255, 0, 128),
            sheet: Rgb::new(255, 200, 0),
            turn: Rgb::new(96, 128, 255),
            coil: Rgb::new(0, 204, 0),
        }
    }
}

/// Per-base colors for nucleic acid residues.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NucleotidePalette {
    pub adenine: Rgb,
    pub uracil: Rgb,
    pub thymine: Rgb,
    pub guanine: Rgb,
    pub cytosine: Rgb,
    pub inosine: Rgb,
}

impl Default for NucleotidePalette {
    fn default() -> Self {
        Self {
            adenine: Rgb::new(220, 60, 60),
            uracil: Rgb::new(60, 60, 220),
            thymine: Rgb::new(60, 60, 220),
            guanine: Rgb::new(60, 180, 60),
            cytosine: Rgb::new(220, 200, 40),
            inosine: Rgb::new(150, 100, 180),
        }
    }
}

/// CPK-style element colors.  `symbols` is keyed by uppercase element symbol.
#[derive(Debug, Clone)]
pub struct ElementPalette {
    pub fallback: Rgb,
    pub symbols: HashMap<String, Rgb>,
}

impl ElementPalette {
    /// Color for an element symbol, case-insensitively.
    pub fn get(&self, symbol: &str) -> Rgb {
        self.symbols
            .get(&symbol.trim().to_ascii_uppercase())
            .copied()
            .unwrap_or(self.fallback)
    }
}

impl Default for ElementPalette {
    fn default() -> Self {
        const CPK: &[(&str, [u8; 3])] = &[
            ("C", [144, 144, 144]),
            ("N", [48, 80, 248]),
            ("O", [255, 13, 13]),
            ("S", [255, 255, 48]),
            ("H", [255, 255, 255]),
            ("P", [255, 128, 0]),
            ("FE", [224, 102, 51]),
            ("MG", [0, 180, 0]),
            ("ZN", [125, 128, 176]),
            ("CA", [61, 255, 0]),
            ("MN", [156, 122, 199]),
            ("CO", [240, 144, 160]),
            ("CU", [200, 128, 51]),
            ("NI", [80, 208, 80]),
            ("CL", [31, 240, 31]),
            ("BR", [166, 41, 41]),
        ];
        Self {
            fallback: Rgb::new(200, 200, 200),
            symbols: CPK
                .iter()
                .map(|(s, c)| ((*s).to_string(), Rgb(*c)))
                .collect(),
        }
    }
}

/// AlphaFold pLDDT confidence bands.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlddtPalette {
    /// pLDDT >= 90
    pub very_high: Rgb,
    /// pLDDT >= 70
    pub high: Rgb,
    /// pLDDT >= 50
    pub low: Rgb,
    /// pLDDT < 50
    pub very_low: Rgb,
}

impl Default for PlddtPalette {
    fn default() -> Self {
        Self {
            very_high: Rgb::new(0, 83, 214),
            high: Rgb::new(101, 203, 243),
            low: Rgb::new(255, 219, 19),
            very_low: Rgb::new(255, 125, 69),
        }
    }
}

/// Endpoints of the B-factor gradient.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BFactorPalette {
    /// Color at the cold end of the range.
    pub low: Rgb,
    /// Color at the hot end of the range.
    pub high: Rgb,
}

impl Default for BFactorPalette {
    fn default() -> Self {
        Self {
            low: Rgb::new(0, 0, 255),
            high: Rgb::new(255, 0, 0),
        }
    }
}

/// Interface-highlighting colors.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InterfacePalette {
    /// Focus chain, in contact with a partner.
    pub focus_contact: Rgb,
    /// Focus chain, away from the interface.
    pub focus_other: Rgb,
    /// Partner chain, in contact with the focus chain.
    pub partner_contact: Rgb,
    /// Partner chain, away from the interface.
    pub partner_other: Rgb,
    /// Ligands, drawn bright so they stand out against the interface coloring.
    pub ligand: Rgb,
}

impl Default for InterfacePalette {
    fn default() -> Self {
        Self {
            focus_contact: Rgb::new(0, 255, 100),
            focus_other: Rgb::new(40, 100, 60),
            partner_contact: Rgb::new(255, 165, 0),
            partner_other: Rgb::new(100, 80, 60),
            ligand: Rgb::new(255, 255, 255),
        }
    }
}

/// Small-molecule colors.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LigandPalette {
    pub ligand: Rgb,
    pub ion: Rgb,
    /// Ligands under the Rainbow scheme, which has no per-residue value for them.
    pub rainbow: Rgb,
}

impl Default for LigandPalette {
    fn default() -> Self {
        Self {
            ligand: Rgb::new(255, 0, 255),
            ion: Rgb::new(0, 255, 255),
            rainbow: Rgb::new(255, 0, 255),
        }
    }
}

/// Colors for residues picked in the sequence panel.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SelectionPalette {
    /// Carbon atoms of the ball-and-stick overlay.  Other elements keep their
    /// CPK colors, so a picked residue reads as itself while still standing
    /// out from the ribbon behind it.
    pub carbon: Rgb,
    /// Marker sphere drawn on a picked residue when ball-and-stick is off.
    pub marker: Rgb,
    /// Background of the sequence panel's cursor cell.
    pub cursor: Rgb,
}

impl Default for SelectionPalette {
    fn default() -> Self {
        Self {
            carbon: Rgb::new(0, 230, 140),
            marker: Rgb::new(0, 230, 140),
            cursor: Rgb::new(255, 200, 0),
        }
    }
}

/// Screen-space structure-edge color used by outline mode.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutlinePalette {
    pub color: Rgb,
}

impl Default for OutlinePalette {
    fn default() -> Self {
        Self {
            // Visible against the transparent/dark background used by most
            // terminals. Light themes can override this with a dark ink.
            color: Rgb::new(216, 222, 233),
        }
    }
}

// ---------------------------------------------------------------------------
// Depth fog
// ---------------------------------------------------------------------------

/// Depth fog: how much a pixel fades toward the background as it recedes.
///
/// The renderer applies `strength` at the far plane of a structure no deeper
/// than `reference_depth`.  Beyond that the ramp bends and the chroma drain
/// comes in, both keyed to how much deeper the structure is, so that a ribosome
/// stays readable where a flat ramp turns it into confetti.  All four of those
/// terms vanish together at the reference depth, which is what lets a small
/// protein look exactly as it did before any of this existed.
///
/// Turning fog off entirely is `strength = 0.0`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Fog {
    /// What distant pixels fade toward.  Well above the terminal background, so
    /// the far side of a structure dims rather than disappearing.
    pub color: Rgb,
    /// Blend at the far plane of a structure at or below `reference_depth`.
    pub strength: f64,
    /// Ceiling on the depth-scaled blend, however deep the structure is.
    pub max_strength: f64,
    /// Depth span, in angstroms, at which `strength` applies as given.  Roughly
    /// the depth of a small single-domain protein.
    pub reference_depth: f64,
    /// How sharply the ramp concentrates its contrast at the front of a deep
    /// structure, in multiples of `ln(span / reference_depth)`.  Higher reads as
    /// a spotlit front shell with the rest in shadow.
    pub curve_gain: f64,
    /// Chroma drained at the far plane once a structure is much deeper than the
    /// reference depth.  `0.0` fades by brightness alone.
    pub desaturation: f64,
}

impl Default for Fog {
    fn default() -> Self {
        Self {
            color: Rgb::new(40, 50, 70),
            strength: 0.35,
            max_strength: 0.85,
            reference_depth: 55.0,
            curve_gain: 2.0,
            desaturation: 1.0,
        }
    }
}

impl Fog {
    fn validate(&self) -> Result<()> {
        let unit = |name: &str, value: f64| -> Result<()> {
            if !(0.0..=1.0).contains(&value) {
                anyhow::bail!("`fog.{name}` must be between 0.0 and 1.0, got {value}");
            }
            Ok(())
        };
        unit("strength", self.strength)?;
        unit("max_strength", self.max_strength)?;
        unit("desaturation", self.desaturation)?;
        if self.max_strength < self.strength {
            anyhow::bail!(
                "`fog.max_strength` ({}) must be at least `fog.strength` ({}): the ceiling cannot sit below the value it caps",
                self.max_strength,
                self.strength
            );
        }
        // Finiteness is checked explicitly: NaN compares false against
        // everything, so a bare `> 0.0` would wave it through.
        if !self.reference_depth.is_finite() || self.reference_depth <= 0.0 {
            anyhow::bail!(
                "`fog.reference_depth` must be a finite depth greater than 0, got {}",
                self.reference_depth
            );
        }
        if !self.curve_gain.is_finite() || self.curve_gain < 0.0 {
            anyhow::bail!(
                "`fog.curve_gain` must be finite and not negative, got {}",
                self.curve_gain
            );
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Startup defaults
// ---------------------------------------------------------------------------

/// What ProteinView opens with, for the settings that also have a command-line
/// flag or a key binding.
///
/// Every field is optional, and `None` means "not configured": the built-in
/// behaviour, including the heuristics that pick a mode from the file itself,
/// still applies.  A command-line flag always wins over the file, and the file
/// always wins over a heuristic -- someone who wrote a preference down meant it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    /// Which named palette to open with.  Must name one that the file defines,
    /// or `default` for the colors written at the top level.
    pub palette: Option<String>,
    /// Render tier: `braille`, `halfblock`, `hdplus` or `fullhd`.  Same
    /// spellings as `--render`.
    #[serde(deserialize_with = "de_render_mode")]
    pub render: Option<RenderMode>,
    /// Visualization: `cartoon`, `backbone` or `wireframe`.  Same spellings as
    /// `--mode`.
    #[serde(deserialize_with = "de_viz_mode")]
    pub mode: Option<VizMode>,
    /// Color scheme: `structure`, `chain`, `element`, `bfactor`, `rainbow` or
    /// `plddt`.  Same spellings as `--color`.
    #[serde(deserialize_with = "de_color_scheme")]
    pub color: Option<ColorSchemeType>,
    /// Draw picked residues as ball-and-stick rather than a single marker.
    pub ball_and_stick: Option<bool>,
    /// Draw ligands and ions.
    pub ligands: Option<bool>,
    /// Spin the structure when nothing else is driving the camera.
    pub auto_rotate: Option<bool>,
    /// Trace screen-space exterior, overlap, and material boundaries.
    pub outline: Option<bool>,
}

/// Deserialize one of the mode names, reusing the parser the CLI uses so the
/// file and the flag can never accept different spellings.
fn de_named<'de, D, T>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
    what: &str,
    accepted: &str,
) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;
    let raw = String::deserialize(deserializer)?;
    parse(&raw).map(Some).ok_or_else(|| {
        D::Error::custom(format!(
            "unknown {what} {raw:?}; expected one of {accepted}"
        ))
    })
}

fn de_render_mode<'de, D: Deserializer<'de>>(d: D) -> Result<Option<RenderMode>, D::Error> {
    de_named(
        d,
        RenderMode::parse,
        "render mode",
        "braille, halfblock, hdplus, fullhd",
    )
}

fn de_viz_mode<'de, D: Deserializer<'de>>(d: D) -> Result<Option<VizMode>, D::Error> {
    de_named(
        d,
        VizMode::parse,
        "visualization mode",
        "cartoon, backbone, wireframe",
    )
}

fn de_color_scheme<'de, D: Deserializer<'de>>(d: D) -> Result<Option<ColorSchemeType>, D::Error> {
    de_named(
        d,
        ColorSchemeType::parse,
        "color scheme",
        "structure, chain, element, bfactor, rainbow, plddt",
    )
}

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

/// The resolved palette: built-in defaults with any configured overrides applied.
#[derive(Debug, Clone)]
pub struct Palette {
    pub structure: StructurePalette,
    pub nucleotide: NucleotidePalette,
    /// Cycled by chain in order; always at least one entry.
    pub chains: Vec<Rgb>,
    pub element: ElementPalette,
    pub plddt: PlddtPalette,
    pub bfactor: BFactorPalette,
    pub interface: InterfacePalette,
    pub ligand: LigandPalette,
    pub selection: SelectionPalette,
    pub outline: OutlinePalette,
    /// Color stops for the Rainbow scheme, N-terminus first, interpolated
    /// across the chain.  `None` keeps the built-in HSV sweep.
    pub rainbow: Option<Vec<Rgb>>,
    /// What empty space is painted with.  `None` leaves it transparent, so the
    /// terminal's own background shows through and a snapshot PNG keeps its
    /// alpha -- which is the behaviour every version so far has had.
    pub background: Option<Rgb>,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            structure: StructurePalette::default(),
            nucleotide: NucleotidePalette::default(),
            chains: vec![
                Rgb::new(0, 180, 255),
                Rgb::new(255, 100, 0),
                Rgb::new(0, 220, 100),
                Rgb::new(255, 50, 150),
                Rgb::new(180, 100, 255),
                Rgb::new(255, 220, 0),
                Rgb::new(0, 200, 200),
                Rgb::new(255, 150, 150),
            ],
            element: ElementPalette::default(),
            plddt: PlddtPalette::default(),
            bfactor: BFactorPalette::default(),
            interface: InterfacePalette::default(),
            ligand: LigandPalette::default(),
            selection: SelectionPalette::default(),
            outline: OutlinePalette::default(),
            rainbow: None,
            background: None,
        }
    }
}

impl Palette {
    /// The Rainbow scheme's color at `t`, where 0.0 is the N-terminus and 1.0
    /// the C-terminus, or `None` when no ramp is configured and the caller
    /// should fall back to the built-in HSV sweep.
    ///
    /// Stops are spread evenly and blended between, so eight colors give seven
    /// gradients rather than eight bands: a ramp, like the sweep it replaces.
    pub fn rainbow_at(&self, t: f64) -> Option<Rgb> {
        let stops = self.rainbow.as_ref()?;
        let t = t.clamp(0.0, 1.0);
        match stops.len() {
            0 => None,
            1 => Some(stops[0]),
            n => {
                let scaled = t * (n - 1) as f64;
                let lower = (scaled.floor() as usize).min(n - 2);
                let frac = scaled - lower as f64;
                let (a, b) = (stops[lower].0, stops[lower + 1].0);
                Some(Rgb([
                    lerp_channel(a[0], b[0], frac),
                    lerp_channel(a[1], b[1], frac),
                    lerp_channel(a[2], b[2], frac),
                ]))
            }
        }
    }

    /// Color for the chain whose id starts with `id`, cycling through `chains`.
    pub fn chain(&self, id: &str) -> Rgb {
        let idx = id.bytes().next().unwrap_or(b'A') as usize % self.chains.len();
        self.chains[idx]
    }
}

/// Blend one channel, rounding rather than truncating so a ramp's midpoint is
/// the midpoint.
fn lerp_channel(a: u8, b: u8, t: f64) -> u8 {
    (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round() as u8
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// A palette under the name the file gave it.
#[derive(Debug, Clone)]
pub struct NamedPalette {
    pub name: String,
    pub palette: Palette,
}

/// Everything resolved from the config file: built-in values with any
/// configured overrides applied.
#[derive(Debug, Clone)]
pub struct Config {
    /// Every palette the file defines, in the order written, and never empty:
    /// index 0 is `default`, built from the top-level color sections.  `p`
    /// cycles through them at runtime, which is why the order is the file's
    /// rather than whatever a hash map happens to yield.
    pub palettes: Vec<NamedPalette>,
    /// Index into `palettes` that the session starts on.
    pub start_palette: usize,
    pub fog: Fog,
    pub defaults: Defaults,
}

/// The name of the palette built from the top-level color sections.
pub const BASE_PALETTE: &str = "default";

impl Default for Config {
    fn default() -> Self {
        Self {
            palettes: vec![NamedPalette {
                name: BASE_PALETTE.to_string(),
                palette: Palette::default(),
            }],
            start_palette: 0,
            fog: Fog::default(),
            defaults: Defaults::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Config file
// ---------------------------------------------------------------------------

/// Element section as written in the file.  `symbols` is *merged* onto the
/// built-in CPK table rather than replacing it, so overriding carbon does not
/// silently drop every other element.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ElementFile {
    /// Color for elements not listed in `symbols`.
    fallback: Option<Rgb>,
    symbols: HashMap<String, Rgb>,
}

/// Chain section.  A section rather than a bare top-level `chains` key, because
/// in TOML a bare key written after any `[table]` header silently binds to that
/// table instead of the document root.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ChainFile {
    /// Replaces the default cycle outright; order is significant.
    colors: Option<Vec<Rgb>>,
}

/// Rainbow section.  A section for the same reason `chain` is one, and because
/// a ramp is a list rather than a single color.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RainbowFile {
    /// Stops from the N-terminus to the C-terminus, blended between.  Replaces
    /// the built-in HSV sweep outright.
    colors: Option<Vec<Rgb>>,
}

/// Background section.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct BackgroundFile {
    /// Paints empty space.  Omitted, it stays transparent.
    color: Option<Rgb>,
}

/// One palette's worth of color sections.
///
/// The top level of the file is one of these, and so is every `[[palette]]`
/// entry, which is what makes a named palette exactly as expressive as the
/// colors written at the root.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PaletteFile {
    /// Names the palette in `[[palette]]`; unused at the top level, which is
    /// always [`BASE_PALETTE`].
    name: String,
    structure: StructurePalette,
    nucleotide: NucleotidePalette,
    chain: ChainFile,
    element: ElementFile,
    plddt: PlddtPalette,
    bfactor: BFactorPalette,
    interface: InterfacePalette,
    ligand: LigandPalette,
    selection: SelectionPalette,
    outline: OutlinePalette,
    rainbow: RainbowFile,
    background: BackgroundFile,
}

impl PaletteFile {
    /// Built-in defaults with this file's colors applied.
    ///
    /// Every palette resolves against the built-ins, including the named ones:
    /// a `[[palette]]` is a whole palette rather than a patch on the top-level
    /// colors, so reading one tells you what it draws without also holding the
    /// rest of the file in your head.  `where` says which palette an error is
    /// about, since by this point they all look alike.
    fn resolve(self, where_: &str) -> Result<Palette> {
        let mut palette = Palette {
            structure: self.structure,
            nucleotide: self.nucleotide,
            chains: Palette::default().chains,
            element: ElementPalette::default(),
            plddt: self.plddt,
            bfactor: self.bfactor,
            interface: self.interface,
            ligand: self.ligand,
            selection: self.selection,
            outline: self.outline,
            rainbow: None,
            background: self.background.color,
        };

        if let Some(colors) = self.rainbow.colors {
            if colors.is_empty() {
                anyhow::bail!("`{where_}rainbow.colors` must list at least one color");
            }
            palette.rainbow = Some(colors);
        }
        if let Some(colors) = self.chain.colors {
            if colors.is_empty() {
                anyhow::bail!("`{where_}chain.colors` must list at least one color");
            }
            palette.chains = colors;
        }
        if let Some(fallback) = self.element.fallback {
            palette.element.fallback = fallback;
        }
        for (symbol, color) in self.element.symbols {
            palette
                .element
                .symbols
                .insert(symbol.trim().to_ascii_uppercase(), color);
        }

        Ok(palette)
    }
}

/// The file as written.  Color sections sit at the top level, where they have
/// always been, so a palette file from before the config grew past colors still
/// parses unchanged; `fog`, `defaults` and `palette` are new sections beside
/// them.
///
/// The color fields are spelled out again rather than shared with
/// [`PaletteFile`] because `serde(flatten)` and `deny_unknown_fields` cannot
/// both apply to one struct, and rejecting typos is worth more than the nine
/// lines.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigFile {
    structure: StructurePalette,
    nucleotide: NucleotidePalette,
    chain: ChainFile,
    element: ElementFile,
    plddt: PlddtPalette,
    bfactor: BFactorPalette,
    interface: InterfacePalette,
    ligand: LigandPalette,
    selection: SelectionPalette,
    outline: OutlinePalette,
    rainbow: RainbowFile,
    background: BackgroundFile,
    fog: Fog,
    defaults: Defaults,
    /// Additional named palettes, as an array of tables so the cycling order is
    /// the order they were written in.
    palette: Vec<PaletteFile>,
}

impl ConfigFile {
    fn resolve(self) -> Result<Config> {
        let base = PaletteFile {
            name: BASE_PALETTE.to_string(),
            structure: self.structure,
            nucleotide: self.nucleotide,
            chain: self.chain,
            element: self.element,
            plddt: self.plddt,
            bfactor: self.bfactor,
            interface: self.interface,
            ligand: self.ligand,
            selection: self.selection,
            outline: self.outline,
            rainbow: self.rainbow,
            background: self.background,
        };

        let mut palettes = vec![NamedPalette {
            name: BASE_PALETTE.to_string(),
            palette: base.resolve("")?,
        }];

        for entry in self.palette {
            let name = entry.name.trim().to_string();
            if name.is_empty() {
                anyhow::bail!(
                    "every `[[palette]]` needs a `name`, which is what `p` shows and `defaults.palette` selects"
                );
            }
            if palettes.iter().any(|p| p.name == name) {
                anyhow::bail!("two palettes are named {name:?}; cycling could not tell them apart");
            }
            let palette = entry.resolve(&format!("palette.{name}."))?;
            palettes.push(NamedPalette { name, palette });
        }

        // A start palette that does not exist is a typo worth stopping for: the
        // alternative is opening in `default` and leaving the user to wonder
        // why their colors did nothing.
        let start_palette = match &self.defaults.palette {
            Some(wanted) => palettes
                .iter()
                .position(|p| p.name == wanted.trim())
                .with_context(|| {
                    let known: Vec<&str> = palettes.iter().map(|p| p.name.as_str()).collect();
                    format!(
                        "`defaults.palette` names {wanted:?}, which is not defined; this file has {}",
                        known.join(", ")
                    )
                })?,
            None => 0,
        };

        self.fog.validate()?;

        Ok(Config {
            palettes,
            start_palette,
            fog: self.fog,
            defaults: self.defaults,
        })
    }
}

/// Parse a config from TOML text, filling anything omitted from the defaults.
pub fn parse(text: &str) -> Result<Config> {
    toml::from_str::<ConfigFile>(text)?.resolve()
}

/// Read and parse a config file.
pub fn load(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read config file {}", path.display()))?;
    parse(&text).with_context(|| format!("invalid config file {}", path.display()))
}

/// Config file locations, in the order they are tried:
/// `$XDG_CONFIG_HOME/proteinview/config.toml` (falling back to `~/.config`),
/// then `palette.toml` beside it.
///
/// The second name is what this file was called when it held nothing but
/// colors.  It still works, so an upgrade does not quietly stop reading a file
/// someone already wrote.
pub fn default_config_paths() -> Vec<PathBuf> {
    let Some(base) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    else {
        return Vec::new();
    };
    let dir = base.join("proteinview");
    vec![dir.join("config.toml"), dir.join("palette.toml")]
}

static CONFIG: OnceLock<Config> = OnceLock::new();

/// Index into `config().palettes` of the palette being drawn.
///
/// The config itself is immutable once resolved, but which of its palettes is
/// active changes whenever the user presses `p`.  An atomic index keeps
/// [`palette`] returning a `&'static Palette`, so the hundred call sites that
/// just want a color stay as they are.
static ACTIVE_PALETTE: AtomicUsize = AtomicUsize::new(0);

/// The active config.  Defaults are used if [`init`] was never called.
pub fn config() -> &'static Config {
    CONFIG.get_or_init(Config::default)
}

/// The palette being drawn, the part of the config nearly every caller wants.
pub fn palette() -> &'static Palette {
    let all = &config().palettes;
    // Saturating rather than wrapping: an index can only be stale if the config
    // was replaced under us, and drawing the last palette beats panicking.
    &all[ACTIVE_PALETTE.load(Ordering::Relaxed).min(all.len() - 1)].palette
}

/// Name of the palette being drawn.
pub fn palette_name() -> &'static str {
    let all = &config().palettes;
    &all[ACTIVE_PALETTE.load(Ordering::Relaxed).min(all.len() - 1)].name
}

/// How many palettes the config defines.  Always at least one.
pub fn palette_count() -> usize {
    config().palettes.len()
}

/// Step to the next palette (or the previous one), wrapping at both ends, and
/// return its name.  A no-op when the file defines only the default.
pub fn cycle_palette(forward: bool) -> &'static str {
    let count = palette_count();
    if count > 1 {
        let current = ACTIVE_PALETTE.load(Ordering::Relaxed).min(count - 1);
        ACTIVE_PALETTE.store(next_index(current, count, forward), Ordering::Relaxed);
    }
    palette_name()
}

/// The neighbouring index in a cycle of `count`, wrapping at both ends.
fn next_index(current: usize, count: usize, forward: bool) -> usize {
    if forward {
        (current + 1) % count
    } else {
        (current + count - 1) % count
    }
}

/// Select a palette by name, returning whether one by that name exists.
pub fn set_palette(name: &str) -> bool {
    match config().palettes.iter().position(|p| p.name == name) {
        Some(index) => {
            ACTIVE_PALETTE.store(index, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

/// Resolve the config once, from `explicit` if given, else from the first
/// default path that exists, else from the built-in defaults.
///
/// An explicit path that cannot be read is an error; so is a malformed file in
/// either location, since silently ignoring a config the user wrote is worse
/// than refusing to start.  Returns the path that was loaded, if any.
pub fn init(explicit: Option<&Path>) -> Result<Option<PathBuf>> {
    let chosen = match explicit {
        Some(path) => Some(path.to_path_buf()),
        None => default_config_paths().into_iter().find(|p| p.is_file()),
    };

    let (resolved, loaded) = match &chosen {
        Some(path) => (load(path)?, Some(path.clone())),
        None => (Config::default(), None),
    };

    ACTIVE_PALETTE.store(resolved.start_palette, Ordering::Relaxed);
    let _ = CONFIG.set(resolved);
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Most of these tests are about colors; unwrap the palette for them.
    fn parse_palette(text: &str) -> Result<Palette> {
        Ok(parse(text)?.palettes.remove(0).palette)
    }

    #[test]
    fn empty_config_is_the_default_palette() {
        let p = parse_palette("").unwrap();
        let d = Palette::default();
        assert_eq!(p.structure.helix, d.structure.helix);
        assert_eq!(p.chains, d.chains);
        assert_eq!(p.element.get("ZN"), d.element.get("ZN"));
        assert_eq!(p.plddt.very_high, d.plddt.very_high);
    }

    #[test]
    fn overrides_apply_and_the_rest_stays_default() {
        let p = parse_palette(
            r##"
            [structure]
            helix = "#112233"
            "##,
        )
        .unwrap();
        assert_eq!(p.structure.helix, Rgb::new(0x11, 0x22, 0x33));
        // Untouched fields keep their defaults.
        assert_eq!(p.structure.sheet, StructurePalette::default().sheet);
        assert_eq!(p.structure.coil, StructurePalette::default().coil);
    }

    #[test]
    fn element_symbols_merge_rather_than_replace() {
        // Overriding carbon must not drop the rest of the CPK table.
        let p = parse_palette(
            r#"
            [element.symbols]
            C = "010203"
            "#,
        )
        .unwrap();
        assert_eq!(p.element.get("C"), Rgb::new(1, 2, 3));
        assert_eq!(p.element.get("ZN"), Rgb::new(125, 128, 176));
        assert_eq!(p.element.get("FE"), Rgb::new(224, 102, 51));
    }

    #[test]
    fn element_lookup_is_case_insensitive_both_ways() {
        let p = parse_palette(
            r#"
            [element.symbols]
            se = "0A0B0C"
            "#,
        )
        .unwrap();
        assert_eq!(p.element.get("SE"), Rgb::new(10, 11, 12));
        assert_eq!(p.element.get("Se"), Rgb::new(10, 11, 12));
        assert_eq!(p.element.get(" fe "), Rgb::new(224, 102, 51));
    }

    #[test]
    fn chains_are_replaced_wholesale_and_cycle() {
        let p = parse_palette("[chain]\ncolors = [\"FF0000\", \"00FF00\"]").unwrap();
        assert_eq!(p.chains.len(), 2);
        // b'A' = 65, so 65 % 2 = 1 picks the second entry.
        assert_eq!(p.chain("A"), Rgb::new(0, 255, 0));
        assert_eq!(p.chain("B"), Rgb::new(255, 0, 0));
        assert_eq!(p.chain("A"), p.chain("C"));
    }

    #[test]
    fn empty_chain_list_is_rejected() {
        let err = parse("[chain]\ncolors = []").unwrap_err().to_string();
        assert!(err.contains("at least one"), "unhelpful error: {err}");
    }

    #[test]
    fn chain_colors_survive_being_written_after_other_sections() {
        // A bare top-level `chains = [...]` would bind to whichever [table]
        // preceded it. Keeping it in its own section makes order irrelevant.
        let p = parse_palette("[structure]\nhelix = \"FF0000\"\n\n[chain]\ncolors = [\"00FF00\"]")
            .unwrap();
        assert_eq!(p.chains, vec![Rgb::new(0, 255, 0)]);
        assert_eq!(p.structure.helix, Rgb::new(255, 0, 0));
    }

    #[test]
    fn a_colors_only_file_still_parses_after_the_config_grew() {
        // The file was once nothing but colors, and those sections still sit at
        // the top level.  Anyone who wrote one before fog and defaults existed
        // keeps a working config.
        let c = parse(
            r#"
            [structure]
            helix = "112233"

            [chain]
            colors = ["FF0000"]

            [element.symbols]
            C = "010203"
            "#,
        )
        .unwrap();
        assert_eq!(
            c.palettes[0].palette.structure.helix,
            Rgb::new(0x11, 0x22, 0x33)
        );
        assert_eq!(c.fog, Fog::default());
        assert_eq!(c.defaults, Defaults::default());
    }

    #[test]
    fn fog_overrides_apply_one_key_at_a_time() {
        let c = parse("[fog]\nstrength = 0.15").unwrap();
        assert_eq!(c.fog.strength, 0.15);
        // Everything else about the fog is untouched.
        assert_eq!(c.fog.max_strength, Fog::default().max_strength);
        assert_eq!(c.fog.color, Fog::default().color);
        assert_eq!(c.fog.desaturation, Fog::default().desaturation);
    }

    #[test]
    fn fog_can_be_turned_off_outright() {
        assert_eq!(parse("[fog]\nstrength = 0.0").unwrap().fog.strength, 0.0);
    }

    #[test]
    fn out_of_range_fog_is_rejected_with_the_key_named() {
        for (text, needle) in [
            ("[fog]\nstrength = 1.5", "fog.strength"),
            ("[fog]\ndesaturation = -0.2", "fog.desaturation"),
            ("[fog]\nreference_depth = 0.0", "fog.reference_depth"),
            ("[fog]\ncurve_gain = -1.0", "fog.curve_gain"),
        ] {
            let err = parse(text).unwrap_err().to_string();
            assert!(
                err.contains(needle),
                "{text:?} gave an unhelpful error: {err}"
            );
        }
    }

    #[test]
    fn a_fog_ceiling_below_its_own_strength_is_rejected() {
        // Silently clamping would leave the file saying one thing and the
        // renderer doing another.
        let err = parse("[fog]\nstrength = 0.6\nmax_strength = 0.4")
            .unwrap_err()
            .to_string();
        assert!(err.contains("max_strength"), "unhelpful error: {err}");
    }

    #[test]
    fn defaults_parse_every_mode_name_the_cli_takes() {
        let c = parse(
            r#"
            [defaults]
            render = "hd+"
            mode = "wireframe"
            color = "b-factor"
            ball_and_stick = false
            ligands = false
            auto_rotate = true
            outline = true
            "#,
        )
        .unwrap();
        assert_eq!(c.defaults.render, Some(RenderMode::HalfBlockPlus));
        assert_eq!(c.defaults.mode, Some(VizMode::Wireframe));
        assert_eq!(c.defaults.color, Some(ColorSchemeType::BFactor));
        assert_eq!(c.defaults.ball_and_stick, Some(false));
        assert_eq!(c.defaults.ligands, Some(false));
        assert_eq!(c.defaults.auto_rotate, Some(true));
        assert_eq!(c.defaults.outline, Some(true));
    }

    #[test]
    fn outline_color_is_palette_scoped_and_configurable() {
        let c = parse(
            r#"
            [outline]
            color = "112233"

            [[palette]]
            name = "paper"
            [palette.outline]
            color = "AABBCC"
            "#,
        )
        .unwrap();
        assert_eq!(
            c.palettes[0].palette.outline.color,
            Rgb::new(0x11, 0x22, 0x33)
        );
        assert_eq!(
            c.palettes[1].palette.outline.color,
            Rgb::new(0xAA, 0xBB, 0xCC)
        );
    }

    #[test]
    fn an_unset_default_stays_unset() {
        // `None` is what lets the file-extension heuristics still run, so an
        // omitted key must not resolve to a built-in value here.
        let c = parse("[defaults]\nrender = \"fullhd\"").unwrap();
        assert_eq!(c.defaults.render, Some(RenderMode::FullHD));
        assert_eq!(c.defaults.mode, None);
        assert_eq!(c.defaults.color, None);
    }

    #[test]
    fn a_misspelled_mode_names_what_was_expected() {
        let err = parse("[defaults]\nmode = \"cartoons\"")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("cartoons"),
            "should quote the bad value: {err}"
        );
        assert!(
            err.contains("cartoon, backbone, wireframe"),
            "should list the accepted values: {err}"
        );
    }

    #[test]
    fn unknown_keys_are_rejected_in_the_new_sections_too() {
        for text in [
            "[fog]\nstrenght = 0.2",
            "[defaults]\nrender_mode = \"fullhd\"",
        ] {
            assert!(
                parse(text).is_err(),
                "a typo should be an error, not silence: {text:?}"
            );
        }
    }

    #[test]
    fn named_palettes_keep_the_order_they_were_written_in() {
        // Cycling order is the file's order, which is why these are an array of
        // tables rather than a map: a map would hand them back in hash order.
        let c = parse(
            r#"
            [[palette]]
            name = "ocean"
            [palette.structure]
            helix = "0088FF"

            [[palette]]
            name = "amber"
            [palette.structure]
            helix = "FFAA00"
            "#,
        )
        .unwrap();

        let names: Vec<&str> = c.palettes.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["default", "ocean", "amber"]);
        assert_eq!(
            c.palettes[1].palette.structure.helix,
            Rgb::new(0, 0x88, 0xFF)
        );
        assert_eq!(
            c.palettes[2].palette.structure.helix,
            Rgb::new(0xFF, 0xAA, 0)
        );
    }

    #[test]
    fn a_named_palette_is_whole_rather_than_a_patch_on_the_base() {
        // Every palette resolves against the built-ins, so reading one tells you
        // what it draws without holding the top of the file in your head.
        let c = parse(
            r#"
            [structure]
            helix = "111111"
            sheet = "222222"

            [[palette]]
            name = "mono"
            [palette.structure]
            helix = "CCCCCC"
            "#,
        )
        .unwrap();

        let mono = &c.palettes[1].palette;
        assert_eq!(mono.structure.helix, Rgb::new(0xCC, 0xCC, 0xCC));
        assert_eq!(
            mono.structure.sheet,
            StructurePalette::default().sheet,
            "an unmentioned color comes from the built-in default, not the base palette"
        );
    }

    #[test]
    fn a_named_palette_can_set_everything_the_top_level_can() {
        let c = parse(
            r#"
            [[palette]]
            name = "full"
            [palette.chain]
            colors = ["FF0000", "00FF00"]
            [palette.element.symbols]
            C = "010203"
            [palette.selection]
            marker = "FFFFFF"
            "#,
        )
        .unwrap();

        let full = &c.palettes[1].palette;
        assert_eq!(full.chains.len(), 2);
        assert_eq!(full.element.get("C"), Rgb::new(1, 2, 3));
        assert_eq!(full.element.get("ZN"), Rgb::new(125, 128, 176));
        assert_eq!(full.selection.marker, Rgb::new(255, 255, 255));
    }

    #[test]
    fn a_nameless_or_repeated_palette_is_rejected() {
        let err = parse("[[palette]]\n[palette.structure]\nhelix = \"FF0000\"")
            .unwrap_err()
            .to_string();
        assert!(err.contains("name"), "unhelpful error: {err}");

        let err = parse("[[palette]]\nname = \"x\"\n\n[[palette]]\nname = \"x\"")
            .unwrap_err()
            .to_string();
        assert!(err.contains("two palettes"), "unhelpful error: {err}");

        // `default` is taken by the top-level colors.
        let err = parse("[[palette]]\nname = \"default\"")
            .unwrap_err()
            .to_string();
        assert!(err.contains("two palettes"), "unhelpful error: {err}");
    }

    #[test]
    fn an_error_inside_a_named_palette_says_which_one() {
        let err = parse("[[palette]]\nname = \"ocean\"\n[palette.chain]\ncolors = []")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("palette.ocean.chain.colors"),
            "the error should locate the palette: {err}"
        );
    }

    #[test]
    fn the_starting_palette_can_be_named_and_a_typo_is_caught() {
        let file = r#"
            [[palette]]
            name = "ocean"

            [defaults]
            palette = "ocean"
            "#;
        assert_eq!(parse(file).unwrap().start_palette, 1);

        let err = parse("[[palette]]\nname = \"ocean\"\n\n[defaults]\npalette = \"ocaen\"")
            .unwrap_err()
            .to_string();
        assert!(err.contains("ocaen"), "should quote the bad name: {err}");
        assert!(
            err.contains("default, ocean"),
            "should list what is available: {err}"
        );
    }

    #[test]
    fn cycling_wraps_in_both_directions() {
        // The index arithmetic, away from the process-wide active palette that
        // the real cycling reads.
        assert_eq!(next_index(0, 3, true), 1);
        assert_eq!(
            next_index(2, 3, true),
            0,
            "forward off the end wraps to the start"
        );
        assert_eq!(
            next_index(0, 3, false),
            2,
            "back off the start wraps to the end"
        );
        assert_eq!(next_index(0, 1, true), 0, "a lone palette cycles to itself");
    }

    #[test]
    fn a_rainbow_ramp_blends_between_its_stops() {
        let p = parse_palette(
            r#"
            [rainbow]
            colors = ["000000", "FFFFFF"]
            "#,
        )
        .unwrap();

        assert_eq!(p.rainbow_at(0.0), Some(Rgb::new(0, 0, 0)));
        assert_eq!(p.rainbow_at(1.0), Some(Rgb::new(255, 255, 255)));
        // Halfway is halfway, rounded rather than truncated.
        assert_eq!(p.rainbow_at(0.5), Some(Rgb::new(128, 128, 128)));
    }

    #[test]
    fn a_ramp_of_many_stops_hits_each_one_in_order() {
        // Eight stops make seven gradients, and every stop is reached exactly.
        let p = parse_palette(
            r#"
            [rainbow]
            colors = ["A3E8C7", "8EDBD8", "8FCBF3", "A5B4F5",
                      "C5A9F0", "E5A3E0", "F5A7C0", "F7C9A0"]
            "#,
        )
        .unwrap();

        assert_eq!(p.rainbow_at(0.0), Some(Rgb::new(0xA3, 0xE8, 0xC7)));
        assert_eq!(p.rainbow_at(1.0), Some(Rgb::new(0xF7, 0xC9, 0xA0)));
        for (i, want) in [
            (0, Rgb::new(0xA3, 0xE8, 0xC7)),
            (3, Rgb::new(0xA5, 0xB4, 0xF5)),
            (7, Rgb::new(0xF7, 0xC9, 0xA0)),
        ] {
            let t = i as f64 / 7.0;
            assert_eq!(p.rainbow_at(t), Some(want), "stop {i} at t={t}");
        }
    }

    #[test]
    fn no_ramp_means_the_built_in_sweep() {
        assert_eq!(parse_palette("").unwrap().rainbow_at(0.5), None);
        let err = parse("[rainbow]\ncolors = []").unwrap_err().to_string();
        assert!(err.contains("at least one"), "unhelpful error: {err}");
    }

    #[test]
    fn a_background_is_off_unless_configured() {
        assert_eq!(parse_palette("").unwrap().background, None);
        assert_eq!(
            parse_palette("[background]\ncolor = \"1a1b26\"")
                .unwrap()
                .background,
            Some(Rgb::new(0x1A, 0x1B, 0x26))
        );
    }

    #[test]
    fn a_named_palette_can_carry_its_own_ramp_and_background() {
        // The whole point of a themed palette: switch to it and the background
        // and the rainbow move with it.
        let c = parse(
            r##"
            [[palette]]
            name = "aurora"

            [palette.background]
            color = "#1a1b26"

            [palette.rainbow]
            colors = ["A3E8C7", "F7C9A0"]
            "##,
        )
        .unwrap();

        let aurora = &c.palettes[1].palette;
        assert_eq!(aurora.background, Some(Rgb::new(0x1A, 0x1B, 0x26)));
        assert_eq!(aurora.rainbow_at(0.0), Some(Rgb::new(0xA3, 0xE8, 0xC7)));
        // And the default palette is untouched by it.
        assert_eq!(c.palettes[0].palette.background, None);
        assert_eq!(c.palettes[0].palette.rainbow_at(0.0), None);
    }

    #[test]
    fn hex_accepts_optional_hash_and_any_case() {
        let p = parse_palette(
            r##"
            [structure]
            helix = "#aabbcc"
            sheet = "AABBCC"
            "##,
        )
        .unwrap();
        assert_eq!(p.structure.helix, Rgb::new(0xAA, 0xBB, 0xCC));
        assert_eq!(p.structure.sheet, p.structure.helix);
    }

    #[test]
    fn malformed_color_is_rejected_with_the_offending_value() {
        let err = parse(
            r#"
            [structure]
            helix = "nope"
            "#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("six hexadecimal"), "unhelpful error: {err}");
        assert!(err.contains("nope"), "error should quote the value: {err}");
    }

    #[test]
    fn unknown_keys_are_rejected_so_typos_are_not_silent() {
        let err = parse(
            r#"
            [structure]
            helics = "FF0000"
            "#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("helics"), "error should name the key: {err}");
    }
}
