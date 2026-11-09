use gyroflow_core::gyro_source::{FileMetadata, LensParams};
use gyroflow_core::stabilization::{ComputeParams, FrameTransform};

#[test]
fn invalid_focal_metadata_keeps_a_finite_nonzero_projection() {
    let params = ComputeParams { width: 6048, height: 3402, output_width: 6048, output_height: 3402, fov_scale: 1.0, ..Default::default() };
    for focal in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        for distortion in [Vec::new(), vec![0.0; 4]] {
            let lens = LensParams { focal_length: Some(focal), pixel_pitch: Some((5930,5930)), capture_area_size: Some((6060.0,3410.0)), distortion_coefficients: distortion, ..Default::default() };
            let metadata = FileMetadata { lens_params: [(0,lens)].into_iter().collect(), ..Default::default() };
            let matrix = FrameTransform::get_lens_data_at_timestamp_with_metadata(&params, &metadata, 0.0, false).0;
            assert!(matrix[(0,0)].is_finite() && matrix[(0,0)] > 0.0, "invalid horizontal focal projection from {focal:?}: {matrix:?}");
            assert!(matrix[(1,1)].is_finite() && matrix[(1,1)] > 0.0, "invalid vertical focal projection from {focal:?}: {matrix:?}");
        }
    }
}