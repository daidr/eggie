use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub(crate) enum Stage {
    PtyRead,
    QueueWait,
    Parser,
    Vte,
    Kitty,
    Base64,
    DecodeWait,
    Zlib,
    ImageProbe,
    ImageDecode,
    Commit,
}

#[derive(Default)]
struct StageStats {
    count: u64,
    total_ns: u64,
    max_ns: u64,
}

impl StageStats {
    fn record(&mut self, duration: Duration) {
        let nanos = duration.as_nanos().min(u128::from(u64::MAX)) as u64;
        self.count = self.count.saturating_add(1);
        self.total_ns = self.total_ns.saturating_add(nanos);
        self.max_ns = self.max_ns.max(nanos);
    }

    fn summary(&self) -> String {
        if self.count == 0 {
            return "-".to_owned();
        }
        format!(
            "{:.3}/{:.3}ms×{}",
            self.total_ns as f64 / self.count as f64 / 1_000_000.,
            self.max_ns as f64 / 1_000_000.,
            self.count,
        )
    }
}

struct Metrics {
    report_at: Instant,
    pty_bytes: u64,
    pending_peak: usize,
    stages: [StageStats; 11],
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            report_at: Instant::now(),
            pty_bytes: 0,
            pending_peak: 0,
            stages: std::array::from_fn(|_| StageStats::default()),
        }
    }
}

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("EGGIE_RENDER_METRICS")
            .is_some_and(|value| !value.is_empty() && value != "0")
    })
}

fn metrics() -> &'static Mutex<Metrics> {
    static METRICS: OnceLock<Mutex<Metrics>> = OnceLock::new();
    METRICS.get_or_init(|| Mutex::new(Metrics::default()))
}

fn stage_index(stage: Stage) -> usize {
    match stage {
        Stage::PtyRead => 0,
        Stage::QueueWait => 1,
        Stage::Parser => 2,
        Stage::Vte => 3,
        Stage::Kitty => 4,
        Stage::Base64 => 5,
        Stage::DecodeWait => 6,
        Stage::Zlib => 7,
        Stage::ImageProbe => 8,
        Stage::ImageDecode => 9,
        Stage::Commit => 10,
    }
}

pub(crate) fn record(stage: Stage, duration: Duration) {
    if !enabled() {
        return;
    }
    metrics().lock().expect("pipeline metrics poisoned").stages[stage_index(stage)]
        .record(duration);
}

pub(crate) fn record_pty_bytes(bytes: usize) {
    if enabled() {
        let mut metrics = metrics().lock().expect("pipeline metrics poisoned");
        metrics.pty_bytes = metrics.pty_bytes.saturating_add(bytes as u64);
    }
}

pub(crate) fn record_pending(pending: usize) {
    if enabled() {
        let mut metrics = metrics().lock().expect("pipeline metrics poisoned");
        metrics.pending_peak = metrics.pending_peak.max(pending);
    }
}

/// Report one global interval for every active terminal pipeline.
pub(crate) fn report_if_due() {
    if !enabled() {
        return;
    }
    let now = Instant::now();
    let mut metrics = metrics().lock().expect("pipeline metrics poisoned");
    let elapsed = now.saturating_duration_since(metrics.report_at);
    if elapsed < Duration::from_secs(1) {
        return;
    }
    let previous = std::mem::replace(
        &mut *metrics,
        Metrics {
            report_at: now,
            ..Metrics::default()
        },
    );
    drop(metrics);
    let mib_per_second = previous.pty_bytes as f64 / 1024. / 1024. / elapsed.as_secs_f64();
    let stage = |stage| previous.stages[stage_index(stage)].summary();
    eprintln!(
        "[eggie-terminal-pipeline] pty={mib_per_second:.1}MiB/s read={} queue={} parser={} vte={} kitty={} base64={} decode-wait={} zlib={} image-probe={} image={} commit={} pending-peak={}",
        stage(Stage::PtyRead),
        stage(Stage::QueueWait),
        stage(Stage::Parser),
        stage(Stage::Vte),
        stage(Stage::Kitty),
        stage(Stage::Base64),
        stage(Stage::DecodeWait),
        stage(Stage::Zlib),
        stage(Stage::ImageProbe),
        stage(Stage::ImageDecode),
        stage(Stage::Commit),
        previous.pending_peak,
    );
}
