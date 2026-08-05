use eggie_domain::SessionId;
use gpui::InputLatencySnapshot;
use parking_lot::Mutex;
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

const SAMPLE_WINDOW: usize = 256;
const REPORT_EVERY: u64 = 16;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct MetalFrameStats {
    pub(crate) command_count: usize,
    pub(crate) glyph_count: usize,
    pub(crate) atlas_hits: usize,
    pub(crate) atlas_misses: usize,
    pub(crate) atlas_resets: usize,
    pub(crate) raster_time: Duration,
    pub(crate) background_raster_jobs: usize,
    pub(crate) background_raster_time: Duration,
    pub(crate) synchronous_raster_jobs: usize,
    pub(crate) synchronous_raster_time: Duration,
    pub(crate) pending_rasters: usize,
    pub(crate) image_uploads: usize,
    pub(crate) image_upload_bytes: usize,
    pub(crate) image_upload_time: Duration,
    pub(crate) draw_calls: usize,
}

#[derive(Clone, Default)]
pub(crate) struct InputLatencyTracker {
    state: Option<Arc<Mutex<InputLatencyState>>>,
}

struct PendingInput {
    sequence: u64,
    input_at: Instant,
}

struct ReadyInput {
    sequence: u64,
    input_at: Instant,
    snapshot_at: Instant,
}

#[derive(Clone, Copy)]
struct LatencySample {
    input_to_snapshot: Duration,
    snapshot_to_metal: Duration,
    end_to_end: Duration,
}

struct InputLatencyState {
    input_enabled: bool,
    render_enabled: bool,
    pending: HashMap<SessionId, VecDeque<PendingInput>>,
    ready: HashMap<SessionId, VecDeque<ReadyInput>>,
    samples: VecDeque<LatencySample>,
    prepare_samples: VecDeque<Duration>,
    metal_samples: VecDeque<Duration>,
    total_samples: u64,
    reported_samples: u64,
    render_report_at: Instant,
    render: RenderInterval,
    last_snapshot_revision: HashMap<SessionId, u64>,
}

#[derive(Default)]
struct RenderInterval {
    snapshots: u64,
    skipped_revisions: u64,
    preparations: u64,
    preparation_cache_hits: u64,
    frames: u64,
    commands: u64,
    glyphs: u64,
    atlas_hits: u64,
    atlas_misses: u64,
    atlas_resets: u64,
    raster_time: Duration,
    background_raster_jobs: u64,
    background_raster_time: Duration,
    synchronous_raster_jobs: u64,
    synchronous_raster_time: Duration,
    pending_rasters_peak: usize,
    image_uploads: u64,
    image_upload_bytes: u64,
    image_upload_time: Duration,
    draw_calls: u64,
}

impl InputLatencyState {
    fn new(input_enabled: bool, render_enabled: bool) -> Self {
        Self {
            input_enabled,
            render_enabled,
            pending: HashMap::default(),
            ready: HashMap::default(),
            samples: VecDeque::default(),
            prepare_samples: VecDeque::default(),
            metal_samples: VecDeque::default(),
            total_samples: 0,
            reported_samples: 0,
            render_report_at: Instant::now(),
            render: RenderInterval::default(),
            last_snapshot_revision: HashMap::default(),
        }
    }
}

impl InputLatencyTracker {
    pub(crate) fn from_environment() -> Self {
        let input_enabled = std::env::var_os("EGGIE_INPUT_LATENCY")
            .is_some_and(|value| !value.is_empty() && value != "0");
        let render_enabled = std::env::var_os("EGGIE_RENDER_METRICS")
            .is_some_and(|value| !value.is_empty() && value != "0");
        if input_enabled {
            eprintln!(
                "[eggie-input-latency] enabled; reporting rolling p50/p95 every {REPORT_EVERY} samples"
            );
        }
        if render_enabled {
            eprintln!(
                "[eggie-render-performance] enabled; reporting snapshot, preparation, Metal, atlas, and image-upload metrics every second"
            );
        }
        Self {
            state: (input_enabled || render_enabled).then(|| {
                Arc::new(Mutex::new(InputLatencyState::new(
                    input_enabled,
                    render_enabled,
                )))
            }),
        }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.state.is_some()
    }

    pub(crate) fn record_input(&self, session_id: SessionId, sequence: u64) {
        let Some(state) = &self.state else {
            return;
        };
        let mut state = state.lock();
        if !state.input_enabled {
            return;
        }
        state
            .pending
            .entry(session_id)
            .or_default()
            .push_back(PendingInput {
                sequence,
                input_at: Instant::now(),
            });
    }

    pub(crate) fn discard_input(&self, session_id: SessionId, sequence: u64) {
        let Some(state) = &self.state else {
            return;
        };
        let mut state = state.lock();
        if !state.input_enabled {
            return;
        }
        if let Some(pending) = state.pending.get_mut(&session_id) {
            pending.retain(|input| input.sequence != sequence);
        }
    }

    pub(crate) fn record_snapshot(
        &self,
        session_id: SessionId,
        last_input_sequence: u64,
        revision: u64,
    ) {
        let Some(state) = &self.state else {
            return;
        };
        let snapshot_at = Instant::now();
        let mut state = state.lock();
        if state.render_enabled {
            state.render.snapshots += 1;
            if let Some(previous) = state.last_snapshot_revision.insert(session_id, revision) {
                state.render.skipped_revisions +=
                    revision.saturating_sub(previous).saturating_sub(1);
            }
        }
        if !state.input_enabled {
            return;
        }
        let Some(mut pending) = state.pending.remove(&session_id) else {
            return;
        };
        let mut acknowledged = VecDeque::new();
        while pending
            .front()
            .is_some_and(|input| input.sequence <= last_input_sequence)
        {
            let input = pending.pop_front().expect("pending input was checked");
            acknowledged.push_back(ReadyInput {
                sequence: input.sequence,
                input_at: input.input_at,
                snapshot_at,
            });
        }
        if !pending.is_empty() {
            state.pending.insert(session_id, pending);
        }
        if !acknowledged.is_empty() {
            state
                .ready
                .entry(session_id)
                .or_default()
                .append(&mut acknowledged);
        }
    }

    pub(crate) fn record_prepare(&self, elapsed: Duration, cache_hit: bool) {
        let Some(state) = &self.state else {
            return;
        };
        let mut state = state.lock();
        push_sample(&mut state.prepare_samples, elapsed);
        if state.render_enabled {
            state.render.preparations += 1;
            state.render.preparation_cache_hits += u64::from(cache_hit);
        }
    }

    pub(crate) fn record_metal(
        &self,
        session_id: SessionId,
        last_input_sequence: u64,
        elapsed: Duration,
        stats: MetalFrameStats,
    ) {
        let Some(state) = &self.state else {
            return;
        };
        let metal_at = Instant::now();
        let mut state = state.lock();
        push_sample(&mut state.metal_samples, elapsed);
        if state.render_enabled {
            state.render.frames += 1;
            state.render.commands += stats.command_count as u64;
            state.render.glyphs += stats.glyph_count as u64;
            state.render.atlas_hits += stats.atlas_hits as u64;
            state.render.atlas_misses += stats.atlas_misses as u64;
            state.render.atlas_resets += stats.atlas_resets as u64;
            state.render.raster_time += stats.raster_time;
            state.render.background_raster_jobs += stats.background_raster_jobs as u64;
            state.render.background_raster_time += stats.background_raster_time;
            state.render.synchronous_raster_jobs += stats.synchronous_raster_jobs as u64;
            state.render.synchronous_raster_time += stats.synchronous_raster_time;
            state.render.pending_rasters_peak =
                state.render.pending_rasters_peak.max(stats.pending_rasters);
            state.render.image_uploads += stats.image_uploads as u64;
            state.render.image_upload_bytes += stats.image_upload_bytes as u64;
            state.render.image_upload_time += stats.image_upload_time;
            state.render.draw_calls += stats.draw_calls as u64;
        }
        if !state.input_enabled {
            return;
        }
        let Some(mut ready) = state.ready.remove(&session_id) else {
            return;
        };
        while ready
            .front()
            .is_some_and(|input| input.sequence <= last_input_sequence)
        {
            let input = ready.pop_front().expect("ready input was checked");
            state.samples.push_back(LatencySample {
                input_to_snapshot: input.snapshot_at.duration_since(input.input_at),
                snapshot_to_metal: metal_at.duration_since(input.snapshot_at),
                end_to_end: metal_at.duration_since(input.input_at),
            });
            state.total_samples += 1;
            if state.samples.len() > SAMPLE_WINDOW {
                state.samples.pop_front();
            }
        }
        if !ready.is_empty() {
            state.ready.insert(session_id, ready);
        }
    }

    pub(crate) fn report_if_due(&self, presented: &InputLatencySnapshot) {
        let Some(state) = &self.state else {
            return;
        };
        let mut state = state.lock();
        if state.render_enabled {
            report_render(&mut state, presented);
        }
        if state.input_enabled && state.total_samples >= state.reported_samples + REPORT_EVERY {
            state.reported_samples = state.total_samples;
            report_input(&state, presented);
        }
    }
}

fn push_sample(samples: &mut VecDeque<Duration>, sample: Duration) {
    samples.push_back(sample);
    if samples.len() > SAMPLE_WINDOW {
        samples.pop_front();
    }
}

fn report_input(state: &InputLatencyState, presented: &InputLatencySnapshot) {
    let input_to_snapshot = state
        .samples
        .iter()
        .map(|sample| sample.input_to_snapshot)
        .collect::<Vec<_>>();
    let snapshot_to_metal = state
        .samples
        .iter()
        .map(|sample| sample.snapshot_to_metal)
        .collect::<Vec<_>>();
    let end_to_end = state
        .samples
        .iter()
        .map(|sample| sample.end_to_end)
        .collect::<Vec<_>>();
    let prepare = state.prepare_samples.iter().copied().collect::<Vec<_>>();
    let metal = state.metal_samples.iter().copied().collect::<Vec<_>>();
    let presented_p50 = presented.latency_histogram.value_at_quantile(0.50) as f64 / 1_000_000.;
    let presented_p95 = presented.latency_histogram.value_at_quantile(0.95) as f64 / 1_000_000.;
    let coalesced_p50 = presented.events_per_frame_histogram.value_at_quantile(0.50);
    let coalesced_p95 = presented.events_per_frame_histogram.value_at_quantile(0.95);
    eprintln!(
        "[eggie-input-latency] samples={} input→snapshot p50={:.2}ms p95={:.2}ms; snapshot→Metal p50={:.2}ms p95={:.2}ms; prepare p50={:.2}ms p95={:.2}ms; Metal encode p50={:.2}ms p95={:.2}ms; end-to-Metal p50={:.2}ms p95={:.2}ms; GPUI input→present p50={presented_p50:.2}ms p95={presented_p95:.2}ms; events/frame p50={coalesced_p50} p95={coalesced_p95}; mid-draw dropped={}",
        state.samples.len(),
        millis(percentile(&input_to_snapshot, 50)),
        millis(percentile(&input_to_snapshot, 95)),
        millis(percentile(&snapshot_to_metal, 50)),
        millis(percentile(&snapshot_to_metal, 95)),
        millis(percentile(&prepare, 50)),
        millis(percentile(&prepare, 95)),
        millis(percentile(&metal, 50)),
        millis(percentile(&metal, 95)),
        millis(percentile(&end_to_end, 50)),
        millis(percentile(&end_to_end, 95)),
        presented.mid_draw_events_dropped,
    );
}

fn report_render(state: &mut InputLatencyState, presented: &InputLatencySnapshot) {
    let elapsed = state.render_report_at.elapsed();
    if elapsed < Duration::from_millis(800) {
        return;
    }
    let seconds = elapsed.as_secs_f64().max(f64::EPSILON);
    let render = std::mem::take(&mut state.render);
    state.render_report_at = Instant::now();
    if render.snapshots == 0 && render.preparations == 0 && render.frames == 0 {
        return;
    }
    let prepare = state.prepare_samples.iter().copied().collect::<Vec<_>>();
    let metal = state.metal_samples.iter().copied().collect::<Vec<_>>();
    let frames = render.frames.max(1);
    let preparations = render.preparations.max(1);
    let atlas_total = render.atlas_hits + render.atlas_misses;
    let cache_hit_percent = render.preparation_cache_hits as f64 * 100. / preparations as f64;
    let atlas_hit_percent = render.atlas_hits as f64 * 100. / atlas_total.max(1) as f64;
    eprintln!(
        "[eggie-render-performance] snapshots={:.1}/s frames={:.1}/s skipped-revisions={} prepare p50={:.2}ms p95={:.2}ms cache-hit={cache_hit_percent:.1}% Metal p50={:.2}ms p95={:.2}ms commands/frame={:.0} glyphs/frame={:.0} draw-calls/frame={:.1} atlas-hit={atlas_hit_percent:.1}% misses={} resets={} paint-raster={:.2}ms async-raster={}/{:.2}ms sync-raster={}/{:.2}ms pending-raster={} image-uploads={} image-upload={:.2}MiB/{:.2}ms mid-draw-dropped={}",
        render.snapshots as f64 / seconds,
        render.frames as f64 / seconds,
        render.skipped_revisions,
        millis(percentile(&prepare, 50)),
        millis(percentile(&prepare, 95)),
        millis(percentile(&metal, 50)),
        millis(percentile(&metal, 95)),
        render.commands as f64 / frames as f64,
        render.glyphs as f64 / frames as f64,
        render.draw_calls as f64 / frames as f64,
        render.atlas_misses,
        render.atlas_resets,
        millis(render.raster_time),
        render.background_raster_jobs,
        millis(render.background_raster_time),
        render.synchronous_raster_jobs,
        millis(render.synchronous_raster_time),
        render.pending_rasters_peak,
        render.image_uploads,
        render.image_upload_bytes as f64 / (1024. * 1024.),
        millis(render.image_upload_time),
        presented.mid_draw_events_dropped,
    );
}

fn percentile(samples: &[Duration], percentile: usize) -> Duration {
    let mut samples = samples.to_vec();
    samples.sort_unstable();
    let index = samples.len().saturating_sub(1).saturating_mul(percentile) / 100;
    samples.get(index).copied().unwrap_or_default()
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_uses_the_requested_rank() {
        let samples = (1..=100).map(Duration::from_millis).collect::<Vec<_>>();
        assert_eq!(percentile(&samples, 50), Duration::from_millis(50));
        assert_eq!(percentile(&samples, 95), Duration::from_millis(95));
    }
}
