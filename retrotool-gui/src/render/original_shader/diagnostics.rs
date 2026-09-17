//! Asynchronous per-instance fault feedback. No frame waits for a readback.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use bevy::{
    prelude::*,
    render::{
        render_resource::Buffer,
        renderer::{RenderDevice, RenderQueue},
    },
};

use super::{buffer, OriginalViewDraws};

/// Last completed GPU checks for this material instance. A fault is sticky until
/// the instance is replaced, so a later clean draw cannot hide an earlier failure.
#[derive(Clone, Debug)]
pub struct Diagnostics(Arc<Mutex<State>>);

#[derive(Debug)]
struct State {
    enabled: bool,
    pipeline_ready: bool,
    completed: u64,
    failure: Option<String>,
}

impl Diagnostics {
    pub(super) fn new(enabled: bool) -> Self {
        Self(Arc::new(Mutex::new(State {
            enabled,
            pipeline_ready: false,
            completed: 0,
            failure: None,
        })))
    }

    pub fn pending(&self) -> bool {
        let state = self.0.lock().unwrap();
        (!state.pipeline_ready || (state.enabled && state.completed == 0))
            && state.failure.is_none()
    }

    pub(super) fn pipeline_ready(&self) { self.0.lock().unwrap().pipeline_ready = true; }

    pub fn failure(&self) -> Option<String> { self.0.lock().unwrap().failure.clone() }

    pub fn completed(&self) -> u64 { self.0.lock().unwrap().completed }

    pub(super) fn fail(&self, error: String) {
        self.0.lock().unwrap().failure.get_or_insert(error);
    }

    fn report(&self, words: [u32; 4]) {
        let mut state = self.0.lock().unwrap();
        state.completed += 1;
        for (stage, pair) in ["vertex", "fragment"].into_iter().zip(words.chunks_exact(2)) {
            if pair[0] != 0 {
                state.failure.get_or_insert_with(|| {
                    format!("Remax {stage} fault {:#x} at native PC {:#x}", pair[0], pair[1])
                });
            }
        }
    }
}

pub(super) struct FaultCapture {
    pub buffer: Buffer,
    dirty: AtomicBool,
    mapping: AtomicBool,
    diagnostics: Diagnostics,
}

impl FaultCapture {
    pub fn new(device: &RenderDevice, diagnostics: Diagnostics) -> Arc<Self> {
        Arc::new(Self {
            buffer: buffer(
                device,
                &[0, u32::MAX, 0, u32::MAX],
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            ),
            dirty: AtomicBool::new(false),
            mapping: AtomicBool::new(false),
            diagnostics,
        })
    }

    pub fn mark_drawn(&self) { self.dirty.store(true, Ordering::Release); }
}

#[derive(Resource, Default)]
pub(super) struct FaultReadbacks(Vec<Arc<FaultCapture>>);

/// Run after Bevy has submitted every view. Fault storage accumulates across all
/// views and frames of one GPU instance. At most one readback per instance is in
/// flight; intervening draws remain covered because GPU fault words never reset.
/// Completion owns only its old diagnostics handle, so Clear or recompilation
/// cannot receive stale faults. A reported failure invalidates that instance.
pub(super) fn read_draw_faults(
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    views: Query<&OriginalViewDraws>,
    mut readbacks: ResMut<FaultReadbacks>,
) {
    // Keep dirty captures until their final copy completes, even if their model
    // becomes hidden or is removed while another copy is in flight.
    readbacks.0.retain(|capture| {
        capture.mapping.load(Ordering::Acquire) || capture.dirty.load(Ordering::Acquire)
    });
    for draws in &views {
        for draw in draws.0.values() {
            let Some(capture) = &draw.faults else { continue };
            if capture.dirty.load(Ordering::Acquire)
                && !readbacks.0.iter().any(|known| Arc::ptr_eq(known, capture))
            {
                readbacks.0.push(capture.clone());
            }
        }
    }
    let mut captures = Vec::new();
    for capture in &readbacks.0 {
        if capture.mapping.load(Ordering::Acquire) || !capture.dirty.swap(false, Ordering::AcqRel) {
            continue;
        }
        capture.mapping.store(true, Ordering::Release);
        captures.push(capture.clone());
    }
    if captures.is_empty() {
        return;
    }
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Remax fault readbacks"),
    });
    let mut mappings = Vec::with_capacity(captures.len());
    for capture in captures {
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Remax draw fault result"),
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(&capture.buffer, 0, &staging, 0, 16);
        mappings.push((staging, capture));
    }
    queue.submit([encoder.finish()]);
    for (staging, capture) in mappings {
        let buffer = staging.clone();
        staging.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            match result {
                Ok(()) => {
                    let bytes = buffer.slice(..).get_mapped_range();
                    let words = std::array::from_fn(|index| {
                        u32::from_le_bytes(bytes[index * 4..index * 4 + 4].try_into().unwrap())
                    });
                    capture.diagnostics.report(words);
                    drop(bytes);
                    buffer.unmap();
                }
                Err(error) => {
                    capture.diagnostics.fail(format!("Remax fault readback failed: {error}"))
                }
            }
            capture.mapping.store(false, Ordering::Release);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faults_are_sticky_and_replacements_have_independent_feedback() {
        let original = Diagnostics::new(true);
        let pending_callback = original.clone();
        let replacement = Diagnostics::new(true);
        assert!(original.pending());
        original.pipeline_ready();
        original.report([0, u32::MAX, 0, u32::MAX]);
        assert!(!original.pending());
        pending_callback.report([0, u32::MAX, 8, 0x778]);
        original.report([0, u32::MAX, 0, u32::MAX]);
        assert_eq!(original.completed(), 3);
        assert!(original.failure().unwrap().contains("0x778"));
        assert!(replacement.pending());
        assert!(replacement.failure().is_none());
    }
}
