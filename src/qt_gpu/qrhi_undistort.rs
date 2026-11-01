// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2021-2022 Adrian <adrian.eddy at gmail>

use gyroflow_core::{ stabilization::ProcessedInfo, gpu::Buffers };
use qml_video_rs::video_player::MDKPlayerWrapper;
use std::sync::Arc;
use crate::core::StabilizationManager;
use crate::core::stabilization::RGBA8;
use cpp::*;
use qmetaobject::{ QSize, QString };
use crate::calibration_lut::GpuLut;

cpp! {{
    #include "src/qt_gpu/qrhi_undistort.cpp"
}}

pub fn render(mdkplayer: &MDKPlayerWrapper, timestamp: f64, frame: usize, width: u32, height: u32, stab: Arc<StabilizationManager>, buffers: &mut Buffers, lut_revision: u64, lut: Option<&GpuLut>) -> Option<ProcessedInfo> {
    if stab.prevent_recompute.load(std::sync::atomic::Ordering::SeqCst) { return None; }

    let mut timestamp_us = (timestamp * 1000.0).round() as i64;
    let mut output_size = QSize::default();
    let mut shader_path = QString::default();
    let mut lut_uniforms = lut.map(|x| x.uniforms).unwrap_or([0.0; 12]);
    let lut_edge = lut.map(|x| x.edge).unwrap_or(0);
    let lut_pixels = lut.map(|x| x.rgba.as_ptr()).unwrap_or(std::ptr::null());

    if let Some(p) = stab.params.try_read() {
        output_size = QSize { width: p.output_size.0 as u32, height: p.output_size.1 as u32 };
        shader_path = {
            let lens = stab.lens.read();
            let distortion_model = lens.distortion_model.as_deref().unwrap_or("opencv_fisheye");
            let digital_lens = lens.digital_lens.as_ref().map(|x| format!("_{}", x)).unwrap_or_else(|| "".into());

            let suffix = if lut.is_some() { "_lut" } else { "" };
            QString::from(format!(":/src/qt_gpu/compiled/undistort_{}{}{}.frag.qsb", distortion_model, digital_lens, suffix))
        };
        lut_uniforms[8] = if p.stab_enabled { 0.0 } else { 1.0 };

        if let Some(scale) = p.fps_scale {
            timestamp_us = (timestamp_us as f64 / scale).round() as i64;
        }
    }
    let lut_uniforms_ptr = lut_uniforms.as_ptr();

    // What the overlays take a while to place, placed with the stabilization not locked
    stab.prepare_overlays(timestamp_us);
    if let Some(mut undist) = stab.stabilization.try_write() {
        undist.ensure_stab_data_at_timestamp::<RGBA8>(timestamp_us, Some(frame), buffers, true);
        stab.draw_overlays(&mut undist, timestamp_us);
    }

    if let Some(undist) = stab.stabilization.try_read() {
        if let Some(itm) = undist.get_undistortion_data(timestamp_us) {
            let params = bytemuck::bytes_of(&itm.kernel_params);
            let params_ptr = params.as_ptr();
            let params_len = params.len() as u32;
            let matrices_ptr = itm.matrices.as_ptr();
            let matrices_len = (itm.matrices.len() * 16 * std::mem::size_of::<f32>()) as u32;
            let canvas = undist.drawing.get_buffer();
            let canvas_ptr = canvas.as_ptr();
            let canvas_len = canvas.len() as u32;
            let mesh_data_ptr = itm.mesh_data.as_ptr();
            let mesh_data_len = itm.mesh_data.len() as u32;

            let size_for_rs = if (itm.kernel_params.flags & 16) == 16 { itm.kernel_params.width } else { itm.kernel_params.height } as u32;

            let canvas_size = undist.drawing.get_size();
            let canvas_size = QSize { width: canvas_size.0 as u32, height: canvas_size.1 as u32 };

            let ok = cpp!(unsafe [mdkplayer as "MDKPlayerWrapper *", output_size as "QSize", shader_path as "QString", width as "uint32_t", height as "uint32_t", params_ptr as "uint8_t*", matrices_ptr as "uint8_t*", canvas_ptr as "uint8_t*", mesh_data_ptr as "float*", mesh_data_len as "uint32_t", matrices_len as "uint32_t", params_len as "uint32_t", canvas_len as "uint32_t", canvas_size as "QSize", size_for_rs as "uint32_t", lut_edge as "uint32_t", lut_revision as "uint64_t", lut_pixels as "const float*", lut_uniforms_ptr as "const float*"] -> bool as "bool" {
                if (!mdkplayer || !mdkplayer->mdkplayer || shader_path.isEmpty() || output_size.isEmpty()) return false;

                auto rhiUndistortion = static_cast<QtRHIUndistort *>(mdkplayer->mdkplayer->userData());

                if (!QFile::exists(shader_path)) {
                    qDebug2("render") << shader_path << "doesn't exist";
                    delete rhiUndistortion;
                    mdkplayer->mdkplayer->setUserData(nullptr);
                    return false;
                }
                if (output_size.width() < 4 || output_size.height() < 4) {
                    delete rhiUndistortion;
                    mdkplayer->mdkplayer->setUserData(nullptr);
                    return true;
                }

                if (!rhiUndistortion
                || rhiUndistortion->outSize() != output_size
                || rhiUndistortion->texSize() != QSize(width, height)
                || rhiUndistortion->shaderPath() != shader_path
                || rhiUndistortion->sizeForRS() != size_for_rs
                || rhiUndistortion->lutEdge() != lut_edge
                || rhiUndistortion->itemTexturePtr() != mdkplayer->mdkplayer->rhiTexture()) {
                    delete rhiUndistortion;
                    rhiUndistortion = new QtRHIUndistort();
                    if (!rhiUndistortion->init(mdkplayer->mdkplayer, QSize(width, height), output_size, shader_path, params_len, size_for_rs, canvas_size, lut_edge)) {
                        qDebug2("render") << "Failed to initialize";
                        delete rhiUndistortion;
                        mdkplayer->mdkplayer->setUserData(nullptr);
                        return false;
                    }
                    qDebug2("render") << "Initialized" << QSize(width, height) << "->" << output_size << shader_path << rhiUndistortion;
                    mdkplayer->mdkplayer->setUserData(static_cast<void *>(rhiUndistortion));
                    mdkplayer->mdkplayer->setUserDataDestructor([](void *ptr) {
                        delete static_cast<QtRHIUndistort *>(ptr);
                    });
                }

                return rhiUndistortion->render(mdkplayer->mdkplayer, params_ptr, params_len, matrices_ptr, matrices_len, canvas_ptr, canvas_len, mesh_data_ptr, mesh_data_len, lut_revision, lut_pixels, lut_uniforms_ptr);
            });
            if ok {
                return Some(ProcessedInfo {
                    fov: itm.fov,
                    minimal_fov: itm.minimal_fov,
                    focal_length: itm.focal_length,
                    backend: "Qt RHI"
                });
            }
        }
    }
    None
}
