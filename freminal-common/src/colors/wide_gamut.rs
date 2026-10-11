// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Parsing of the CSS Color Module Level 4 `oklch()` and `lab()` colour
//! functions, converted to 8-bit sRGB.
//!
//! The syntax follows CSS Color 4 §9 ("Lab and LCH" / "`OKLab` and `OKLCH`"),
//! with the following deliberate simplifications for terminal use:
//!
//! - The optional `/ alpha` component is parsed and validated but ignored;
//!   the resulting colour is always opaque.
//! - Components may be separated by ASCII whitespace and/or commas.
//! - `none` is accepted for any component and means zero.
//! - Relative-colour syntax (`from <colour>`) and `calc()` are not supported.
//!
//! Colours outside the sRGB gamut are mapped into it with the CSS Color 4
//! §14.2.2 "Binary Search Gamut Mapping with Local MINDE" algorithm, and the
//! colour-space conversions use the matrices and transfer functions from the
//! CSS Color 4 §19 sample code. `OKLab` itself is due to Björn Ottosson
//! (<https://bottosson.github.io/posts/oklab/>).

use super::spec::ColorSpecError;
use conv2::{ConvUtil, RoundToNearest};

type Vec3 = [f64; 3];
type Mat3 = [[f64; 3]; 3];

/// `D50` reference white (CSS Color 4 §19, "standard white points").
const D50: Vec3 = [0.3457 / 0.3585, 1.0, (1.0 - 0.3457 - 0.3585) / 0.3585];

/// CIE `κ` (`29^3 / 3^3`), CSS Color 4 §19 `Lab_to_XYZ`.
const KAPPA: f64 = 24389.0 / 27.0;
/// CIE `ε` (`6^3 / 29^3`), CSS Color 4 §19 `Lab_to_XYZ`.
const EPSILON_CIE: f64 = 216.0 / 24389.0;

/// Bradford chromatic adaptation from `D50` to `D65` (CSS Color 4 §19).
const D50_TO_D65: Mat3 = [
    [
        0.955_473_421_488_075,
        -0.023_098_454_948_764_71,
        0.063_259_243_200_570_72,
    ],
    [
        -0.028_369_709_333_863_7,
        1.009_995_398_081_304_1,
        0.021_041_441_191_917_323,
    ],
    [
        0.012_314_014_864_481_998,
        -0.020_507_649_298_898_964,
        1.330_365_926_242_124,
    ],
];

/// Linear-light sRGB to CIE XYZ (`D65`), CSS Color 4 §19 `lin_sRGB_to_XYZ`.
const LIN_SRGB_TO_XYZ: Mat3 = [
    [
        506_752.0 / 1_228_815.0,
        87_881.0 / 245_763.0,
        12_673.0 / 70_218.0,
    ],
    [
        87_098.0 / 409_605.0,
        175_762.0 / 245_763.0,
        12_673.0 / 175_545.0,
    ],
    [
        7_918.0 / 409_605.0,
        87_881.0 / 737_289.0,
        1_001_167.0 / 1_053_270.0,
    ],
];

/// CIE XYZ (`D65`) to linear-light sRGB, CSS Color 4 §19 `XYZ_to_lin_sRGB`.
const XYZ_TO_LIN_SRGB: Mat3 = [
    [12_831.0 / 3_959.0, -329.0 / 214.0, -1_974.0 / 3_959.0],
    [
        -851_781.0 / 878_810.0,
        1_648_619.0 / 878_810.0,
        36_519.0 / 878_810.0,
    ],
    [705.0 / 12_673.0, -2_585.0 / 12_673.0, 705.0 / 667.0],
];

/// CIE XYZ (`D65`) to `LMS`, CSS Color 4 §19 `XYZ_to_OKLab`.
const XYZ_TO_LMS: Mat3 = [
    [
        0.819_022_437_996_703,
        0.361_906_260_052_890_4,
        -0.128_873_781_520_987_9,
    ],
    [
        0.032_983_653_932_388_5,
        0.929_286_861_586_343_4,
        0.036_144_666_350_642_4,
    ],
    [
        0.048_177_189_359_624_2,
        0.264_239_531_752_730_8,
        0.633_547_828_469_430_9,
    ],
];

/// Non-linear LMS to `OKLab`, CSS Color 4 §19 `XYZ_to_OKLab`.
const LMS_TO_OKLAB: Mat3 = [
    [
        0.210_454_268_309_314,
        0.793_617_774_702_305_4,
        -0.004_072_043_011_619_3,
    ],
    [
        1.977_998_532_431_168_4,
        -2.428_592_242_048_58,
        0.450_593_709_617_411,
    ],
    [
        0.025_904_042_465_547_8,
        0.782_771_712_457_529_6,
        -0.808_675_754_923_077_4,
    ],
];

/// LMS to CIE XYZ (`D65`), CSS Color 4 §19 `OKLab_to_XYZ`.
const LMS_TO_XYZ: Mat3 = [
    [
        1.226_879_875_845_924_3,
        -0.557_814_994_460_217_1,
        0.281_391_045_665_964_7,
    ],
    [
        -0.040_575_745_214_800_8,
        1.112_286_803_280_317,
        -0.071_711_058_065_516_4,
    ],
    [
        -0.076_372_936_674_660_1,
        -0.421_493_332_402_243_2,
        1.586_924_019_836_781_6,
    ],
];

/// `OKLab` to non-linear LMS, CSS Color 4 §19 `OKLab_to_XYZ`.
const OKLAB_TO_LMS: Mat3 = [
    [1.0, 0.396_337_777_376_174_9, 0.215_803_757_309_913_6],
    [1.0, -0.105_561_345_815_658_6, -0.063_854_172_825_813_3],
    [1.0, -0.089_484_177_529_811_9, -1.291_485_548_019_409_2],
];

/// Chroma that `100%` maps to in `oklch()` (CSS Color 4 §9.3).
const OKLCH_CHROMA_PERCENT_BASIS: f64 = 0.4;
/// Magnitude that `100%` maps to for the `a` / `b` axes of `lab()` (CSS Color 4 §9.2).
const LAB_AB_PERCENT_BASIS: f64 = 125.0;

/// Just-noticeable difference in `deltaEOK` terms (CSS Color 4 §14.2.2).
const JND: f64 = 0.02;
/// Chroma search tolerance (CSS Color 4 §14.2.2).
const GAMUT_EPSILON: f64 = 0.0001;
/// Probe chroma above which a bisection probe is known to take the
/// `max = probe` branch without any colour conversion being needed; see the
/// fast-forward in [`gamut_map_oklch`].
const FORCED_STEP_CHROMA: f64 = 1.0;
/// Upper bound on binary-search steps.
///
/// After the fast-forward in [`gamut_map_oklch`] the search interval is at
/// most `2 * FORCED_STEP_CHROMA` wide, and every step halves it, so
/// [`GAMUT_EPSILON`] is reached in 15 steps.  The bound only guarantees
/// termination; the compile-time assertion below proves it is never the
/// reason the search stops.
const MAX_GAMUT_ITERATIONS: usize = 32;

/// Upper bound on fast-forward halvings.  The widest finite `f64` is below
/// `2^1024`, so at most 1,024 halvings bring any chroma to `<= 2.0`; the
/// bound only guarantees termination.
const FAST_FORWARD_MAX_STEPS: usize = 1_100;

const _: () = {
    let mut width = 2.0 * FORCED_STEP_CHROMA;
    let mut step = 0;
    while step < MAX_GAMUT_ITERATIONS {
        width /= 2.0;
        step += 1;
    }
    assert!(
        width <= GAMUT_EPSILON,
        "MAX_GAMUT_ITERATIONS must be enough for the epsilon stop to be reached"
    );
};

/// Parse the text following the `oklch(` prefix, including the closing `)`.
///
/// # Errors
///
/// Returns a [`ColorSpecError`] for malformed syntax or components.
pub(super) fn parse_oklch(rest: &str) -> Result<(u8, u8, u8), ColorSpecError> {
    let (main, alpha) = split_arguments(rest)?;
    let [l, c, h] = components(main)?;

    let l = match parse_component(l)? {
        Component::None => 0.0,
        Component::Number(n) => n,
        Component::Percent(p) => p / 100.0,
    }
    .clamp(0.0, 1.0);
    let c = match parse_component(c)? {
        Component::None => 0.0,
        Component::Number(n) => n,
        Component::Percent(p) => p / 100.0 * OKLCH_CHROMA_PERCENT_BASIS,
    }
    .max(0.0);
    let h = parse_hue(h)?;
    validate_alpha(alpha)?;

    finish(gamut_map_oklch([l, c, h]))
}

/// Parse the text following the `lab(` prefix, including the closing `)`.
///
/// # Errors
///
/// Returns a [`ColorSpecError`] for malformed syntax or components.
pub(super) fn parse_lab(rest: &str) -> Result<(u8, u8, u8), ColorSpecError> {
    let (main, alpha) = split_arguments(rest)?;
    let [l, a, b] = components(main)?;

    let l = match parse_component(l)? {
        Component::None => 0.0,
        Component::Number(n) | Component::Percent(n) => n,
    }
    .clamp(0.0, 100.0);
    let axis = |token: &str| -> Result<f64, ColorSpecError> {
        Ok(match parse_component(token)? {
            Component::None => 0.0,
            Component::Number(n) => n,
            Component::Percent(p) => p / 100.0 * LAB_AB_PERCENT_BASIS,
        })
    };
    let a = axis(a)?;
    let b = axis(b)?;
    validate_alpha(alpha)?;

    // Lab (D50) -> XYZ (D50) -> XYZ (D65) -> `OKLab` -> `OKLCh`.
    let xyz_d65 = multiply(&D50_TO_D65, lab_to_xyz_d50([l, a, b]));
    let oklch = oklab_to_oklch(xyz_to_oklab(xyz_d65));
    if !oklch.iter().all(|v| v.is_finite()) {
        return Err(ColorSpecError::InvalidWideGamutComponent);
    }
    finish(gamut_map_oklch(oklch))
}

// ---------------------------------------------------------------------------
// Syntax
// ---------------------------------------------------------------------------

/// A parsed, unit-less component.
#[derive(Clone, Copy)]
enum Component {
    /// The `none` keyword (treated as zero).
    None,
    /// A bare number.
    Number(f64),
    /// A number with a `%` suffix (the number before the `%`, not divided).
    Percent(f64),
}

/// Remove the closing `)` and split off the optional `/ alpha` component.
///
/// Returns the colour components text and the optional alpha text.
fn split_arguments(rest: &str) -> Result<(&str, Option<&str>), ColorSpecError> {
    let args = rest
        .strip_suffix(')')
        .ok_or(ColorSpecError::InvalidWideGamut)?;
    match args.split_once('/') {
        None => Ok((args, None)),
        Some((main, alpha)) => Ok((main, Some(alpha))),
    }
}

/// Split the colour components on whitespace and commas, requiring exactly three.
fn components(main: &str) -> Result<[&str; 3], ColorSpecError> {
    let tokens: Vec<&str> = main
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .collect();
    match tokens.as_slice() {
        [a, b, c] => Ok([a, b, c]),
        other => Err(ColorSpecError::WideGamutComponentCount { found: other.len() }),
    }
}

/// Validate the alpha component, which is otherwise ignored.
///
/// It must be exactly one number, percentage or `none` once surrounding
/// whitespace is removed.
fn validate_alpha(alpha: Option<&str>) -> Result<(), ColorSpecError> {
    let Some(alpha) = alpha else {
        return Ok(());
    };
    let alpha = alpha.trim_matches(|c: char| c.is_ascii_whitespace());
    if alpha.is_empty() {
        return Err(ColorSpecError::InvalidWideGamut);
    }
    parse_component(alpha).map(|_| ())
}

/// Parse a finite decimal number.  `inf`, `infinity` and `nan` (any case)
/// are rejected, as is any result that is not finite (e.g. `1e999`).
fn parse_number(token: &str) -> Option<f64> {
    let value: f64 = token.parse().ok()?;
    value.is_finite().then_some(value)
}

/// Parse a number, percentage or `none`.
fn parse_component(token: &str) -> Result<Component, ColorSpecError> {
    if token.eq_ignore_ascii_case("none") {
        return Ok(Component::None);
    }
    token
        .strip_suffix('%')
        .map_or_else(
            || parse_number(token).map(Component::Number),
            |number| parse_number(number).map(Component::Percent),
        )
        .ok_or(ColorSpecError::InvalidWideGamutComponent)
}

/// Strip `suffix` from the end of `s`, ASCII case-insensitively.
fn strip_suffix_ignore_ascii_case<'a>(s: &'a str, suffix: &str) -> Option<&'a str> {
    let split = s.len().checked_sub(suffix.len())?;
    let tail = s.get(split..)?;
    if tail.eq_ignore_ascii_case(suffix) {
        s.get(..split)
    } else {
        None
    }
}

/// Hue angle units and their size in degrees (CSS Values 4 §6.1).
const HUE_UNITS: [(&str, f64); 4] = [
    ("grad", 360.0 / 400.0),
    ("deg", 1.0),
    ("rad", 180.0 / std::f64::consts::PI),
    ("turn", 360.0),
];

/// Parse a hue to degrees, normalised into `[0, 360)`.
///
/// Accepts a number (degrees), a number with a `deg` / `grad` / `rad` /
/// `turn` unit (CSS Values 4 §6.1), or `none`.  Percentages are rejected.
fn parse_hue(token: &str) -> Result<f64, ColorSpecError> {
    if token.eq_ignore_ascii_case("none") {
        return Ok(0.0);
    }
    // `grad` must be tried before `rad`, as it ends in the same letters.
    let degrees = HUE_UNITS
        .iter()
        .find_map(|(suffix, per_unit)| {
            strip_suffix_ignore_ascii_case(token, suffix)
                .map(|number| parse_number(number).map(|v| v * per_unit))
        })
        .unwrap_or_else(|| parse_number(token));
    degrees
        .filter(|d| d.is_finite())
        .map(|d| d.rem_euclid(360.0))
        .ok_or(ColorSpecError::InvalidWideGamutComponent)
}

// ---------------------------------------------------------------------------
// Colour-space conversions (CSS Color 4 §19 sample code)
// ---------------------------------------------------------------------------

/// Multiply a 3x3 matrix by a 3-vector.
fn multiply(m: &Mat3, v: Vec3) -> Vec3 {
    m.map(|row| row[2].mul_add(v[2], row[1].mul_add(v[1], row[0] * v[0])))
}

/// `Lab_to_XYZ`: CIE Lab to `D50`-relative XYZ.
fn lab_to_xyz_d50(lab: Vec3) -> Vec3 {
    let [lightness, a_axis, b_axis] = lab;
    let f_y = (lightness + 16.0) / 116.0;
    let f_x = a_axis / 500.0 + f_y;
    let f_z = f_y - b_axis / 200.0;

    let f_to_relative = |f: f64| {
        if f.powi(3) > EPSILON_CIE {
            f.powi(3)
        } else {
            116.0f64.mul_add(f, -16.0) / KAPPA
        }
    };
    let y_relative = if lightness > KAPPA * EPSILON_CIE {
        f_y.powi(3)
    } else {
        lightness / KAPPA
    };
    [
        f_to_relative(f_x) * D50[0],
        y_relative * D50[1],
        f_to_relative(f_z) * D50[2],
    ]
}

/// `XYZ_to_OKLab`: `D65`-relative XYZ to `OKLab`.
fn xyz_to_oklab(xyz: Vec3) -> Vec3 {
    let lms = multiply(&XYZ_TO_LMS, xyz);
    // `f64::cbrt` is sign-preserving, as the sample code requires.
    multiply(&LMS_TO_OKLAB, lms.map(f64::cbrt))
}

/// `OKLab_to_XYZ`: `OKLab` to `D65`-relative XYZ.
fn oklab_to_xyz(oklab: Vec3) -> Vec3 {
    let lms = multiply(&OKLAB_TO_LMS, oklab);
    multiply(&LMS_TO_XYZ, lms.map(|c| c.powi(3)))
}

/// `OKLCH_to_OKLab` (hue in degrees).
fn oklch_to_oklab(oklch: Vec3) -> Vec3 {
    let [lightness, chroma, hue] = oklch;
    let radians = hue.to_radians();
    [lightness, chroma * radians.cos(), chroma * radians.sin()]
}

/// `OKLab_to_OKLCH`, without the achromatic-hue `NaN` of the sample code:
/// hue is simply `atan2` in `[0, 360)`, which is harmless because the
/// gamut-mapping algorithm only varies chroma.
fn oklab_to_oklch(oklab: Vec3) -> Vec3 {
    let [lightness, a_axis, b_axis] = oklab;
    let chroma = a_axis.hypot(b_axis);
    let hue = b_axis.atan2(a_axis).to_degrees().rem_euclid(360.0);
    [lightness, chroma, hue]
}

/// `gam_sRGB` (extended, sign-preserving) for one component.
fn gamma_encode(v: f64) -> f64 {
    let abs = v.abs();
    if abs > 0.003_130_8 {
        v.signum() * 1.055f64.mul_add(abs.powf(1.0 / 2.4), -0.055)
    } else {
        12.92 * v
    }
}

/// `lin_sRGB` (extended, sign-preserving) for one component.
fn gamma_decode(v: f64) -> f64 {
    let abs = v.abs();
    if abs <= 0.040_45 {
        v / 12.92
    } else {
        v.signum() * ((abs + 0.055) / 1.055).powf(2.4)
    }
}

/// `OKLab` to gamma-encoded sRGB, not clamped (may be out of gamut).
fn oklab_to_srgb(oklab: Vec3) -> Vec3 {
    multiply(&XYZ_TO_LIN_SRGB, oklab_to_xyz(oklab)).map(gamma_encode)
}

/// Gamma-encoded sRGB to `OKLab`.
fn srgb_to_oklab(srgb: Vec3) -> Vec3 {
    xyz_to_oklab(multiply(&LIN_SRGB_TO_XYZ, srgb.map(gamma_decode)))
}

// ---------------------------------------------------------------------------
// Gamut mapping (CSS Color 4 §14.2.2)
// ---------------------------------------------------------------------------

/// `inGamut` for sRGB: every gamma-encoded component is within `[0, 1]`.
fn in_gamut(srgb: Vec3) -> bool {
    srgb.iter().all(|v| (0.0..=1.0).contains(v))
}

/// `deltaEOK`: Euclidean distance in `OKLab`.
fn delta_e_ok(a: Vec3, b: Vec3) -> f64 {
    let dl = a[0] - b[0];
    let da = a[1] - b[1];
    let db = a[2] - b[2];
    db.mul_add(db, da.mul_add(da, dl * dl)).sqrt()
}

/// Binary Search Gamut Mapping with Local MINDE, destination sRGB.
///
/// Returns gamma-encoded sRGB with every component within `[0, 1]`.
fn gamut_map_oklch(origin: Vec3) -> Vec3 {
    let [lightness, chroma, hue] = origin;
    if lightness >= 1.0 {
        return [1.0, 1.0, 1.0];
    }
    if lightness <= 0.0 {
        return [0.0, 0.0, 0.0];
    }

    let origin_srgb = oklab_to_srgb(oklch_to_oklab(origin));
    if in_gamut(origin_srgb) {
        return origin_srgb;
    }

    let clip = |srgb: Vec3| srgb.map(|v| v.clamp(0.0, 1.0));
    let mut clipped = clip(origin_srgb);
    let mut e = delta_e_ok(srgb_to_oklab(clipped), oklch_to_oklab(origin));
    if e < JND {
        return clipped;
    }

    let mut min = 0.0_f64;
    let mut max = chroma;
    let mut min_in_gamut = true;

    // Fast-forward over probes that cannot change the outcome.  `chroma` is
    // finite but unbounded (`oklch(0.5 1e308 30)` parses), and bisecting it
    // directly costs ~1,000 colour conversions per value.
    //
    // At loop entry `min == 0.0` and `min_in_gamut` is true, so the first
    // probe is `midpoint(0, max) == max / 2` exactly (`f64::midpoint` with a
    // zero operand is `max / 2.0` for every magnitude; pinned by a test).
    // For a probe chroma `p > FORCED_STEP_CHROMA` (1.0):
    //   * the largest OKLab chroma of any sRGB colour is about 0.32, so the
    //     colour is out of gamut and `deltaEOK(clip(c), c) >= p - 0.33`,
    //     which is far above `JND`; or
    //   * the conversion overflows to a non-finite value, in which case
    //     `e < JND` is false.
    // Either way the original algorithm takes the `max = probe` branch (and
    // `min`, `min_in_gamut` are untouched), so doing that step as a pure
    // halving, with no conversion, is exact.  Afterwards `max <= 2.0`.
    for _ in 0..FAST_FORWARD_MAX_STEPS {
        if max / 2.0 <= FORCED_STEP_CHROMA {
            break;
        }
        max /= 2.0;
    }
    // `clipped` needs no fix-up for the skipped steps: if any step was
    // skipped, `max` is now in `(1.0, 2.0]`, so the first real probe
    // (`max / 2`, in `(0.5, 1.0]`) is out of gamut, takes the clip path
    // below, and overwrites `clipped` before any exit that returns it.

    for _ in 0..MAX_GAMUT_ITERATIONS {
        if max - min <= GAMUT_EPSILON {
            break;
        }
        let probe = f64::midpoint(min, max);
        let current_oklab = oklch_to_oklab([lightness, probe, hue]);
        let current_srgb = oklab_to_srgb(current_oklab);
        if min_in_gamut && in_gamut(current_srgb) {
            min = probe;
            continue;
        }
        clipped = clip(current_srgb);
        e = delta_e_ok(srgb_to_oklab(clipped), current_oklab);
        if e < JND {
            if JND - e < GAMUT_EPSILON {
                return clipped;
            }
            min_in_gamut = false;
            min = probe;
        } else {
            max = probe;
        }
    }
    clipped
}

/// Convert gamma-encoded sRGB in `[0, 1]` to 8-bit channels.
fn finish(srgb: Vec3) -> Result<(u8, u8, u8), ColorSpecError> {
    let channel = |v: f64| -> Result<u8, ColorSpecError> {
        // `f64::round` rounds half away from zero.
        (v.clamp(0.0, 1.0) * 255.0)
            .round()
            .approx_as_by::<u8, RoundToNearest>()
            .map_err(|_| ColorSpecError::InvalidWideGamutComponent)
    };
    Ok((channel(srgb[0])?, channel(srgb[1])?, channel(srgb[2])?))
}

#[cfg(test)]
mod tests {
    use super::super::spec::parse_color_spec;
    use super::*;
    use proptest::prelude::*;

    fn rgb(spec: &str) -> Result<(u8, u8, u8), ColorSpecError> {
        parse_color_spec(spec)
    }

    fn assert_vectors(vectors: &[(&str, (u8, u8, u8))]) {
        for (spec, expected) in vectors {
            assert_eq!(rgb(spec), Ok(*expected), "spec {spec:?}");
        }
    }

    #[test]
    fn in_gamut_vectors() {
        assert_vectors(&[
            ("oklch(0.5 0.1 180)", (0, 117, 101)),
            ("oklch(0.7 0.15 140)", (104, 180, 87)),
            ("oklch(0.9 0.05 265)", (206, 222, 255)),
            ("oklch(1 0 0)", (255, 255, 255)),
            ("oklch(0 0 0)", (0, 0, 0)),
            ("lab(100 0 0)", (255, 255, 255)),
            ("lab(0 0 0)", (0, 0, 0)),
        ]);
    }

    #[test]
    fn gamut_mapped_and_computed_vectors() {
        assert_vectors(&[
            ("oklch(0.7 0.35 25)", (255, 86, 85)),
            ("oklch(0.5 0.5 0)", (187, 0, 94)),
            ("oklch(0.5 0 0)", (99, 99, 99)),
            ("oklch(0.9 0.3 120)", (204, 242, 0)),
            ("oklch(40.101% 0.12332 21.555)", (125, 35, 41)),
            ("lab(50 0 0)", (119, 119, 119)),
            ("lab(29.2345 39.3825 20.0664)", (125, 35, 41)),
            ("lab(50 60 40)", (214, 60, 55)),
            ("lab(80 -120 120)", (0, 228, 62)),
            ("lab(50 50 50)", (202, 75, 34)),
            ("lab(50 -50 -50)", (0, 132, 159)),
            ("lab(90 0 0)", (226, 226, 226)),
            ("lab(70 50 -30)", (238, 134, 227)),
        ]);
    }

    #[test]
    fn commas_are_separators() {
        let expected = rgb("oklch(0.5 0.1 180)");
        assert_eq!(rgb("oklch(0.5,0.1,180)"), expected);
        assert_eq!(rgb("oklch(0.5, 0.1, 180)"), expected);
        assert_eq!(rgb("oklch(0.5 ,  0.1\t180)"), expected);
    }

    #[test]
    fn percentages_scale_per_component() {
        assert_eq!(rgb("oklch(50% 25% 180)"), rgb("oklch(0.5 0.1 180)"));
        assert_eq!(rgb("lab(50% 0% 0%)"), rgb("lab(50 0 0)"));
        assert_eq!(rgb("lab(50 40% -20%)"), rgb("lab(50 50 -25)"));
    }

    #[test]
    fn hue_units_agree_with_degrees() {
        let expected = rgb("oklch(0.5 0.1 180)");
        assert_eq!(rgb("oklch(0.5 0.1 180deg)"), expected);
        assert_eq!(rgb("oklch(0.5 0.1 0.5turn)"), expected);
        assert_eq!(rgb("oklch(0.5 0.1 200grad)"), expected);
        assert_eq!(rgb("oklch(0.5 0.1 3.141592653589793rad)"), expected);
        assert_eq!(rgb("oklch(0.5 0.1 180DEG)"), expected);
    }

    #[test]
    fn hue_wraps() {
        assert_eq!(rgb("oklch(0.5 0.1 540)"), rgb("oklch(0.5 0.1 180)"));
        assert_eq!(rgb("oklch(0.5 0.1 -180)"), rgb("oklch(0.5 0.1 180)"));
    }

    #[test]
    fn none_is_zero() {
        assert_eq!(rgb("oklch(0.5 none 180)"), rgb("oklch(0.5 0 180)"));
        assert_eq!(rgb("oklch(0.5 0.1 none)"), rgb("oklch(0.5 0.1 0)"));
        assert_eq!(rgb("oklch(NONE 0.1 180)"), rgb("oklch(0 0.1 180)"));
        assert_eq!(rgb("lab(50 none none)"), rgb("lab(50 0 0)"));
    }

    #[test]
    fn alpha_is_ignored() {
        let expected = rgb("oklch(0.5 0.1 180)");
        assert_eq!(rgb("oklch(0.5 0.1 180 / 0.5)"), expected);
        assert_eq!(rgb("oklch(0.5 0.1 180/50%)"), expected);
        assert_eq!(rgb("oklch(0.5 0.1 180 /none)"), expected);
        assert_eq!(rgb("lab(50 0 0 / 0.1)"), rgb("lab(50 0 0)"));
    }

    #[test]
    fn case_and_whitespace_are_tolerated() {
        assert_eq!(rgb("OKLCH(0.5 0.1 180)"), rgb("oklch(0.5 0.1 180)"));
        assert_eq!(rgb("LAB(50 0 0)"), rgb("lab(50 0 0)"));
        assert_eq!(rgb("  oklch(0.5 0.1 180)\n"), rgb("oklch(0.5 0.1 180)"));
        assert_eq!(rgb("oklch(  0.5 0.1 180  )"), rgb("oklch(0.5 0.1 180)"));
    }

    #[test]
    fn number_forms() {
        let expected = rgb("oklch(0.5 0.1 180)");
        assert_eq!(rgb("oklch(.5 .1 180)"), expected);
        assert_eq!(rgb("oklch(+0.5 1e-1 +180)"), expected);
    }

    #[test]
    fn lightness_is_clamped() {
        assert_eq!(rgb("oklch(1.5 0.1 180)"), Ok((255, 255, 255)));
        assert_eq!(rgb("oklch(-0.5 0.1 180)"), Ok((0, 0, 0)));
        assert_eq!(rgb("lab(-5 0 0)"), Ok((0, 0, 0)));
        assert_eq!(rgb("lab(150 0 0)"), Ok((255, 255, 255)));
    }

    #[test]
    fn negative_chroma_is_clamped_to_zero() {
        assert_eq!(rgb("oklch(0.5 -0.3 180)"), rgb("oklch(0.5 0 180)"));
    }

    #[test]
    fn rejections() {
        for spec in [
            "oklch()",
            "oklch(0.5)",
            "oklch(0.5 0.1)",
            "oklch(0.5 0.1 180 0.2)",
            "oklch(a b c)",
            "oklch(0.5 0.1 180",
            "oklch(0.5 0.1 180)x",
            "oklch(0.5 0.1 50%)",
            "oklch(nan 0 0)",
            "oklch(inf 0 0)",
            "oklch(0 Infinity 0)",
            "oklch(1e999 0 0)",
            "lab(50)",
            "lab(50 0deg 0)",
            "lab(50 0 0 0)",
            "oklch (0.5 0.1 180)",
            "oklch(0.5 0.1 180 /)",
            "oklch(0.5 0.1 180 / 0.5 0.5)",
            "oklch(0.5 0.1 180 / 0.5 / 0.5)",
            "oklch(0.5 0.1 180 / x)",
            "oklch(0.5deg 0.1 180)",
            "oklch(0.5 0.1 180 deg)",
        ] {
            assert!(rgb(spec).is_err(), "spec {spec:?} should be rejected");
        }
    }

    #[test]
    fn error_variants_describe_the_failure() {
        assert_eq!(
            rgb("oklch(0.5 0.1)"),
            Err(ColorSpecError::WideGamutComponentCount { found: 2 })
        );
        assert_eq!(
            rgb("oklch(0.5 0.1 180"),
            Err(ColorSpecError::InvalidWideGamut)
        );
        assert_eq!(
            rgb("oklch(0.5 0.1 50%)"),
            Err(ColorSpecError::InvalidWideGamutComponent)
        );
        assert_eq!(
            ColorSpecError::WideGamutComponentCount { found: 2 }.to_string(),
            "'oklch()' or 'lab()' has 2 components; expected 3"
        );
    }

    #[test]
    fn huge_values_never_panic() {
        assert!(matches!(
            rgb("lab(50 1e308 1e308)"),
            Ok(_) | Err(ColorSpecError::InvalidWideGamutComponent)
        ));
        assert!(matches!(
            rgb("oklch(0.5 1e308 1e308)"),
            Ok(_) | Err(ColorSpecError::InvalidWideGamutComponent)
        ));
    }

    /// The original, unaccelerated CSS Color 4 §14.2.2 bisection: no
    /// fast-forward, and the step cap that the widest finite `f64` chroma
    /// needs.  The oracle the production algorithm must match bit for bit.
    fn reference_gamut_map_oklch(origin: Vec3) -> Vec3 {
        const REFERENCE_MAX_ITERATIONS: usize = 1_100;
        let [lightness, chroma, hue] = origin;
        if lightness >= 1.0 {
            return [1.0, 1.0, 1.0];
        }
        if lightness <= 0.0 {
            return [0.0, 0.0, 0.0];
        }
        let origin_srgb = oklab_to_srgb(oklch_to_oklab(origin));
        if in_gamut(origin_srgb) {
            return origin_srgb;
        }
        let clip = |srgb: Vec3| srgb.map(|v| v.clamp(0.0, 1.0));
        let mut clipped = clip(origin_srgb);
        let mut e = delta_e_ok(srgb_to_oklab(clipped), oklch_to_oklab(origin));
        if e < JND {
            return clipped;
        }
        let mut min = 0.0_f64;
        let mut max = chroma;
        let mut min_in_gamut = true;
        for _ in 0..REFERENCE_MAX_ITERATIONS {
            if max - min <= GAMUT_EPSILON {
                break;
            }
            let probe = f64::midpoint(min, max);
            let current_oklab = oklch_to_oklab([lightness, probe, hue]);
            let current_srgb = oklab_to_srgb(current_oklab);
            if min_in_gamut && in_gamut(current_srgb) {
                min = probe;
                continue;
            }
            clipped = clip(current_srgb);
            e = delta_e_ok(srgb_to_oklab(clipped), current_oklab);
            if e < JND {
                if JND - e < GAMUT_EPSILON {
                    return clipped;
                }
                min_in_gamut = false;
                min = probe;
            } else {
                max = probe;
            }
        }
        clipped
    }

    #[test]
    fn fast_forward_is_bit_identical_to_the_unaccelerated_algorithm() {
        let chromas = [
            0.3, 0.9, 1.0, 1.0001, 1.7, 2.0, 2.0001, 3.0, 50.0, 1e4, 1e100, 1e308,
        ];
        for chroma in chromas {
            for l_step in 1_u16..40 {
                let lightness = f64::from(l_step) / 40.0;
                for h_step in 0_u16..36 {
                    let hue = f64::from(h_step) * 10.0;
                    let origin = [lightness, chroma, hue];
                    let got = gamut_map_oklch(origin).map(f64::to_bits);
                    let want = reference_gamut_map_oklch(origin).map(f64::to_bits);
                    assert_eq!(got, want, "L={lightness} C={chroma} h={hue}");
                }
            }
        }
    }

    #[test]
    fn midpoint_with_zero_is_exact_halving() {
        let mut samples = vec![
            f64::MAX,
            f64::MAX / 2.0,
            f64::MIN_POSITIVE,
            f64::MIN_POSITIVE * 2.0,
            f64::MIN_POSITIVE * 3.0,
            f64::from_bits(1),
            f64::from_bits(2),
            f64::from_bits(3),
            1e308,
            1e-310,
            1.0,
            3.0,
        ];
        let mut x = 1e-320_f64;
        for _ in 0..700 {
            samples.push(x);
            samples.push(x * 1.234_567_89);
            x *= 7.3;
        }
        for x in samples {
            assert_eq!(
                f64::midpoint(0.0, x).to_bits(),
                (x / 2.0).to_bits(),
                "midpoint(0, {x:e})"
            );
        }
    }

    #[test]
    fn iteration_bound_exceeds_what_the_epsilon_stop_needs() {
        // 2.0 is the widest interval after the fast-forward; each step
        // halves it.  The compile-time assertion next to the constant is the
        // authority; this pins the same arithmetic as an observable test.
        let steps_needed = (1_u16..=64)
            .find(|&n| 2.0 * FORCED_STEP_CHROMA / 2.0_f64.powi(i32::from(n)) <= GAMUT_EPSILON)
            .unwrap_or(u16::MAX);
        assert_eq!(steps_needed, 15);
        assert!(usize::from(steps_needed) < MAX_GAMUT_ITERATIONS);
    }

    proptest! {
        #[test]
        fn never_panics_on_oklch_prefixed(s in "\\PC{0,32}") {
            let _ = parse_color_spec(&format!("oklch({s}"));
        }

        #[test]
        fn never_panics_on_lab_prefixed(s in "\\PC{0,32}") {
            let _ = parse_color_spec(&format!("lab({s}"));
        }

        #[test]
        fn never_panics_on_closed_oklch_and_lab(s in "[-+0-9a-zA-Z .,%/]{0,32}") {
            let _ = parse_color_spec(&format!("oklch({s})"));
            let _ = parse_color_spec(&format!("lab({s})"));
        }

        #[test]
        fn sane_oklch_ranges_always_parse(
            l in 0.0_f64..=1.0,
            c in 0.0_f64..=0.5,
            h in 0.0_f64..360.0,
        ) {
            let spec = format!("oklch({l} {c} {h})");
            prop_assert!(parse_color_spec(&spec).is_ok());
        }
    }
}
