use eggie_completion::{
    CompletionContext, CompletionEngine, CompletionKind, PromptInput, rank, rank_indices,
};
use std::{hint::black_box, time::Instant};

fn measure(name: &str, mut run: impl FnMut()) {
    let mut samples = Vec::with_capacity(300);
    for _ in 0..300 {
        let started = Instant::now();
        run();
        samples.push(started.elapsed().as_secs_f64() * 1_000.);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "{name}: p50={:.3}ms p95={:.3}ms",
        samples[150], samples[285]
    );
}

fn main() {
    let input = PromptInput {
        text: "cat file-001".into(),
        cursor: 12,
        cwd: std::env::current_dir().unwrap(),
        search_path: std::env::var("PATH").unwrap_or_default(),
        home: std::env::var_os("HOME").unwrap_or_default().into(),
        ..Default::default()
    };
    let context = CompletionContext::parse(&input).unwrap();
    let candidates: Vec<_> = (0..2_000)
        .map(|i| {
            context.item(
                &format!("file-{i:04} with spaces.txt"),
                "A candidate with a description",
                CompletionKind::File,
            )
        })
        .collect();
    measure("rebuild + rank (previous UI path)", || {
        black_box(rank(
            candidates
                .iter()
                .map(|i| context.item(&i.label, &i.description, i.kind))
                .collect(),
            &context.query,
        ));
    });
    measure("borrowed refilter (current UI path)", || {
        black_box(rank_indices(&candidates, &context.query));
    });
    let engine = CompletionEngine::default();
    let commands = PromptInput {
        text: "gi".into(),
        cursor: 2,
        ..input
    };
    let started = Instant::now();
    let count = engine.complete(&commands, &[]).len();
    println!(
        "initial PATH query: {:.3}ms ({count} matches)",
        started.elapsed().as_secs_f64() * 1_000.
    );
    measure("cached PATH query", || {
        black_box(engine.complete(&commands, &[]));
    });
}
