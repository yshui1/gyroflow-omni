// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared Nikon NRAW / R3D NE decoder and container handling.

use std::io::{Read, Seek, SeekFrom};

/// MDK's RED reader handles both Nikon RAW formats with the RED SDK runtime.
/// Keep CPU selection consistent between preview and analysis decoding.
pub fn red_decoder(gpu_decoding: bool, options: &str) -> String {
    format!("R3D:gpu={}{}", if gpu_decoding { "auto" } else { "no" }, options)
}

/// Nikon RAW uses a QuickTime container with the `niko` brand, unlike segmented
/// RED camera R3D files. A group of Nikon clips must not be treated as one clip.
/// Preserve the caller's stream position, including on an unsuccessful probe.
pub fn is_nikon_container<T: Read + Seek>(stream: &mut T) -> std::io::Result<bool> {
    let position = stream.stream_position()?;
    let result = (|| {
        stream.seek(SeekFrom::Start(0))?;
        let mut header = [0u8; 12];
        stream.read_exact(&mut header)?;
        if &header[4..8] != b"ftyp" { return Ok(false); }
        let size = u32::from_be_bytes(header[..4].try_into().unwrap());
        if size == 1 {
            // Extended-size box: finish the 64-bit size, then read the brand.
            let mut rest = [0u8; 8];
            stream.read_exact(&mut rest)?;
            return Ok(&rest[4..8] == b"niko");
        }
        Ok(size >= 16 && &header[8..12] == b"niko")
    })();
    stream.seek(SeekFrom::Start(position))?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn cpu_decode_is_used_in_preview_and_analysis_when_requested() {
        assert_eq!(red_decoder(false, ":scale=1920x1080"), "R3D:gpu=no:scale=1920x1080");
        assert_eq!(red_decoder(true, ""), "R3D:gpu=auto");
    }

    #[test]
    fn nikon_batch_detection_preserves_position_and_rejects_other_containers() {
        let mut normal = Cursor::new(b"\0\0\0\x14ftypniko\0\0\0\x01niko".to_vec());
        normal.set_position(7);
        assert!(is_nikon_container(&mut normal).unwrap());
        assert_eq!(normal.position(), 7);
        let mut extended = Cursor::new(b"\0\0\0\x01ftyp\0\0\0\0\0\0\0\x1cniko\0\0\0\x01niko".to_vec());
        assert!(is_nikon_container(&mut extended).unwrap());
        let mut other = Cursor::new(b"\0\0\0\x14ftypisom\0\0\0\0isom".to_vec());
        assert!(!is_nikon_container(&mut other).unwrap());
        let mut truncated = Cursor::new(vec![0u8; 5]);
        truncated.set_position(2);
        assert!(is_nikon_container(&mut truncated).is_err());
        assert_eq!(truncated.position(), 2);
    }

    #[test]
    #[ignore = "requires Nikon ZR camera samples"]
    fn nikon_camera_samples_are_independent_quicktime_clips() {
        let directory = std::env::var_os("GYROFLOW_NIKON_SAMPLE_DIR").expect("set GYROFLOW_NIKON_SAMPLE_DIR");
        for name in ["DSC_0073.NEV", "DSC_0073.R3D", "DSC_0003.R3D"] {
            let mut file = std::fs::File::open(std::path::Path::new(&directory).join(name)).unwrap();
            assert!(is_nikon_container(&mut file).unwrap(), "{name}");
        }
    }
}