use std::error::Error;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};
use winit::event_loop::EventLoopBuilder;

#[derive(Clone, Copy, Debug)]
enum Backend {
    Auto,
    Wayland,
    X11,
}

impl Backend {
    fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Wayland => "Wayland",
            Self::X11 => "X11",
        }
    }
}

#[derive(Debug)]
struct Options {
    backend: Backend,
    nodes: usize,
    code_lines: usize,
    render_instances: u32,
    capture_seconds: u64,
    warmup_frames: usize,
    metrics_out: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            backend: Backend::Auto,
            nodes: 10_000,
            code_lines: 20_000,
            render_instances: 2_000,
            capture_seconds: 0,
            warmup_frames: 120,
            metrics_out: None,
        }
    }
}

impl Options {
    fn parse() -> Result<Self, Box<dyn Error>> {
        let mut options = Self::default();
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--backend" => {
                    options.backend = match args.next().as_deref() {
                        Some("auto") => Backend::Auto,
                        Some("wayland") => Backend::Wayland,
                        Some("x11") => Backend::X11,
                        Some(value) => return Err(format!("unsupported backend: {value}").into()),
                        None => return Err("--backend requires auto, wayland, or x11".into()),
                    };
                }
                "--nodes" => options.nodes = parse_value(&mut args, "--nodes")?,
                "--code-lines" => options.code_lines = parse_value(&mut args, "--code-lines")?,
                "--render-instances" => {
                    options.render_instances = parse_value(&mut args, "--render-instances")?;
                }
                "--capture-seconds" => {
                    options.capture_seconds = parse_value(&mut args, "--capture-seconds")?;
                }
                "--warmup-frames" => {
                    options.warmup_frames = parse_value(&mut args, "--warmup-frames")?;
                }
                "--metrics-out" => {
                    options.metrics_out = Some(PathBuf::from(
                        args.next().ok_or("--metrics-out requires a file path")?,
                    ));
                }
                "--help" | "-h" => {
                    println!(
                        "Phase 0 native spike\n\
                         --backend auto|wayland|x11\n\
                         --nodes N --code-lines N --render-instances N\n\
                         --capture-seconds N --warmup-frames N --metrics-out PATH"
                    );
                    std::process::exit(0);
                }
                _ => return Err(format!("unknown argument: {arg}").into()),
            }
        }
        Ok(options)
    }
}

fn parse_value<T: std::str::FromStr>(
    args: &mut impl Iterator<Item = String>,
    name: &str,
) -> Result<T, Box<dyn Error>>
where
    T::Err: Error + 'static,
{
    args.next()
        .ok_or_else(|| format!("{name} requires a value"))?
        .parse()
        .map_err(Into::into)
}

#[derive(Clone)]
struct TreeRow {
    label: String,
    depth: usize,
}

struct Samples {
    update_ms: Vec<f64>,
    interval_ms: Vec<f64>,
    long_intervals: usize,
}

impl Samples {
    fn new() -> Self {
        Self {
            update_ms: Vec::new(),
            interval_ms: Vec::new(),
            long_intervals: 0,
        }
    }

    fn observe(&mut self, update: Duration, interval: Option<Duration>) {
        self.update_ms.push(update.as_secs_f64() * 1_000.0);
        if let Some(interval) = interval {
            let millis = interval.as_secs_f64() * 1_000.0;
            self.long_intervals += usize::from(millis > 100.0);
            self.interval_ms.push(millis);
        }
    }

    fn percentile(values: &[f64], percentile: f64) -> Option<f64> {
        if values.is_empty() {
            return None;
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let rank = ((percentile * sorted.len() as f64).ceil() as usize).max(1);
        Some(sorted[rank.saturating_sub(1).min(sorted.len() - 1)])
    }
}

struct SpikeApp {
    options: Options,
    rows: Vec<TreeRow>,
    source: String,
    adapter: String,
    started: Instant,
    capture_started: Option<Instant>,
    last_frame: Option<Instant>,
    frame_count: usize,
    samples: Samples,
    saved_metrics: bool,
}

impl SpikeApp {
    fn new(cc: &eframe::CreationContext<'_>, options: Options) -> Self {
        let render_state = cc
            .wgpu_render_state
            .as_ref()
            .expect("Phase 0 spike requires the wgpu renderer");
        let adapter = render_state.adapter.get_info();
        let adapter_summary = format!(
            "{} · {:?} · {:?} · {} · {}",
            adapter.name, adapter.backend, adapter.device_type, adapter.driver, adapter.driver_info
        );

        let shader = render_state
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("phase-0 cube grid"),
                source: wgpu::ShaderSource::Wgsl(include_str!("cube.wgsl").into()),
            });
        let bind_group_layout =
            render_state
                .device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("phase-0 cube grid uniforms"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: std::num::NonZeroU64::new(16),
                        },
                        count: None,
                    }],
                });
        let pipeline_layout =
            render_state
                .device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("phase-0 cube grid pipeline layout"),
                    bind_group_layouts: &[Some(&bind_group_layout)],
                    immediate_size: 0,
                });
        let pipeline =
            render_state
                .device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("phase-0 cube grid"),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_main"),
                        buffers: &[],
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                    },
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some("fs_main"),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: render_state.target_format,
                            blend: Some(wgpu::BlendState::REPLACE),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                    }),
                    multiview_mask: None,
                    cache: None,
                });
        let uniform_buffer = render_state.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("phase-0 cube grid uniforms"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = render_state
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("phase-0 cube grid uniforms"),
                layout: &bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                }],
            });
        render_state
            .renderer
            .write()
            .callback_resources
            .insert(CubeResources {
                pipeline,
                bind_group,
                uniform_buffer,
            });

        let rows = make_tree(options.nodes);
        let source = make_source(options.code_lines);
        Self {
            options,
            rows,
            source,
            adapter: adapter_summary,
            started: Instant::now(),
            capture_started: None,
            last_frame: None,
            frame_count: 0,
            samples: Samples::new(),
            saved_metrics: false,
        }
    }

    fn finish_metrics(&mut self) {
        if self.saved_metrics || self.options.metrics_out.is_none() {
            return;
        }
        let count = self.samples.interval_ms.len();
        let fps = Samples::percentile(&self.samples.interval_ms, 0.50)
            .filter(|value| *value > 0.0)
            .map(|millis| 1_000.0 / millis);
        let report = format!(
            concat!(
                "{{\n",
                "  \"backend\": \"{}\",\n",
                "  \"adapter\": {},\n",
                "  \"nodes\": {},\n",
                "  \"code_lines\": {},\n",
                "  \"render_instances\": {},\n",
                "  \"warmup_frames\": {},\n",
                "  \"capture_seconds_requested\": {},\n",
                "  \"capture_elapsed_ms\": {},\n",
                "  \"total_frames\": {},\n",
                "  \"sampled_frames\": {},\n",
                "  \"ui_update_cpu_ms_p50\": {},\n",
                "  \"ui_update_cpu_ms_p95\": {},\n",
                "  \"ui_update_cpu_ms_p99\": {},\n",
                "  \"present_interval_ms_p50\": {},\n",
                "  \"present_interval_ms_p95\": {},\n",
                "  \"present_interval_ms_p99\": {},\n",
                "  \"estimated_fps_from_median_interval\": {},\n",
                "  \"intervals_over_100ms\": {},\n",
                "  \"rss_kib_at_capture\": {}\n",
                "}}\n"
            ),
            self.options.backend.label(),
            json_string(&self.adapter),
            self.options.nodes,
            self.options.code_lines,
            self.options.render_instances,
            self.options.warmup_frames,
            self.options.capture_seconds,
            self.capture_started
                .map(|started| started.elapsed().as_secs_f64() * 1_000.0)
                .unwrap_or(0.0),
            self.frame_count,
            count,
            json_number(Samples::percentile(&self.samples.update_ms, 0.50)),
            json_number(Samples::percentile(&self.samples.update_ms, 0.95)),
            json_number(Samples::percentile(&self.samples.update_ms, 0.99)),
            json_number(Samples::percentile(&self.samples.interval_ms, 0.50)),
            json_number(Samples::percentile(&self.samples.interval_ms, 0.95)),
            json_number(Samples::percentile(&self.samples.interval_ms, 0.99)),
            json_number(fps),
            self.samples.long_intervals,
            process_rss_kib().unwrap_or(0),
        );
        if let Some(path) = &self.options.metrics_out {
            match std::fs::write(path, report) {
                Ok(()) => {
                    eprintln!("wrote benchmark report to {}", path.display());
                    self.saved_metrics = true;
                }
                Err(error) => eprintln!("failed to write benchmark report: {error}"),
            }
        }
    }
}

impl eframe::App for SpikeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let update_started = Instant::now();
        let interval = self
            .last_frame
            .replace(update_started)
            .map(|last| last.elapsed());
        self.frame_count += 1;

        ui.vertical(|ui| {
            ui.heading("Native Linux editor spike · Phase 0");
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Window backend requested: {}", self.options.backend.label()));
                ui.separator();
                ui.label(format!("GPU: {}", self.adapter));
                ui.separator();
                ui.label(format!("Tree rows: {}", self.options.nodes));
                ui.separator();
                ui.label(format!("Luau-like lines: {}", self.options.code_lines));
            });
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(270.0);
                    ui.heading("Explorer workload");
                    ui.label("Virtualized hierarchy; generated data only.");
                    egui::ScrollArea::vertical()
                        .id_salt("phase-0-tree")
                        .auto_shrink([false, false])
                        .show_rows(ui, 19.0, self.rows.len(), |ui, range| {
                            for row_index in range {
                                let row = &self.rows[row_index];
                                ui.horizontal(|ui| {
                                    ui.add_space((row.depth as f32) * 13.0);
                                    ui.label(&row.label);
                                });
                            }
                        });
                });

                ui.separator();
                ui.vertical(|ui| {
                    ui.heading("Interactive viewport workload");
                    ui.horizontal(|ui| {
                        ui.label("Instanced cubes");
                        ui.add(
                            egui::Slider::new(
                                &mut self.options.render_instances,
                                100..=50_000,
                            )
                            .logarithmic(true),
                        );
                    });
                    egui::Frame::canvas(ui.style()).show(ui, |ui| {
                        let size = egui::vec2(ui.available_width(), 330.0);
                        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
                            rect,
                            CubeCallback {
                                instances: self.options.render_instances,
                            },
                        ));
                    });

                    ui.add_space(8.0);
                    ui.heading("Long source editor workload");
                    ui.label("Editable generated source; this spike does not execute scripts.");
                    let available = ui.available_size();
                    egui::ScrollArea::both()
                        .id_salt("phase-0-source")
                        .show(ui, |ui| {
                            ui.add_sized(
                                [available.x, available.y.max(300.0)],
                                egui::TextEdit::multiline(&mut self.source)
                                    .font(egui::TextStyle::Monospace)
                                    .code_editor()
                                    .desired_width(f32::INFINITY),
                            );
                        });
                });
            });

            ui.separator();
            ui.label(format!(
                "Elapsed: {:.1}s · captured frames: {} · warmup: {} frames",
                self.started.elapsed().as_secs_f64(),
                self.samples.interval_ms.len(),
                self.options.warmup_frames
            ));
            ui.label("Frame intervals include event-loop/compositor pacing; they are not GPU execution time.");
        });

        if self.frame_count > self.options.warmup_frames {
            let capture_started = *self.capture_started.get_or_insert_with(Instant::now);
            self.samples.observe(update_started.elapsed(), interval);
            if self.options.capture_seconds > 0
                && capture_started.elapsed() >= Duration::from_secs(self.options.capture_seconds)
            {
                self.finish_metrics();
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(16));
            }
        } else {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
    }
}

struct CubeResources {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
}

struct CubeCallback {
    instances: u32,
}

impl egui_wgpu::CallbackTrait for CubeCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(resources) = resources.get::<CubeResources>() {
            let values = [self.instances, 0, 0, 0];
            let mut bytes = [0u8; 16];
            for (index, value) in values.iter().enumerate() {
                bytes[index * 4..index * 4 + 4].copy_from_slice(&value.to_ne_bytes());
            }
            queue.write_buffer(&resources.uniform_buffer, 0, &bytes);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(resources) = resources.get::<CubeResources>() else {
            return;
        };
        render_pass.set_pipeline(&resources.pipeline);
        render_pass.set_bind_group(0, &resources.bind_group, &[]);
        render_pass.draw(0..18, 0..self.instances);
    }
}

fn make_tree(nodes: usize) -> Vec<TreeRow> {
    let mut rows = Vec::with_capacity(nodes + nodes.div_ceil(100) + 1);
    rows.push(TreeRow {
        label: "Workspace".to_owned(),
        depth: 0,
    });
    for first in (0..nodes).step_by(100) {
        rows.push(TreeRow {
            label: format!("Model_{:05}", first / 100),
            depth: 1,
        });
        for index in first..(first + 100).min(nodes) {
            rows.push(TreeRow {
                label: format!("Part_{index:06}"),
                depth: 2,
            });
        }
    }
    rows
}

fn make_source(lines: usize) -> String {
    let mut source = String::with_capacity(lines.saturating_mul(48));
    for index in 0..lines {
        let _ = writeln!(
            source,
            "local value_{index:06}: number = {index} -- generated editor workload"
        );
    }
    source
}

fn json_number(value: Option<f64>) -> String {
    value
        .filter(|number| number.is_finite())
        .map(|number| format!("{number:.4}"))
        .unwrap_or_else(|| "null".to_owned())
}

fn json_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    format!("\"{escaped}\"")
}

fn process_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = Options::parse()?;
    let backend = options.backend;
    let event_loop_builder = match backend {
        Backend::Auto => None,
        Backend::Wayland => Some(
            Box::new(|builder: &mut EventLoopBuilder<eframe::UserEvent>| {
                use winit::platform::wayland::EventLoopBuilderExtWayland;
                builder.with_wayland();
            }) as eframe::EventLoopBuilderHook,
        ),
        Backend::X11 => Some(
            Box::new(|builder: &mut EventLoopBuilder<eframe::UserEvent>| {
                use winit::platform::x11::EventLoopBuilderExtX11;
                builder.with_x11();
            }) as eframe::EventLoopBuilderHook,
        ),
    };
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        event_loop_builder,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 1000.0])
            .with_title("Phase 0 native Linux spike"),
        ..Default::default()
    };
    eframe::run_native(
        "Phase 0 native Linux spike",
        native_options,
        Box::new(move |cc| Ok(Box::new(SpikeApp::new(cc, options)))),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Samples, make_source, make_tree};

    #[test]
    fn percentile_uses_nearest_rank_and_handles_empty_input() {
        assert_eq!(Samples::percentile(&[], 0.95), None);
        assert_eq!(Samples::percentile(&[4.0, 1.0, 3.0, 2.0], 0.50), Some(2.0));
        assert_eq!(Samples::percentile(&[4.0, 1.0, 3.0, 2.0], 0.95), Some(4.0));
    }

    #[test]
    fn tree_fixture_has_parent_rows_and_all_requested_parts() {
        let rows = make_tree(101);
        assert_eq!(rows.len(), 104);
        assert_eq!(rows[0].label, "Workspace");
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[2].depth, 2);
        assert_eq!(
            rows.last().map(|row| row.label.as_str()),
            Some("Part_000100")
        );
    }

    #[test]
    fn generated_source_contains_exact_requested_line_count() {
        let source = make_source(12);
        assert_eq!(source.lines().count(), 12);
        assert!(source.contains("local value_000011: number = 11"));
    }
}
