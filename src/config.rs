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
//! Procedural schemes (Rainbow's HSV sweep) are not covered here yet.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
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
        }
    }
}

impl Palette {
    /// Color for the chain whose id starts with `id`, cycling through `chains`.
    pub fn chain(&self, id: &str) -> Rgb {
        let idx = id.bytes().next().unwrap_or(b'A') as usize % self.chains.len();
        self.chains[idx]
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Everything resolved from the config file: built-in values with any
/// configured overrides applied.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub palette: Palette,
    pub fog: Fog,
    pub defaults: Defaults,
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

/// The file as written.  Color sections sit at the top level, where they have
/// always been, so a palette file from before the config grew past colors still
/// parses unchanged; `fog` and `defaults` are new sections beside them.
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
    fog: Fog,
    defaults: Defaults,
}

impl ConfigFile {
    fn resolve(self) -> Result<Config> {
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
        };

        if let Some(colors) = self.chain.colors {
            if colors.is_empty() {
                anyhow::bail!("`chain.colors` must list at least one color");
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

        self.fog.validate()?;

        Ok(Config {
            palette,
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

/// The active config.  Defaults are used if [`init`] was never called.
pub fn config() -> &'static Config {
    CONFIG.get_or_init(Config::default)
}

/// The active palette, the part of the config nearly every caller wants.
pub fn palette() -> &'static Palette {
    &config().palette
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

    let _ = CONFIG.set(resolved);
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Most of these tests are about colors; unwrap the palette for them.
    fn parse_palette(text: &str) -> Result<Palette> {
        Ok(parse(text)?.palette)
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
        assert_eq!(c.palette.structure.helix, Rgb::new(0x11, 0x22, 0x33));
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
            "#,
        )
        .unwrap();
        assert_eq!(c.defaults.render, Some(RenderMode::HalfBlockPlus));
        assert_eq!(c.defaults.mode, Some(VizMode::Wireframe));
        assert_eq!(c.defaults.color, Some(ColorSchemeType::BFactor));
        assert_eq!(c.defaults.ball_and_stick, Some(false));
        assert_eq!(c.defaults.ligands, Some(false));
        assert_eq!(c.defaults.auto_rotate, Some(true));
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
