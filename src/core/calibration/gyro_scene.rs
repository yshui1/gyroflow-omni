// SPDX-License-Identifier: GPL-3.0-or-later

//! Lens calibration without a chessboard: from the camera's own motion data and any distant scene.
//!
//! A feature far enough away moves in the picture by the camera's rotation alone, and the gyro measures that rotation.
//! So a feature at pixel `a` in one frame and `b` in a later one has `b = P(R·P⁻¹(a))`: `P` is the lens (the focal
//! lengths and the distortion, the unknowns) and `R` the rotation the gyro measured between the moments their rows
//! were read out. The lens that makes this true for every such pair is the calibration. Where a chessboard can't:
//! - the focal length is measured against the gyro's angles. A board seen nearly flat-on can't tell it from the board's
//!   distance (the tilt of each view absorbs it), which is how anamorphic calibrations came out 10x too long;
//! - the scene fills the frame, its sides too, where a board rarely gets on a 2.4:1 or 3:1 frame;
//! - it measures the lens at the distance it's focused at when filming, which on a front anamorphic changes the
//!   squeeze (a Kowa 2x: 2.00 at 1.5 m, 2.09 at infinity);
//! - fx and fy are fitted on their own, so the squeeze is what the fit says it is.
//!
//! It needs motion data in the file, rotation in every direction (pan, tilt and some roll), and a scene that's mostly
//! 10 m away or more: a nearer feature also moves with the camera's translation. Those, the ones on moving things and
//! the lost tracks are outliers, which a robust loss and rounds of rejection take out as long as they're a minority.
//! The principal point stays at the frame's centre: rotation alone pins it down poorly. The time offset between the
//! video and the motion data is fitted too, around the one the clip already has.
//!
//! On a Sony FX5 with a Kowa 2x anamorphic (9984x3328 de-squeezed), 785 frames of a handheld beach clip: fx 7614,
//! fy 7967, where the chessboard calibration of the same lens had 88280.

use nalgebra::{ DMatrix, DVector, Matrix3, Vector3 };
use rayon::prelude::*;
use std::collections::VecDeque;
use crate::gyro_source::Quat64;
use crate::stabilization::{ ComputeParams, FrameTransform };
use crate::synchronization::optical_motion::{ PairKlt, KltDevice };
use super::solver::{ Model, Start };

/// A feature seen in two frames
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SceneObservation {
    /// The frames, for the motion data of their part of a video cut from recordings
    pub frames: (usize, usize),
    /// When the feature's row was read out in each, on the video's clock (ms): the frame's time, its per-frame offset
    /// and the row's place in the rolling shutter's readout
    pub times_ms: (f64, f64),
    /// Its pixel in each, in the calibration frame: de-squeezed, with the sensor shifts of in-body stabilization undone
    pub a: (f64, f64),
    pub b: (f64, f64),
}

/// The camera's orientation read out in a frame at a video time (ms), as `GyroSource::org_quat_at_frame` gives it
pub type Orientation<'a> = dyn Fn(usize, f64) -> Quat64 + Sync + 'a;

#[derive(Debug, Clone)]
pub struct SceneCalibration {
    /// Focal lengths and principal point, in pixels of the calibration frame
    pub f: (f64, f64),
    pub c: (f64, f64),
    /// All of the model's coefficients, as `Model::project` reads them
    pub k: Vec<f64>,
    /// How many of `k` a profile stores (see `solver::Calibration::fitted`)
    pub fitted: usize,
    /// Residual of the inliers, px: RMS and median
    pub rms: f64,
    pub median: f64,
    /// Observations the fit kept, of all given
    pub inliers: usize,
    pub observations: usize,
    /// The time offset fitted on top of the clip's own, ms
    pub time_offset_ms: f64,
    /// One standard deviation of fx and fy
    pub f_sigma: (f64, f64),
    /// The largest angle from the optical axis an inlier is seen at, radians
    pub max_angle: f64,
    /// How far towards the sides and towards the top and bottom the inliers reach, as a fraction of half the frame
    pub reach: (f64, f64),
}

/// The motion data's orientations take a ray in the camera's axes (y up, z back) into the world. The rotation that
/// takes a ray of camera `a` to camera `b`, in the image's axes (x right, y down, z forward): `F·R_b⁻¹·R_a·F`, F the
/// flip of y and z the render's `kernel_rotation` does too
fn relative(orientation: &Orientation, o: &SceneObservation, dt: f64) -> Matrix3<f64> {
    let qa = orientation(o.frames.0, o.times_ms.0 + dt);
    let qb = orientation(o.frames.1, o.times_ms.1 + dt);
    let mut r = (qb.inverse() * qa).to_rotation_matrix().into_inner();
    r[(0, 1)] = -r[(0, 1)]; r[(0, 2)] = -r[(0, 2)];
    r[(1, 0)] = -r[(1, 0)]; r[(2, 0)] = -r[(2, 0)];
    r
}

/// The model's lens with the fit's intrinsics, `intr`: fx, fy, cx, cy, then the coefficients
struct Lens<'m, M: Model + ?Sized> { model: &'m M, start: Start }

impl<M: Model + ?Sized> Lens<'_, M> {
    /// Pixel → unit ray, by Newton's method on the start lens's normalized image point: a model is a mild change of it.
    /// `None` where it doesn't converge, or where the curve has folded (no ray is seen there)
    fn unproject(&self, intr: &[f64], p: (f64, f64)) -> Option<Vector3<f64>> {
        let target = ((p.0 - intr[2]) / intr[0], (p.1 - intr[3]) / intr[1]);
        let k = &intr[4..];
        let image = |q: (f64, f64)| -> Option<((f64, f64), Vector3<f64>)> {
            let ray = self.start.ray(q.0, q.1)?;
            let i = self.model.project(k, &ray);
            (i.0.is_finite() && i.1.is_finite()).then_some((i, ray))
        };
        let mut q = target;
        for _ in 0..30 {
            let (i, ray) = image(q)?;
            let e = (i.0 - target.0, i.1 - target.1);
            if e.0.abs().max(e.1.abs()) < 1e-11 { return Some(ray); }
            const H: f64 = 1e-7;
            let (ix, _) = image((q.0 + H, q.1))?;
            let (iy, _) = image((q.0, q.1 + H))?;
            let (j00, j10) = ((ix.0 - i.0) / H, (ix.1 - i.1) / H);
            let (j01, j11) = ((iy.0 - i.0) / H, (iy.1 - i.1) / H);
            let det = j00 * j11 - j01 * j10;
            // Folded: past where the image stops moving outwards with the angle
            if !(det > 1e-12) { return None; }
            q = (q.0 - (j11 * e.0 - j01 * e.1) / det, q.1 - (j00 * e.1 - j10 * e.0) / det);
        }
        let (i, ray) = image(q)?;
        ((i.0 - target.0).abs().max((i.1 - target.1).abs()) < 1e-8).then_some(ray)
    }

    /// Unit ray → pixel. A pinhole model has no image of a ray at or behind 90°
    fn project(&self, intr: &[f64], ray: &Vector3<f64>) -> Option<(f64, f64)> {
        if self.start == Start::Pinhole && ray.z <= 1e-6 { return None; }
        let (u, v) = self.model.project(&intr[4..], ray);
        let p = (intr[0] * u + intr[2], intr[1] * v + intr[3]);
        (p.0.is_finite() && p.1.is_finite()).then_some(p)
    }

    /// Where `o`'s feature lands in frame `b` against where it was seen, px
    fn residual(&self, intr: &[f64], rel: &Matrix3<f64>, o: &SceneObservation) -> Option<(f64, f64)> {
        let ray = rel * self.unproject(intr, o.a)?;
        let p = self.project(intr, &ray)?;
        Some((p.0 - o.b.0, p.1 - o.b.1))
    }

    /// Whether the image stops moving outwards with the angle anywhere inside `max_angle`, in any direction
    fn folds(&self, intr: &[f64], max_angle: f64) -> bool {
        (0..=6).any(|d| {
            let phi = (d as f64 * 15.0).to_radians();
            let mut last = 0.0;
            (1..=100).any(|s| {
                let theta = max_angle * s as f64 / 100.0;
                let ray = Vector3::new(theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos());
                let Some(p) = self.project(intr, &ray) else { return true };
                let r = (p.0 - intr[2]).hypot(p.1 - intr[3]);
                let folded = r <= last;
                last = r;
                folded
            })
        })
    }
}

/// Soft L1: quadratic within `sigma`, linear far out, so an outlier pulls with a constant force rather than its square
fn rho(sq: f64, sigma: f64) -> f64 { 2.0 * sigma * sigma * ((1.0 + sq / (sigma * sigma)).sqrt() - 1.0) }
fn weight(sq: f64, sigma: f64) -> f64 { 1.0 / (1.0 + sq / (sigma * sigma)).sqrt() }

#[derive(Clone, Debug)]
struct Fit { intr: Vec<f64>, dt: f64 }

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() { return f64::NAN; }
    let mid = v.len() / 2;
    *v.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1
}

/// The robust normal equations at `fit` over the observations `idx`: `JᵀWJ`, `JᵀWr` and `Σ w·|r|²`. Columns: the
/// intrinsics `cols`, then the time offset with `fit_dt`. A central difference, `rel_dt` the rotations at `fit.dt ± h`
#[allow(clippy::too_many_arguments)]
fn linearize<M: Model + ?Sized>(lens: &Lens<M>, obs: &[SceneObservation], idx: &[usize], fit: &Fit, cols: &[usize], rel: &[Matrix3<f64>], rel_dt: Option<(&[Matrix3<f64>], &[Matrix3<f64>], f64)>, sigma: f64) -> (DMatrix<f64>, DVector<f64>, f64) {
    let np = cols.len() + rel_dt.is_some() as usize;
    let n = fit.intr.len();
    let sums = idx.par_iter().enumerate().fold(|| (DMatrix::<f64>::zeros(np, np), DVector::<f64>::zeros(np), 0.0), |(mut a, mut g, mut wr), (m, &i)| {
        let o = &obs[i];
        let Some(r0) = lens.residual(&fit.intr, &rel[m], o) else { return (a, g, wr) };
        let w = weight(r0.0 * r0.0 + r0.1 * r0.1, sigma);
        let (mut jx, mut jy) = ([0.0f64; 24], [0.0f64; 24]);
        let mut x = [0.0f64; 24];
        x[..n].copy_from_slice(&fit.intr);
        for (c, &p) in cols.iter().enumerate() {
            let h = 1e-6 * fit.intr[p].abs().max(1e-2);
            x[p] = fit.intr[p] + h;
            let rp = lens.residual(&x[..n], &rel[m], o);
            x[p] = fit.intr[p] - h;
            let rm = lens.residual(&x[..n], &rel[m], o);
            x[p] = fit.intr[p];
            let (Some(rp), Some(rm)) = (rp, rm) else { return (a, g, wr) };
            jx[c] = (rp.0 - rm.0) / (2.0 * h);
            jy[c] = (rp.1 - rm.1) / (2.0 * h);
        }
        if let Some((plus, minus, h)) = rel_dt {
            let (Some(rp), Some(rm)) = (lens.residual(&fit.intr, &plus[m], o), lens.residual(&fit.intr, &minus[m], o)) else { return (a, g, wr) };
            jx[np - 1] = (rp.0 - rm.0) / (2.0 * h);
            jy[np - 1] = (rp.1 - rm.1) / (2.0 * h);
        }
        for r in 0..np {
            g[r] += w * (jx[r] * r0.0 + jy[r] * r0.1);
            for c in r..np { a[(r, c)] += w * (jx[r] * jx[c] + jy[r] * jy[c]); }
        }
        wr += w * (r0.0 * r0.0 + r0.1 * r0.1);
        (a, g, wr)
    }).reduce(|| (DMatrix::<f64>::zeros(np, np), DVector::<f64>::zeros(np), 0.0), |x, y| (x.0 + y.0, x.1 + y.1, x.2 + y.2));
    let (mut a, g, wr) = sums;
    // Only the upper triangle was summed
    for r in 0..np { for c in 0..r { a[(r, c)] = a[(c, r)]; } }
    (a, g, wr)
}

/// The robust cost of `intr` with the rotations `rel` over `idx`. An observation the lens can't map costs as much as
/// one 50 sigma off
fn cost<M: Model + ?Sized>(lens: &Lens<M>, obs: &[SceneObservation], idx: &[usize], intr: &[f64], rel: &[Matrix3<f64>], sigma: f64) -> f64 {
    let lost = rho((50.0 * sigma).powi(2), sigma);
    idx.par_iter().zip(rel.par_iter()).map(|(&i, r)| match lens.residual(intr, r, &obs[i]) {
        Some(e) => rho(e.0 * e.0 + e.1 * e.1, sigma),
        None => lost,
    }).sum()
}

fn rotations(obs: &[SceneObservation], idx: &[usize], orientation: &Orientation, dt: f64) -> Vec<Matrix3<f64>> {
    idx.par_iter().map(|&i| relative(orientation, &obs[i], dt)).collect()
}

/// Levenberg-Marquardt on the robust cost over `idx`: the intrinsics `free` marks, and the time offset with `fit_dt`
#[allow(clippy::too_many_arguments)]
fn refine<M: Model + ?Sized>(lens: &Lens<M>, obs: &[SceneObservation], idx: &[usize], orientation: &Orientation, fit: &mut Fit, free: &[bool], fit_dt: bool, sigma: f64, max_iter: usize) {
    const H_DT: f64 = 0.05;
    let cols: Vec<usize> = (0..free.len()).filter(|&i| free[i]).collect();
    let np = cols.len() + fit_dt as usize;
    let mut rel = rotations(obs, idx, orientation, fit.dt);
    let mut current = cost(lens, obs, idx, &fit.intr, &rel, sigma);
    if !current.is_finite() { return; }
    let mut lambda = 1e-3;
    for _ in 0..max_iter {
        let around = fit_dt.then(|| (rotations(obs, idx, orientation, fit.dt + H_DT), rotations(obs, idx, orientation, fit.dt - H_DT)));
        let (jtj, jtr, _) = linearize(lens, obs, idx, fit, &cols, &rel, around.as_ref().map(|(p, m)| (&p[..], &m[..], H_DT)), sigma);
        let mut stepped = false;
        let mut converged = false;
        while lambda < 1e10 {
            let mut a = jtj.clone();
            for d in 0..np { a[(d, d)] += lambda * a[(d, d)].max(1e-12); }
            let Some(step) = a.cholesky().map(|c| c.solve(&-&jtr)) else { lambda *= 10.0; continue };
            let mut next = fit.clone();
            for (c, &p) in cols.iter().enumerate() { next.intr[p] += step[c]; }
            if fit_dt { next.dt += step[np - 1]; }
            if !(next.intr[0] > 0.0 && next.intr[1] > 0.0) { lambda *= 10.0; continue; }
            let next_rel = if fit_dt { rotations(obs, idx, orientation, next.dt) } else { Vec::new() };
            let next_cost = cost(lens, obs, idx, &next.intr, if fit_dt { &next_rel } else { &rel }, sigma);
            if next_cost.is_finite() && next_cost < current {
                converged = current - next_cost < 1e-9 * current;
                *fit = next;
                if fit_dt { rel = next_rel; }
                current = next_cost;
                lambda = (lambda / 10.0).max(1e-9);
                stepped = true;
                break;
            }
            lambda *= 10.0;
        }
        if !stepped || converged { break; }
    }
}

/// Each observation's residual length, `None` where the lens can't map it
fn errors<M: Model + ?Sized>(lens: &Lens<M>, obs: &[SceneObservation], idx: &[usize], orientation: &Orientation, fit: &Fit) -> Vec<Option<f64>> {
    let rel = rotations(obs, idx, orientation, fit.dt);
    idx.par_iter().zip(rel.par_iter()).map(|(&i, r)| lens.residual(&fit.intr, r, &obs[i]).map(|e| e.0.hypot(e.1))).collect()
}

/// Calibrates `model` on features followed through a video of a distant scene (`obs`, in pixels of the calibration
/// frame `size`) against the camera's `orientation`. `noise_px` is how far apart two pixels of the frames the features
/// were tracked in are in the calibration frame: the scale of their precision. `None` when there's too little to fit
pub fn calibrate<M: Model + ?Sized>(model: &M, obs: &[SceneObservation], size: (usize, usize), orientation: &Orientation, noise_px: f64) -> Option<SceneCalibration> {
    const MIN_OBSERVATIONS: usize = 200;
    const MAX_OBSERVATIONS: usize = 150_000;
    if obs.len() < MIN_OBSERVATIONS || size.0 == 0 || size.1 == 0 { return None; }
    let (w, h) = (size.0 as f64, size.1 as f64);
    let (cx, cy) = (w * 0.5, h * 0.5);
    let half_diag = cx.hypot(cy);
    let noise = noise_px.max(0.5);
    let lens = Lens { model, start: model.start() };
    let started = std::time::Instant::now();

    // Spread evenly over the clip
    let every = |n: usize| -> Vec<usize> { let s = (obs.len() as f64 / n as f64).max(1.0); (0..obs.len().min(n)).map(|i| (i as f64 * s) as usize).collect() };
    let all = every(MAX_OBSERVATIONS);

    // The start: the focal length (the frame's half-diagonal seeing 5° to 170°) and the time offset (±150 ms) that
    // carry the features best with the model's starting lens, by the median error over a few thousand of them
    let probe = every(6000);
    let focals: Vec<f64> = (1..=34).filter_map(|i| Some(half_diag / lens.start.radius((i as f64 * 5.0).to_radians())?)).collect();
    let base: Vec<f64> = [1.0, 1.0, cx, cy].into_iter().chain(model.initial()).collect();
    let with_f = |f: f64| -> Vec<f64> { let mut i = base.clone(); i[0] = f; i[1] = f; i };
    let score = |intr: &[f64], rel: &[Matrix3<f64>]| -> f64 {
        median(probe.iter().zip(rel).map(|(&i, r)| lens.residual(intr, r, &obs[i]).map_or(1e9, |e| e.0.hypot(e.1))).collect())
    };
    let coarse = (-30..=30).into_par_iter().map(|s| {
        let dt = s as f64 * 5.0;
        let rel = rotations(obs, &probe, orientation, dt);
        focals.iter().map(|&f| (score(&with_f(f), &rel), f, dt)).min_by(|a, b| a.0.total_cmp(&b.0)).unwrap_or((f64::INFINITY, 0.0, 0.0))
    }).min_by(|a, b| a.0.total_cmp(&b.0))?;
    let (_, f0, dt0) = (-20..=20).into_par_iter().map(|s| {
        let dt = coarse.2 + s as f64 * 0.25;
        let rel = rotations(obs, &probe, orientation, dt);
        // and the focal length between the steps of the coarse search
        (-4..=4).map(|j| coarse.1 * 1.03f64.powi(j)).map(|f| (score(&with_f(f), &rel), f, dt)).min_by(|a, b| a.0.total_cmp(&b.0)).unwrap()
    }).min_by(|a, b| a.0.total_cmp(&b.0))?;
    ::log::info!("Scene calibration: {} observations, start f = {f0:.1}, time offset {dt0:+.2} ms, median error {:.2} px (coarse {:.2} px at f = {:.1}, {:+.0} ms)",
        obs.len(), score(&with_f(f0), &rotations(obs, &probe, orientation, dt0)), coarse.0, coarse.1, coarse.2);
    if !f0.is_finite() || f0 <= 0.0 { return None; }

    // Then the fit, in stages that free more of the lens: the focal lengths, then the first two coefficients the model
    // fits, then all of them. Each with rounds that leave out the observations far off the rest
    let free_k: Vec<usize> = model.free().iter().enumerate().filter(|x| *x.1).map(|x| x.0 + 4).collect();
    let mask = |k: &[usize]| -> Vec<bool> { (0..base.len()).map(|i| i < 2 || k.contains(&i)).collect() };
    let stages: Vec<Vec<bool>> = [0, 2.min(free_k.len()), free_k.len()].into_iter()
        .collect::<std::collections::BTreeSet<usize>>().into_iter()
        .map(|n| mask(&free_k[..n])).collect();

    let mut fit = Fit { intr: with_f(f0), dt: dt0 };
    let mut inliers = all.clone();
    let mut best: Option<(Fit, Vec<usize>, f64, usize)> = None; // fit, inliers, median error, stage
    for (stage, free) in stages.iter().enumerate() {
        let mut trial = fit.clone();
        let mut kept = inliers.clone();
        let mut med = f64::NAN;
        for round in 0..4 {
            refine(&lens, obs, &kept, orientation, &mut trial, free, true, noise, 40);
            let e = errors(&lens, obs, &all, orientation, &trial);
            let kept_errors: Vec<f64> = all.iter().zip(&e).filter(|(i, _)| kept.binary_search(i).is_ok()).filter_map(|(_, e)| *e).collect();
            med = median(kept_errors);
            let limit = (3.0 * med).max(2.0 * noise);
            let next: Vec<usize> = all.iter().zip(&e).filter(|(_, e)| e.is_some_and(|e| e < limit)).map(|(i, _)| *i).collect();
            ::log::debug!("Scene calibration: stage {stage}, round {round}: f = ({:.1}, {:.1}), k = {:?}, dt {:+.2} ms, median {med:.3} px; {} of {} under {limit:.2} px",
                trial.intr[0], trial.intr[1], &trial.intr[4..], trial.dt, next.len(), all.len());
            if next.len() < MIN_OBSERVATIONS { return best.and_then(|b| finish(&lens, obs, orientation, b.0, b.1, b.2, noise, size)); }
            let settled = next == kept;
            kept = next;
            if settled { break; }
        }
        let max_angle = max_angle(&lens, obs, &kept, &trial);
        let folds = lens.folds(&trial.intr, max_angle * 1.02);
        // More coefficients only where they explain the features clearly better, and still describe a lens
        let better = best.as_ref().map_or(true, |b| med < b.2 * 0.97);
        ::log::info!("Scene calibration: stage {stage} ({} coefficients): f = ({:.1}, {:.1}), k = {:?}, dt {:+.2} ms, median {med:.3} px over {} inliers{}",
            free.iter().skip(4).filter(|x| **x).count(), trial.intr[0], trial.intr[1], &trial.intr[4..], trial.dt, kept.len(),
            if folds { ", but the curve folds inside the field: not used" } else if !better { ", no better: not used" } else { "" });
        if folds || !better || !med.is_finite() { continue; }
        fit = trial.clone();
        inliers = kept.clone();
        best = Some((trial, kept, med, stage));
    }
    let (fit, kept, med, _) = best?;
    let result = finish(&lens, obs, orientation, fit, kept, med, noise, size);
    if let Some(c) = &result {
        ::log::info!("Scene calibration in {:.1}s: f = ({:.2} ± {:.2}, {:.2} ± {:.2}) (fx/fy {:.4}), k = {:?}, time offset {:+.2} ms, rms {:.3} px, median {:.3} px, {} of {} observations, up to {:.1}° from the axis, reaching {:.0}% / {:.0}% of the half width / height",
            started.elapsed().as_secs_f64(), c.f.0, c.f_sigma.0, c.f.1, c.f_sigma.1, c.f.0 / c.f.1, &c.k[..c.fitted], c.time_offset_ms, c.rms, c.median, c.inliers, c.observations,
            c.max_angle.to_degrees(), c.reach.0 * 100.0, c.reach.1 * 100.0);
    }
    result
}

/// The largest angle from the axis the observations `idx` are seen at
fn max_angle<M: Model + ?Sized>(lens: &Lens<M>, obs: &[SceneObservation], idx: &[usize], fit: &Fit) -> f64 {
    idx.par_iter().filter_map(|&i| lens.unproject(&fit.intr, obs[i].a)).map(|r| r.z.clamp(-1.0, 1.0).acos()).reduce(|| 0.0, f64::max)
}

#[allow(clippy::too_many_arguments)]
fn finish<M: Model + ?Sized>(lens: &Lens<M>, obs: &[SceneObservation], orientation: &Orientation, fit: Fit, kept: Vec<usize>, med: f64, noise: f64, size: (usize, usize)) -> Option<SceneCalibration> {
    let e: Vec<f64> = errors(lens, obs, &kept, orientation, &fit).into_iter().flatten().collect();
    if e.is_empty() { return None; }
    let rms = (e.iter().map(|x| x * x).sum::<f64>() / e.len() as f64).sqrt();
    // fx and fy's uncertainty from the robust normal equations of the focal lengths and the time offset
    let rel = rotations(obs, &kept, orientation, fit.dt);
    let around = (rotations(obs, &kept, orientation, fit.dt + 0.05), rotations(obs, &kept, orientation, fit.dt - 0.05));
    let (jtj, _, wr) = linearize(lens, obs, &kept, &fit, &[0, 1], &rel, Some((&around.0, &around.1, 0.05)), noise);
    let s2 = wr / (2.0 * kept.len() as f64 - 3.0).max(1.0);
    let f_sigma = jtj.try_inverse().map_or((f64::NAN, f64::NAN), |c| ((c[(0, 0)] * s2).sqrt(), (c[(1, 1)] * s2).sqrt()));
    let (cx, cy) = (fit.intr[2], fit.intr[3]);
    let reach = kept.iter().fold((0.0f64, 0.0f64), |r, &i| {
        let (a, b) = (obs[i].a, obs[i].b);
        (r.0.max((a.0 - cx).abs()).max((b.0 - cx).abs()), r.1.max((a.1 - cy).abs()).max((b.1 - cy).abs()))
    });
    Some(SceneCalibration {
        f: (fit.intr[0], fit.intr[1]),
        c: (cx, cy),
        k: fit.intr[4..].to_vec(),
        fitted: lens.model.free().iter().rposition(|f| *f).map_or(0, |i| i + 1),
        rms,
        median: med,
        inliers: kept.len(),
        observations: obs.len(),
        time_offset_ms: fit.dt,
        f_sigma,
        max_angle: max_angle(lens, obs, &kept, &fit),
        reach: (reach.0 / (size.0 as f64 * 0.5), reach.1 / (size.1 as f64 * 0.5)),
    })
}

/// Features of one frame followed into a later one, in pixels of the frames they were tracked in
#[derive(Clone, Debug, Default)]
pub struct TrackedPair {
    pub frames: (usize, usize),
    pub timestamps_us: (i64, i64),
    pub a: Vec<(f32, f32)>,
    pub b: Vec<(f32, f32)>,
}

/// Follows corners from each frame into the one `step` after it, as the frames of the video come in order
pub struct SceneTracker {
    klt: PairKlt,
    size: (u32, u32),
    step: usize,
    recent: VecDeque<(usize, i64, Vec<u8>, Vec<[f32; 2]>)>, // frame, timestamp, pixels, corners
    pub pairs: Vec<TrackedPair>,
}

impl SceneTracker {
    pub const MAX_CORNERS: usize = 400;
    /// How far a point tracked forward and back may end up from where it started, px of the tracked frame
    const FB_MAX: f32 = 0.5;

    /// Frames of `size` (gray, rows `size.0` apart) of a video at `fps`: each is paired with the one a 15th of a second
    /// later, so that the camera turns enough between the two to measure the lens by
    pub fn new(device: KltDevice, size: (u32, u32), fps: f64) -> Self {
        let step = (fps / 15.0).round().max(1.0) as usize;
        Self { klt: PairKlt::new(device, Self::MAX_CORNERS), size, step, recent: VecDeque::new(), pairs: Vec::new() }
    }

    pub fn feed(&mut self, frame: usize, timestamp_us: i64, pixels: &[u8]) {
        let (w, h) = self.size;
        if pixels.len() < (w * h) as usize { return; }
        // Spread over the frame, so its sides are measured as well as its middle
        let min_distance = ((w as f64 * h as f64) / Self::MAX_CORNERS as f64).sqrt() * 0.5;
        let corners = self.klt.corners(timestamp_us, pixels, (w, h), w as usize, Self::MAX_CORNERS, 0.01, min_distance)
            .unwrap_or_else(|e| { ::log::debug!("Scene calibration: no corners in frame {frame}: {e}"); Vec::new() });
        if let Some((pf, pts, ppx, pc)) = self.recent.iter().find(|x| x.0 + self.step == frame) {
            if !pc.is_empty() {
                match self.klt.track((*pts, ppx), (timestamp_us, pixels), (w, h), w as usize, pc, Self::FB_MAX) {
                    Ok(moved) => {
                        let inside = |p: [f32; 2]| p[0] >= 0.0 && p[0] < w as f32 && p[1] >= 0.0 && p[1] < h as f32;
                        let (a, b): (Vec<(f32, f32)>, Vec<(f32, f32)>) = pc.iter().zip(moved)
                            .filter_map(|(&a, b)| b.filter(|&b| inside(a) && inside(b)).map(|b| ((a[0], a[1]), (b[0], b[1]))))
                            .unzip();
                        if a.len() >= 10 {
                            self.pairs.push(TrackedPair { frames: (*pf, frame), timestamps_us: (*pts, timestamp_us), a, b });
                        }
                    },
                    Err(e) => ::log::debug!("Scene calibration: nothing tracked from frame {pf} to {frame}: {e}"),
                }
            }
        }
        self.recent.push_back((frame, timestamp_us, pixels[..(w * h) as usize].to_vec(), corners));
        while self.recent.len() > self.step { self.recent.pop_front(); }
    }
}

/// The tracked pairs as observations: in the calibration frame's pixels, with what the render undoes before the lens
/// (`points_on_sensor`: the stretch, in-body stabilization's shifts), and each point's readout time
pub fn scene_observations(params: &ComputeParams, pairs: &[TrackedPair], points_dims: (u32, u32)) -> Vec<SceneObservation> {
    let side = |pts: &[(f32, f32)], frame: usize, timestamp_us: i64| -> (Vec<(f32, f32)>, Vec<f64>) {
        let ts = timestamp_us as f64 / 1000.0;
        let on_sensor = crate::stabilization::points_on_sensor(pts, ts, frame, params, points_dims);
        let gyro = params.gyro.read();
        let md = gyro.file_metadata.read();
        let readout = FrameTransform::get_frame_readout_time(params, false, ts, &md);
        let t0 = ts + md.frame_time_offset(frame);
        let horizontal = params.frame_readout_direction.is_horizontal();
        let times = pts.iter().map(|p| {
            let pos = if horizontal { p.0 as f64 / points_dims.0 as f64 } else { p.1 as f64 / points_dims.1 as f64 };
            t0 + readout * (pos - 0.5)
        }).collect();
        (on_sensor, times)
    };
    pairs.par_iter().flat_map_iter(|p| {
        let (a, ta) = side(&p.a, p.frames.0, p.timestamps_us.0);
        let (b, tb) = side(&p.b, p.frames.1, p.timestamps_us.1);
        let finite = |p: (f32, f32)| p.0.is_finite() && p.1.is_finite();
        (0..a.len()).filter(|&i| finite(a[i]) && finite(b[i])).map(|i| SceneObservation {
            frames: p.frames,
            times_ms: (ta[i], tb[i]),
            a: (a[i].0 as f64, a[i].1 as f64),
            b: (b[i].0 as f64, b[i].1 as f64),
        }).collect::<Vec<_>>()
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stabilization::distortion_models::OpenCVFisheye;

    /// A camera turning on a smooth path, its lens anamorphic-like (fx ≠ fy) and distorted: features at random places
    /// carried by the true lens and rotation, with up to ±0.85 px of noise and a fifth of them wrong, the motion data
    /// 12 ms off the clip's clock. The fit has to find the lens and the offset
    #[test]
    fn recovers_a_lens_from_rotation() {
        let truth = [7600.0, 7950.0, 4992.0, 1664.0, -0.18, -0.12, 0.0, 0.0];
        let size = (9984usize, 3328usize);
        let model = OpenCVFisheye::default();
        let lens = Lens { model: &model, start: Start::Equidistant };
        let true_dt = 12.0;
        // Orientation of the motion data's time (ms): a wobble about every axis
        let path = |t: f64| -> Quat64 {
            let t = t / 1000.0;
            Quat64::from_euler_angles(0.05 * (1.3 * t).sin() + 0.01 * (9.0 * t).sin(), 0.08 * (0.9 * t).sin() + 0.01 * (7.0 * t).cos(), 0.03 * (1.7 * t).sin())
        };
        // The video's clock is `true_dt` behind the motion data's
        let orientation = |_frame: usize, t: f64| path(t - true_dt);
        let mut seed = 12345u64;
        let mut rnd = || { seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); (seed >> 11) as f64 / (1u64 << 53) as f64 };
        let mut obs = Vec::new();
        for frame in 0..400usize {
            let ta = frame as f64 * 1000.0 / 30.0;
            let tb = ta + 2000.0 / 30.0;
            let o0 = SceneObservation { frames: (frame, frame + 2), times_ms: (ta, tb), a: (0.0, 0.0), b: (0.0, 0.0) };
            let rel = relative(&|f, t| orientation(f, t + true_dt), &o0, 0.0);
            for _ in 0..60 {
                let a = (rnd() * size.0 as f64, rnd() * size.1 as f64);
                let Some(ray) = lens.unproject(&truth, a) else { continue };
                let Some(b) = lens.project(&truth, &(rel * ray)) else { continue };
                let wrong = rnd() < 0.2;
                let b = if wrong { (b.0 + (rnd() - 0.5) * 200.0, b.1 + (rnd() - 0.5) * 200.0) } else { (b.0 + (rnd() - 0.5) * 1.7, b.1 + (rnd() - 0.5) * 1.7) };
                if b.0 < 0.0 || b.1 < 0.0 || b.0 >= size.0 as f64 || b.1 >= size.1 as f64 { continue; }
                obs.push(SceneObservation { a, b, ..o0 });
            }
        }
        let c = calibrate(&model, &obs, size, &orientation, 1.0).expect("a fit");
        println!("{c:?}");
        assert!((c.f.0 - truth[0]).abs() < 15.0, "fx {}", c.f.0);
        assert!((c.f.1 - truth[1]).abs() < 15.0, "fy {}", c.f.1);
        assert!((c.k[0] - truth[4]).abs() < 0.01, "k1 {}", c.k[0]);
        assert!((c.time_offset_ms - true_dt).abs() < 0.5, "dt {}", c.time_offset_ms);
    }
}
