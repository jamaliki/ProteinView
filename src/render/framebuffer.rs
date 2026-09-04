use image::{RgbImage, RgbaImage};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use rayon::prelude::*;
use std::sync::OnceLock;

use crate::config::{Fog, palette};

/// The palette's background color, if it configures one.
///
/// Read at conversion time rather than baked into the framebuffer: finite
/// depth is the authoritative coverage mask, so painting the background into
/// every pixel would make empty space look drawn.
fn background_color() -> Option<[u8; 3]> {
    palette().background.map(|rgb| rgb.0)
}

/// The fog's own parameters -- reference depth, ceiling, curvature and chroma
/// drain -- live on [`Fog`], where the config file can reach them.  The defaults
/// there are the values this renderer was tuned at, and the doc comments on each
/// field say what moving one does.
///
/// Entries in the per-frame fog ramp table.
///
/// The ramp needs an `exp` per pixel, which at 4 MP costs more than the rest
/// of the pass put together.  Tabulating it quantizes the blend to under
/// 1/1000, well inside one 8-bit color step.
const FOG_RAMP_STEPS: usize = 1024;

/// Fog one pixel: drain its chroma, then blend it toward the fog colour.
///
/// Draining first is what makes the two cues add rather than cancel.  Blending
/// a saturated colour toward a dark blue-gray shifts its hue as much as its
/// brightness, so a distant magenta arrives somewhere near the fog's own hue
/// while still looking like a colour; pulling it toward its own grey first
/// means what the fog then tints is already neutral, and the pixel reads as
/// "further away" rather than "differently coloured".
///
/// f32 throughout, and the chroma drain is skipped when it is off: this runs
/// once per drawn pixel of every frame, where a shallow structure's ramp never
/// desaturates at all and would otherwise pay for a luma it does not use.
#[inline]
fn blend_fog(c: &mut [u8; 3], fog_color: [u8; 3], blend: f32, desaturation: f32) {
    if desaturation <= 0.0 {
        for (channel, &fog) in c.iter_mut().zip(fog_color.iter()) {
            let own = f32::from(*channel);
            *channel = own.mul_add(1.0 - blend, f32::from(fog) * blend) as u8;
        }
        return;
    }

    // Rec. 601 luma. Cheap, and close enough to perceived brightness that a
    // fully drained pixel keeps the weight it had against its neighbours.
    let luma = 0.299f32.mul_add(
        f32::from(c[0]),
        0.587f32.mul_add(f32::from(c[1]), 0.114 * f32::from(c[2])),
    );
    for (channel, &fog) in c.iter_mut().zip(fog_color.iter()) {
        let own = f32::from(*channel);
        let drained = (luma - own).mul_add(desaturation, own);
        *channel = (f32::from(fog) - drained).mul_add(blend, drained) as u8;
    }
}

/// Manhattan distance between normalized RGB chromaticities. Pure brightness
/// changes have distance zero, while distinct material hues approach two.
fn chroma_distance(a: [u8; 3], b: [u8; 3]) -> f32 {
    let a_sum = f32::from(a[0]) + f32::from(a[1]) + f32::from(a[2]);
    let b_sum = f32::from(b[0]) + f32::from(b[1]) + f32::from(b[2]);
    if a_sum == 0.0 || b_sum == 0.0 {
        return if a == b { 0.0 } else { 2.0 };
    }
    a.iter()
        .zip(b.iter())
        .map(|(&left, &right)| (f32::from(left) / a_sum - f32::from(right) / b_sum).abs())
        .sum()
}

fn rgb_key(color: [u8; 3]) -> u32 {
    (u32::from(color[0]) << 16) | (u32::from(color[1]) << 8) | u32::from(color[2])
}

/// RGB pixel framebuffer with z-buffer for software rasterization.
///
/// Pixel coordinates: (0,0) is top-left, x increases right, y increases down.
/// The framebuffer dimensions are in *pixels*, not terminal cells.
/// For half-block rendering each terminal row maps to 2 pixel rows,
/// so `height` should typically be `terminal_rows * 2`.
pub struct Framebuffer {
    pub width: usize,
    pub height: usize,
    /// RGB color per pixel, row-major: index = y * width + x
    pub color: Vec<[u8; 3]>,
    /// Depth (z) per pixel for z-buffer tests. Smaller z = closer to viewer.
    /// Uses f32 to halve memory bandwidth for depth operations and improve
    /// cache utilization (~7 decimal digits is more than enough for screen-space z).
    pub depth: Vec<f32>,
}

/// A triangle in screen space, ready for rasterization.
#[allow(dead_code)]
pub struct Triangle {
    /// Three vertices in screen-space [x, y, z].
    /// x,y are pixel coordinates; z is depth for z-buffering.
    pub verts: [[f64; 3]; 3],
    /// Base RGB color before shading is applied.
    pub color: [u8; 3],
    /// Unit face normal in world/view space for Lambert shading.
    pub normal: [f64; 3],
}

impl Framebuffer {
    /// Create a new framebuffer initialized to black with infinite depth.
    ///
    /// Width and height are clamped to at least 1 to avoid creating an empty
    /// framebuffer where any `set_pixel` call would panic with index-out-of-bounds.
    pub fn new(width: usize, height: usize) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let n = width * height;
        Self {
            width,
            height,
            color: vec![[0, 0, 0]; n],
            depth: vec![f32::INFINITY; n],
        }
    }

    /// Reset the framebuffer to black pixels and infinite depth.
    #[cfg(test)]
    pub fn clear(&mut self) {
        for c in self.color.iter_mut() {
            *c = [0, 0, 0];
        }
        for d in self.depth.iter_mut() {
            *d = f32::INFINITY;
        }
    }

    /// Set a single pixel if it passes the z-buffer test.
    #[inline]
    fn set_pixel(&mut self, x: usize, y: usize, z: f32, color: [u8; 3]) {
        let idx = y * self.width + x;
        if z < self.depth[idx] {
            self.depth[idx] = z;
            self.color[idx] = color;
        }
    }

    /// Convenience wrapper around `rasterize_triangle_depth` for tests.
    /// See that method for shading details.
    #[cfg(test)]
    pub fn rasterize_triangle(&mut self, tri: &Triangle, light_dir: [f64; 3]) {
        self.rasterize_triangle_depth(tri, light_dir);
    }

    /// Rasterize a triangle with Lambert shading and z-buffering.
    ///
    /// `light_dir` should be a *unit* vector pointing toward the light source.
    /// The triangle's `normal` is expected to be a unit vector as well.
    ///
    /// Shading uses two-sided half-Lambert wrap lighting with an ambient term.
    /// Depth fog is handled separately via [`apply_depth_tint`] as a post-pass.
    #[allow(dead_code)]
    pub fn rasterize_triangle_depth(&mut self, tri: &Triangle, light_dir: [f64; 3]) {
        const AMBIENT: f64 = 0.55;

        // --- Two-sided Lambert shading with wrap lighting ---
        // Use abs(dot) so back-facing triangles also get proper lighting,
        // then apply a half-Lambert wrap to soften the falloff
        let dot = tri.normal[0] * light_dir[0]
            + tri.normal[1] * light_dir[1]
            + tri.normal[2] * light_dir[2];
        let half_lambert = dot.abs() * 0.4 + 0.6;
        let intensity = AMBIENT + (1.0 - AMBIENT) * half_lambert;

        // Precompute flat shaded color.
        let shaded: [u8; 3] = [
            (tri.color[0] as f64 * intensity).min(255.0) as u8,
            (tri.color[1] as f64 * intensity).min(255.0) as u8,
            (tri.color[2] as f64 * intensity).min(255.0) as u8,
        ];

        // --- Extract vertices ---
        let [v0, v1, v2] = tri.verts;

        // --- Bounding box (clamped to framebuffer) ---
        let min_x = v0[0].min(v1[0]).min(v2[0]).floor() as isize;
        let max_x = v0[0].max(v1[0]).max(v2[0]).ceil() as isize;
        let min_y = v0[1].min(v1[1]).min(v2[1]).floor() as isize;
        let max_y = v0[1].max(v1[1]).max(v2[1]).ceil() as isize;

        let min_x = min_x.max(0) as usize;
        let max_x = max_x.max(0).min(self.width as isize - 1) as usize;
        let min_y = min_y.max(0) as usize;
        let max_y = max_y.max(0).min(self.height as isize - 1) as usize;

        // Triangle entirely off-screen
        if min_x > max_x || min_y > max_y {
            return;
        }

        // --- Precompute barycentric denominator ---
        // For vertices A(v0), B(v1), C(v2), the signed area * 2:
        //   denom = (B.y - C.y)*(A.x - C.x) + (C.x - B.x)*(A.y - C.y)
        let denom = (v1[1] - v2[1]) * (v0[0] - v2[0]) + (v2[0] - v1[0]) * (v0[1] - v2[1]);
        if denom.abs() < 1e-12 {
            return; // degenerate triangle
        }
        let inv_denom = 1.0 / denom;

        // --- Precompute x-step and y-offset terms for barycentric coords ---
        // u = u_x_step * (pf_x - v2[0]) + u_y
        // v = v_x_step * (pf_x - v2[0]) + v_y
        // where u_y, v_y depend only on pf_y (constant per scanline).
        let u_x_step = (v1[1] - v2[1]) * inv_denom;
        let v_x_step = (v2[1] - v0[1]) * inv_denom;
        let u_y_coeff = (v2[0] - v1[0]) * inv_denom;
        let v_y_coeff = (v0[0] - v2[0]) * inv_denom;

        // --- Rasterize pixels in bounding box ---
        for py in min_y..=max_y {
            let pf_y = py as f64 + 0.5; // pixel center
            let dy = pf_y - v2[1];
            let u_y = u_y_coeff * dy;
            let v_y = v_y_coeff * dy;

            for px in min_x..=max_x {
                let pf_x = px as f64 + 0.5; // pixel center
                let dx = pf_x - v2[0];

                // Barycentric coordinates (hoisted: 3 muls + 3 adds vs 8 muls + 6 adds)
                let u = u_x_step * dx + u_y;
                let v = v_x_step * dx + v_y;
                let w = 1.0 - u - v;

                // Inside test (with a tiny epsilon for edge cases)
                if u >= -1e-6 && v >= -1e-6 && w >= -1e-6 {
                    // Interpolate z (computed in f64 for precision, stored as f32)
                    let z = u * v0[2] + v * v1[2] + w * v2[2];
                    self.set_pixel(px, py, z as f32, shaded);
                }
            }
        }
    }

    /// Apply a depth-based color tint to all rasterized pixels in the framebuffer.
    ///
    /// This is a post-pass that runs after all geometry has been rasterized.
    /// For each pixel with a valid depth (not `f32::INFINITY`), its color is
    /// lerped toward `fog_color` based on how far it is from the camera:
    ///
    /// - Nearest pixels (z == z_min) keep their original color
    /// - Farthest pixels (z == z_max) are blended most toward `fog_color`
    /// - The `fog_strength` parameter (0.0..=1.0) sets the blend at the far
    ///   plane for a structure no deeper than [`Fog::reference_depth`]
    ///
    /// # Why the ramp bends for deep structures
    ///
    /// The ramp is normalized across the structure's own depth range, so the
    /// contrast between two features a fixed distance apart is inversely
    /// proportional to how deep the whole structure is.  At a fixed strength a
    /// 10 A separation is a clear 0.065 of blend in a small protein but 0.015
    /// in a ribosome -- invisible, which is exactly when depth cues matter
    /// most, because that is when everything overlaps.
    ///
    /// Raising the far-plane strength alone does not fix that: it spreads the
    /// extra contrast evenly over a 227 A span, and 8XT3 still renders as flat
    /// confetti.  What the eye needs is contrast where the geometry is, and in
    /// a dense structure viewed head-on most visible pixels sit in the front
    /// half (median depth 0.32 of the span for 8XT3, because the back is only
    /// glimpsed through gaps).  Bending the ramp exponentially concentrates
    /// the budget there, in the same way real distance fog does, at the cost
    /// of flattening the far end that was barely readable anyway.
    ///
    /// Luminance is also not enough on its own.  Against a saturated chain
    /// palette a 0.4 blend toward a dark blue-gray still reads as vivid green,
    /// so the ramp drains chroma as well ([`Fog::desaturation`]); that is
    /// the cue that survives when near and far material interleaves pixel by
    /// pixel.
    ///
    /// All three terms -- strength, curvature and desaturation -- are keyed to
    /// the span/reference ratio and vanish together at it, so structures at or
    /// below the reference depth keep exactly the appearance they always had.
    pub fn apply_depth_tint(&mut self, fog: &Fog) {
        if fog.strength <= 0.0 {
            return; // fog turned off in the config -- nothing to do
        }
        let fog_color = fog.color.0;
        // Find z_min and z_max across all valid (non-background) pixels.
        let (z_min, z_max) = self
            .depth
            .par_iter()
            .filter(|d| **d < f32::INFINITY)
            .fold(
                || (f32::INFINITY, f32::NEG_INFINITY),
                |(lo, hi), &d| (lo.min(d), hi.max(d)),
            )
            .reduce(
                || (f32::INFINITY, f32::NEG_INFINITY),
                |a, b| (a.0.min(b.0), a.1.max(b.1)),
            );

        // No valid pixels, or all at the same depth — nothing to tint.
        let z_range = z_max - z_min;
        if z_range.abs() < 1e-6 {
            return;
        }

        // Depth is in world units (angstroms): the projection applies zoom only
        // to x and y, so this scaling is independent of how far the user has
        // zoomed in.
        let strength = (fog.strength * f64::from(z_range) / fog.reference_depth)
            .clamp(fog.strength, fog.max_strength);

        let inv_range = 1.0 / z_range;
        let span_ratio = f64::from(z_range) / fog.reference_depth;

        // Small structures take the straight ramp, untouched and untabulated,
        // so their frames stay bit-for-bit what they were.
        if span_ratio <= 1.0 {
            self.color
                .par_iter_mut()
                .zip(self.depth.par_iter())
                .for_each(|(c, &d)| {
                    if d >= f32::INFINITY {
                        return; // background pixel — leave black
                    }
                    let t = ((d - z_min) * inv_range).clamp(0.0, 1.0);
                    blend_fog(c, fog_color, t * strength as f32, 0.0);
                });
            return;
        }

        let curvature = fog.curve_gain * span_ratio.ln();
        let curve_norm = 1.0 / (1.0 - (-curvature).exp());
        // Ties the chroma drain to the same ratio as the other two terms, so a
        // structure that only just clears the reference depth barely changes.
        let desaturation = fog.desaturation * (1.0 - 1.0 / span_ratio);

        let mut blend_ramp = [0.0f32; FOG_RAMP_STEPS];
        let mut desat_ramp = [0.0f32; FOG_RAMP_STEPS];
        for (i, (blend, desat)) in blend_ramp.iter_mut().zip(desat_ramp.iter_mut()).enumerate() {
            let t = i as f64 / (FOG_RAMP_STEPS - 1) as f64;
            let fraction = (1.0 - (-curvature * t).exp()) * curve_norm;
            *blend = (fraction * strength) as f32;
            *desat = (fraction * desaturation) as f32;
        }
        let last_step = (FOG_RAMP_STEPS - 1) as f32;

        // Per-pixel and independent -- parallelize over rows.
        self.color
            .par_iter_mut()
            .zip(self.depth.par_iter())
            .for_each(|(c, &d)| {
                if d >= f32::INFINITY {
                    return; // background pixel — leave black
                }
                let t = ((d - z_min) * inv_range).clamp(0.0, 1.0);
                let step = ((t * last_step) as usize).min(FOG_RAMP_STEPS - 1);
                blend_fog(c, fog_color, blend_ramp[step], desat_ramp[step]);
            });
    }

    /// Trace screen-space structure edges around and within rendered geometry.
    ///
    /// The original depth mask is expanded by `radius` pixels for the exterior
    /// ring. Internal contours are added on the farther side of occlusion depth
    /// jumps and on one side of abrupt chroma changes, separating overlapping
    /// structures and differently colored regions without double-width lines.
    /// A finite sentinel depth marks the exterior ring as rendered while
    /// keeping it behind real geometry during braille color selection.
    pub fn apply_outline(&mut self, color: [u8; 3], radius: usize) {
        if radius == 0 {
            return;
        }

        let original: Vec<u8> = self
            .depth
            .par_iter()
            .map(|depth| u8::from(depth.is_finite()))
            .collect();
        if !original.iter().any(|occupied| *occupied != 0) {
            return;
        }

        // Detect internal structure boundaries before expanding the mask.
        // Color is compared as chromaticity, making the test insensitive to
        // Lambert brightness changes across one material. Depth contours are
        // drawn on the farther surface, like an ink line at an occlusion.
        const DEPTH_EDGE_THRESHOLD: f32 = 0.8;
        let internal_edges: Vec<u8> = (0..self.depth.len())
            .into_par_iter()
            .map(|index| {
                if original[index] == 0 {
                    return 0;
                }
                let x = index % self.width;
                let y = index / self.width;
                let y0 = y.saturating_sub(1);
                let y1 = (y + 1).min(self.height - 1);
                let x0 = x.saturating_sub(1);
                let x1 = (x + 1).min(self.width - 1);
                let depth = self.depth[index];
                let pixel = self.color[index];
                for near_y in y0..=y1 {
                    for near_x in x0..=x1 {
                        let near_index = near_y * self.width + near_x;
                        if original[near_index] == 0 || near_index == index {
                            continue;
                        }
                        if self.depth[near_index] + DEPTH_EDGE_THRESHOLD < depth {
                            return 1;
                        }
                        let near_pixel = self.color[near_index];
                        if chroma_distance(pixel, near_pixel) > 0.45
                            && rgb_key(pixel) < rgb_key(near_pixel)
                        {
                            return 1;
                        }
                    }
                }
                0
            })
            .collect();

        let mut expanded = original.clone();
        for _ in 0..radius {
            let previous = expanded;
            let mut next = previous.clone();
            next.par_chunks_mut(self.width)
                .enumerate()
                .for_each(|(y, row)| {
                    for (x, occupied) in row.iter_mut().enumerate() {
                        if *occupied != 0 {
                            continue;
                        }
                        let y0 = y.saturating_sub(1);
                        let y1 = (y + 1).min(self.height - 1);
                        let x0 = x.saturating_sub(1);
                        let x1 = (x + 1).min(self.width - 1);
                        if (y0..=y1).any(|near_y| {
                            (x0..=x1).any(|near_x| previous[near_y * self.width + near_x] != 0)
                        }) {
                            *occupied = 1;
                        }
                    }
                });
            expanded = next;
        }

        self.color
            .par_iter_mut()
            .zip(self.depth.par_iter_mut())
            .zip(
                original
                    .par_iter()
                    .zip(expanded.par_iter())
                    .zip(internal_edges.par_iter()),
            )
            .for_each(
                |((pixel, depth), ((&was_occupied, &is_occupied), &internal_edge))| {
                    if internal_edge != 0 {
                        *pixel = color;
                    } else if was_occupied == 0 && is_occupied != 0 {
                        *pixel = color;
                        *depth = f32::MAX;
                    }
                },
            );
    }

    /// Cohen-Sutherland line clipping against framebuffer bounds [0, width) x [0, height).
    ///
    /// Returns `Some((clipped_p1, clipped_p2))` with z interpolated at clipped
    /// endpoints, or `None` if the line is entirely outside the framebuffer.
    fn clip_line_3d(&self, p1: [f64; 3], p2: [f64; 3]) -> Option<([f64; 3], [f64; 3])> {
        // Outcode bit flags
        const INSIDE: u8 = 0b0000;
        const LEFT: u8 = 0b0001;
        const RIGHT: u8 = 0b0010;
        const BOTTOM: u8 = 0b0100;
        const TOP: u8 = 0b1000;

        let x_min = 0.0_f64;
        let y_min = 0.0_f64;
        let x_max = (self.width as f64) - 1.0;
        let y_max = (self.height as f64) - 1.0;

        let outcode = |x: f64, y: f64| -> u8 {
            let mut code = INSIDE;
            if x < x_min {
                code |= LEFT;
            } else if x > x_max {
                code |= RIGHT;
            }
            if y < y_min {
                code |= TOP;
            } else if y > y_max {
                code |= BOTTOM;
            }
            code
        };

        let mut x0 = p1[0];
        let mut y0 = p1[1];
        let mut z0 = p1[2];
        let mut x1 = p2[0];
        let mut y1 = p2[1];
        let mut z1 = p2[2];

        let mut code0 = outcode(x0, y0);
        let mut code1 = outcode(x1, y1);

        loop {
            if (code0 | code1) == INSIDE {
                // Both inside — accept
                return Some(([x0, y0, z0], [x1, y1, z1]));
            }
            if (code0 & code1) != INSIDE {
                // Both on same side — trivial reject
                return None;
            }

            // Pick the endpoint that is outside
            let code_out = if code0 != INSIDE { code0 } else { code1 };

            // Find intersection with clip boundary
            let dx = x1 - x0;
            let dy = y1 - y0;

            let (x, y, t);
            if (code_out & TOP) != 0 {
                // Clip against top edge (y_min)
                t = (y_min - y0) / dy;
                x = x0 + dx * t;
                y = y_min;
            } else if (code_out & BOTTOM) != 0 {
                // Clip against bottom edge (y_max)
                t = (y_max - y0) / dy;
                x = x0 + dx * t;
                y = y_max;
            } else if (code_out & RIGHT) != 0 {
                // Clip against right edge (x_max)
                t = (x_max - x0) / dx;
                x = x_max;
                y = y0 + dy * t;
            } else {
                // Clip against left edge (x_min)
                t = (x_min - x0) / dx;
                x = x_min;
                y = y0 + dy * t;
            }

            // Interpolate z at the clipped point
            let z = z0 + (z1 - z0) * t;

            if code_out == code0 {
                x0 = x;
                y0 = y;
                z0 = z;
                code0 = outcode(x0, y0);
            } else {
                x1 = x;
                y1 = y;
                z1 = z;
                code1 = outcode(x1, y1);
            }
        }
    }

    /// Draw a 3D line with Bresenham's algorithm and z-interpolation.
    ///
    /// Useful for wireframe and ball-and-stick rendering modes.
    /// `p1` and `p2` are `[x, y, z]` in screen/pixel space.
    /// Uses Cohen-Sutherland clipping to avoid long Bresenham walks for
    /// mostly off-screen lines.
    pub fn draw_line_3d(&mut self, p1: [f64; 3], p2: [f64; 3], color: [u8; 3]) {
        // Clip the line to framebuffer bounds before rasterizing.
        let (cp1, cp2) = match self.clip_line_3d(p1, p2) {
            Some(clipped) => clipped,
            None => return,
        };

        let mut x0 = cp1[0].round() as isize;
        let mut y0 = cp1[1].round() as isize;
        let x1 = cp2[0].round() as isize;
        let y1 = cp2[1].round() as isize;

        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx: isize = if x0 < x1 { 1 } else { -1 };
        let sy: isize = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;

        // Total Manhattan-ish distance for z interpolation
        let total_steps = dx.max(-dy) as f64;

        loop {
            // Compute interpolation parameter t
            let t = if total_steps > 0.0 {
                let from_start_x = (x0 - cp1[0].round() as isize).unsigned_abs() as f64;
                let from_start_y = (y0 - cp1[1].round() as isize).unsigned_abs() as f64;
                from_start_x.max(from_start_y) / total_steps
            } else {
                0.0
            };
            let z = (cp1[2] * (1.0 - t) + cp2[2] * t) as f32;

            // After clipping, all pixels should be in bounds, but guard anyway
            if x0 >= 0 && y0 >= 0 && (x0 as usize) < self.width && (y0 as usize) < self.height {
                self.set_pixel(x0 as usize, y0 as usize, z, color);
            }

            if x0 == x1 && y0 == y1 {
                break;
            }

            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    /// Draw a 3D dashed line with z-interpolation.
    ///
    /// Like [`draw_line_3d`] but alternates between drawing (`dash_len` pixels)
    /// and skipping (`gap_len` pixels) based on accumulated pixel distance
    /// along the line.  Z-interpolation is maintained throughout so that the
    /// dashed segments interact correctly with the z-buffer.
    /// Uses Cohen-Sutherland clipping to avoid long Bresenham walks for
    /// mostly off-screen lines.
    pub fn draw_dashed_line_3d(
        &mut self,
        p1: [f64; 3],
        p2: [f64; 3],
        color: [u8; 3],
        dash_len: f64,
        gap_len: f64,
    ) {
        // Guard: if cycle is zero, non-finite, or either length is negative,
        // fall back to a solid line to avoid NaN from `accumulated % 0.0`.
        let cycle = dash_len + gap_len;
        if cycle <= 0.0 || !cycle.is_finite() || dash_len < 0.0 || gap_len < 0.0 {
            self.draw_line_3d(p1, p2, color);
            return;
        }

        // Clip the line to framebuffer bounds before rasterizing.
        let (cp1, cp2) = match self.clip_line_3d(p1, p2) {
            Some(clipped) => clipped,
            None => return,
        };

        let mut x0 = cp1[0].round() as isize;
        let mut y0 = cp1[1].round() as isize;
        let x1 = cp2[0].round() as isize;
        let y1 = cp2[1].round() as isize;

        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx: isize = if x0 < x1 { 1 } else { -1 };
        let sy: isize = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;

        // Total Manhattan-ish distance for z interpolation.
        let total_steps = dx.max(-dy) as f64;

        let mut accumulated: f64 = 0.0;
        let mut prev_x = x0;
        let mut prev_y = y0;

        loop {
            // Compute interpolation parameter t for z.
            let t = if total_steps > 0.0 {
                let from_start_x = (x0 - cp1[0].round() as isize).unsigned_abs() as f64;
                let from_start_y = (y0 - cp1[1].round() as isize).unsigned_abs() as f64;
                from_start_x.max(from_start_y) / total_steps
            } else {
                0.0
            };
            let z = (cp1[2] * (1.0 - t) + cp2[2] * t) as f32;

            // Accumulate Euclidean distance from previous pixel.
            let step_dx = (x0 - prev_x) as f64;
            let step_dy = (y0 - prev_y) as f64;
            accumulated += (step_dx * step_dx + step_dy * step_dy).sqrt();
            prev_x = x0;
            prev_y = y0;

            // Determine if we are in a dash or gap segment.
            let phase = accumulated % cycle;
            let drawing = phase < dash_len;

            // After clipping, all pixels should be in bounds, but guard anyway
            if drawing
                && x0 >= 0
                && y0 >= 0
                && (x0 as usize) < self.width
                && (y0 as usize) < self.height
            {
                self.set_pixel(x0 as usize, y0 as usize, z, color);
            }

            if x0 == x1 && y0 == y1 {
                break;
            }

            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    /// Draw a bond as a shaded cylinder of the given pixel thickness.
    ///
    /// The old implementation stamped the same flat colour along a fan of
    /// half-pixel-offset Bresenham lines.  That has two visible faults: a
    /// diagonal bond's edges come out combed, because neighbouring offset lines
    /// land on the same pixel row, and a bar of one constant colour next to
    /// shaded spheres reads as a flat ribbon rather than a stick.
    ///
    /// Rasterizing the bar as an oriented rectangle instead gives every pixel
    /// an exact position along and across the bond, and across it the surface
    /// normal sweeps the way a cylinder's cross-section does.  Feeding that
    /// normal through the same [`shade_sphere`] the atoms use is what makes a
    /// stick meet a ball without a seam.
    ///
    /// `world_per_px` converts the pixel radius into the depth buffer's world
    /// units; see [`draw_sphere`](Self::draw_sphere).  For `thickness` values
    /// <= 1.0 this falls back to the single-pixel `draw_line_3d`.
    pub fn draw_thick_line_3d(
        &mut self,
        p1: [f64; 3],
        p2: [f64; 3],
        color: [u8; 3],
        thickness: f64,
        world_per_px: f64,
    ) {
        if thickness <= 1.0 {
            self.draw_line_3d(p1, p2, color);
            return;
        }

        let half = thickness / 2.0;

        // Direction vector in screen-space (xy only).
        let dx = p2[0] - p1[0];
        let dy = p2[1] - p1[1];
        let len = (dx * dx + dy * dy).sqrt();

        if len < 1e-6 {
            // Degenerate (zero-length) line – just draw a dot.
            self.draw_sphere(p1[0], p1[1], p1[2], half, world_per_px, color);
            return;
        }

        // Axis and perpendicular unit vectors in screen space.
        let ax = dx / len;
        let ay = dy / len;
        let ux = -ay;
        let uy = ax;

        let iy_min = ((p1[1].min(p2[1]) - half).floor() as isize).max(0) as usize;
        let iy_max = ((p1[1].max(p2[1]) + half).ceil() as isize)
            .max(0)
            .min(self.height as isize - 1) as usize;
        if iy_min > iy_max {
            return;
        }

        let inv_half = 1.0 / half;
        let dz_dt = (p2[2] - p1[2]) / len;
        let world_radius = half * world_per_px;
        let light = surface_light();
        let color = widen(color);
        let x_last = self.width as f64 - 1.0;

        // Both spans are affine in x with a coefficient that is the same on
        // every scanline, so the reciprocals -- and the degenerate axis-aligned
        // cases, where one of the two stops depending on x at all -- come out of
        // the loop.  A division per scanline costs more than the pixels it saves
        // on the short bonds that make up most of a structure.
        let (ux_flat, inv_ux) = (ux.abs() < 1e-12, 1.0 / ux);
        let (ax_flat, inv_ax) = (ax.abs() < 1e-12, 1.0 / ax);

        for py in iy_min..=iy_max {
            let fy = py as f64 + 0.5 - p1[1];
            let cu = fy * uy;
            let ct = fy * ay;
            let mut lo = f64::NEG_INFINITY;
            let mut hi = f64::INFINITY;
            if ux_flat {
                if cu.abs() > half {
                    continue;
                }
            } else if !clip_span(inv_ux, -half - cu, half - cu, &mut lo, &mut hi) {
                continue;
            }
            if ax_flat {
                if ct < 0.0 || ct > len {
                    continue;
                }
            } else if !clip_span(inv_ax, -ct, len - ct, &mut lo, &mut hi) {
                continue;
            }

            // `lo`/`hi` bound the pixel *centre* offset from p1, so shift by the
            // half-pixel before rounding to whole columns.
            let first = (lo + p1[0] - 0.5).ceil().max(0.0);
            let last = (hi + p1[0] - 0.5).floor().min(x_last);
            if first > last {
                continue;
            }

            // Position across and along the bar both step by a constant per
            // pixel, so the span walks them instead of recomputing.
            let fx = first + 0.5 - p1[0];
            let mut u = fx * ux + cu;
            let mut t = fx * ax + ct;
            for px in (first as usize)..=(last as usize) {
                // Signed position across the bar, in radii: the cylinder's
                // normal tips from one limb to the other over this range.
                let n = u * inv_half;
                let nz = (1.0 - n * n).max(0.0).sqrt();
                let z = p1[2] + dz_dt * t - nz * world_radius;
                let shaded = shade_sphere(ux * n, uy * n, nz, light, color);
                self.set_pixel(px, py, z as f32, shaded);
                u += ux;
                t += ax;
            }
        }
    }

    /// Draw a filled circle at pixel coordinates `(cx, cy)` with the given radius and color.
    #[cfg(test)]
    pub fn draw_circle(&mut self, cx: f64, cy: f64, radius: f64, color: [u8; 3]) {
        self.draw_sphere(cx, cy, 0.0, radius, 0.0, color);
    }

    /// Convert this framebuffer's pixel data into an `image::RgbImage`.
    ///
    /// The resulting image has the same width and height as the framebuffer,
    /// with each pixel's RGB channels copied directly.  This is used by the
    /// ratatui-image integration to send the framebuffer to the terminal via
    /// Sixel, Kitty, or other graphics protocols.
    ///
    /// Empty space takes the palette's background color, or stays black when
    /// none is configured -- black being what an RGB image has to put there,
    /// having no alpha to say "nothing here" with.
    pub fn to_rgb_image(&self) -> RgbImage {
        let background = background_color().unwrap_or([0, 0, 0]);
        let mut buf = vec![0u8; self.color.len() * 3];
        buf.par_chunks_mut(3)
            .zip(self.color.par_iter())
            .zip(self.depth.par_iter())
            .for_each(|((out, c), d)| {
                out.copy_from_slice(if *d >= f32::INFINITY { &background } else { c })
            });
        RgbImage::from_raw(self.width as u32, self.height as u32, buf)
            .expect("buffer is exactly width * height * 3 bytes")
    }

    /// Write this framebuffer into `dst` as RGBA8, one byte per channel.
    ///
    /// `dst` must be exactly `width * height * 4` bytes; the caller owns the
    /// allocation, which lets the interactive path write straight into a shared
    /// memory mapping instead of building an intermediate image.
    ///
    /// Background pixels (depth == INFINITY) get alpha = 0 so the terminal
    /// background shows through, or the palette's background color at full
    /// alpha when one is configured.  Drawn pixels get alpha = 255.
    ///
    /// # Panics
    ///
    /// If `dst` is not exactly `width * height * 4` bytes long.
    pub fn write_rgba(&self, dst: &mut [u8]) {
        assert_eq!(
            dst.len(),
            self.color.len() * 4,
            "destination must be exactly width * height * 4 bytes"
        );
        // Per-pixel and independent; a full frame at Retina resolution is tens
        // of megabytes, enough for the copy to be worth spreading over cores.
        let background = background_color();
        dst.par_chunks_mut(4)
            .zip(self.color.par_iter())
            .zip(self.depth.par_iter())
            .for_each(|((out, c), d)| {
                let (color, alpha) = if *d < f32::INFINITY {
                    (*c, 255)
                } else {
                    // Nothing here: the configured background at full alpha, or
                    // transparent so the terminal shows through.
                    match background {
                        Some(bg) => (bg, 255),
                        None => (*c, 0),
                    }
                };
                out[0] = color[0];
                out[1] = color[1];
                out[2] = color[2];
                out[3] = alpha;
            });
    }

    /// Convert this framebuffer into an `image::RgbaImage` with transparency.
    ///
    /// Background pixels (depth == INFINITY, color == black) get alpha = 0 so
    /// the terminal background shows through.  Drawn pixels get alpha = 255.
    pub fn to_rgba_image(&self) -> RgbaImage {
        let mut buf = vec![0u8; self.color.len() * 4];
        self.write_rgba(&mut buf);
        RgbaImage::from_raw(self.width as u32, self.height as u32, buf)
            .expect("buffer is exactly width * height * 4 bytes")
    }

    /// Draw an atom as a shaded sphere centred at `(cx, cy)` and depth `z`.
    ///
    /// Every pixel gets the depth of the sphere's *front surface*, not the
    /// centre's.  Writing one flat depth made two overlapping atoms sort
    /// entirely by centre, so their intersection came out as a hard straight
    /// edge -- a seam no ball-and-stick renderer has -- and the depth fog read
    /// one distance for the whole disc.  A sphere costs the same square root
    /// the shading already needed, so the curve is close to free.
    ///
    /// `radius` is in pixels but depth is in world units, so `world_per_px`
    /// (the reciprocal of the camera's zoom) converts between them.  Passing
    /// zero keeps the old flat disc, which is what a caller with no camera
    /// wants.
    pub fn draw_sphere(
        &mut self,
        cx: f64,
        cy: f64,
        z: f64,
        radius: f64,
        world_per_px: f64,
        color: [u8; 3],
    ) {
        let ix_min = ((cx - radius).floor() as isize).max(0) as usize;
        let ix_max = ((cx + radius).ceil() as isize)
            .max(0)
            .min(self.width as isize - 1) as usize;
        let iy_min = ((cy - radius).floor() as isize).max(0) as usize;
        let iy_max = ((cy + radius).ceil() as isize)
            .max(0)
            .min(self.height as isize - 1) as usize;

        // Circle entirely off-screen
        if ix_min > ix_max || iy_min > iy_max {
            return;
        }

        let r_sq = radius * radius;
        let inv_r = if radius > 0.0 { 1.0 / radius } else { 0.0 };
        let inv_r_sq = inv_r * inv_r;
        let light = surface_light();
        let color = widen(color);
        // Depth grows away from the viewer, so the near cap is *subtracted*.
        let world_radius = radius * world_per_px;

        for py in iy_min..=iy_max {
            let dy = py as f64 + 0.5 - cy;
            let dy_sq = dy * dy;
            if dy_sq > r_sq {
                continue;
            }
            let ny = dy * inv_r;
            for px in ix_min..=ix_max {
                let dx = px as f64 + 0.5 - cx;
                let d_sq = dx * dx + dy_sq;
                if d_sq <= r_sq {
                    let nz = (1.0 - d_sq * inv_r_sq).max(0.0).sqrt();
                    let shaded = shade_sphere(dx * inv_r, ny, nz, light, color);
                    self.set_pixel(px, py, (z - nz * world_radius) as f32, shaded);
                }
            }
        }
    }
}

/// Narrow `[lo, hi]` to the values of `x` that satisfy `min <= x / inv_coef <=
/// max`, given the reciprocal of the coefficient rather than the coefficient.
///
/// Returns false once the interval is empty, which lets a scanline bail before
/// it looks at a single pixel.  The caller must have ruled out a zero
/// coefficient, where the constraint stops depending on `x` at all.
#[inline]
fn clip_span(inv_coef: f64, min: f64, max: f64, lo: &mut f64, hi: &mut f64) -> bool {
    let (a, b) = (min * inv_coef, max * inv_coef);
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    *lo = lo.max(a);
    *hi = hi.min(b);
    *lo <= *hi
}

/// Quantize a single color channel by rounding to the nearest multiple of
/// `step`.  This reduces the number of distinct RGB triples in the output so
/// that more adjacent cells share the same color and get merged into longer
/// runs, dramatically cutting the number of ANSI escape sequences emitted per
/// frame.
///
/// A `step` of 1 is a no-op (full precision).  A `step` of 8 reduces 256
/// levels to 32 distinct values -- visually almost imperceptible but can cut
/// the span count (and therefore terminal output size) by 3-5x.
#[inline]
fn quantize_channel(v: u8, step: u8) -> u8 {
    if step <= 1 {
        return v;
    }
    let half = step / 2;
    // Round to nearest multiple of `step`, clamped to 255.
    let q = ((v as u16 + half as u16) / step as u16) * step as u16;
    q.min(255) as u8
}

/// Quantize an RGB triple.  Black `[0,0,0]` is kept exactly black so that the
/// blank-cell optimisation still fires.
#[inline]
fn quantize_color(c: [u8; 3], step: u8) -> [u8; 3] {
    if c == [0, 0, 0] {
        return c;
    }
    let q = [
        quantize_channel(c[0], step),
        quantize_channel(c[1], step),
        quantize_channel(c[2], step),
    ];
    // Avoid rounding a near-black color *to* black, which would make it
    // invisible.  Clamp to at least `step` in the brightest channel.
    if q == [0, 0, 0] {
        [step, step, step]
    } else {
        q
    }
}

/// Convert a [`Framebuffer`] into a ratatui [`Paragraph`] widget using half-block characters.
///
/// Each terminal row maps to two pixel rows:
/// - Top pixel  -> foreground color
/// - Bottom pixel -> background color
/// - Character: `'▀'` (upper half block, U+2580)
///
/// Consecutive cells with identical (fg, bg) pairs are merged into a single
/// [`Span`] to reduce the number of styled segments ratatui needs to process.
///
/// Color quantization is available via `quant_step` but currently disabled
/// (`quant_step = 1`, a no-op).  Set it to e.g. 4 or 8 to reduce distinct
/// colors and increase run-length merging at the expense of color precision.
#[cfg(test)]
pub fn framebuffer_to_widget(fb: &Framebuffer) -> Paragraph<'static> {
    // Quantization step: 4 gives 64 levels per channel -- preserves smooth
    // shading gradients while still merging runs.  Use 1 (no quantization)
    // for tiny framebuffers where output is already small.
    let quant_step: u8 = 1;

    let term_rows = (fb.height + 1) / 2;
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(term_rows);

    for tr in 0..term_rows {
        let top_row = tr * 2;
        let bot_row = top_row + 1;

        let mut spans: Vec<Span<'static>> = Vec::new();

        // We classify each cell into one of 4 cases and track runs of same type
        // Case 0: blank (both black) → space, no styling (terminal bg shows through)
        // Case 1: both have color → '▀' with fg=top, bg=bottom
        // Case 2: top only → '▀' with fg=top, bg=Reset
        // Case 3: bottom only → '▄' with fg=bottom, bg=Reset
        #[derive(PartialEq, Clone, Copy)]
        enum CellKind {
            Blank,
            Both([u8; 3], [u8; 3]),
            TopOnly([u8; 3]),
            BotOnly([u8; 3]),
        }

        let mut run_text = String::new();
        let mut run_kind = CellKind::Blank;
        let mut run_started = false;

        let flush = |spans: &mut Vec<Span<'static>>, text: &str, kind: &CellKind| {
            if text.is_empty() {
                return;
            }
            let style = match kind {
                CellKind::Blank => Style::default(),
                CellKind::Both(top, bot) => Style::default()
                    .fg(Color::Rgb(top[0], top[1], top[2]))
                    .bg(Color::Rgb(bot[0], bot[1], bot[2])),
                CellKind::TopOnly(top) => Style::default().fg(Color::Rgb(top[0], top[1], top[2])),
                CellKind::BotOnly(bot) => Style::default().fg(Color::Rgb(bot[0], bot[1], bot[2])),
            };
            spans.push(Span::styled(text.to_string(), style));
        };

        for col in 0..fb.width {
            let top = quantize_color(fb.color[top_row * fb.width + col], quant_step);
            let bot = if bot_row < fb.height {
                quantize_color(fb.color[bot_row * fb.width + col], quant_step)
            } else {
                [0, 0, 0]
            };

            let top_black = top == [0, 0, 0];
            let bot_black = bot == [0, 0, 0];

            let kind = if top_black && bot_black {
                CellKind::Blank
            } else if !top_black && !bot_black {
                CellKind::Both(top, bot)
            } else if !top_black {
                CellKind::TopOnly(top)
            } else {
                CellKind::BotOnly(bot)
            };

            if run_started && kind == run_kind {
                match kind {
                    CellKind::Blank => run_text.push(' '),
                    CellKind::Both(..) | CellKind::TopOnly(_) => run_text.push('\u{2580}'),
                    CellKind::BotOnly(_) => run_text.push('\u{2584}'),
                }
            } else {
                if run_started {
                    flush(&mut spans, &run_text, &run_kind);
                }
                run_text.clear();
                match kind {
                    CellKind::Blank => run_text.push(' '),
                    CellKind::Both(..) | CellKind::TopOnly(_) => run_text.push('\u{2580}'),
                    CellKind::BotOnly(_) => run_text.push('\u{2584}'),
                }
                run_kind = kind;
                run_started = true;
            }
        }

        if run_started {
            flush(&mut spans, &run_text, &run_kind);
        }

        lines.push(Line::from(spans));
    }

    Paragraph::new(lines)
}

/// Convert a [`Framebuffer`] rendered at braille resolution into a ratatui
/// [`Paragraph`] widget using colored Unicode braille characters.
///
/// The framebuffer is expected to have dimensions `(cols * 2, rows * 4)` where
/// `cols` and `rows` are the target terminal cell dimensions.  Each terminal
/// cell maps to a 2x4 block of pixels.  Non-black pixels become "on" braille
/// dots; their average RGB color is used as the cell's foreground color.
///
/// This gives 4x the spatial resolution of half-block rendering at the cost of
/// per-cell (rather than per-pixel) coloring.
///
/// Consecutive cells with the same foreground color are merged into a single
/// [`Span`] for performance (run-length encoding).
pub fn framebuffer_to_braille_widget(fb: &Framebuffer) -> Paragraph<'static> {
    framebuffer_to_braille_widget_ssaa(fb, 1, 1)
}

/// Supersampled variant of [`framebuffer_to_braille_widget`].
///
/// The framebuffer is expected to have dimensions `(cols * 2 * ssaa, rows * 4 *
/// ssaa)`, so that each braille dot is backed by an `ssaa x ssaa` block of
/// samples rather than a single pixel.  Each block is box-filtered: the dot is
/// lit once the block is at least half covered, and the cell's foreground color
/// averages every covered sample of its lit dots.  This anti-aliases silhouettes
/// and keeps colors stable as the camera rotates, at no cost in emitted bytes --
/// the widget is still exactly `cols x rows` braille characters.
///
/// `quant_step` rounds each channel to a multiple of `step` before run-length
/// merging; `1` disables it.  Larger values merge many more cells into a single
/// [`Span`], which cuts the number of SGR color escapes written to the terminal
/// -- the dominant cost of this render path over SSH -- at a small loss of color
/// precision.
#[allow(clippy::needless_range_loop)]
pub fn framebuffer_to_braille_widget_ssaa(
    fb: &Framebuffer,
    ssaa: usize,
    quant_step: u8,
) -> Paragraph<'static> {
    let ssaa = ssaa.max(1);

    // Terminal cell grid dimensions derived from the framebuffer.
    let term_cols = fb.width.div_ceil(2 * ssaa);
    let term_rows = fb.height.div_ceil(4 * ssaa);

    if term_cols == 0 || term_rows == 0 {
        return Paragraph::new("");
    }

    // Braille dot bit values indexed by (dx, dy) within the 2x4 cell block.
    // Layout:
    //   Col 0  Col 1
    //   bit 0  bit 3   (row 0)
    //   bit 1  bit 4   (row 1)
    //   bit 2  bit 5   (row 2)
    //   bit 6  bit 7   (row 3)
    const BRAILLE_BITS: [[u8; 4]; 2] = [
        [0x01, 0x02, 0x04, 0x40], // column 0: rows 0-3
        [0x08, 0x10, 0x20, 0x80], // column 1: rows 0-3
    ];

    // Samples backing one braille dot, and the coverage needed to light it.
    // At `ssaa == 1` the threshold is 1, i.e. "any non-black pixel lights the
    // dot" -- identical to the non-supersampled behaviour.
    let samples_per_dot = ssaa * ssaa;
    let coverage_threshold = samples_per_dot.div_ceil(2) as u32;

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(term_rows);

    for tr in 0..term_rows {
        let py_base = tr * 4;

        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut run_text = String::new();
        // Track the current run's color; None means blank (space) run.
        let mut run_color: Option<[u8; 3]> = None;
        let mut run_started = false;

        // Every cell carries the background, lit or not: a braille cell's unlit
        // dots show the terminal through, so painting only the blank cells would
        // leave the structure's own cells a different color from the space
        // around them.
        let background = background_color();
        let flush = |spans: &mut Vec<Span<'static>>, text: &str, color: &Option<[u8; 3]>| {
            if text.is_empty() {
                return;
            }
            let mut style = match color {
                Some(c) => Style::default().fg(Color::Rgb(c[0], c[1], c[2])),
                None => Style::default(),
            };
            if let Some(bg) = background {
                style = style.bg(Color::Rgb(bg[0], bg[1], bg[2]));
            }
            spans.push(Span::styled(text.to_string(), style));
        };

        for tc in 0..term_cols {
            let px_base = tc * 2;

            // Build braille bit pattern and accumulate color of "on" dots.
            let mut bits: u8 = 0;
            let mut r_sum: u32 = 0;
            let mut g_sum: u32 = 0;
            let mut b_sum: u32 = 0;
            let mut on_count: u32 = 0;
            // Color of the frontmost covered sample in the cell.  A braille cell
            // carries a single foreground color, but its eight dots can straddle
            // several structures at different depths; averaging them blends a pink
            // helix and a green coil into brown.  Taking the nearest sample instead
            // shows what is actually in front, and keeps colors saturated.
            let mut cell_near_z = f32::INFINITY;
            let mut cell_near_c = [0u8; 3];

            for (dx, col_bits) in BRAILLE_BITS.iter().enumerate() {
                let sx0 = (px_base + dx) * ssaa;
                if sx0 >= fb.width {
                    continue;
                }
                for (dy, &bit) in col_bits.iter().enumerate() {
                    let sy0 = (py_base + dy) * ssaa;
                    if sy0 >= fb.height {
                        continue;
                    }

                    // Box-filter the sample block backing this dot.
                    let mut covered: u32 = 0;
                    let mut r: u32 = 0;
                    let mut g: u32 = 0;
                    let mut b: u32 = 0;
                    let mut dot_near_z = f32::INFINITY;
                    let mut dot_near_c = [0u8; 3];
                    for sy in sy0..(sy0 + ssaa).min(fb.height) {
                        let row = sy * fb.width;
                        for sx in sx0..(sx0 + ssaa).min(fb.width) {
                            let c = fb.color[row + sx];
                            let z = fb.depth[row + sx];
                            // Finite depth is the authoritative coverage mask.
                            // The color fallback preserves compatibility with
                            // test/utility framebuffers written without z.
                            if z.is_finite() || c != [0, 0, 0] {
                                covered += 1;
                                r += c[0] as u32;
                                g += c[1] as u32;
                                b += c[2] as u32;
                                if z < dot_near_z {
                                    dot_near_z = z;
                                    dot_near_c = c;
                                }
                            }
                        }
                    }

                    if covered >= coverage_threshold {
                        bits |= bit;
                        r_sum += r;
                        g_sum += g;
                        b_sum += b;
                        on_count += covered;
                        if dot_near_z < cell_near_z {
                            cell_near_z = dot_near_z;
                            cell_near_c = dot_near_c;
                        }
                    }
                }
            }

            if bits == 0 {
                // All dots off — emit a space.
                let cell_color: Option<[u8; 3]> = None;
                if run_started && run_color == cell_color {
                    run_text.push(' ');
                } else {
                    if run_started {
                        flush(&mut spans, &run_text, &run_color);
                    }
                    run_text.clear();
                    run_text.push(' ');
                    run_color = cell_color;
                    run_started = true;
                }
            } else {
                // Compute average color of "on" pixels.
                // Fall back to the mean when no covered sample carried a finite
                // depth -- a framebuffer whose colors were written without z.
                let raw = if cell_near_z.is_finite() {
                    cell_near_c
                } else {
                    [
                        (r_sum / on_count) as u8,
                        (g_sum / on_count) as u8,
                        (b_sum / on_count) as u8,
                    ]
                };
                let avg = quantize_color(raw, quant_step);
                let cell_color = Some(avg);

                let braille_char = char::from_u32(0x2800u32 + bits as u32).unwrap_or(' ');

                if run_started && run_color == cell_color {
                    run_text.push(braille_char);
                } else {
                    if run_started {
                        flush(&mut spans, &run_text, &run_color);
                    }
                    run_text.clear();
                    run_text.push(braille_char);
                    run_color = cell_color;
                    run_started = true;
                }
            }
        }

        if run_started {
            flush(&mut spans, &run_text, &run_color);
        }

        lines.push(Line::from(spans));
    }

    Paragraph::new(lines)
}

/// Normalize a 3-component vector in place and return the result.
/// If the vector has zero length, returns `[0.0, 0.0, 0.0]`.
pub fn normalize(v: [f64; 3]) -> [f64; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-12 {
        [0.0, 0.0, 0.0]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

/// Default light direction (normalized) pointing from the upper-right-front.
///
/// The X component is negative to compensate for the camera's X-negation
/// (which corrects chirality so L-amino acids render correctly).  In screen
/// space this still appears as upper-right lighting.
/// Ambient floor for sphere shading, and with it the contrast across an atom.
///
/// Kept a little above the ribbon's own floor: an atom is often only a few
/// pixels across, where the full ribbon contrast turns into high-frequency
/// speckle rather than form.  A one-sided Lambert term would average barely
/// half the colour, which read as markedly darker than the flat discs it
/// replaces once depth fog was applied on top.
const SPHERE_AMBIENT: f64 = 0.50;

/// Weight of the specular highlight, as a fraction of full white.
///
/// A diffuse-only ball reads as matte plastic; the small bright spot is most of
/// what makes a molecular viewer's atoms look like the glossy spheres people
/// recognise.  Kept well under half so it reads as a highlight on the atom's
/// own colour rather than bleaching a hole in it.
const SPHERE_SPECULAR: f64 = 0.30;

/// The ambient floor and the half-Lambert wrap, folded into the slope and
/// intercept of a single multiply-add: `intensity = slope * dot + base`.
const SPHERE_WRAP_SLOPE: f64 = (1.0 - SPHERE_AMBIENT) * 0.5;
const SPHERE_WRAP_BASE: f64 = SPHERE_AMBIENT + (1.0 - SPHERE_AMBIENT) * 0.5;

/// Directions needed to shade a curved surface, resolved once per primitive.
///
/// The Blinn-Phong halfway vector is a normalize away from the light, and a
/// primitive covers hundreds of pixels; computing it per pixel would double the
/// square roots in the inner loop for a value that never changes.
#[derive(Clone, Copy)]
pub struct SurfaceLight {
    dir: [f64; 3],
    half: [f64; 3],
}

impl SurfaceLight {
    /// `dir` points toward the light in a y-up view space with +z toward the
    /// viewer, which is where [`default_light_dir`] lives.
    fn new(dir: [f64; 3]) -> Self {
        // The viewer sits at +z in this space, so the halfway vector is the
        // light nudged one unit toward the camera.
        let half = normalize([dir[0], dir[1], dir[2] + 1.0]);
        Self { dir, half }
    }
}

/// The scene light, resolved once for the life of the process.
///
/// Every sphere and every stick wants the same two unit vectors, and a large
/// structure submits tens of thousands of primitives per frame; two square
/// roots apiece is pure overhead against a value that never changes.
fn surface_light() -> SurfaceLight {
    static LIGHT: OnceLock<SurfaceLight> = OnceLock::new();
    *LIGHT.get_or_init(|| SurfaceLight::new(default_light_dir()))
}

/// Shade one pixel of a curved surface -- a sphere's cap or a cylinder's flank.
///
/// Backbone and ligand atoms are drawn as filled circles of one flat colour.
/// On a small structure that is fine, but a large one is tens of thousands of
/// them overlapping, and a field of flat discs has no form at all -- the eye
/// gets no cue for which atom is in front of which, and the render reads as
/// confetti.  Treating each disc as the silhouette of a sphere costs one
/// square root per pixel and gives every atom a highlight and a shaded limb,
/// which is what makes a dense structure legible.
///
/// `nx`/`ny`/`nz` are the unit surface normal at this pixel.  Screen y runs
/// downward while the light is specified in a y-up view space, hence the flip;
/// `nz` is the viewer-facing component and is positive on everything the
/// caller can see.  `color` arrives already widened because the caller has one
/// colour for the whole primitive and this runs once per pixel.
#[inline]
fn shade_sphere(nx: f64, ny: f64, nz: f64, light: SurfaceLight, color: [f64; 3]) -> [u8; 3] {
    // The z axis here is the light's own, not the depth buffer's: `light` has a
    // positive z and is meant to sit in front of the scene, so the face of the
    // sphere pointing at the viewer takes +z and catches the highlight.  The
    // triangle rasterizer never had to pick a side because it shades with
    // `abs(dot)`; a sphere does, or the highlight lands on the wrong limb.
    let dot = nx * light.dir[0] - ny * light.dir[1] + nz * light.dir[2];
    // Half-Lambert wrap, as the triangle rasterizer uses: it keeps the unlit
    // limb from going flat black, which matters when atoms are only a few
    // pixels across and a hard terminator would just read as noise.  Ambient
    // and wrap are folded into one multiply-add; see the two constants.
    let intensity = dot.mul_add(SPHERE_WRAP_SLOPE, SPHERE_WRAP_BASE);

    // Blinn-Phong, raised to 32 by repeated squaring rather than `powf` -- five
    // multiplies against a transcendental call, on the hottest loop there is.
    let spec_dot = (nx * light.half[0] - ny * light.half[1] + nz * light.half[2]).max(0.0);
    let s2 = spec_dot * spec_dot;
    let s4 = s2 * s2;
    let s8 = s4 * s4;
    let s16 = s8 * s8;
    let spec = s16 * s16 * (SPHERE_SPECULAR * 255.0);

    [
        color[0].mul_add(intensity, spec).min(255.0) as u8,
        color[1].mul_add(intensity, spec).min(255.0) as u8,
        color[2].mul_add(intensity, spec).min(255.0) as u8,
    ]
}

/// Widen a primitive's colour once, outside the per-pixel loop.
#[inline]
fn widen(color: [u8; 3]) -> [f64; 3] {
    [
        f64::from(color[0]),
        f64::from(color[1]),
        f64::from(color[2]),
    ]
}

pub fn default_light_dir() -> [f64; 3] {
    normalize([-0.3, 0.8, 0.5])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_and_clear() {
        let mut fb = Framebuffer::new(4, 4);
        assert_eq!(fb.color.len(), 16);
        assert_eq!(fb.depth.len(), 16);
        assert!(fb.depth[0].is_infinite());
        assert_eq!(fb.color[0], [0, 0, 0]);

        // Write something, then clear
        fb.color[0] = [255, 0, 0];
        fb.depth[0] = 1.0;
        fb.clear();
        assert_eq!(fb.color[0], [0, 0, 0]);
        assert!(fb.depth[0].is_infinite());
    }

    #[test]
    fn test_zbuffer() {
        let mut fb = Framebuffer::new(4, 4);
        fb.set_pixel(1, 1, 5.0, [255, 0, 0]);
        assert_eq!(fb.color[1 * 4 + 1], [255, 0, 0]);

        // Closer fragment wins
        fb.set_pixel(1, 1, 3.0, [0, 255, 0]);
        assert_eq!(fb.color[1 * 4 + 1], [0, 255, 0]);

        // Farther fragment is rejected
        fb.set_pixel(1, 1, 4.0, [0, 0, 255]);
        assert_eq!(fb.color[1 * 4 + 1], [0, 255, 0]);
    }

    #[test]
    fn outline_adds_only_an_exterior_ring() {
        let mut fb = Framebuffer::new(7, 7);
        fb.set_pixel(3, 3, 2.0, [200, 100, 50]);
        fb.apply_outline([1, 2, 3], 1);

        let drawn = fb.depth.iter().filter(|depth| depth.is_finite()).count();
        assert_eq!(drawn, 9, "one pixel should gain its eight neighbours");
        assert_eq!(fb.color[3 * 7 + 3], [200, 100, 50]);
        assert_eq!(fb.depth[3 * 7 + 3], 2.0);
        assert_eq!(fb.color[2 * 7 + 2], [1, 2, 3]);
        assert_eq!(fb.depth[2 * 7 + 2], f32::MAX);
        assert!(fb.depth[0].is_infinite());
    }

    #[test]
    fn outline_radius_expands_without_filling_the_original() {
        let mut fb = Framebuffer::new(7, 7);
        fb.set_pixel(3, 3, 2.0, [200, 100, 50]);
        fb.apply_outline([8, 9, 10], 2);

        assert_eq!(
            fb.depth.iter().filter(|depth| depth.is_finite()).count(),
            25
        );
        assert_eq!(fb.color[3 * 7 + 3], [200, 100, 50]);
        assert_eq!(fb.color[1 * 7 + 1], [8, 9, 10]);
        assert!(fb.depth[0].is_infinite());
    }

    #[test]
    fn outline_traces_an_internal_occlusion_boundary() {
        let mut fb = Framebuffer::new(8, 3);
        for y in 0..3 {
            for x in 0..8 {
                let depth = if x < 4 { 1.0 } else { 4.0 };
                fb.set_pixel(x, y, depth, [120, 80, 40]);
            }
        }
        fb.apply_outline([1, 2, 3], 1);

        assert_eq!(fb.color[4], [1, 2, 3], "far side should carry the ink");
        assert_eq!(
            fb.color[3],
            [120, 80, 40],
            "near side should remain visible"
        );
        assert_eq!(
            fb.color[7],
            [120, 80, 40],
            "flat interior should stay unchanged"
        );
    }

    #[test]
    fn outline_traces_material_changes_but_not_brightness_changes() {
        let mut materials = Framebuffer::new(8, 3);
        let mut shading = Framebuffer::new(8, 3);
        for y in 0..3 {
            for x in 0..8 {
                let material = if x < 4 { [220, 20, 80] } else { [230, 190, 10] };
                let shade = if x < 4 { [200, 100, 50] } else { [100, 50, 25] };
                materials.set_pixel(x, y, 1.0, material);
                shading.set_pixel(x, y, 1.0, shade);
            }
        }
        materials.apply_outline([1, 2, 3], 1);
        shading.apply_outline([1, 2, 3], 1);

        assert!(materials.color.iter().any(|pixel| *pixel == [1, 2, 3]));
        assert!(!shading.color.iter().any(|pixel| *pixel == [1, 2, 3]));
    }

    #[test]
    fn test_normalize() {
        let v = normalize([3.0, 0.0, 4.0]);
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        assert!((len - 1.0).abs() < 1e-9);
        assert!((v[0] - 0.6).abs() < 1e-9);
        assert!((v[2] - 0.8).abs() < 1e-9);
    }

    #[test]
    fn test_default_light_dir_is_unit() {
        let d = default_light_dir();
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        assert!((len - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_rasterize_covers_pixels() {
        let mut fb = Framebuffer::new(10, 10);
        let tri = Triangle {
            verts: [[2.0, 2.0, 1.0], [8.0, 2.0, 1.0], [5.0, 8.0, 1.0]],
            color: [200, 100, 50],
            normal: [0.0, 0.0, 1.0],
        };
        fb.rasterize_triangle(&tri, normalize([0.0, 0.0, 1.0]));

        // The centroid (5,4) should definitely be filled
        let idx = 4 * 10 + 5;
        assert_ne!(fb.color[idx], [0, 0, 0]);
        // A corner outside the triangle should remain black
        assert_eq!(fb.color[0], [0, 0, 0]);
    }

    #[test]
    fn test_draw_line_3d() {
        let mut fb = Framebuffer::new(10, 10);
        fb.draw_line_3d([0.0, 0.0, 1.0], [9.0, 9.0, 2.0], [255, 255, 255]);
        // Diagonal line should touch (0,0) and (9,9)
        assert_eq!(fb.color[0], [255, 255, 255]);
        assert_eq!(fb.color[9 * 10 + 9], [255, 255, 255]);
    }

    /// The default fog, at a chosen far-plane strength.
    fn fog_at(strength: f64) -> Fog {
        Fog {
            strength,
            ..Fog::default()
        }
    }

    /// Build a framebuffer whose drawn pixels span `depth` angstroms.
    fn fb_with_depth_span(depth: f32) -> Framebuffer {
        let mut fb = Framebuffer::new(16, 1);
        for (i, (c, d)) in fb.color.iter_mut().zip(fb.depth.iter_mut()).enumerate() {
            *c = [200, 40, 160];
            *d = i as f32 / 15.0 * depth;
        }
        fb
    }

    /// A structure no deeper than the reference keeps exactly the flat ramp it
    /// always had: no curvature, no chroma drain.
    #[test]
    fn shallow_structures_keep_the_linear_ramp() {
        let span = Fog::default().reference_depth as f32 * 0.9;
        let mut fb = fb_with_depth_span(span);
        fb.apply_depth_tint(&fog_at(0.35));

        let fog = [40.0, 50.0, 70.0];
        let base = [200.0, 40.0, 160.0];
        for (i, c) in fb.color.iter().enumerate() {
            let blend = (i as f64 / 15.0) * 0.35;
            for ch in 0..3 {
                let want = (base[ch] + (fog[ch] - base[ch]) * blend).round() as i32;
                assert!(
                    (i32::from(c[ch]) - want).abs() <= 1,
                    "pixel {i} channel {ch}: got {} want ~{want}",
                    c[ch]
                );
            }
        }
    }

    /// The config's strength knob has to reach the pixels, and zero has to mean
    /// off rather than "a little".
    #[test]
    fn fog_strength_is_what_the_config_says_it_is() {
        let far = |strength: f64| {
            let mut fb = fb_with_depth_span(Fog::default().reference_depth as f32 * 0.9);
            fb.apply_depth_tint(&fog_at(strength));
            *fb.color.last().unwrap()
        };

        let unfogged = [200, 40, 160];
        assert_eq!(
            far(0.0),
            unfogged,
            "strength 0.0 must leave the frame alone"
        );

        // Toward the fog colour, monotonically, and the light setting has to be
        // visibly lighter than the default -- that is the whole point of the knob.
        let (light, default, heavy) = (far(0.15), far(0.35), far(0.6));
        assert!(
            light[0] > default[0] && default[0] > heavy[0],
            "more strength should mean more fog: {light:?} {default:?} {heavy:?}"
        );
        assert!(
            i32::from(unfogged[0]) - i32::from(light[0])
                < i32::from(unfogged[0]) - i32::from(default[0]),
            "0.15 should fog less than 0.35: {light:?} vs {default:?}"
        );
    }

    /// The nearest pixel is never fogged, however deep the structure.
    #[test]
    fn the_nearest_pixel_keeps_its_colour() {
        for span in [30.0, 227.0] {
            let mut fb = fb_with_depth_span(span);
            fb.apply_depth_tint(&fog_at(0.35));
            assert_eq!(fb.color[0], [200, 40, 160], "span {span}");
        }
    }

    /// A ribosome-deep structure must both darken and desaturate its far side,
    /// and reach further toward the fog colour than a shallow one does.
    #[test]
    fn deep_structures_drain_colour_with_distance() {
        let chroma = |c: [u8; 3]| {
            let hi = c.iter().max().copied().unwrap_or(0) as i32;
            let lo = c.iter().min().copied().unwrap_or(0) as i32;
            hi - lo
        };

        let mut shallow = fb_with_depth_span(Fog::default().reference_depth as f32 * 0.9);
        shallow.apply_depth_tint(&fog_at(0.35));
        let mut deep = fb_with_depth_span(227.0);
        deep.apply_depth_tint(&fog_at(0.35));

        let far_shallow = *shallow.color.last().unwrap();
        let far_deep = *deep.color.last().unwrap();

        assert!(
            chroma(far_deep) < chroma(far_shallow),
            "deep far plane should lose chroma: {far_deep:?} vs {far_shallow:?}"
        );
        // Front half must stay clearly more vivid than the back.
        let near_deep = deep.color[2];
        assert!(
            chroma(near_deep) > chroma(far_deep) * 2,
            "front should stay vivid against the back: {near_deep:?} vs {far_deep:?}"
        );
    }

    /// With the drain off, fogging is a plain lerp toward the fog colour.
    #[test]
    fn blend_fog_without_desaturation_is_a_plain_lerp() {
        let mut c = [200, 40, 160];
        blend_fog(&mut c, [40, 50, 70], 0.5, 0.0);
        assert_eq!(c, [120, 45, 115]);
    }

    #[test]
    fn test_draw_circle() {
        let mut fb = Framebuffer::new(20, 20);
        fb.draw_circle(10.0, 10.0, 3.0, [128, 64, 32]);
        // Center pixel is filled, shaded rather than the flat input colour.
        let center = fb.color[10 * 20 + 10];
        assert!(
            center != [0, 0, 0],
            "centre should be drawn, got {center:?}"
        );
        assert!(
            center[0] <= 128 && center[0] > 64,
            "centre should be lit but not brighter than the base colour, got {center:?}"
        );
        // Far corner should not be
        assert_eq!(fb.color[0], [0, 0, 0]);
    }

    /// Atoms are shaded as spheres, so a disc is not a flat patch of colour:
    /// it has a highlight toward the light and a darker limb away from it.
    #[test]
    fn draw_circle_shades_like_a_sphere() {
        let mut fb = Framebuffer::new(40, 40);
        fb.draw_circle(20.0, 20.0, 10.0, [200, 200, 200]);

        let at = |x: usize, y: usize| fb.color[y * 40 + x][0];
        let centre = at(20, 20);
        // The default light comes from the upper left.
        let toward_light = at(15, 15);
        let away_from_light = at(25, 25);

        assert!(
            toward_light > away_from_light,
            "lit limb {toward_light} should be brighter than the far limb {away_from_light}"
        );
        assert!(
            toward_light >= centre && centre >= away_from_light,
            "brightness should fall off across the sphere: {toward_light} / {centre} / {away_from_light}"
        );
    }

    /// The depth written for an atom is the depth of its front surface, so the
    /// buffer bulges a full world radius toward the viewer at the centre and
    /// falls back to the atom's own depth at the silhouette.
    #[test]
    fn sphere_depth_bulges_toward_the_viewer() {
        let mut fb = Framebuffer::new(40, 40);
        // 10 pixels of radius at 0.1 world units per pixel: a 1 A atom.
        fb.draw_sphere(20.0, 20.0, 5.0, 10.0, 0.1, [200, 200, 200]);

        let at = |x: usize, y: usize| fb.depth[y * 40 + x];
        assert!(
            (at(20, 20) - 4.0).abs() < 0.05,
            "centre should sit a world radius in front of 5.0, got {}",
            at(20, 20)
        );
        // The bulge falls away toward the silhouette, where it reaches the
        // atom's own depth.
        assert!(
            at(20, 20) < at(25, 20) && at(25, 20) < at(29, 20) && at(29, 20) < 5.0,
            "depth should rise toward the limb: {} / {} / {}",
            at(20, 20),
            at(25, 20),
            at(29, 20)
        );
    }

    /// Two atoms used to sort entirely by centre depth, so the further one was
    /// rejected across the whole of the nearer one's disc and their overlap came
    /// out as a hard straight edge.  With per-pixel depth the further atom wins
    /// wherever its own surface genuinely bulges in front.
    #[test]
    fn a_further_atom_shows_where_its_surface_comes_forward() {
        let render = |world_per_px: f64| {
            let mut fb = Framebuffer::new(80, 60);
            fb.draw_sphere(25.0, 30.0, 0.0, 20.0, world_per_px, [255, 0, 0]);
            fb.draw_sphere(35.0, 30.0, 0.3, 20.0, world_per_px, [0, 255, 0]);
            fb.color[30 * 80 + 42]
        };

        // (42, 30) is well inside the near atom's disc.
        let flat = render(0.0);
        assert!(
            flat[0] > flat[1],
            "with one depth per disc the further atom must lose outright, got {flat:?}"
        );
        let curved = render(1.0 / 20.0);
        assert!(
            curved[1] > curved[0],
            "the further atom's surface is in front here, so it should win, got {curved:?}"
        );
    }

    /// A bond is a cylinder, not a flat bar: across its width the normal sweeps
    /// from one limb to the other, so the lit side, the crown and the far limb
    /// all differ.
    #[test]
    fn a_stick_is_shaded_across_its_width() {
        let mut fb = Framebuffer::new(60, 40);
        fb.draw_thick_line_3d(
            [10.0, 20.0, 0.0],
            [50.0, 20.0, 0.0],
            [200, 200, 200],
            11.0,
            0.0,
        );

        let at = |y: usize| fb.color[y * 60 + 30][0];
        // Screen y runs downward and the light sits above, so the upper limb is
        // the lit one.
        let (lit, crown, far) = (at(16), at(20), at(24));
        assert!(
            lit > crown && crown > far,
            "brightness should fall across the bar: {lit} / {crown} / {far}"
        );
    }

    /// A stick's depth follows the cylinder too, so a bond meets the atoms at
    /// its ends without a step in the depth buffer.
    #[test]
    fn stick_depth_bulges_along_its_crown() {
        let mut fb = Framebuffer::new(60, 40);
        fb.draw_thick_line_3d(
            [10.0, 20.0, 4.0],
            [50.0, 20.0, 4.0],
            [200, 200, 200],
            10.0,
            0.1,
        );

        let at = |y: usize| fb.depth[y * 60 + 30];
        assert!(
            (at(20) - 3.5).abs() < 0.1,
            "crown should sit a world radius in front of 4.0, got {}",
            at(20)
        );
        assert!(
            at(20) < at(23),
            "the crown {} should be nearer than the limb {}",
            at(20),
            at(23)
        );
    }

    /// The old thick line stamped a fan of half-pixel-offset Bresenham lines,
    /// which left a combed edge on any diagonal.  An oriented-rectangle
    /// rasterizer covers every scanline it touches in one unbroken run.
    #[test]
    fn a_diagonal_stick_has_no_holes() {
        let mut fb = Framebuffer::new(60, 60);
        fb.draw_thick_line_3d(
            [10.0, 12.0, 0.0],
            [48.0, 46.0, 0.0],
            [200, 200, 200],
            7.0,
            0.0,
        );

        let mut rows = 0;
        for y in 0..60 {
            let lit: Vec<usize> = (0..60)
                .filter(|&x| fb.color[y * 60 + x] != [0, 0, 0])
                .collect();
            if lit.is_empty() {
                continue;
            }
            rows += 1;
            assert_eq!(
                lit.last().unwrap() - lit[0] + 1,
                lit.len(),
                "row {y} has a gap: {lit:?}"
            );
        }
        assert!(
            rows > 30,
            "the stick should span most of the frame, got {rows}"
        );
    }

    /// The span solver is what decides which pixels a stick covers, so its
    /// interval arithmetic is worth pinning down on its own.
    #[test]
    fn clip_span_intersects_intervals() {
        let (mut lo, mut hi) = (f64::NEG_INFINITY, f64::INFINITY);
        // 2 <= 2x <= 8  ->  x in [1, 4]
        assert!(clip_span(0.5, 2.0, 8.0, &mut lo, &mut hi));
        assert!((lo - 1.0).abs() < 1e-9 && (hi - 4.0).abs() < 1e-9);

        // A negative coefficient flips the interval rather than emptying it.
        let (mut lo, mut hi) = (f64::NEG_INFINITY, f64::INFINITY);
        assert!(clip_span(-0.5, 2.0, 8.0, &mut lo, &mut hi));
        assert!((lo + 4.0).abs() < 1e-9 && (hi + 1.0).abs() < 1e-9);

        // Disjoint constraints leave nothing.
        let (mut lo, mut hi) = (0.0, 1.0);
        assert!(!clip_span(1.0, 5.0, 9.0, &mut lo, &mut hi));
    }

    #[test]
    fn test_framebuffer_to_widget_basic() {
        let mut fb = Framebuffer::new(2, 4);
        fb.color[0] = [255, 0, 0]; // row 0, col 0 (top pixel of term row 0)
        fb.color[2] = [0, 255, 0]; // row 1, col 0 (bottom pixel of term row 0)
        // The widget should produce 2 terminal rows for 4 pixel rows
        let _widget = framebuffer_to_widget(&fb);
        // Just ensure it doesn't panic; visual inspection is manual
    }

    #[test]
    fn test_quantize_channel_no_op() {
        // step=1 should be a no-op
        assert_eq!(quantize_channel(0, 1), 0);
        assert_eq!(quantize_channel(127, 1), 127);
        assert_eq!(quantize_channel(255, 1), 255);
    }

    #[test]
    fn test_quantize_channel_step_8() {
        // 0 stays 0
        assert_eq!(quantize_channel(0, 8), 0);
        // 4 rounds to 8 (nearest multiple of 8)
        assert_eq!(quantize_channel(4, 8), 8);
        // 3 rounds to 0
        assert_eq!(quantize_channel(3, 8), 0);
        // 128 stays 128
        assert_eq!(quantize_channel(128, 8), 128);
        // 255 should quantize to a valid u8 (256 clamped to 255)
        let q255 = quantize_channel(255, 8);
        assert!(
            q255 == 248 || q255 == 255,
            "unexpected quantize(255, 8): {}",
            q255
        );
        // 252 should round up to 248 or be clamped
        let q252 = quantize_channel(252, 8);
        assert!(
            q252 == 248 || q252 == 255,
            "unexpected quantize(252, 8): {}",
            q252
        );
    }

    #[test]
    fn test_quantize_color_black_unchanged() {
        assert_eq!(quantize_color([0, 0, 0], 8), [0, 0, 0]);
    }

    #[test]
    fn test_quantize_color_near_black_stays_visible() {
        // A dim color like [3, 2, 1] would quantize to [0,0,0] without the
        // guard.  The function should keep it visible.
        let q = quantize_color([3, 2, 1], 8);
        assert_ne!(q, [0, 0, 0], "near-black color should not vanish");
    }

    #[test]
    fn test_quantize_color_normal() {
        let q = quantize_color([200, 100, 50], 8);
        // Each channel should be a multiple of 8 (or 255 if clamped)
        for &c in &q {
            assert!(c % 8 == 0 || c == 255, "channel {} not quantized", c);
        }
    }

    #[test]
    fn test_apply_depth_tint_blends_colors() {
        let mut fb = Framebuffer::new(4, 1);
        // Place pixels at different depths: near (z=1) and far (z=10)
        let near_color = [200, 100, 50];
        let far_color = [200, 100, 50];
        fb.color[0] = near_color;
        fb.depth[0] = 1.0;
        fb.color[1] = far_color;
        fb.depth[1] = 10.0;
        // Pixels 2 and 3 remain at INFINITY (background)

        fb.apply_depth_tint(&fog_at(0.5));

        // Near pixel (z=1, t=0.0) should stay unchanged
        assert_eq!(
            fb.color[0], near_color,
            "nearest pixel should keep original color"
        );

        // Far pixel (z=10, t=1.0) should be blended halfway toward fog
        // new = base + (fog - base) * 1.0 * 0.5
        // R: 200 + (40 - 200) * 0.5 = 200 - 80 = 120
        // G: 100 + (50 - 100) * 0.5 = 100 - 25 = 75
        // B:  50 + (70 -  50) * 0.5 =  50 + 10 = 60
        assert_eq!(
            fb.color[1],
            [120, 75, 60],
            "farthest pixel should blend toward fog"
        );
    }

    #[test]
    fn test_apply_depth_tint_skips_background() {
        let mut fb = Framebuffer::new(4, 1);
        // Set one valid pixel and leave others at INFINITY
        fb.color[0] = [200, 100, 50];
        fb.depth[0] = 5.0;
        fb.color[1] = [180, 90, 40];
        fb.depth[1] = 10.0;
        // Pixels 2 and 3 are background (depth = INFINITY, color = [0,0,0])

        fb.apply_depth_tint(&fog_at(0.5));

        // Background pixels must remain [0,0,0]
        assert_eq!(
            fb.color[2],
            [0, 0, 0],
            "background pixel at index 2 should stay black"
        );
        assert_eq!(
            fb.color[3],
            [0, 0, 0],
            "background pixel at index 3 should stay black"
        );
    }

    #[test]
    fn test_draw_dashed_line_3d_has_gaps() {
        let mut fb = Framebuffer::new(20, 1);
        // Horizontal dashed line across the entire width: dash=3, gap=3
        fb.draw_dashed_line_3d([0.0, 0.0, 1.0], [19.0, 0.0, 2.0], [255, 255, 255], 3.0, 3.0);

        // Count drawn (non-black) and gap (black) pixels.
        let drawn: usize = fb.color[..20].iter().filter(|c| **c != [0, 0, 0]).count();
        let gap: usize = fb.color[..20].iter().filter(|c| **c == [0, 0, 0]).count();

        // There should be both drawn and gap pixels (not all drawn, not all gap).
        assert!(drawn > 0, "dashed line should draw some pixels");
        assert!(gap > 0, "dashed line should leave some gaps");
    }

    #[test]
    fn test_draw_dashed_line_3d_z_interpolation() {
        let mut fb = Framebuffer::new(10, 1);
        fb.draw_dashed_line_3d(
            [0.0, 0.0, 1.0],
            [9.0, 0.0, 10.0],
            [255, 255, 255],
            100.0,
            0.0,
        );

        // With dash_len >> line length, all pixels should be drawn.
        // Check z interpolation: first pixel should be near 1.0, last near 10.0.
        assert!(
            (fb.depth[0] - 1.0).abs() < 0.5,
            "start depth should be near 1.0, got {}",
            fb.depth[0]
        );
        assert!(
            (fb.depth[9] - 10.0).abs() < 0.5,
            "end depth should be near 10.0, got {}",
            fb.depth[9]
        );
    }

    #[test]
    fn test_draw_dashed_line_3d_respects_zbuffer() {
        let mut fb = Framebuffer::new(10, 1);
        // Draw a solid foreground line at z=1
        fb.draw_line_3d([0.0, 0.0, 1.0], [9.0, 0.0, 1.0], [255, 0, 0]);
        // Draw a dashed background line at z=5 (farther) — should not overwrite
        fb.draw_dashed_line_3d([0.0, 0.0, 5.0], [9.0, 0.0, 5.0], [0, 255, 0], 100.0, 0.0);

        // All pixels should remain red (closer z wins)
        for i in 0..10 {
            assert_eq!(
                fb.color[i],
                [255, 0, 0],
                "pixel {} should stay red (z-buffer)",
                i
            );
        }
    }

    #[test]
    fn test_draw_dashed_line_3d_diagonal() {
        let mut fb = Framebuffer::new(10, 10);
        fb.draw_dashed_line_3d([0.0, 0.0, 1.0], [9.0, 9.0, 2.0], [128, 128, 128], 2.0, 2.0);

        // At least the first pixel should be drawn (start of first dash).
        assert_ne!(
            fb.color[0],
            [0, 0, 0],
            "first pixel of diagonal dashed line should be drawn"
        );

        // Count total drawn pixels — should be roughly half of the diagonal length.
        let total_drawn: usize = fb.color.iter().filter(|c| **c != [0, 0, 0]).count();
        assert!(
            total_drawn > 0 && total_drawn < 14,
            "diagonal should have partial coverage, got {}",
            total_drawn
        );
    }

    #[test]
    fn test_draw_dashed_line_3d_offscreen_clipped() {
        let mut fb = Framebuffer::new(10, 10);
        // Both endpoints off the same side — should be silently skipped.
        fb.draw_dashed_line_3d(
            [-5.0, -5.0, 1.0],
            [-1.0, -1.0, 1.0],
            [255, 255, 255],
            2.0,
            1.0,
        );
        let drawn: usize = fb.color.iter().filter(|c| **c != [0, 0, 0]).count();
        assert_eq!(drawn, 0, "fully off-screen dashed line should draw nothing");
    }

    #[test]
    fn test_draw_dashed_line_3d_zero_cycle_falls_back_to_solid() {
        let mut fb = Framebuffer::new(20, 1);
        // dash_len=0, gap_len=0 => cycle=0, which would produce NaN via `% 0.0`.
        // The guard should fall back to a solid line instead.
        fb.draw_dashed_line_3d([0.0, 0.0, 1.0], [19.0, 0.0, 2.0], [255, 255, 255], 0.0, 0.0);

        // Every pixel along the horizontal line should be drawn (solid fallback).
        let drawn: usize = fb.color[..20].iter().filter(|c| **c != [0, 0, 0]).count();
        assert_eq!(
            drawn, 20,
            "zero-cycle dashed line should draw solid (got {} pixels)",
            drawn
        );

        // Also verify z-interpolation is intact: endpoints should have expected depths.
        assert!(
            (fb.depth[0] - 1.0).abs() < 0.5,
            "start depth should be near 1.0"
        );
        assert!(
            (fb.depth[19] - 2.0).abs() < 0.5,
            "end depth should be near 2.0"
        );
    }

    #[test]
    fn test_draw_dashed_line_3d_negative_args_falls_back_to_solid() {
        let mut fb = Framebuffer::new(10, 1);
        // Negative dash_len should trigger the guard and draw a solid line.
        fb.draw_dashed_line_3d([0.0, 0.0, 1.0], [9.0, 0.0, 1.0], [200, 100, 50], -1.0, 3.0);

        let drawn: usize = fb.color[..10].iter().filter(|c| **c != [0, 0, 0]).count();
        assert_eq!(
            drawn, 10,
            "negative dash_len should draw solid (got {} pixels)",
            drawn
        );
    }

    // ---------------------------------------------------------------------
    // Supersampled braille conversion
    // ---------------------------------------------------------------------

    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;

    /// Render a widget into a cell buffer so the emitted glyphs and colors can
    /// be inspected directly.
    fn render_cells(widget: Paragraph<'static>, w: u16, h: u16) -> Buffer {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        buf
    }

    /// Count foreground-color changes along each row. This is the quantity that
    /// drives how many SGR escape sequences reach the terminal, i.e. the
    /// bandwidth cost of a frame.
    fn color_runs(buf: &Buffer, w: u16, h: u16) -> usize {
        let mut runs = 0;
        for y in 0..h {
            let mut prev: Option<Color> = None;
            for x in 0..w {
                let fg = buf[(x, y)].fg;
                if prev != Some(fg) {
                    runs += 1;
                    prev = Some(fg);
                }
            }
        }
        runs
    }

    #[test]
    fn ssaa_one_lights_a_dot_from_any_non_black_pixel() {
        // At ssaa == 1 the coverage threshold is 1, preserving the original
        // "any non-black pixel lights the dot" behaviour exactly.
        let mut fb = Framebuffer::new(2, 4);
        fb.color[0] = [255, 0, 0]; // dot (dx=0, dy=0) -> bit 0x01

        let buf = render_cells(framebuffer_to_braille_widget(&fb), 1, 1);
        let expected = char::from_u32(0x2800 + 0x01).unwrap().to_string();
        assert_eq!(buf[(0, 0)].symbol(), expected);
    }

    #[test]
    fn ssaa_dot_needs_half_coverage_to_light() {
        // 4x8 framebuffer at ssaa = 2 is exactly one terminal cell: each of the
        // 2x4 braille dots is backed by a 2x2 block of samples.
        let mut fb = Framebuffer::new(4, 8);

        // Dot (dx=0, dy=0) covers samples x in [0,2), y in [0,2).
        // One covered sample out of four is 25% -- below threshold, stays dark.
        fb.color[0] = [255, 0, 0];

        // Dot (dx=1, dy=0) covers samples x in [2,4), y in [0,2).
        // Two covered samples out of four is 50% -- lights up (bit 0x08).
        fb.color[2] = [0, 255, 0];
        fb.color[3] = [0, 255, 0];

        let buf = render_cells(framebuffer_to_braille_widget_ssaa(&fb, 2, 1), 1, 1);
        let expected = char::from_u32(0x2800 + 0x08).unwrap().to_string();
        assert_eq!(
            buf[(0, 0)].symbol(),
            expected,
            "only the half-covered dot should light"
        );
    }

    #[test]
    fn ssaa_grid_maps_to_the_same_cell_dimensions() {
        // A supersampled framebuffer must still produce cols x rows cells --
        // supersampling buys quality, never extra characters on the wire.
        let (cols, rows, ssaa) = (7usize, 3usize, 2usize);
        let fb = Framebuffer::new(cols * 2 * ssaa, rows * 4 * ssaa);

        let plain = Framebuffer::new(cols * 2, rows * 4);
        let a = render_cells(
            framebuffer_to_braille_widget_ssaa(&fb, ssaa, 1),
            cols as u16,
            rows as u16,
        );
        let b = render_cells(
            framebuffer_to_braille_widget(&plain),
            cols as u16,
            rows as u16,
        );
        assert_eq!(a, b, "ssaa must not change the emitted cell grid");
    }

    #[test]
    fn quantization_merges_color_runs() {
        // Eight cells whose colors differ by only 2 per channel -- the kind of
        // near-identical neighbours supersampled shading produces.
        let cols = 8usize;
        let mut fb = Framebuffer::new(cols * 2, 4);
        for cell in 0..cols {
            for dx in 0..2 {
                for dy in 0..4 {
                    let idx = dy * fb.width + cell * 2 + dx;
                    fb.color[idx] = [100 + 2 * cell as u8, 150, 200];
                }
            }
        }

        let unquantized = color_runs(
            &render_cells(
                framebuffer_to_braille_widget_ssaa(&fb, 1, 1),
                cols as u16,
                1,
            ),
            cols as u16,
            1,
        );
        let quantized = color_runs(
            &render_cells(
                framebuffer_to_braille_widget_ssaa(&fb, 1, 8),
                cols as u16,
                1,
            ),
            cols as u16,
            1,
        );

        assert_eq!(
            unquantized, cols,
            "every cell should differ without quantization"
        );
        assert!(
            quantized < unquantized,
            "quantization should merge runs (got {quantized}, unquantized {unquantized})"
        );
    }

    #[test]
    fn quantization_never_darkens_a_lit_cell_to_black() {
        // A very dark but non-black cell must not quantize to black, which
        // would make lit geometry invisible.
        let mut fb = Framebuffer::new(2, 4);
        for i in 0..fb.color.len() {
            fb.color[i] = [1, 1, 1];
        }
        let buf = render_cells(framebuffer_to_braille_widget_ssaa(&fb, 1, 8), 1, 1);
        assert_ne!(buf[(0, 0)].fg, Color::Rgb(0, 0, 0));
    }

    #[test]
    fn cell_color_takes_the_frontmost_sample_not_the_mean() {
        // One cell whose dots straddle two structures at different depths. The
        // cell carries a single foreground color, so averaging a red front and a
        // green back yields a muddy olive; it must show the front color instead.
        let mut fb = Framebuffer::new(2, 4);
        fb.color[0] = [255, 0, 0];
        fb.depth[0] = 10.0; // far
        fb.color[1] = [0, 255, 0];
        fb.depth[1] = 1.0; // near

        let buf = render_cells(framebuffer_to_braille_widget(&fb), 1, 1);
        assert_eq!(
            buf[(0, 0)].fg,
            Color::Rgb(0, 255, 0),
            "cell should take the nearer sample's color, not the blend"
        );
    }

    #[test]
    fn cell_color_falls_back_to_the_mean_without_depth() {
        // A framebuffer whose colors were written without z has no frontmost
        // sample to pick, so the mean is the only sensible answer.
        let mut fb = Framebuffer::new(2, 4);
        fb.color[0] = [255, 0, 0];
        fb.color[1] = [0, 255, 0];
        // depths left at INFINITY

        let buf = render_cells(framebuffer_to_braille_widget(&fb), 1, 1);
        assert_eq!(buf[(0, 0)].fg, Color::Rgb(127, 127, 0));
    }
}
