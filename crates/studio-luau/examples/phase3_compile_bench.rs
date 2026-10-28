use std::{env, fs, hint::black_box, process, time::Instant};

use studio_luau::{ScriptDocument, compile};

fn main() {
    let mut arguments = env::args().skip(1);
    let path = arguments.next().unwrap_or_else(|| usage());
    let warmups = parse_count(arguments.next(), "warmups");
    let iterations = parse_count(arguments.next(), "iterations");
    if arguments.next().is_some() || warmups == 0 || iterations < 30 {
        usage();
    }

    let source = fs::read_to_string(path).unwrap_or_else(|error| {
        eprintln!("could not read benchmark source: {error}");
        process::exit(2);
    });
    let source_line_count = source.lines().count();
    let document = ScriptDocument::new(source.as_str());

    for _ in 0..warmups {
        compile(&document).unwrap_or_else(|error| {
            eprintln!("benchmark source did not compile: {error}");
            process::exit(2);
        });
    }

    let mut samples_ms = Vec::with_capacity(iterations);
    let mut bytecode_bytes = 0;
    for _ in 0..iterations {
        let start = Instant::now();
        let compiled = compile(black_box(&document)).unwrap_or_else(|error| {
            eprintln!("benchmark source did not compile: {error}");
            process::exit(2);
        });
        samples_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        bytecode_bytes = compiled.bytecode_size();
        black_box(bytecode_bytes);
    }

    samples_ms.sort_by(f64::total_cmp);
    let p50_ms = nearest_rank(&samples_ms, 0.50);
    let p95_ms = nearest_rank(&samples_ms, 0.95);
    let peak_rss_kib = peak_rss_kib().unwrap_or(0);
    let samples_json = samples_ms
        .iter()
        .map(|sample| format!("{sample:.4}"))
        .collect::<Vec<_>>()
        .join(",");

    println!(
        "{{\"operation\":\"compile\",\"source_lines\":{source_line_count},\"source_bytes\":{},\"warmups\":{warmups},\"iterations\":{iterations},\"bytecode_bytes\":{bytecode_bytes},\"p50_ms\":{p50_ms:.4},\"p95_ms\":{p95_ms:.4},\"peak_rss_kib\":{peak_rss_kib},\"samples_ms\":[{samples_json}]}}",
        source.len()
    );
}

fn parse_count(value: Option<String>, name: &str) -> usize {
    value
        .unwrap_or_else(|| usage())
        .parse::<usize>()
        .unwrap_or_else(|_| {
            eprintln!("{name} must be a positive integer");
            process::exit(2);
        })
}

fn nearest_rank(sorted_values: &[f64], percentile: f64) -> f64 {
    let rank = (percentile * sorted_values.len() as f64).ceil() as usize;
    sorted_values[rank.saturating_sub(1)]
}

fn peak_rss_kib() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

fn usage() -> ! {
    eprintln!("usage: phase3_compile_bench <source-file> <warmups> <iterations>=30");
    process::exit(2)
}
