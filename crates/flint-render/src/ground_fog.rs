//! Ground fog (ADR 0106): fog that lies on a floor of its own, read from a
//! map over a world rectangle, so a mist can fill hollows at many heights.
//! The height fog (ADR 0101) is one level layer over the whole scene: low
//! enough to fill a high valley it buries the plain, and at the plain it
//! misses every valley above it.
//!
//! Each texel of a [`GroundFog`] holds a floor (world Y) and an amount (0
//! to 1). Over a texel the fog's density is `amount × ground_fog_density`
//! at its floor, falling by e every `1 / ground_fog_falloff` metres above
//! it, and holding at the floor's under it. The composite pass marches
//! each view ray through the box the map covers, from the lowest floor up
//! to where the fog over the highest has thinned to nothing. Its steps
//! lengthen with distance from the eye, so the drifts near it are seen
//! fine wherever the march begins. The map is read at each step's ends, at
//! the mip level the step spans, and along a step the height over the
//! floor runs straight from one end's to the other's, so on a slope the
//! floor follows the ground. The falloff is integrated along each step
//! exactly, so a ray that dips into a thin layer between two reads still
//! meets it.
//!
//! The look (colour, density, falloff, steps) is on
//! [`PostProcessConfig`](crate::PostProcessConfig) and can change every
//! frame; the map is set once with `SceneRenderer::set_ground_fog`, and
//! again when the ground changes. The fog isn't lit: the game gives it the
//! colour the hour's light would.

/// A ground fog's map over a world rectangle, for
/// `SceneRenderer::set_ground_fog`.
#[derive(Clone, Copy, Debug)]
pub struct GroundFog<'a> {
    /// The rectangle's minimum corner, world X and Z.
    pub origin: [f32; 2],
    /// The rectangle's extent in world units, X and Z.
    pub size: [f32; 2],
    /// Texels across X and along Z.
    pub width: u32,
    pub depth: u32,
    /// `[floor, amount]` per texel, `width × depth` of them, row by row
    /// toward +Z. Each is the value at the middle of its texel's cell, and
    /// the fog blends the four round a point. The floor is a world Y; give
    /// a texel with no fog the ground's height too, since its floor still
    /// shapes the fog at its edges. The amount scales the density, 0 to 1.
    pub texels: &'a [[f32; 2]],
}

impl GroundFog<'_> {
    /// Errs when the texels don't fill the stated size, the rectangle is
    /// empty, or a value isn't finite.
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0 || self.depth == 0 {
            return Err("ground fog: the map has no texels".into());
        }
        let n = self.width as usize * self.depth as usize;
        if self.texels.len() != n {
            return Err(format!(
                "ground fog: {} texels for a {}×{} map",
                self.texels.len(),
                self.width,
                self.depth
            ));
        }
        if !(self.size[0] > 0.0 && self.size[1] > 0.0) {
            return Err(format!("ground fog: size {:?} is empty", self.size));
        }
        if !self.origin.iter().all(|v| v.is_finite())
            || !self.size.iter().all(|v| v.is_finite())
            || !self.texels.iter().flatten().all(|v| v.is_finite())
        {
            return Err("ground fog: a value isn't finite".into());
        }
        Ok(())
    }
}

/// A map made ready for the GPU: its mip chain, with each floor stored over
/// `floor_base` so half floats keep it fine.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FogMips {
    /// `(width, depth, [floor - floor_base, amount])` per level, finest first.
    pub levels: Vec<(u32, u32, Vec<[f32; 2]>)>,
    /// The lowest floor: the bottom of the box the pass marches.
    pub floor_base: f32,
    /// The highest floor under any fog.
    pub floor_top: f32,
}

/// The mip chain of `fog`, down to one texel. Each level is the last's size
/// halved, rounding down as the GPU does, so an odd last row or column
/// joins the texel beside it. A texel's amount is the mean of those under
/// it and its floor the mean of theirs weighted by their amount, so a
/// coarse level keeps the fog on the floors it lies on. `None` when no
/// texel has any fog.
pub(crate) fn mip_chain(fog: &GroundFog) -> Option<FogMips> {
    let fogged = fog.texels.iter().filter(|t| t[1] > 0.0);
    let floor_top = fogged.map(|t| t[0]).fold(f32::NEG_INFINITY, f32::max);
    if floor_top == f32::NEG_INFINITY {
        return None;
    }
    let floor_base = fog
        .texels
        .iter()
        .map(|t| t[0])
        .fold(f32::INFINITY, f32::min);
    let first: Vec<[f32; 2]> = fog
        .texels
        .iter()
        .map(|&[floor, amount]| [floor - floor_base, amount.clamp(0.0, 1.0)])
        .collect();
    let mut levels = vec![(fog.width, fog.depth, first)];
    loop {
        let (w, d, last) = levels.last().unwrap();
        if *w == 1 && *d == 1 {
            break;
        }
        let (w, d) = (*w as usize, *d as usize);
        let (nw, nd) = ((w / 2).max(1), (d / 2).max(1));
        // The fine texels under coarse texel `c` of `n`, of `fine`.
        let under =
            |c: usize, n: usize, fine: usize| 2 * c..if c + 1 == n { fine } else { 2 * c + 2 };
        let mut next = Vec::with_capacity(nw * nd);
        for b in 0..nd {
            for a in 0..nw {
                let (mut amount, mut weighted, mut floors, mut count) = (0.0, 0.0, 0.0, 0.0);
                for z in under(b, nd, d) {
                    for x in under(a, nw, w) {
                        let [floor, amt] = last[z * w + x];
                        amount += amt;
                        weighted += amt * floor;
                        floors += floor;
                        count += 1.0;
                    }
                }
                let floor = if amount > 0.0 {
                    weighted / amount
                } else {
                    floors / count
                };
                next.push([floor, amount / count]);
            }
        }
        levels.push((nw as u32, nd as u32, next));
    }
    Some(FogMips {
        levels,
        floor_base,
        floor_top,
    })
}

/// A ground fog on the GPU, as the composite pass reads it. Made by
/// `SceneRenderer::set_ground_fog`.
pub struct GroundFogGpu {
    _texture: wgpu::Texture,
    pub(crate) view: wgpu::TextureView,
    pub(crate) origin: [f32; 2],
    pub(crate) size: [f32; 2],
    /// World units a finest texel spans (the larger of X and Z).
    pub(crate) texel: f32,
    pub(crate) mip_count: u32,
    pub(crate) floor_base: f32,
    pub(crate) floor_top: f32,
}

/// The texture format: two half floats, filterable on every backend.
pub(crate) const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;

impl GroundFogGpu {
    /// Upload `fog`. `Ok(None)` when it holds no fog at all.
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        fog: &GroundFog,
    ) -> Result<Option<Self>, String> {
        fog.validate()?;
        let Some(mips) = mip_chain(fog) else {
            return Ok(None);
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Ground Fog Map"),
            size: wgpu::Extent3d {
                width: fog.width,
                height: fog.depth,
                depth_or_array_layers: 1,
            },
            mip_level_count: mips.levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, (w, d, texels)) in mips.levels.iter().enumerate() {
            let bytes: Vec<u8> = texels
                .iter()
                .flat_map(|t| t.iter().flat_map(|&v| f16_bits(v).to_le_bytes()))
                .collect();
            queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &bytes,
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(*d),
                },
                wgpu::Extent3d {
                    width: *w,
                    height: *d,
                    depth_or_array_layers: 1,
                },
            );
        }
        Ok(Some(Self {
            view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
            _texture: texture,
            origin: fog.origin,
            size: fog.size,
            texel: (fog.size[0] / fog.width as f32).max(fog.size[1] / fog.depth as f32),
            mip_count: mips.levels.len() as u32,
            floor_base: mips.floor_base,
            floor_top: mips.floor_top,
        }))
    }
}

/// `v` as an IEEE half float's bits, rounded to nearest even, saturating
/// to infinity past the half range.
pub(crate) fn f16_bits(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let man = bits & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if man != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        // Subnormal, or too small for one.
        if e < -10 {
            return sign;
        }
        let man = man | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = 1u32 << (shift - 1);
        let rest = man & ((1 << shift) - 1);
        let mut out = man >> shift;
        if rest > half || (rest == half && out & 1 == 1) {
            out += 1;
        }
        return sign | out as u16;
    }
    let mut out = ((e as u32) << 10) | (man >> 13);
    let rest = man & 0x1fff;
    if rest > 0x1000 || (rest == 0x1000 && out & 1 == 1) {
        // A carry into the exponent is the right rounding too.
        out += 1;
    }
    sign | out as u16
}

/// The optical depth a step of length `len` gathers through fog of
/// density `density` at its floor, falling by e every `1 / k` above it,
/// from `ya` to `yb` over the floor. Under the floor the density holds at
/// the floor's. Exact while the height over the floor runs straight; the WGSL
/// `fog_step` in `composite_shader.wgsl` is this, line for line.
#[cfg(test)]
pub(crate) fn fog_step(density: f32, k: f32, ya: f32, yb: f32, len: f32) -> f32 {
    // Above the floor: len' × e^-k·lo × (1 - e^-x) / x over the part
    // above, x = k × (hi - lo). Under it, the part under at full density.
    let (lo, hi) = if ya < yb { (ya, yb) } else { (yb, ya) };
    let span = hi - lo;
    if span < 1e-4 {
        return density * len * (-k * lo.max(0.0)).exp();
    }
    let under = ((0.0 - lo) / span).clamp(0.0, 1.0);
    let lo = lo.max(0.0);
    let x = k * (hi - lo);
    let share = if x > 1e-3 {
        (1.0 - (-x).exp()) / x
    } else {
        1.0 - 0.5 * x
    };
    density * len * (under + (1.0 - under) * (-k * lo).exp() * share)
}

/// Where step `i` of `n` starts on a march from `t0` to `t1` along the
/// ray: each step longer than the last in proportion to its distance from
/// the eye (counted from a metre behind it). The WGSL `fog_step_at` is this.
#[cfg(test)]
pub(crate) fn step_at(i: f32, n: f32, t0: f32, t1: f32) -> f32 {
    (t0 + 1.0) * ((t1 + 1.0) / (t0 + 1.0)).powf(i / n) - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numeric(density: f32, k: f32, ya: f32, yb: f32, len: f32) -> f32 {
        let steps = 20_000;
        (0..steps)
            .map(|s| {
                let y = ya + (yb - ya) * (s as f32 + 0.5) / steps as f32;
                density * (-k * y.max(0.0)).exp()
            })
            .sum::<f32>()
            * len
            / steps as f32
    }

    #[test]
    fn a_step_gathers_the_fog_along_it() {
        for (ya, yb) in [
            (2.0, 2.0),
            (0.5, 6.0),
            (9.0, 0.2),
            (-3.0, 4.0),
            (4.0, -1.0),
            (-2.0, -0.5),
            (1.0, 1.0005),
        ] {
            let (d, k, len) = (0.3, 0.8, 40.0);
            let closed = fog_step(d, k, ya, yb, len);
            let num = numeric(d, k, ya, yb, len);
            assert!(
                (closed - num).abs() < 2e-3 * num.max(1e-3),
                "{ya}..{yb}: {closed} vs {num}"
            );
        }
        assert_eq!(fog_step(0.0, 0.8, 0.0, 1.0, 10.0), 0.0);
    }

    #[test]
    fn steps_lengthen_with_distance_and_cover_the_march() {
        let n = 24.0;
        // From the eye out: fine near it, coarse far off.
        let (t0, t1) = (0.0, 1000.0);
        assert!((step_at(0.0, n, t0, t1) - t0).abs() < 1e-4);
        assert!((step_at(n, n, t0, t1) - t1).abs() < 1e-2);
        let first = step_at(1.0, n, t0, t1) - step_at(0.0, n, t0, t1);
        let last = step_at(n, n, t0, t1) - step_at(n - 1.0, n, t0, t1);
        assert!(first < 0.5 && last > 100.0, "{first} {last}");
        // Entering the fog from far above: about even, so the steps at the
        // ground, where the mist lies, are as fine as those at the top.
        let (t0, t1) = (300.0, 800.0);
        let first = step_at(1.0, n, t0, t1) - step_at(0.0, n, t0, t1);
        let last = step_at(n, n, t0, t1) - step_at(n - 1.0, n, t0, t1);
        assert!(last < 3.0 * first && last < 40.0, "{first} {last}");
    }

    #[test]
    fn the_mips_keep_the_fog_on_its_floors() {
        // A 4×2 map: fog on a floor at 10 beside clear ground at 40.
        let texels = [
            [10.0, 1.0],
            [40.0, 0.0],
            [10.0, 0.5],
            [12.0, 0.0],
            [10.0, 1.0],
            [40.0, 0.0],
            [10.0, 0.0],
            [12.0, 0.0],
        ];
        let fog = GroundFog {
            origin: [0.0, 0.0],
            size: [40.0, 20.0],
            width: 4,
            depth: 2,
            texels: &texels,
        };
        fog.validate().unwrap();
        let mips = mip_chain(&fog).unwrap();
        assert_eq!(mips.floor_base, 10.0);
        assert_eq!(mips.floor_top, 10.0);
        let sizes: Vec<_> = mips.levels.iter().map(|(w, d, _)| (*w, *d)).collect();
        assert_eq!(sizes, [(4, 2), (2, 1), (1, 1)]);
        // Half fogged, all of it on the floor at 10, not halfway up to 40.
        assert_eq!(mips.levels[1].2[0], [0.0, 0.5]);
        // An eighth fogged, on the fogged texel's floor.
        assert_eq!(mips.levels[1].2[1], [0.0, 0.125]);
        assert_eq!(mips.levels[2].2[0], [0.0, 0.3125]);
        // No fog anywhere: nothing to upload.
        let clear = [[3.0, 0.0]; 4];
        let fog = GroundFog {
            texels: &clear,
            width: 2,
            depth: 2,
            ..fog
        };
        assert!(mip_chain(&fog).is_none());
        let short = GroundFog {
            texels: &clear[..3],
            ..fog
        };
        assert!(short.validate().is_err());
    }

    #[test]
    fn an_odd_map_has_the_gpus_mip_sizes_and_loses_no_fog() {
        // As many levels as wgpu allows, each `size >> level`, at least 1.
        for (w, d) in [(5, 3), (555, 555), (7, 1), (1, 9)] {
            let texels = vec![[0.0, 1.0]; (w * d) as usize];
            let fog = GroundFog {
                origin: [0.0; 2],
                size: [1.0; 2],
                width: w,
                depth: d,
                texels: &texels,
            };
            let mips = mip_chain(&fog).unwrap();
            assert_eq!(mips.levels.len() as u32, w.max(d).ilog2() + 1, "{w}×{d}");
            for (l, (lw, ld, t)) in mips.levels.iter().enumerate() {
                assert_eq!((*lw, *ld), ((w >> l).max(1), (d >> l).max(1)));
                assert!(t.iter().all(|&[_, a]| a == 1.0), "{w}×{d} level {l}");
            }
        }
        // The odd column joins the last texel: one fogged texel of five
        // shows in the coarse level's last.
        let mut texels = vec![[0.0, 0.0]; 5];
        texels[4] = [0.0, 1.0];
        let fog = GroundFog {
            origin: [0.0; 2],
            size: [5.0, 1.0],
            width: 5,
            depth: 1,
            texels: &texels,
        };
        let mips = mip_chain(&fog).unwrap();
        assert_eq!(mips.levels[1].2[1][1], 1.0 / 3.0);
    }

    #[test]
    fn half_floats_round_to_nearest() {
        for (v, bits) in [
            (0.0, 0x0000),
            (1.0, 0x3c00),
            (-2.0, 0xc000),
            (0.5, 0x3800),
            (65504.0, 0x7bff),
            (1e6, 0x7c00),
            (6.1035156e-5, 0x0400),
            (5.9604645e-8, 0x0001),
            (1.0 + 1.0 / 2048.0, 0x3c00),
            (1.0 + 3.0 / 2048.0, 0x3c02),
        ] {
            assert_eq!(f16_bits(v), bits, "{v}");
        }
        // A floor 37.3 m over the base keeps better than 2 cm.
        let bits = f16_bits(37.3);
        let e = ((bits >> 10) & 0x1f) as i32 - 15;
        let back = (1.0 + (bits & 0x3ff) as f32 / 1024.0) * 2f32.powi(e);
        assert!((back - 37.3).abs() < 0.02, "{back}");
    }
}
