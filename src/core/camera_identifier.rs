// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2021-2022 Adrian <adrian.eddy at gmail>

use serde::{ Serialize, Deserialize };

use telemetry_parser::Input;
use telemetry_parser::tags_impl::{ GetWithType, GroupId, TagId };
use std::io::{Read, Result, Seek, SeekFrom};

#[derive(Deserialize, Serialize, Default, Clone, Debug)]
#[serde(default)]
pub struct CameraIdentifier {
    pub brand: String,
    pub model: String,
    pub lens_model: String,
    pub lens_info: String,
    pub focal_length: Option<f64>,
    pub camera_setting: String,
    pub fps: usize,
    pub video_width: usize,
    pub video_height: usize,
    pub additional: String,

    pub identifier: String
}

impl CameraIdentifier {
    pub(crate) fn fill_sony_mxf_lens_model(&mut self, stream: &mut (impl Read + Seek), url: &str) {
        if self.brand != "Sony" || !self.lens_model.trim().is_empty() { return; }
        let filename = crate::filesystem::get_filename(url);
        let Some((stem, ext)) = filename.rsplit_once('.') else { return; };
        if !ext.eq_ignore_ascii_case("mxf") { return; }

        let lens_model = read_sony_mxf_lens(stream).or_else(|| {
            let folder = crate::filesystem::get_folder(url);
            let sidecar = crate::filesystem::get_file_url(&folder, &format!("{stem}M01.xml"), false);
            let mut xml = crate::filesystem::open_file(&sidecar, false, false).ok()?;
            if xml.size > SONY_METADATA_LIMIT { return None; }
            sony_xml_lens_model(xml.get_file().take(SONY_METADATA_LIMIT))
        });
        if let Some(lens_model) = lens_model {
            self.lens_model = lens_model;
            self.identifier = self.get_identifier();
            log::debug!("Sony MXF lens identified as {:?}", self.lens_model);
        }
    }

    pub fn from_telemetry_parser(input: &Input, video_width: usize, video_height: usize, fps: f64) -> Result<Self> {
        let fps = (fps * 1000.0).round() as usize;
        let brand = input.camera_type();
        let model = input.camera_model().cloned().unwrap_or_default();

        let mut id = Self {
            brand: brand.clone(),
            model,
            video_width,
            video_height,
            fps,

            ..Default::default()
        };

        match id.brand.to_ascii_lowercase().as_str() {
            "runcam" | "caddx" => id.lens_info = "wide".into(),
            _ => { }
        }

        if !id.brand.is_empty() {
            id.model = id.model.to_string().replace(&id.brand, "").trim().to_string();
        }

        match brand.as_str() {
            "GoPro" => {
                if let Some(ref samples) = input.samples {
                    for info in samples {
                        if let Some(ref tag_map) = info.tag_map {
                            if let Some(map) = tag_map.get(&GroupId::Default) {
                                if let Some(v) = map.get_t(TagId::Unknown(0x45495341/*EISA*/)) as Option<&String> {
                                    if v != "N/A" {
                                        id.additional = if v == "Y" || v == "N" {
                                            format!("EIS-{}", v)
                                        } else {
                                            v.clone()
                                        };
                                    }
                                }
                                if let Some(v) = map.get_t(TagId::Unknown(0x45495345/*EISE*/)) as Option<&String> {
                                    if id.additional.is_empty() {
                                        id.additional = format!("EIS-{}", v);
                                    }
                                }
                                if id.additional == "EIS-N" {
                                    id.additional = "NO-EIS".into();
                                }
                                if let Some(v) = map.get_t(TagId::Unknown(0x56464f56/*VFOV*/)) as Option<&String> {
                                    match v.as_str() {
                                        "X" => id.lens_info = "Max".into(),
                                        "W" => id.lens_info = "Wide".into(),
                                        "S" => id.lens_info = "Super".into(),
                                        "H" => id.lens_info = "Hyper".into(),
                                        "L" => id.lens_info = "Linear".into(),
                                        "N" => id.lens_info = "Narrow".into(),
                                        "M" => id.lens_info = "Medium".into(),
                                        _ => id.lens_info = v.into()
                                    };
                                }
                                if let Some(v) = map.get_t(TagId::Unknown(0x5a464f56/*ZFOV*/)) as Option<&f32> {
                                    if id.lens_info == "Linear" && *v < 80.0 {
                                        id.lens_info = "Narrow".into();
                                    }
                                }
                                if let Some(v) = map.get_t(TagId::Unknown(0x50524a54/*PRJT*/)) as Option<&String> {
                                    if v.as_str() == "GPMW" {
                                        id.lens_info = "Max Wide".into();
                                    }
                                }
                                break;
                            }
                        }
                    }
                }
            },
            "Sony" => {
                if let Some(ref samples) = input.samples {
                    if let Some(info) = samples.iter().next() {
                        if let Some(ref tag_map) = info.tag_map {
                            if let Some(v) = tag_map.get(&GroupId::Lens).and_then(|map| map.get_t(TagId::FocalLength) as Option<&f32>) {
                                id.lens_info = format!("{:.2} mm", v);
                                id.focal_length = Some(*v as f64);
                            }
                            if let Some(v) = tag_map.get(&GroupId::Lens).and_then(|map| map.get_t(TagId::DisplayName) as Option<&String>) {
                                id.lens_model = v.clone();
                            }
                            if let Some(v) = tag_map.get(&GroupId::Custom("LensDistortion".into())).and_then(|map| map.get_t(TagId::Data) as Option<&serde_json::Value>) {
                                if id.lens_info.is_empty() {
                                    let mut hasher = crc32fast::Hasher::new();
                                    // use previous json for hash to load previous lens profiles
                                    if let Some(v) = v.as_object() {
                                        if v.contains_key("focal_length_nm") {
                                            let s = serde_json::json!({
                                                "unk1": [v["focal_length_nm"], v["effective_sensor_height_nm"]],
                                                "unk2": v["unk1"],
                                                "unk3": v["coeff_scale"],
                                                "unk4": v["coeffs"]
                                            }).to_string();
                                            hasher.update(s.as_bytes());

                                            id.lens_info = format!("{:x}", hasher.finalize());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "Insta360" => {
                if let Some(ref samples) = input.samples {
                    for info in samples {
                        if let Some(ref tag_map) = info.tag_map {
                            if let Some(map) = tag_map.get(&GroupId::Default) {
                                if let Some(v) = map.get_t(TagId::Metadata) as Option<&serde_json::Value> {
                                    if let Some(fov_type) = v.get("fov_type").and_then(|v| v.as_str()) {
                                        id.lens_info = fov_type.replace("FovType", "");
                                    }
                                    if let Some(fov) = v.get("fov").and_then(|v| v.as_f64()) {
                                        if fov > 0.0 {
                                            id.lens_info.push_str(&format!(" {:.0}", fov));
                                        }
                                    }
                                    if let Some(flowstate) = v.get("is_flowstate_online").and_then(|v| v.as_bool()) {
                                        id.additional = if flowstate { "EIS" } else { "NO-EIS" }.into();
                                    }
                                }
                                break;
                            }
                        }
                    }
                }
            }
            _ => {
                if let Some(ref samples) = input.samples {
                    let mut try_again = false;
                    for info in samples {
                        if let Some(ref tag_map) = info.tag_map {
                            if let Some(v) = tag_map.get(&GroupId::Lens).and_then(|map| map.get_t(TagId::FocalLength) as Option<&f32>) {
                                id.lens_info = format!("{:.2} mm", v);
                                id.focal_length = Some(*v as f64);
                            }
                            if brand != "Runcam" {
                                if let Some(v) = tag_map.get(&GroupId::Lens).and_then(|map| map.get_t(TagId::Name) as Option<&String>) {
                                    id.lens_model = v.clone();
                                }
                            }
                            if let Some(map) = tag_map.get(&GroupId::Default) {
                                if let Some(v) = map.get_t(TagId::Metadata) as Option<&serde_json::Value> {
                                    log::debug!("Camera ID Brand: {}, Model: {}, Metadata: {:?}", id.brand, id.model, v);
                                    if let Some(v) = v.get("lens_info")             .and_then(|v| v.as_str()) { id.lens_info      = v.to_string(); }
                                    if let Some(v) = v.get("focal_length")          .and_then(|v| v.as_f64()) { id.lens_info      = format!("{:.2}mm", v); id.focal_length = Some(v); }
                                    if let Some(v) = v.get("focal_length")          .and_then(|v| v.as_str()) { id.lens_info      = v.to_string(); id.focal_length = v.replace("mm", "").parse::<f64>().ok(); }
                                    if let Some(v) = v.get("lens_type")             .and_then(|v| v.as_str()) { id.lens_model     = v.to_string(); }
                                    if let Some(v) = v.get("resolution_format_name").and_then(|v| v.as_str()) { id.camera_setting = v.to_string(); }
                                }
                            }
                        }
                        if id.lens_info.is_empty() && !try_again {
                            try_again = true;
                            continue;
                        }
                        break;
                    }
                }
            }
        }

        id.identifier = id.get_identifier();

        log::debug!("{:#?}", id);

        Ok(id)
    }

    pub fn get_identifier_for_autoload(&self) -> String {
        self.identifier.replace("hero12", "hero11")
                       .replace("hero13", "hero11")
                       .replace("hero11blackmini", "hero11black")
    }

    fn get_identifier(&self) -> String {
        if self.brand.is_empty() || self.model.is_empty() || self.lens_info.is_empty() { return String::new(); }
        let fps = match self.brand.as_ref() {
            "RED" | "RED RAW" => 0, // RED doesn't do any sensor crop while maintaining the resolution, so we can skip fps
            _ => self.fps
        };

        let mut id = format!("{}-{}-{}-{}-{}x{}@{}-{}", self.brand, self.model, self.lens_model, self.lens_info, self.video_width, self.video_height, fps, self.additional);
        id = id.replace(' ', "");
        id = id.replace("--", "-");
        id = id.replace("--", "-");
        let x: &[_] = &['-', ' '];
        id.trim_matches(x).to_lowercase()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

const SONY_METADATA_LIMIT: u64 = 1024 * 1024;

fn read_sony_mxf_lens(stream: &mut (impl Read + Seek)) -> Option<String> {
    let position = stream.stream_position().ok()?;
    stream.seek(SeekFrom::Start(0)).ok()?;
    let mut header = Vec::new();
    let result = stream.take(SONY_METADATA_LIMIT).read_to_end(&mut header);
    stream.seek(SeekFrom::Start(position)).ok()?;
    result.ok()?;

    let start_tag = b"<NonRealTimeMeta";
    let end_tag = b"</NonRealTimeMeta>";
    let start = header.windows(start_tag.len()).position(|bytes| bytes == start_tag)?;
    let xml = &header[start..];
    let end = xml.windows(end_tag.len()).position(|bytes| bytes == end_tag)?;
    sony_xml_lens_model(&xml[..end + end_tag.len()])
}

fn sony_xml_lens_model(xml: impl Read) -> Option<String> {
    use xml::reader::{EventReader, XmlEvent};

    let mut depth = 0;
    let mut lens = None;
    for event in EventReader::new(xml) {
        match event.ok()? {
            XmlEvent::StartElement { name, attributes, .. } => {
                if depth == 0 && name.local_name != "NonRealTimeMeta" { return None; }
                if depth == 1 && name.local_name == "Lens" {
                    lens = attributes.into_iter().find(|attr| attr.name.local_name == "modelName")
                        .map(|attr| attr.value.trim().to_owned()).filter(|value| !value.is_empty());
                }
                depth += 1;
            }
            XmlEvent::EndElement { .. } => depth -= 1,
            _ => {}
        }
    }
    lens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_sony_mxf_lens_with_manufacturer_attribute_first() {
        let mut stream = std::io::Cursor::new(br#"MXF<NonRealTimeMeta xmlns="urn:sony"><Lens manufacturer="" modelName="17-40mm F1.8 DC | Art 025"/></NonRealTimeMeta>MXF"#);
        stream.set_position(2);
        assert_eq!(read_sony_mxf_lens(&mut stream).as_deref(), Some("17-40mm F1.8 DC | Art 025"));
        assert_eq!(stream.position(), 2);
    }
}
