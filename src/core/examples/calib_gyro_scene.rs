// The lens calibration from the scene and the motion data (`calibration::gyro_scene`) end to end on a clip, without the
// GUI: the clip's own motion data, its frames from a raw dump of gray frames (each an i32 frame number, width and height,
// then the pixels: what `xocn-dump` writes for Sony X-OCN, which nothing else here decodes), the tracker and the fit.
// Needs the OpenCV environment `just` exports:
// cargo run --release --features use-opencv --example calib_gyro_scene -- <clip> <frames.raw> <video width> <video height> <fps> [horizontal stretch] [readout ms] [band]
// `band` (e.g. 0.04-0.40) keeps only the features in that part of the frame's height, for comparing with a fit that did.

#[cfg(not(feature = "use-opencv"))]
fn main() { eprintln!("needs --features use-opencv"); }

#[cfg(feature = "use-opencv")]
fn main() {
    use gyroflow_core::{ StabilizationManager, filesystem, telemetry_parser::util::VideoMetadata };
    use gyroflow_core::calibration::gyro_scene;
    use gyroflow_core::stabilization::{ ComputeParams, distortion_models::{ OpenCVFisheye, OpenCVStandard, GenericPolynomial } };
    use gyroflow_core::synchronization::optical_motion::KltDevice;
    use std::io::Read;

    // The fit's own log, of this module only
    struct Stdout;
    impl log::Log for Stdout {
        fn enabled(&self, m: &log::Metadata) -> bool { m.target().contains("gyro_scene") }
        fn log(&self, r: &log::Record) { if self.enabled(r.metadata()) { println!("  [{}] {}", r.level(), r.args()); } }
        fn flush(&self) {}
    }
    static LOGGER: Stdout = Stdout;
    let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(if std::env::var("SCENE_DEBUG").is_ok() { log::LevelFilter::Debug } else { log::LevelFilter::Info }));
    let args: Vec<String> = std::env::args().collect();
    let (clip, raw) = (&args[1], &args[2]);
    let (vw, vh): (usize, usize) = (args[3].parse().unwrap(), args[4].parse().unwrap());
    let fps: f64 = { let r: Vec<f64> = args[5].split('/').map(|x| x.parse().unwrap()).collect(); if r.len() == 2 { r[0] / r[1] } else { r[0] } };
    let stretch: f64 = args.get(6).and_then(|x| x.parse().ok()).unwrap_or(1.0);
    let readout: f64 = args.get(7).and_then(|x| x.parse().ok()).unwrap_or(0.0);
    let band: Option<(f64, f64)> = args.get(8).map(|b| { let v: Vec<f64> = b.split('-').map(|x| x.parse().unwrap()).collect(); (v[0], v[1]) });

    let mut data = Vec::new();
    std::fs::File::open(raw).unwrap().read_to_end(&mut data).unwrap();
    let header = |o: usize, i: usize| i32::from_le_bytes(data[o + i * 4..o + i * 4 + 4].try_into().unwrap());
    let (tw, th) = (header(0, 1) as u32, header(0, 2) as u32);
    let frame_bytes = 12 + (tw * th) as usize;
    let frames = data.len() / frame_bytes;

    let stab = StabilizationManager::default();
    let url = filesystem::path_to_url(clip);
    let mut file = std::fs::File::open(clip).unwrap();
    let size = file.metadata().unwrap().len();
    let md = VideoMetadata { width: vw, height: vh, fps, duration_s: frames as f64 / fps, rotation: 0 };
    stab.load_video_file(&mut file, size, &url, Some(md), false).unwrap();
    stab.lens.write().input_horizontal_stretch = stretch;
    stab.params.write().frame_readout_time = readout;
    {
        let gyro = stab.gyro.read();
        let md = gyro.file_metadata.read();
        println!("{} orientations, in-body stabilization data: {}, per-frame time offsets: {}", gyro.quaternions.len(), md.camera_stab_at(0).is_some(), md.frame_time_offset(0));
    }

    let t = std::time::Instant::now();
    let mut tracker = gyro_scene::SceneTracker::new(KltDevice::Gpu(None), (tw, th), fps);
    for i in 0..frames {
        let o = i * frame_bytes;
        let frame = header(o, 0) as usize;
        tracker.feed(frame, (frame as f64 / fps * 1_000_000.0).round() as i64, &data[o + 12..o + frame_bytes]);
    }
    let tracked: usize = tracker.pairs.iter().map(|p| p.a.len()).sum();
    println!("{frames} frames of {tw}x{th}: {} pairs, {tracked} features, in {:.1}s", tracker.pairs.len(), t.elapsed().as_secs_f64());

    let params = ComputeParams::from_manager(&stab);
    let mut obs = gyro_scene::scene_observations(&params, &tracker.pairs, (tw, th));
    let calib = ((vw as f64 * stretch).round() as usize, vh);
    // NO_IBIS: the tracked pixels only scaled and stretched, without undoing in-body stabilization, to see what it does
    if std::env::var("NO_IBIS").is_ok() {
        let gyro = stab.gyro.read();
        let md = gyro.file_metadata.read();
        let (sx, sy) = (calib.0 as f64 / tw as f64, calib.1 as f64 / th as f64);
        let time = |frame: usize, ts_us: i64, y: f32| ts_us as f64 / 1000.0 + md.frame_time_offset(frame) + readout * (y as f64 / th as f64 - 0.5);
        obs = tracker.pairs.iter().flat_map(|p| p.a.iter().zip(&p.b).map(|(a, b)| gyro_scene::SceneObservation {
            frames: p.frames,
            times_ms: (time(p.frames.0, p.timestamps_us.0, a.1), time(p.frames.1, p.timestamps_us.1, b.1)),
            a: (a.0 as f64 * sx, a.1 as f64 * sy),
            b: (b.0 as f64 * sx, b.1 as f64 * sy),
        }).collect::<Vec<_>>()).collect();
        println!("Without undoing in-body stabilization: {} observations", obs.len());
    }
    if let Some((lo, hi)) = band {
        obs.retain(|o| o.a.1 >= lo * calib.1 as f64 && o.a.1 <= hi * calib.1 as f64);
        println!("{} observations in the band {lo}-{hi} of the height", obs.len());
    }
    let gyro = stab.gyro.read().clone_for_lookups(true);
    let orientation = |frame: usize, ts: f64| gyro.org_quat_at_frame(frame, ts);
    let noise = vh as f64 / th as f64;
    let models = std::env::var("SCENE_MODELS").unwrap_or("opencv_fisheye,opencv_standard,generic_polynomial".into());
    // SCENE_MODELS=opencv_fisheye fits that one alone
    for model in models.split(',') {
        let t = std::time::Instant::now();
        let c = match model {
            "opencv_fisheye" => gyro_scene::calibrate(&OpenCVFisheye::default(), &obs, calib, &orientation, noise),
            "opencv_standard" => gyro_scene::calibrate(&OpenCVStandard::default(), &obs, calib, &orientation, noise),
            _ => gyro_scene::calibrate(&GenericPolynomial::default(), &obs, calib, &orientation, noise),
        };
        match c {
            Some(c) => println!("{model}: {:.1}s, f ({:.1} ± {:.1}, {:.1} ± {:.1}), fx/fy {:.4}, k {:?}, time offset {:+.2} ms, rms {:.2} px, median {:.2} px, {} of {} kept, up to {:.1}°, reach {:.0}% / {:.0}%",
                t.elapsed().as_secs_f64(), c.f.0, c.f_sigma.0, c.f.1, c.f_sigma.1, c.f.0 / c.f.1, &c.k[..c.fitted], c.time_offset_ms, c.rms, c.median, c.inliers, c.observations,
                c.max_angle.to_degrees(), c.reach.0 * 100.0, c.reach.1 * 100.0),
            None => println!("{model}: no fit"),
        }
    }
}
