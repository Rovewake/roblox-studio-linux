use std::{
    error::Error,
    fs,
    hint::black_box,
    path::PathBuf,
    process,
    sync::atomic::{AtomicU64, Ordering},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use rbx_dom_weak::types::{CFrame, Matrix3, Vector3};
use studio_project::{FileFormat, ProjectDocument};

static DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

fn main() {
    if let Err(error) = run() {
        eprintln!("format I/O benchmark failed: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut format = None;
    let mut instances = None;
    let mut iterations = 31usize;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--format" => format = args.next().and_then(|value| parse_format(&value)),
            "--instances" => instances = args.next().and_then(|value| value.parse().ok()),
            "--iterations" => {
                iterations = args
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0)
            }
            "--bench" => {}
            _ => return Err(format!("unknown argument {argument:?}").into()),
        }
    }
    let format = format.ok_or("--format must be rbxl, rbxm, rbxlx, or rbxmx")?;
    let instances: usize = instances.ok_or("--instances must be a positive integer")?;
    if instances == 0 || iterations < 30 {
        return Err("--instances must be positive and --iterations must be at least 30".into());
    }

    let document = make_document(format, instances)?;
    let bytes = document.to_bytes(format)?;

    for _ in 0..5 {
        black_box(ProjectDocument::from_bytes(format, black_box(&bytes))?);
        black_box(document.to_bytes(format)?);
    }

    let mut parse_times = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        black_box(ProjectDocument::from_bytes(format, black_box(&bytes))?);
        parse_times.push(started.elapsed());
    }

    let mut verified_encode_times = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        black_box(document.to_bytes(format)?);
        verified_encode_times.push(started.elapsed());
    }

    let directory = ScratchDirectory::new()?;
    let path = directory.0.join(format!("baseline.{}", format.extension()));
    fs::write(&path, &bytes)?;
    for _ in 0..5 {
        let warmup = document.save_atomic(&path)?;
        remove_backup(warmup.backup_path)?;
    }

    let mut atomic_save_times = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        let receipt = document.save_atomic(&path)?;
        atomic_save_times.push(started.elapsed());
        remove_backup(receipt.backup_path)?;
    }

    println!(
        "{{\"format\":\"{}\",\"extension\":\"{}\",\"requested_instances\":{},\"serialized_instances\":{},\"file_bytes\":{},\"iterations\":{},\"parse_encode_warmups\":5,\"atomic_save_warmups\":5,\"parse_p50_ms\":{:.3},\"parse_p95_ms\":{:.3},\"verified_encode_p50_ms\":{:.3},\"verified_encode_p95_ms\":{:.3},\"atomic_save_p50_ms\":{:.3},\"atomic_save_p95_ms\":{:.3},\"peak_rss_kib\":{},\"os\":\"{}\",\"arch\":\"{}\"}}",
        format_name(format),
        format.extension(),
        instances,
        instances + 1,
        bytes.len(),
        iterations,
        percentile_ms(&parse_times, 0.50),
        percentile_ms(&parse_times, 0.95),
        percentile_ms(&verified_encode_times, 0.50),
        percentile_ms(&verified_encode_times, 0.95),
        percentile_ms(&atomic_save_times, 0.50),
        percentile_ms(&atomic_save_times, 0.95),
        peak_rss_kib()?,
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    Ok(())
}

fn parse_format(value: &str) -> Option<FileFormat> {
    match value {
        "rbxl" => Some(FileFormat::PlaceBinary),
        "rbxm" => Some(FileFormat::ModelBinary),
        "rbxlx" => Some(FileFormat::PlaceXml),
        "rbxmx" => Some(FileFormat::ModelXml),
        _ => None,
    }
}

fn format_name(format: FileFormat) -> &'static str {
    match format {
        FileFormat::PlaceBinary => "place-binary",
        FileFormat::PlaceXml => "place-xml",
        FileFormat::ModelBinary => "model-binary",
        FileFormat::ModelXml => "model-xml",
    }
}

fn make_document(format: FileFormat, parts: usize) -> Result<ProjectDocument, Box<dyn Error>> {
    let mut document = ProjectDocument::new(format);
    let workspace = document.insert(document.root_id(), "Workspace", "Benchmark Workspace")?;
    for index in 0..parts {
        let part = document.insert(workspace, "Part", format!("Part {index}"))?;
        document.set_property(part, "Anchored", true.into())?;
        document.set_property(
            part,
            "CFrame",
            CFrame::new(Vector3::new(index as f32, 0.0, 0.0), Matrix3::identity()).into(),
        )?;
    }
    Ok(document)
}

fn percentile_ms(samples: &[std::time::Duration], percentile: f64) -> f64 {
    let mut samples = samples.to_vec();
    samples.sort_unstable();
    let rank = (percentile * samples.len() as f64).ceil().max(1.0) as usize;
    samples[rank - 1].as_secs_f64() * 1000.0
}

fn peak_rss_kib() -> Result<u64, Box<dyn Error>> {
    for line in fs::read_to_string("/proc/self/status")?.lines() {
        if let Some(value) = line.strip_prefix("VmHWM:") {
            return Ok(value
                .split_whitespace()
                .next()
                .ok_or("VmHWM is empty")?
                .parse()?);
        }
    }
    Err("/proc/self/status does not report VmHWM".into())
}

fn remove_backup(path: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    if let Some(path) = path {
        fs::remove_file(path)?;
    }
    Ok(())
}

struct ScratchDirectory(PathBuf);

impl ScratchDirectory {
    fn new() -> Result<Self, Box<dyn Error>> {
        let epoch = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let id = DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "studio-project-bench-{}-{epoch}-{id}",
            process::id()
        ));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
