//! Compile the desktop REDline adapter without Qt and exercise its missing-tool path.
mod external_sdk {
    lazy_static::lazy_static! {
        pub static ref SDK_PATH: std::io::Result<std::path::PathBuf> = Ok(std::env::temp_dir().join("gyroflow-sdk-test-not-installed"));
    }
    pub mod r3d {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../external_sdk/r3d.rs"));
    }
}

#[test]
fn nikon_conversion_reports_missing_redline_without_rendering_raw() {
    use external_sdk::r3d::REDSdk;
    if !REDSdk::find_redline().is_empty() {
        println!("REDline is installed; the missing-tool case does not apply on this system.");
        return;
    }
    let mut results = Vec::new();
    REDSdk::convert_r3d("file:///missing.NEV", 0, false, 7, 2, "", |result| results.push(result), std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)));
    assert_eq!(results.len(), 1);
    assert!(results[0].1.contains("REDline"));
    assert!(!results[0].1.is_empty());
    assert!(results[0].2.is_empty(), "a failed conversion must not supply an output URL");
}