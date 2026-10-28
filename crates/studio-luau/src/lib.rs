//! A Linux-native Luau compiler and bounded, language-only execution service.
//!
//! This crate intentionally does not implement Roblox `Instance`, services,
//! scheduling, physics, or Studio behavior. Its execution API is for bounded
//! language experiments and future tooling, not Roblox Studio playtesting.

use std::{
    ffi::{CStr, c_char},
    io,
    ptr::{self, NonNull},
    slice,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use thiserror::Error;

const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;
const MIN_VM_MEMORY_BYTES: u64 = 1024 * 1024;
const MAX_VM_MEMORY_BYTES: u64 = 256 * 1024 * 1024;
const MAX_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_WORKERS: usize = 16;

#[repr(C)]
struct NativeDiagnostic {
    kind: i32,
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
    message: *mut c_char,
}

#[repr(C)]
struct NativeCompileResult {
    bytecode: *mut u8,
    bytecode_size: usize,
    diagnostics: *mut NativeDiagnostic,
    diagnostic_count: usize,
}

#[repr(C)]
struct NativeRunResult {
    status: i32,
    line: u32,
    column: u32,
    output: *mut u8,
    output_size: usize,
    message: *mut c_char,
}

#[repr(C)]
struct NativeControl {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn studio_luau_compile(
        source: *const c_char,
        source_size: usize,
        result: *mut NativeCompileResult,
    ) -> i32;
    fn studio_luau_compile_result_free(result: *mut NativeCompileResult);
    fn studio_luau_analyze(
        source: *const c_char,
        source_size: usize,
        result: *mut NativeCompileResult,
    ) -> i32;

    fn studio_luau_control_new() -> *mut NativeControl;
    fn studio_luau_control_cancel(control: *mut NativeControl);
    fn studio_luau_control_free(control: *mut NativeControl);

    fn studio_luau_run(
        bytecode: *const u8,
        bytecode_size: usize,
        memory_limit_bytes: u64,
        timeout_millis: u64,
        control: *mut NativeControl,
        result: *mut NativeRunResult,
    ) -> i32;
    fn studio_luau_run_result_free(result: *mut NativeRunResult);
}

/// An editable script buffer that keeps the exact source bytes as UTF-8 text.
///
/// Compilation failures do not mutate or reject the source. Line endings and
/// trailing newlines are preserved exactly as supplied.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScriptDocument {
    source: Arc<str>,
}

impl ScriptDocument {
    /// Construct a script document without normalizing its source.
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: Arc::from(source.into()),
        }
    }

    /// Return the exact source string stored in this document.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Replace the source exactly as supplied, including line endings.
    pub fn set_source(&mut self, source: impl Into<String>) {
        self.source = Arc::from(source.into());
    }
}

/// Luau compiler or runtime diagnostic category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// Luau parser rejected the source.
    Syntax,
    /// Luau bytecode generation rejected the parsed source.
    Compile,
    /// The VM reported a script execution error.
    Runtime,
    /// The native bridge encountered an unexpected error.
    Internal,
    /// The official Luau Frontend reported a type-checking error.
    Type,
    /// The official Luau linter reported an error.
    LintError,
    /// The official Luau linter reported a warning.
    LintWarning,
    /// The official Frontend exceeded its cooperative per-module time limit.
    AnalysisTimeout,
}

/// A source span with one-based lines and UTF-8 byte columns.
///
/// Zero means Luau did not provide that coordinate. End positions are
/// exclusive, matching the parser's source ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRange {
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

/// One Luau diagnostic with source location where available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub range: SourceRange,
    pub message: String,
}

/// A compiler result containing executable bytecode from the pinned Luau build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledScript {
    bytecode: Vec<u8>,
}

impl CompiledScript {
    /// Size of the compiled bytecode in bytes.
    pub fn bytecode_size(&self) -> usize {
        self.bytecode.len()
    }
}

/// Why source compilation could not produce bytecode.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CompileFailure {
    /// Source exceeded the implementation's bounded input size.
    #[error("script source exceeds the {limit}-byte limit ({actual} bytes)")]
    SourceTooLarge { actual: usize, limit: usize },
    /// Luau returned parser/compiler diagnostics.
    #[error("Luau returned one or more diagnostics")]
    Diagnostics(Vec<Diagnostic>),
    /// The native compiler bridge failed unexpectedly.
    #[error("Luau compiler failed: {0}")]
    Internal(String),
}

/// Why a script could not be analyzed.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AnalysisFailure {
    /// Source exceeded the implementation's bounded input size.
    #[error("script source exceeds the {limit}-byte limit ({actual} bytes)")]
    SourceTooLarge { actual: usize, limit: usize },
    /// The native Luau Frontend failed unexpectedly.
    #[error("Luau analyzer failed: {0}")]
    Internal(String),
}

/// Compile a script using the version-pinned official Luau parser and compiler.
pub fn compile(document: &ScriptDocument) -> Result<CompiledScript, CompileFailure> {
    let source = document.source.as_bytes();
    if source.len() > MAX_SOURCE_BYTES {
        return Err(CompileFailure::SourceTooLarge {
            actual: source.len(),
            limit: MAX_SOURCE_BYTES,
        });
    }

    let mut native_result = NativeCompileResult {
        bytecode: ptr::null_mut(),
        bytecode_size: 0,
        diagnostics: ptr::null_mut(),
        diagnostic_count: 0,
    };
    // SAFETY: the source slice remains alive for the call, and native_result is
    // a valid writable C-layout result that is freed by its matching function.
    let status = unsafe {
        studio_luau_compile(
            source.as_ptr().cast::<c_char>(),
            source.len(),
            &mut native_result,
        )
    };
    let guard = CompileResultGuard(&mut native_result);

    if status == 0 {
        if native_result.bytecode.is_null() && native_result.bytecode_size != 0 {
            return Err(CompileFailure::Internal(
                "native compiler returned a null bytecode buffer".to_owned(),
            ));
        }
        // SAFETY: the native bridge owns a buffer of bytecode_size bytes until
        // CompileResultGuard releases the result below.
        let bytecode = if native_result.bytecode_size == 0 {
            Vec::new()
        } else {
            // SAFETY: the native bridge owns a bytecode_size-byte buffer until
            // CompileResultGuard releases the result below.
            unsafe {
                slice::from_raw_parts(native_result.bytecode, native_result.bytecode_size).to_vec()
            }
        };
        drop(guard);
        return Ok(CompiledScript { bytecode });
    }

    let diagnostics = native_diagnostics(&native_result);
    drop(guard);
    if diagnostics.is_empty() {
        Err(CompileFailure::Internal(format!(
            "native compiler returned status {status} without diagnostics"
        )))
    } else if diagnostics
        .iter()
        .all(|diagnostic| diagnostic.kind == DiagnosticKind::Internal)
    {
        Err(CompileFailure::Internal(
            diagnostics
                .into_iter()
                .map(|diagnostic| diagnostic.message)
                .collect::<Vec<_>>()
                .join("; "),
        ))
    } else {
        Err(CompileFailure::Diagnostics(diagnostics))
    }
}

/// Type-check and lint one in-memory script with the official Luau Frontend.
///
/// This check has no Roblox Engine definitions and cannot resolve project
/// `require` dependencies. Source directives such as `--!strict`,
/// `--!nonstrict`, and `--!nocheck` are handled by Luau. The Frontend's
/// two-second per-module limit is cooperative, not a hard real-time bound.
/// Call this from a background worker rather than a UI thread.
pub fn analyze(document: &ScriptDocument) -> Result<Vec<Diagnostic>, AnalysisFailure> {
    let source = document.source.as_bytes();
    if source.len() > MAX_SOURCE_BYTES {
        return Err(AnalysisFailure::SourceTooLarge {
            actual: source.len(),
            limit: MAX_SOURCE_BYTES,
        });
    }

    let mut native_result = NativeCompileResult {
        bytecode: ptr::null_mut(),
        bytecode_size: 0,
        diagnostics: ptr::null_mut(),
        diagnostic_count: 0,
    };
    // SAFETY: source remains alive for the call and the initialized result is
    // released by CompileResultGuard after copying owned Rust diagnostics.
    let status = unsafe {
        studio_luau_analyze(
            source.as_ptr().cast::<c_char>(),
            source.len(),
            &mut native_result,
        )
    };
    let guard = CompileResultGuard(&mut native_result);
    let diagnostics = native_diagnostics(&native_result);
    drop(guard);

    if status == 0 {
        return Ok(diagnostics);
    }
    if diagnostics.is_empty() {
        return Err(AnalysisFailure::Internal(format!(
            "native analyzer returned status {status} without diagnostics"
        )));
    }
    Err(AnalysisFailure::Internal(
        diagnostics
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect::<Vec<_>>()
            .join("; "),
    ))
}

/// Limits for script execution on an isolated native worker thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionLimits {
    /// Wall-clock limit checked by Luau VM interrupt callbacks. Default: 2 s.
    pub timeout: Duration,
    /// Per-VM allocation limit, excluding compiler/host memory. Default: 64 MiB.
    pub vm_memory_bytes: u64,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(2),
            vm_memory_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Result state for one worker execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionStatus {
    /// Script returned normally.
    Completed,
    /// The compiler rejected the source with these structured diagnostics.
    CompileDiagnostics(Vec<Diagnostic>),
    /// Compilation was refused by a bounded input or internal failure.
    CompileRejected(String),
    /// Script raised a runtime error.
    RuntimeError(Diagnostic),
    /// The caller cancelled the run.
    Cancelled,
    /// The wall-clock execution limit expired.
    TimedOut,
    /// The VM memory allocation limit was reached.
    MemoryLimitExceeded,
    /// The native bridge failed unexpectedly.
    InternalError(String),
}

/// Script output and terminal execution state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionResult {
    pub status: ExecutionStatus,
    /// Captured `print` output, capped at 1 MiB per execution.
    pub output: String,
}

/// A bounded pool of workers for compile-and-run jobs.
#[derive(Debug, Clone)]
pub struct ExecutionService {
    max_workers: usize,
    active_workers: Arc<AtomicUsize>,
}

impl Default for ExecutionService {
    fn default() -> Self {
        Self::new(2).expect("the built-in worker limit is valid")
    }
}

impl ExecutionService {
    /// Create a worker service with a fixed concurrency ceiling between 1 and 16.
    pub fn new(max_workers: usize) -> Result<Self, ExecutionServiceError> {
        if !(1..=MAX_WORKERS).contains(&max_workers) {
            return Err(ExecutionServiceError::InvalidWorkerCount(max_workers));
        }
        Ok(Self {
            max_workers,
            active_workers: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Start compiling and executing a script without blocking the caller.
    ///
    /// Call [`ExecutionHandle::try_result`] from a UI loop, or use
    /// [`ExecutionHandle::wait`] from a non-UI context. Dropping an unfinished
    /// handle requests cancellation; it does not wait for the worker. The
    /// source-size limit is checked before scheduling. Timeout and cancellation
    /// are observed during VM execution; the upstream parser/compiler has no
    /// interrupt hook, so a bounded compile can finish before cancellation is
    /// returned.
    pub fn start(
        &self,
        document: &ScriptDocument,
        limits: ExecutionLimits,
    ) -> Result<ExecutionHandle, ExecutionServiceError> {
        validate_limits(limits)?;
        if document.source.len() > MAX_SOURCE_BYTES {
            return Err(ExecutionServiceError::SourceTooLarge {
                actual: document.source.len(),
                limit: MAX_SOURCE_BYTES,
            });
        }
        self.reserve_worker()?;

        let slot = WorkerSlot(Arc::clone(&self.active_workers));
        let (sender, receiver) = mpsc::channel();
        let control = Arc::new(NativeCancellation::new()?);
        let control_for_worker = Arc::clone(&control);
        let document = document.clone();
        let timeout_millis = limits.timeout.as_millis() as u64;
        let memory_limit_bytes = limits.vm_memory_bytes;

        let worker = thread::Builder::new()
            .name("studio-luau-worker".to_owned())
            .spawn(move || {
                let result = compile_and_run(
                    &document,
                    timeout_millis,
                    memory_limit_bytes,
                    &control_for_worker,
                );
                let _ = sender.send(result);
                drop(slot);
            })?;

        Ok(ExecutionHandle {
            receiver,
            worker: Some(worker),
            control,
            completed: false,
        })
    }

    fn reserve_worker(&self) -> Result<(), ExecutionServiceError> {
        let mut current = self.active_workers.load(Ordering::Acquire);
        loop {
            if current >= self.max_workers {
                return Err(ExecutionServiceError::WorkerLimitReached {
                    limit: self.max_workers,
                });
            }
            match self.active_workers.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(actual) => current = actual,
            }
        }
    }
}

/// A nonblocking handle to a Luau worker job.
pub struct ExecutionHandle {
    receiver: Receiver<ExecutionResult>,
    worker: Option<JoinHandle<()>>,
    control: Arc<NativeCancellation>,
    completed: bool,
}

impl ExecutionHandle {
    /// Request cancellation. The VM observes it at a Luau execution safepoint.
    pub fn cancel(&self) {
        self.control.cancel();
    }

    /// Check for a finished job without waiting.
    pub fn try_result(&mut self) -> Option<ExecutionResult> {
        if self.completed {
            return None;
        }
        match self.receiver.try_recv() {
            Ok(result) => {
                self.completed = true;
                self.release_worker_handle_without_waiting();
                Some(result)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.completed = true;
                self.release_worker_handle_without_waiting();
                Some(internal_result(
                    "Luau worker exited without returning a result",
                ))
            }
        }
    }

    /// Wait for completion. Do not call this from a UI thread.
    pub fn wait(mut self) -> ExecutionResult {
        let result = self
            .receiver
            .recv()
            .unwrap_or_else(|_| internal_result("Luau worker exited without returning a result"));
        self.completed = true;
        self.join_worker();
        result
    }

    fn join_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    fn release_worker_handle_without_waiting(&mut self) {
        if let Some(worker) = self.worker.take()
            && worker.is_finished()
        {
            let _ = worker.join();
        }
        // An unfinished JoinHandle detaches on drop; the worker owns its
        // cancellation token and slot guard until it exits.
    }
}

impl Drop for ExecutionHandle {
    fn drop(&mut self) {
        if !self.completed {
            self.control.cancel();
        }
    }
}

/// Failures that prevent a worker job from starting.
#[derive(Debug, Error)]
pub enum ExecutionServiceError {
    #[error("worker count must be between 1 and {MAX_WORKERS}, got {0}")]
    InvalidWorkerCount(usize),
    #[error("worker limit reached ({limit} active job(s))")]
    WorkerLimitReached { limit: usize },
    #[error("script source exceeds the {limit}-byte execution limit ({actual} bytes)")]
    SourceTooLarge { actual: usize, limit: usize },
    #[error("execution timeout must be greater than zero and no more than 60 seconds")]
    InvalidTimeout,
    #[error("VM memory limit must be between 1 MiB and 256 MiB")]
    InvalidMemoryLimit,
    #[error("could not create native Luau cancellation control")]
    NativeControlAllocation,
    #[error("could not start Luau worker: {0}")]
    ThreadSpawn(#[from] io::Error),
}

struct NativeCancellation {
    raw: NonNull<NativeControl>,
}

// SAFETY: the opaque native control contains only an atomic cancellation flag.
unsafe impl Send for NativeCancellation {}
// SAFETY: cancellation is the only operation shared across the worker boundary.
unsafe impl Sync for NativeCancellation {}

impl NativeCancellation {
    fn new() -> Result<Self, ExecutionServiceError> {
        // SAFETY: the native constructor returns an owned control or null.
        let raw = unsafe { studio_luau_control_new() };
        NonNull::new(raw)
            .map(|raw| Self { raw })
            .ok_or(ExecutionServiceError::NativeControlAllocation)
    }

    fn cancel(&self) {
        // SAFETY: the pointer stays valid while this Arc is alive; the native
        // function only sets its atomic cancellation flag.
        unsafe { studio_luau_control_cancel(self.raw.as_ptr()) };
    }
}

impl Drop for NativeCancellation {
    fn drop(&mut self) {
        // SAFETY: this is the unique final Arc owner and frees the allocation
        // returned by studio_luau_control_new.
        unsafe { studio_luau_control_free(self.raw.as_ptr()) };
    }
}

struct CompileResultGuard(*mut NativeCompileResult);

impl Drop for CompileResultGuard {
    fn drop(&mut self) {
        // SAFETY: the bridge accepts its own initialized result struct and
        // releases every native allocation returned by the compile call.
        unsafe { studio_luau_compile_result_free(self.0) };
    }
}

struct RunResultGuard(*mut NativeRunResult);

impl Drop for RunResultGuard {
    fn drop(&mut self) {
        // SAFETY: the bridge accepts its own initialized result struct and
        // releases every native allocation returned by the run call.
        unsafe { studio_luau_run_result_free(self.0) };
    }
}

struct WorkerSlot(Arc<AtomicUsize>);

impl Drop for WorkerSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn validate_limits(limits: ExecutionLimits) -> Result<(), ExecutionServiceError> {
    if limits.timeout.is_zero() || limits.timeout > MAX_TIMEOUT {
        return Err(ExecutionServiceError::InvalidTimeout);
    }
    if !(MIN_VM_MEMORY_BYTES..=MAX_VM_MEMORY_BYTES).contains(&limits.vm_memory_bytes) {
        return Err(ExecutionServiceError::InvalidMemoryLimit);
    }
    Ok(())
}

fn compile_and_run(
    document: &ScriptDocument,
    timeout_millis: u64,
    memory_limit_bytes: u64,
    control: &NativeCancellation,
) -> ExecutionResult {
    let compiled = match compile(document) {
        Ok(compiled) => compiled,
        Err(CompileFailure::Diagnostics(diagnostics)) => {
            return ExecutionResult {
                status: ExecutionStatus::CompileDiagnostics(diagnostics),
                output: String::new(),
            };
        }
        Err(error) => {
            return ExecutionResult {
                status: ExecutionStatus::CompileRejected(error.to_string()),
                output: String::new(),
            };
        }
    };

    let mut native_result = NativeRunResult {
        status: 5,
        line: 0,
        column: 0,
        output: ptr::null_mut(),
        output_size: 0,
        message: ptr::null_mut(),
    };
    // SAFETY: bytecode remains alive for the full call, control outlives the
    // worker, and native_result is released by RunResultGuard below.
    let bridge_status = unsafe {
        studio_luau_run(
            compiled.bytecode.as_ptr(),
            compiled.bytecode.len(),
            memory_limit_bytes,
            timeout_millis,
            control.raw.as_ptr(),
            &mut native_result,
        )
    };
    let guard = RunResultGuard(&mut native_result);

    let output = if native_result.output.is_null() || native_result.output_size == 0 {
        String::new()
    } else {
        // SAFETY: the native bridge owns output_size initialized bytes until
        // RunResultGuard releases its result.
        let bytes =
            unsafe { slice::from_raw_parts(native_result.output, native_result.output_size) };
        String::from_utf8_lossy(bytes).into_owned()
    };
    let message = native_message(native_result.message);
    let line = native_result.line;
    let status = match native_result.status {
        0 => ExecutionStatus::Completed,
        1 => ExecutionStatus::RuntimeError(runtime_diagnostic(line, message)),
        2 => ExecutionStatus::Cancelled,
        3 => ExecutionStatus::TimedOut,
        4 => ExecutionStatus::MemoryLimitExceeded,
        _ => ExecutionStatus::InternalError(if message.is_empty() {
            format!("native runtime bridge returned status {bridge_status}")
        } else {
            message
        }),
    };
    drop(guard);

    ExecutionResult { status, output }
}

fn native_diagnostics(result: &NativeCompileResult) -> Vec<Diagnostic> {
    if result.diagnostics.is_null() || result.diagnostic_count == 0 {
        return Vec::new();
    }
    // SAFETY: native result pointer/count are set together by the bridge and
    // remain valid until its matching free function runs.
    let entries = unsafe { slice::from_raw_parts(result.diagnostics, result.diagnostic_count) };
    entries
        .iter()
        .map(|entry| Diagnostic {
            kind: diagnostic_kind(entry.kind),
            range: SourceRange {
                start_line: entry.start_line,
                start_column: entry.start_column,
                end_line: entry.end_line,
                end_column: entry.end_column,
            },
            message: native_message(entry.message),
        })
        .collect()
}

fn diagnostic_kind(kind: i32) -> DiagnosticKind {
    match kind {
        1 => DiagnosticKind::Syntax,
        2 => DiagnosticKind::Compile,
        3 => DiagnosticKind::Runtime,
        5 => DiagnosticKind::Type,
        6 => DiagnosticKind::LintError,
        7 => DiagnosticKind::LintWarning,
        8 => DiagnosticKind::AnalysisTimeout,
        _ => DiagnosticKind::Internal,
    }
}

fn native_message(message: *const c_char) -> String {
    if message.is_null() {
        return String::new();
    }
    // SAFETY: the native bridge returns NUL-terminated diagnostic messages.
    unsafe { CStr::from_ptr(message) }
        .to_string_lossy()
        .into_owned()
}

fn runtime_diagnostic(line: u32, message: String) -> Diagnostic {
    Diagnostic {
        kind: DiagnosticKind::Runtime,
        range: SourceRange {
            start_line: line,
            start_column: 0,
            end_line: line,
            end_column: 0,
        },
        message,
    }
}

fn internal_result(message: &str) -> ExecutionResult {
    ExecutionResult {
        status: ExecutionStatus::InternalError(message.to_owned()),
        output: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(source: &str) -> ExecutionResult {
        ExecutionService::default()
            .start(&ScriptDocument::new(source), ExecutionLimits::default())
            .expect("worker should start")
            .wait()
    }

    #[test]
    fn official_luau_compiler_accepts_typed_reference_script() {
        let document = ScriptDocument::new(
            "--!strict\nlocal function add(a: number, b: number): number\n    return a + b\nend\nassert(add(2, 3) == 5)\n",
        );
        let compiled = compile(&document).expect("typed Luau should compile");
        assert!(compiled.bytecode_size() > 0);
    }

    #[test]
    fn official_frontend_checks_types_and_respects_nocheck() {
        let strict = ScriptDocument::new(
            "--!strict\nlocal value: number = \"not a number\"\nprint(value)\n",
        );
        let diagnostics = analyze(&strict).expect("analysis should complete");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == DiagnosticKind::Type && diagnostic.range.start_line == 2
        }));

        let unchecked = ScriptDocument::new(
            "--!nocheck\nlocal value: number = \"not a number\"\nprint(value)\n",
        );
        let diagnostics = analyze(&unchecked).expect("analysis should complete");
        assert!(
            !diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind == DiagnosticKind::Type)
        );
    }

    #[test]
    fn frontend_reports_lints_and_keeps_engine_globals_undefined() {
        let linted = analyze(&ScriptDocument::new("local unusedValue = 1\n"))
            .expect("analysis should complete");
        assert!(
            linted
                .iter()
                .any(|diagnostic| diagnostic.kind == DiagnosticKind::LintWarning)
        );

        let engine_reference = analyze(&ScriptDocument::new("--!strict\nprint(game.Name)\n"))
            .expect("analysis should complete");
        assert!(engine_reference.iter().any(|diagnostic| {
            diagnostic.kind == DiagnosticKind::Type && diagnostic.message.contains("game")
        }));
    }

    #[test]
    fn syntax_diagnostic_has_stable_one_based_location_and_source_is_preserved() {
        let source = "local ok = true\nlocal = false\r\n";
        let document = ScriptDocument::new(source);
        let failure = compile(&document).expect_err("invalid syntax should be diagnosed");
        assert_eq!(document.source(), source);
        let CompileFailure::Diagnostics(diagnostics) = failure else {
            panic!("expected syntax diagnostics");
        };
        assert_eq!(diagnostics[0].kind, DiagnosticKind::Syntax);
        assert_eq!(diagnostics[0].range.start_line, 2);
        assert!(diagnostics[0].range.start_column > 0);
    }

    #[test]
    fn exact_source_preserves_unicode_and_crlf() {
        let source = "local title = \"世界\"\r\nprint(title)\r\n";
        let mut document = ScriptDocument::new(source);
        assert_eq!(document.source(), source);
        document.set_source(source);
        assert_eq!(document.source().as_bytes(), source.as_bytes());
        assert!(compile(&document).is_ok());
    }

    #[test]
    fn language_only_runtime_captures_print_output() {
        let result = run("print(\"hello\", 7)\n");
        assert_eq!(result.status, ExecutionStatus::Completed);
        assert_eq!(result.output, "hello 7\n");
    }

    #[test]
    fn missing_engine_globals_are_reported_without_faking_them() {
        let result = run("print(game.Name)\n");
        let ExecutionStatus::RuntimeError(diagnostic) = result.status else {
            panic!(
                "expected missing engine global error, got {:?}",
                result.status
            );
        };
        assert_eq!(diagnostic.kind, DiagnosticKind::Runtime);
        assert_eq!(diagnostic.range.start_line, 1, "{}", diagnostic.message);
        assert!(diagnostic.message.contains("global 'game' is unavailable"));
        assert!(
            diagnostic
                .message
                .contains("Roblox Engine APIs are not implemented")
        );
    }

    #[test]
    fn infinite_loop_times_out_on_worker_thread() {
        let service = ExecutionService::new(1).expect("valid worker limit");
        let limits = ExecutionLimits {
            timeout: Duration::from_millis(100),
            ..ExecutionLimits::default()
        };
        let handle = service
            .start(&ScriptDocument::new("while true do end"), limits)
            .expect("worker should start");
        let result = handle.wait();
        assert_eq!(result.status, ExecutionStatus::TimedOut);
    }

    #[test]
    fn start_is_nonblocking_and_cancellation_interrupts_running_script() {
        let limits = ExecutionLimits {
            timeout: Duration::from_secs(5),
            ..ExecutionLimits::default()
        };
        let mut handle = ExecutionService::default()
            .start(&ScriptDocument::new("while true do end"), limits)
            .expect("worker should start");
        assert!(handle.try_result().is_none());
        handle.cancel();
        let result = handle.wait();
        assert_eq!(result.status, ExecutionStatus::Cancelled);
    }

    #[test]
    fn captured_output_has_a_hard_one_mebibyte_cap() {
        let result = run("print(string.rep(\"x\", 2 * 1024 * 1024))");
        assert_eq!(result.status, ExecutionStatus::Completed);
        assert!(result.output.len() <= 1024 * 1024);
        assert!(result.output.ends_with("[output truncated at 1 MiB]\n"));
    }

    #[test]
    fn vm_memory_limit_is_reported() {
        let limits = ExecutionLimits {
            vm_memory_bytes: MIN_VM_MEMORY_BYTES,
            ..ExecutionLimits::default()
        };
        let result = ExecutionService::default()
            .start(
                &ScriptDocument::new("local value = string.rep(\"x\", 32 * 1024 * 1024)"),
                limits,
            )
            .expect("worker should start")
            .wait();
        assert_eq!(result.status, ExecutionStatus::MemoryLimitExceeded);
    }

    #[test]
    fn runtime_diagnostics_have_a_separate_bounded_message_limit() {
        let result = run("error(string.rep(\"x\", 1024 * 1024))");
        let ExecutionStatus::RuntimeError(diagnostic) = result.status else {
            panic!("expected runtime error, got {:?}", result.status);
        };
        assert!(diagnostic.message.len() <= 16 * 1024);
        assert!(
            diagnostic
                .message
                .ends_with("[diagnostic truncated at 16 KiB]")
        );
    }

    #[test]
    fn worker_limit_is_enforced_without_waiting_for_the_first_job() {
        let service = ExecutionService::new(1).expect("valid worker limit");
        let limits = ExecutionLimits {
            timeout: Duration::from_secs(5),
            ..ExecutionLimits::default()
        };
        let _first = service
            .start(&ScriptDocument::new("while true do end"), limits)
            .expect("first worker should start");
        let second = service.start(&ScriptDocument::new("return"), limits);
        assert!(matches!(
            second,
            Err(ExecutionServiceError::WorkerLimitReached { limit: 1 })
        ));
    }

    #[test]
    fn source_and_runtime_limits_are_validated() {
        let too_large = ScriptDocument::new(" ".repeat(MAX_SOURCE_BYTES + 1));
        assert!(matches!(
            compile(&too_large),
            Err(CompileFailure::SourceTooLarge { .. })
        ));

        let service = ExecutionService::default();
        assert!(matches!(
            service.start(&too_large, ExecutionLimits::default()),
            Err(ExecutionServiceError::SourceTooLarge { .. })
        ));
        let invalid_limits = ExecutionLimits {
            timeout: Duration::ZERO,
            ..ExecutionLimits::default()
        };
        assert!(matches!(
            service.start(&ScriptDocument::default(), invalid_limits),
            Err(ExecutionServiceError::InvalidTimeout)
        ));
    }
}
