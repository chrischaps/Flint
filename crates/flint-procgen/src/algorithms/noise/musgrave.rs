//! The Musgrave fractal variants (fBm, multifractal, ridged, hybrid and
//! hetero terrain) over any [`NoiseSource`], plain or tiled.

use super::NoiseSource;

// ---------------------------------------------------------------------------
// Musgrave FBM variants
// ---------------------------------------------------------------------------

/// Musgrave noise type selector.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MusgraveType {
    Fbm,
    Multifractal,
    RidgedMultifractal,
    HybridMultifractal,
    HeteroTerrain,
}

/// Sample a Musgrave-type FBM variant from a noise source.
///
/// Parameters:
/// - `dimension`: fractal dimension (controls how fast amplitude drops per octave)
/// - `offset`: used by ridged/hybrid/hetero variants
/// - `gain`: used by ridged/hybrid variants
///
/// Returns a value roughly centered on 0 but range depends on variant.
#[allow(clippy::too_many_arguments)]
pub fn musgrave_sample(
    source: &dyn NoiseSource,
    x: f64,
    y: f64,
    musgrave_type: MusgraveType,
    frequency: f64,
    octaves: u32,
    lacunarity: f64,
    dimension: f64,
    offset: f64,
    gain: f64,
) -> f64 {
    let octave = |f: f64| source.sample_2d(x * f, y * f);
    musgrave_octaves(
        &octave,
        musgrave_type,
        frequency,
        octaves,
        lacunarity,
        dimension,
        offset,
        gain,
    )
}

/// [`musgrave_sample`] over a field that repeats every `period_x` ×
/// `period_y` in `(x, y)`: each octave samples
/// [`NoiseSource::sample_2d_tiled`] with the period scaled by its frequency,
/// so every octave (and so the sum) tiles.
#[allow(clippy::too_many_arguments)]
pub fn musgrave_sample_tiled(
    source: &dyn NoiseSource,
    x: f64,
    y: f64,
    period_x: f64,
    period_y: f64,
    musgrave_type: MusgraveType,
    frequency: f64,
    octaves: u32,
    lacunarity: f64,
    dimension: f64,
    offset: f64,
    gain: f64,
) -> f64 {
    let octave = |f: f64| source.sample_2d_tiled(x * f, y * f, period_x * f, period_y * f);
    musgrave_octaves(
        &octave,
        musgrave_type,
        frequency,
        octaves,
        lacunarity,
        dimension,
        offset,
        gain,
    )
}

/// The Musgrave variants over `octave(frequency)`, one noise sample per octave.
#[allow(clippy::too_many_arguments)]
fn musgrave_octaves(
    octave: &dyn Fn(f64) -> f64,
    musgrave_type: MusgraveType,
    frequency: f64,
    octaves: u32,
    lacunarity: f64,
    dimension: f64,
    offset: f64,
    gain: f64,
) -> f64 {
    match musgrave_type {
        MusgraveType::Fbm => musgrave_fbm(octave, frequency, octaves, lacunarity, dimension),
        MusgraveType::Multifractal => {
            musgrave_multifractal(octave, frequency, octaves, lacunarity, dimension, offset)
        }
        MusgraveType::RidgedMultifractal => musgrave_ridged(
            octave, frequency, octaves, lacunarity, dimension, offset, gain,
        ),
        MusgraveType::HybridMultifractal => musgrave_hybrid(
            octave, frequency, octaves, lacunarity, dimension, offset, gain,
        ),
        MusgraveType::HeteroTerrain => {
            musgrave_hetero(octave, frequency, octaves, lacunarity, dimension, offset)
        }
    }
}

fn musgrave_fbm(
    octave: &dyn Fn(f64) -> f64,
    frequency: f64,
    octaves: u32,
    lacunarity: f64,
    dimension: f64,
) -> f64 {
    let mut value = 0.0;
    let mut freq = frequency;
    for i in 0..octaves {
        let amp = freq.powf(-dimension);
        value += octave(freq) * amp;
        freq *= lacunarity;
        // Small offset per octave to decorrelate
        let _ = i;
    }
    value
}

#[allow(clippy::too_many_arguments)]
fn musgrave_multifractal(
    octave: &dyn Fn(f64) -> f64,
    frequency: f64,
    octaves: u32,
    lacunarity: f64,
    dimension: f64,
    offset: f64,
) -> f64 {
    let mut value = 1.0;
    let mut freq = frequency;
    for _ in 0..octaves {
        let amp = freq.powf(-dimension);
        value *= (octave(freq) + offset) * amp;
        freq *= lacunarity;
    }
    value
}

#[allow(clippy::too_many_arguments)]
fn musgrave_ridged(
    octave: &dyn Fn(f64) -> f64,
    frequency: f64,
    octaves: u32,
    lacunarity: f64,
    dimension: f64,
    offset: f64,
    gain: f64,
) -> f64 {
    let mut value = 0.0;
    let mut weight = 1.0;
    let mut freq = frequency;
    for _ in 0..octaves {
        let amp = freq.powf(-dimension);
        let signal = offset - octave(freq).abs();
        let signal = signal * signal * weight;
        weight = (signal * gain).clamp(0.0, 1.0);
        value += signal * amp;
        freq *= lacunarity;
    }
    value
}

#[allow(clippy::too_many_arguments)]
fn musgrave_hybrid(
    octave: &dyn Fn(f64) -> f64,
    frequency: f64,
    octaves: u32,
    lacunarity: f64,
    dimension: f64,
    offset: f64,
    gain: f64,
) -> f64 {
    let mut freq = frequency;
    let amp0 = freq.powf(-dimension);
    let mut value = (octave(freq) + offset) * amp0;
    let mut weight = value;
    freq *= lacunarity;

    for _ in 1..octaves {
        weight = (weight * gain).clamp(0.0, 1.0);
        let amp = freq.powf(-dimension);
        let signal = (octave(freq) + offset) * amp;
        value += weight * signal;
        freq *= lacunarity;
    }
    value
}

#[allow(clippy::too_many_arguments)]
fn musgrave_hetero(
    octave: &dyn Fn(f64) -> f64,
    frequency: f64,
    octaves: u32,
    lacunarity: f64,
    dimension: f64,
    offset: f64,
) -> f64 {
    let mut freq = frequency;
    let amp0 = freq.powf(-dimension);
    let mut value = (octave(freq) + offset) * amp0;
    freq *= lacunarity;

    for _ in 1..octaves {
        let amp = freq.powf(-dimension);
        let increment = (octave(freq) + offset) * amp * value;
        value += increment;
        freq *= lacunarity;
    }
    value
}
