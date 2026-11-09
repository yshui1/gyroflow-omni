// SPDX-License-Identifier: GPL-3.0-or-later
//! Calibration-only .cube processing. Entries are red-fastest, as specified by .cube.

pub const BUILTINS: &[(&str, &str)] = &[
    ("DJI_OSMO_DlogM", include_str!("../resources/luts/DJI_OSMO_DlogM.cube")),
    ("DJI_Pocket4P_Dlog", include_str!("../resources/luts/DJI_Pocket4P_Dlog.cube")),
    ("DJI_Pocket4P_Dlog2", include_str!("../resources/luts/DJI_Pocket4P_Dlog2.cube")),
    ("Gopro_Mission1_GPLOG2", include_str!("../resources/luts/Gopro_Mission1_GPLOG2.cube")),
    ("Nikon_Log3G10", include_str!("../resources/luts/Nikon_Log3G10.Cube")),
    ("Nikon_NLog", include_str!("../resources/luts/Nikon_NLog.cube")),
    ("Panasonic_LogC3", include_str!("../resources/luts/Panasonic_LogC3.cube")),
    ("Panasonic_Vlog", include_str!("../resources/luts/Panasonic_Vlog.cube")),
    ("Sony_Slog3", include_str!("../resources/luts/Sony_Slog3.cube")),
];

pub struct CubeLut {
    size: usize,
    min: [f32; 3],
    max: [f32; 3],
    values: Vec<[f32; 3]>,
    gpu: std::sync::OnceLock<GpuLut>,
}

/// Tiled blue slices; RG are bilinearly filtered within each slice on the GPU.
pub struct GpuLut {
    pub edge: u32,
    pub uniforms: [f32; 12],
    pub rgba: Vec<f32>,
}

impl CubeLut {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut size = None;
        let mut min = [0.0; 3];
        let mut max = [1.0; 3];
        let mut values = Vec::new();
        let triple = |parts: &[&str]| -> Result<[f32; 3], String> {
            if parts.len() != 3 { return Err("Expected three numbers".into()); }
            let mut result = [0.0; 3];
            for (i, part) in parts.iter().enumerate() {
                result[i] = part.parse::<f32>().map_err(|_| "Invalid LUT number")?;
                if !result[i].is_finite() { return Err("Non-finite LUT number".into()); }
            }
            Ok(result)
        };
        for line in text.trim_start_matches('\u{feff}').lines() {
            let parts: Vec<_> = line.split('#').next().unwrap_or("").split_whitespace().collect();
            let Some(key) = parts.first() else { continue; };
            match *key {
                "TITLE" => {},
                "LUT_3D_SIZE" => {
                    if size.is_some() || parts.len() != 2 { return Err("Invalid LUT_3D_SIZE".into()); }
                    let n = parts[1].parse::<usize>().map_err(|_| "Invalid LUT size")?;
                    if !(2..=129).contains(&n) { return Err("LUT size must be between 2 and 129".into()); }
                    size = Some(n);
                },
                "DOMAIN_MIN" => min = triple(&parts[1..])?,
                "DOMAIN_MAX" => max = triple(&parts[1..])?,
                _ => {
                    let n = size.ok_or("Expected LUT_3D_SIZE before LUT entries; only 3D .cube LUTs are supported")?;
                    if values.len() >= n * n * n { return Err("Too many LUT entries".into()); }
                    values.push(triple(&parts)?);
                }
            }
        }
        let size = size.ok_or("Missing LUT_3D_SIZE; only 3D .cube LUTs are supported")?;
        if values.len() != size * size * size { return Err("Incorrect number of LUT entries".into()); }
        if (0..3).any(|i| max[i] <= min[i] || !(max[i] - min[i]).is_finite()) { return Err("Invalid LUT domain".into()); }
        Ok(Self { size, min, max, values, gpu: Default::default() })
    }

    pub fn gpu(&self) -> &GpuLut {
        self.gpu.get_or_init(|| {
            let tiles = (self.size as f32).sqrt().ceil() as usize;
            let edge = tiles * self.size;
            let mut rgba = vec![0.0; edge * edge * 4];
            for b in 0..self.size { for g in 0..self.size { for r in 0..self.size {
                let x = b % tiles * self.size + r;
                let y = b / tiles * self.size + g;
                let v = self.values[r + self.size * (g + self.size * b)];
                rgba[(y * edge + x) * 4..(y * edge + x) * 4 + 3].copy_from_slice(&v);
                rgba[(y * edge + x) * 4 + 3] = 1.0;
            }}}
            GpuLut {
                edge: edge as u32,
                uniforms: [self.min[0], self.min[1], self.min[2], self.size as f32,
                           self.max[0], self.max[1], self.max[2], tiles as f32,
                           0.0, 0.0, 0.0, 0.0],
                rgba,
            }
        })
    }

    pub fn sample(&self, rgb: [f32; 3]) -> [f32; 3] {
        let p: [f32; 3] = std::array::from_fn(|i| ((rgb[i] - self.min[i]) / (self.max[i] - self.min[i])).clamp(0.0, 1.0) * (self.size - 1) as f32);
        let lo = p.map(|v| v.floor() as usize);
        let hi = lo.map(|v| (v + 1).min(self.size - 1));
        let t: [f32; 3] = std::array::from_fn(|i| p[i] - lo[i] as f32);
        let mut out = [0.0; 3];
        for b in 0..2 { for g in 0..2 { for r in 0..2 {
            let indices = [r, g, b];
            let xyz: [usize; 3] = std::array::from_fn(|i| if indices[i] == 0 { lo[i] } else { hi[i] });
            let weight: f32 = (0..3).map(|i| if indices[i] == 0 { 1.0 - t[i] } else { t[i] }).product();
            let v = self.values[xyz[0] + self.size * (xyz[1] + self.size * xyz[2])];
            for i in 0..3 { out[i] += v[i] * weight; }
        }}}
        out
    }

    /// Preserve decoded precision until the transformed Rec.709 luma is quantized for OpenCV.
    pub fn rgb48_to_gray(&self, pixels: &[u8], width: usize, height: usize, stride: usize) -> Vec<u8> {
        let mut gray = vec![0; width * height];
        for y in 0..height {
            for x in 0..width {
                let offset = y * stride + x * 6;
                let rgb = std::array::from_fn(|i| u16::from_le_bytes([pixels[offset + i * 2], pixels[offset + i * 2 + 1]]) as f32 / 65535.0);
                let v = self.sample(rgb);
                gray[y * width + x] = ((v[0] * 0.2126 + v[1] * 0.7152 + v[2] * 0.0722).clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
        gray
    }
}

/// Only explicit gamma names qualify. A camera model alone cannot identify log footage.
pub fn detect(brand: &str, model: &str, gamma: &str) -> &'static str {
    let normalize = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
    let brand = normalize(brand);
    let model = normalize(model);
    let gamma = normalize(gamma);
    match (brand.as_str(), gamma.as_str()) {
        ("sony", "slog3") => "Sony_Slog3",
        ("nikon", "nlog") => "Nikon_NLog",
        ("nikon", "log3g10") => "Nikon_Log3G10",
        ("panasonic", "vlog") => "Panasonic_Vlog",
        ("panasonic", "logc3") => "Panasonic_LogC3",
        ("dji", "dlogm") if model.contains("osmo") => "DJI_OSMO_DlogM",
        ("dji", "dlog") if model.contains("pocket4") => "DJI_Pocket4P_Dlog",
        ("dji", "dlog2") if model.contains("pocket4") => "DJI_Pocket4P_Dlog2",
        ("gopro", "gplog2") if model.contains("mission1") => "Gopro_Mission1_GPLOG2",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity() -> CubeLut {
        CubeLut::parse("LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1").unwrap()
    }
    #[test]
    fn interpolation_and_channel_order() {
        let lut = identity();
        for rgb in [[0.13, 0.7, 0.9], [0.0, 1.0, 0.0], [1.0; 3]] {
            let out = lut.sample(rgb);
            for i in 0..3 { assert!((rgb[i] - out[i]).abs() < 1e-6); }
        }
        assert_eq!(lut.sample([-1.0, 2.0, 0.0]), [0.0, 1.0, 0.0]);
    }
    #[test]
    fn domains_and_malformed_files() {
        let mut lut = identity();
        lut.min = [-1.0; 3]; lut.max = [1.0; 3];
        assert_eq!(lut.sample([0.0; 3]), [0.5; 3]);
        for text in ["", "LUT_1D_SIZE 2", "LUT_3D_SIZE 999", "LUT_3D_SIZE 2\nNaN 0 0", "LUT_3D_SIZE 2\n0 0 0"] {
            assert!(CubeLut::parse(text).is_err(), "{text}");
        }
    }
    #[test]
    fn bundled_luts_are_valid() {
        assert_eq!(BUILTINS.len(), 9);
        for (name, text) in BUILTINS { assert!(CubeLut::parse(text).is_ok(), "{name}"); }
    }
    #[test]
    fn gpu_atlas_matches_cube_entries_and_is_cached() {
        for (_, text) in BUILTINS {
            let lut = CubeLut::parse(text).unwrap();
            let atlas = lut.gpu();
            assert!(std::ptr::eq(atlas, lut.gpu()));
            assert!(atlas.edge <= 2048);
            assert_eq!(&atlas.uniforms[..3], &lut.min);
            assert_eq!(&atlas.uniforms[4..7], &lut.max);
            let tiles = atlas.uniforms[7] as usize;
            for b in 0..lut.size { for g in 0..lut.size { for r in 0..lut.size {
                let x = b % tiles * lut.size + r;
                let y = b / tiles * lut.size + g;
                let offset = (y * atlas.edge as usize + x) * 4;
                assert_eq!(&atlas.rgba[offset..offset + 3], &lut.values[r + lut.size * (g + lut.size * b)]);
            }}}
        }
    }
    #[test]
    fn rgb48_stride_and_luma() {
        let pixels = [255, 255, 0, 0, 0, 0, 42, 42, 0, 0, 255, 255, 0, 0, 42, 42];
        assert_eq!(identity().rgb48_to_gray(&pixels, 1, 2, 8), vec![54, 182]);
    }
    #[test]
    fn detection_requires_explicit_gamma_and_supported_camera() {
        assert_eq!(detect("Sony", "FX3", "S-Log3"), "Sony_Slog3");
        assert_eq!(detect("Panasonic", "S5", "V-Log"), "Panasonic_Vlog");
        assert_eq!(detect("Sony", "FX3", "Rec.709"), "");
        assert_eq!(detect("Sony", "FX3", ""), "");
        assert_eq!(detect("DJI", "Mavic", "D-Log"), "");
        assert_eq!(detect("GoPro", "HERO13", "GP-Log2"), "");
    }
}
