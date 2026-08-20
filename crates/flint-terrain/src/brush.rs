//! Terrain brush tools for interactive sculpting and splat painting

use noise::{NoiseFn, Perlin};

/// Parameters for brush operations
#[derive(Debug, Clone, Copy)]
pub struct BrushParams {
    /// Brush radius in UV space (0..1)
    pub radius: f32,
    /// Brush strength (0..1)
    pub strength: f32,
    /// Falloff curve (0 = hard edge, 1 = smooth gaussian)
    pub falloff: f32,
}

impl Default for BrushParams {
    fn default() -> Self {
        Self {
            radius: 0.05,
            strength: 0.5,
            falloff: 0.7,
        }
    }
}

/// Height brush modes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeightBrushMode {
    Raise,
    Lower,
    Smooth,
    Flatten,
    Noise,
}

/// Splat brush modes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplatBrushMode {
    Paint,
    Erase,
}

/// Apply a height brush stroke at UV position (u, v) on the heightmap buffer.
pub fn apply_height_brush(
    heights: &mut [f32],
    res: u32,
    u: f32,
    v: f32,
    mode: HeightBrushMode,
    params: &BrushParams,
    dt: f32,
    seed: u64,
) {
    let res = res as usize;
    let cx = (u * (res - 1) as f32) as i32;
    let cy = (v * (res - 1) as f32) as i32;
    let pixel_radius = (params.radius * res as f32).ceil() as i32;

    let rate = params.strength * dt;

    // For flatten: compute average height under brush
    let flatten_target = if mode == HeightBrushMode::Flatten {
        compute_brush_average(heights, res, cx, cy, pixel_radius, params.falloff)
    } else {
        0.0
    };

    let noise_gen = if mode == HeightBrushMode::Noise {
        Some(Perlin::new((seed & 0xFFFF_FFFF) as u32))
    } else {
        None
    };

    for dy in -pixel_radius..=pixel_radius {
        for dx in -pixel_radius..=pixel_radius {
            let px = cx + dx;
            let py = cy + dy;

            if px < 0 || px >= res as i32 || py < 0 || py >= res as i32 {
                continue;
            }

            let dist = ((dx * dx + dy * dy) as f32).sqrt() / pixel_radius.max(1) as f32;
            if dist > 1.0 {
                continue;
            }

            let weight = falloff_weight(dist, params.falloff);
            let idx = py as usize * res + px as usize;

            match mode {
                HeightBrushMode::Raise => {
                    heights[idx] += rate * weight;
                }
                HeightBrushMode::Lower => {
                    heights[idx] -= rate * weight;
                }
                HeightBrushMode::Smooth => {
                    let avg = local_average(heights, res, px, py);
                    let diff = avg - heights[idx];
                    heights[idx] += diff * rate * weight;
                }
                HeightBrushMode::Flatten => {
                    let diff = flatten_target - heights[idx];
                    heights[idx] += diff * rate * weight;
                }
                HeightBrushMode::Noise => {
                    if let Some(ref ng) = noise_gen {
                        let nx = px as f64 * 0.1;
                        let ny = py as f64 * 0.1;
                        let n = ng.get([nx, ny]) as f32;
                        heights[idx] += n * rate * weight;
                    }
                }
            }
        }
    }
}

/// Apply a splat brush stroke at UV position (u, v) on the splat buffer.
pub fn apply_splat_brush(
    splat: &mut [u8],
    res: u32,
    u: f32,
    v: f32,
    layer: u8,
    mode: SplatBrushMode,
    params: &BrushParams,
    dt: f32,
) {
    if layer > 3 {
        return;
    }

    let res = res as usize;
    let cx = (u * (res - 1) as f32) as i32;
    let cy = (v * (res - 1) as f32) as i32;
    let pixel_radius = (params.radius * res as f32).ceil() as i32;
    let rate = params.strength * dt;

    for dy in -pixel_radius..=pixel_radius {
        for dx in -pixel_radius..=pixel_radius {
            let px = cx + dx;
            let py = cy + dy;

            if px < 0 || px >= res as i32 || py < 0 || py >= res as i32 {
                continue;
            }

            let dist = ((dx * dx + dy * dy) as f32).sqrt() / pixel_radius.max(1) as f32;
            if dist > 1.0 {
                continue;
            }

            let weight = falloff_weight(dist, params.falloff) * rate;
            let idx = (py as usize * res + px as usize) * 4;

            match mode {
                SplatBrushMode::Paint => {
                    // Increase target layer, decrease others proportionally
                    let current = splat[idx + layer as usize] as f32;
                    let add = (255.0 - current) * weight;
                    splat[idx + layer as usize] = (current + add).round().clamp(0.0, 255.0) as u8;

                    // Redistribute remaining to other channels
                    let new_val = splat[idx + layer as usize] as f32;
                    let others_total: f32 = (0..4)
                        .filter(|&c| c != layer as usize)
                        .map(|c| splat[idx + c] as f32)
                        .sum();

                    let remaining = 255.0 - new_val;
                    if others_total > 0.0 {
                        for c in 0..4 {
                            if c != layer as usize {
                                splat[idx + c] = (splat[idx + c] as f32 * remaining / others_total)
                                    .round()
                                    .clamp(0.0, 255.0)
                                    as u8;
                            }
                        }
                    }
                }
                SplatBrushMode::Erase => {
                    // Decrease target layer, increase layer 0 as fallback
                    let current = splat[idx + layer as usize] as f32;
                    let sub = current * weight;
                    splat[idx + layer as usize] = (current - sub).round().clamp(0.0, 255.0) as u8;

                    // Give the removed weight to layer 0 (unless erasing layer 0)
                    let fallback = if layer == 0 { 1 } else { 0 };
                    splat[idx + fallback] = (splat[idx + fallback] as f32 + sub)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}

/// Cast a ray against a heightmap and find the UV intersection point.
///
/// Uses ray marching with binary search refinement.
/// Returns `Some((u, v))` if the ray hits the terrain.
pub fn ray_heightmap_intersect(
    origin: [f32; 3],
    dir: [f32; 3],
    heights: &[f32],
    res: u32,
    width: f32,
    depth: f32,
    height_scale: f32,
) -> Option<(f32, f32)> {
    let res = res as usize;

    // Normalize direction
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if len < 1e-8 {
        return None;
    }
    let d = [dir[0] / len, dir[1] / len, dir[2] / len];

    // March along ray with adaptive step
    let max_dist = (width * width + depth * depth + (height_scale * height_scale)).sqrt() * 2.0;
    let step = (width.min(depth)) / res as f32;

    let mut t = 0.0f32;
    let mut prev_above = true;

    while t < max_dist {
        let px = origin[0] + d[0] * t;
        let py = origin[1] + d[1] * t;
        let pz = origin[2] + d[2] * t;

        // Convert to UV
        let u = px / width;
        let v = pz / depth;

        if !(-0.1..=1.1).contains(&u) || !(-0.1..=1.1).contains(&v) {
            // Way out of bounds — if we were in bounds before, we've exited
            if t > step * 2.0 {
                return None;
            }
            t += step;
            continue;
        }

        let terrain_h =
            sample_bilinear(heights, res, u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)) * height_scale;
        let above = py > terrain_h;

        if !above && prev_above && t > 0.0 {
            // Binary search refinement
            let mut lo = t - step;
            let mut hi = t;
            for _ in 0..16 {
                let mid = (lo + hi) * 0.5;
                let mx = origin[0] + d[0] * mid;
                let my = origin[1] + d[1] * mid;
                let mz = origin[2] + d[2] * mid;
                let mu = (mx / width).clamp(0.0, 1.0);
                let mv = (mz / depth).clamp(0.0, 1.0);
                let mh = sample_bilinear(heights, res, mu, mv) * height_scale;
                if my > mh {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }

            let final_t = (lo + hi) * 0.5;
            let fu = ((origin[0] + d[0] * final_t) / width).clamp(0.0, 1.0);
            let fv = ((origin[2] + d[2] * final_t) / depth).clamp(0.0, 1.0);
            return Some((fu, fv));
        }

        prev_above = above;
        t += step;
    }

    None
}

fn falloff_weight(dist: f32, falloff: f32) -> f32 {
    if falloff <= 0.0 {
        // Hard edge
        if dist <= 1.0 {
            1.0
        } else {
            0.0
        }
    } else {
        // Gaussian-ish falloff
        let sigma = falloff * 0.5;
        (-dist * dist / (2.0 * sigma * sigma)).exp()
    }
}

fn local_average(heights: &[f32], res: usize, x: i32, y: i32) -> f32 {
    let mut sum = 0.0f32;
    let mut count = 0;

    for dy in -1..=1 {
        for dx in -1..=1 {
            let nx = x + dx;
            let ny = y + dy;
            if nx >= 0 && nx < res as i32 && ny >= 0 && ny < res as i32 {
                sum += heights[ny as usize * res + nx as usize];
                count += 1;
            }
        }
    }

    if count > 0 {
        sum / count as f32
    } else {
        heights[y as usize * res + x as usize]
    }
}

fn compute_brush_average(
    heights: &[f32],
    res: usize,
    cx: i32,
    cy: i32,
    pixel_radius: i32,
    falloff: f32,
) -> f32 {
    let mut sum = 0.0f32;
    let mut weight_sum = 0.0f32;

    for dy in -pixel_radius..=pixel_radius {
        for dx in -pixel_radius..=pixel_radius {
            let px = cx + dx;
            let py = cy + dy;
            if px < 0 || px >= res as i32 || py < 0 || py >= res as i32 {
                continue;
            }
            let dist = ((dx * dx + dy * dy) as f32).sqrt() / pixel_radius.max(1) as f32;
            if dist > 1.0 {
                continue;
            }
            let w = falloff_weight(dist, falloff);
            sum += heights[py as usize * res + px as usize] * w;
            weight_sum += w;
        }
    }

    if weight_sum > 0.0 {
        sum / weight_sum
    } else {
        0.0
    }
}

fn sample_bilinear(heights: &[f32], res: usize, u: f32, v: f32) -> f32 {
    let u = u.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);
    let fx = u * (res - 1) as f32;
    let fy = v * (res - 1) as f32;
    let x0 = (fx as usize).min(res - 2);
    let y0 = (fy as usize).min(res - 2);
    let x1 = x0 + 1;
    let y1 = y0 + 1;
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;
    let h00 = heights[y0 * res + x0];
    let h10 = heights[y0 * res + x1];
    let h01 = heights[y1 * res + x0];
    let h11 = heights[y1 * res + x1];
    let h0 = h00 * (1.0 - tx) + h10 * tx;
    let h1 = h01 * (1.0 - tx) + h11 * tx;
    h0 * (1.0 - ty) + h1 * ty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raise_increases_heights() {
        let res = 16u32;
        let mut heights = vec![0.5; (res * res) as usize];
        let before = heights[8 * 16 + 8];

        apply_height_brush(
            &mut heights,
            res,
            0.5,
            0.5,
            HeightBrushMode::Raise,
            &BrushParams {
                radius: 0.2,
                strength: 1.0,
                falloff: 0.5,
            },
            0.1,
            42,
        );

        let after = heights[8 * 16 + 8];
        assert!(after > before);
    }

    #[test]
    fn smooth_converges() {
        let res = 16u32;
        // Create a spike
        let mut heights = vec![0.0; (res * res) as usize];
        heights[8 * 16 + 8] = 1.0;

        let params = BrushParams {
            radius: 0.3,
            strength: 1.0,
            falloff: 1.0,
        };

        // Apply smooth several times
        for _ in 0..10 {
            apply_height_brush(
                &mut heights,
                res,
                0.5,
                0.5,
                HeightBrushMode::Smooth,
                &params,
                0.1,
                42,
            );
        }

        // The spike should have smoothed out
        assert!(heights[8 * 16 + 8] < 0.8);
    }

    #[test]
    fn ray_intersect_flat_terrain() {
        let res = 16u32;
        let heights = vec![0.5; (res * res) as usize];

        // Ray from above, pointing down
        let hit = ray_heightmap_intersect(
            [50.0, 100.0, 50.0], // origin above center
            [0.0, -1.0, 0.0],    // straight down
            &heights,
            res,
            100.0,
            100.0,
            50.0, // height_scale=50 * 0.5 = 25.0 terrain height
        );

        assert!(hit.is_some());
        let (u, v) = hit.unwrap();
        assert!((u - 0.5).abs() < 0.1);
        assert!((v - 0.5).abs() < 0.1);
    }
}
