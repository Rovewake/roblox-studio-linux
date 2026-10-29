//! A Linux-native Luau compiler and bounded, language-only execution/debug service.
//!
//! This crate intentionally does not implement Roblox `Instance`, services,
//! scheduling, physics, or Studio behavior. Its execution API is for bounded
//! language experiments and future tooling, not Roblox Studio playtesting.

use std::{
    collections::{HashMap, HashSet},
    ffi::{CStr, c_char, c_void},
    io,
    ptr::{self, NonNull},
    slice,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use stylua_lib::{LineEndings, LuaVersion, OutputVerification, SortRequiresConfig};
use thiserror::Error;

const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;
const MIN_VM_MEMORY_BYTES: u64 = 1024 * 1024;
const MAX_VM_MEMORY_BYTES: u64 = 256 * 1024 * 1024;
const MAX_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_WORKERS: usize = 16;
const MAX_DEBUG_BREAKPOINTS: usize = 256;
const MAX_ANALYSIS_NODES: usize = 100_000;
const MAX_ANALYSIS_MODULES: usize = 512;
const MAX_ANALYSIS_SOURCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_ANALYSIS_ID_BYTES: usize = 256;
const MAX_ANALYSIS_NAME_BYTES: usize = 512;
const MAX_COMPLETION_ITEMS: usize = 256;

#[repr(C)]
struct NativeDiagnostic {
    kind: i32,
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
    message: *mut c_char,
    module_id: *mut c_char,
}

#[repr(C)]
struct NativeCompileResult {
    bytecode: *mut u8,
    bytecode_size: usize,
    diagnostics: *mut NativeDiagnostic,
    diagnostic_count: usize,
}

#[repr(C)]
struct NativeAnalysisNode {
    id: *const c_char,
    id_size: usize,
    parent_id: *const c_char,
    parent_id_size: usize,
    name: *const c_char,
    name_size: usize,
    source: *const c_char,
    source_size: usize,
    kind: i32,
}

#[repr(C)]
struct NativeCompletionItem {
    label: *mut c_char,
    kind: i32,
    insert_text: *mut c_char,
}

#[repr(C)]
struct NativeCompletionResult {
    items: *mut NativeCompletionItem,
    item_count: usize,
    context: i32,
}

#[repr(C)]
#[derive(Default)]
struct NativeDefinitionResult {
    found: i32,
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
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
    fn studio_luau_analyze_project(
        project_root_id: *const c_char,
        project_root_id_size: usize,
        entry_script_id: *const c_char,
        entry_script_id_size: usize,
        nodes: *const NativeAnalysisNode,
        node_count: usize,
        result: *mut NativeCompileResult,
    ) -> i32;
    fn studio_luau_complete_project(
        project_root_id: *const c_char,
        project_root_id_size: usize,
        entry_script_id: *const c_char,
        entry_script_id_size: usize,
        nodes: *const NativeAnalysisNode,
        node_count: usize,
        zero_based_line: u32,
        zero_based_column: u32,
        result: *mut NativeCompletionResult,
    ) -> i32;
    fn studio_luau_completion_result_free(result: *mut NativeCompletionResult);
    fn studio_luau_find_local_definition(
        source: *const c_char,
        source_size: usize,
        zero_based_line: u32,
        zero_based_column: u32,
        result: *mut NativeDefinitionResult,
    ) -> i32;

    fn studio_luau_control_new() -> *mut NativeControl;
    fn studio_luau_control_cancel(control: *mut NativeControl);
    fn studio_luau_control_resume(control: *mut NativeControl);
    fn studio_luau_control_free(control: *mut NativeControl);

    fn studio_luau_run(
        bytecode: *const u8,
        bytecode_size: usize,
        memory_limit_bytes: u64,
        timeout_millis: u64,
        control: *mut NativeControl,
        result: *mut NativeRunResult,
    ) -> i32;
    fn studio_luau_debug_run(
        bytecode: *const u8,
        bytecode_size: usize,
        breakpoint_lines: *const u32,
        breakpoint_count: usize,
        memory_limit_bytes: u64,
        timeout_millis: u64,
        control: *mut NativeControl,
        event_callback: unsafe extern "C" fn(*mut c_void, u32),
        event_user_data: *mut c_void,
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

/// A non-mutating result from the explicit Luau format-preview operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatPreview {
    /// The proposed source text. The caller must explicitly apply it as an
    /// undoable editor change after presenting the preview to the user.
    pub formatted_source: String,
    /// Whether the formatter proposed any text changes.
    pub changed: bool,
}

/// Why a formatting preview could not be produced safely.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// Source exceeded the same bounded size accepted by the Luau compiler.
    #[error("script source exceeds the {limit}-byte formatting limit ({actual} bytes)")]
    SourceTooLarge { actual: usize, limit: usize },
    /// The formatter only accepts uniform LF or CRLF line endings.
    #[error("formatting preview requires uniform LF or CRLF line endings")]
    UnsupportedLineEndings,
    /// The pinned Luau compiler did not accept the input source.
    #[error("source is not accepted by the pinned Luau compiler: {0}")]
    InvalidLuau(String),
    /// StyLua rejected the input or could not verify its formatted AST.
    #[error("StyLua could not produce a verified formatting preview: {0}")]
    FormatterRejected(String),
    /// The formatter output is not accepted by the pinned Luau compiler.
    #[error("formatted output is not accepted by the pinned Luau compiler: {0}")]
    InvalidFormatterOutput(String),
    /// Formatted output exceeded the source-size limit.
    #[error("formatted source exceeds the {limit}-byte limit ({actual} bytes)")]
    FormattedSourceTooLarge { actual: usize, limit: usize },
}

/// Produce a Luau formatting preview without modifying the source document.
///
/// StyLua's parser is independent from Luau's official parser. The input and
/// output are therefore both checked by the pinned Luau compiler, in addition
/// to StyLua's full AST verification. Formatting can be expensive and has no
/// cancellation API; callers must run this function away from the UI thread.
pub fn format_preview(document: &ScriptDocument) -> Result<FormatPreview, FormatError> {
    let source = document.source();
    if source.len() > MAX_SOURCE_BYTES {
        return Err(FormatError::SourceTooLarge {
            actual: source.len(),
            limit: MAX_SOURCE_BYTES,
        });
    }

    let line_endings = detect_uniform_line_endings(source)?;
    compile(document).map_err(|error| FormatError::InvalidLuau(error.to_string()))?;

    let config = stylua_lib::Config {
        syntax: LuaVersion::Luau,
        line_endings,
        sort_requires: SortRequiresConfig { enabled: false },
        ..Default::default()
    };

    let formatted_source = stylua_lib::format_code(source, config, None, OutputVerification::Full)
        .map_err(|error| FormatError::FormatterRejected(error.to_string()))?;

    if formatted_source.len() > MAX_SOURCE_BYTES {
        return Err(FormatError::FormattedSourceTooLarge {
            actual: formatted_source.len(),
            limit: MAX_SOURCE_BYTES,
        });
    }
    compile(&ScriptDocument::new(formatted_source.clone()))
        .map_err(|error| FormatError::InvalidFormatterOutput(error.to_string()))?;

    Ok(FormatPreview {
        changed: formatted_source != source,
        formatted_source,
    })
}

fn detect_uniform_line_endings(source: &str) -> Result<LineEndings, FormatError> {
    let bytes = source.as_bytes();
    let mut saw_lf = false;
    let mut saw_crlf = false;
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                saw_crlf = true;
                index += 2;
            }
            b'\r' => return Err(FormatError::UnsupportedLineEndings),
            b'\n' => {
                saw_lf = true;
                index += 1;
            }
            _ => index += 1,
        }
    }

    if saw_lf && saw_crlf {
        return Err(FormatError::UnsupportedLineEndings);
    }
    Ok(if saw_crlf {
        LineEndings::Windows
    } else {
        LineEndings::Unix
    })
}

/// Node kind in a project snapshot supplied to the Luau Frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectNodeKind {
    /// A non-script Instance that can appear between a script and a child.
    Container,
    /// A module that may be loaded by `require`.
    ModuleScript,
    /// A server Script used as the analysis entry point.
    Script,
    /// A LocalScript used as the analysis entry point.
    LocalScript,
}

impl ProjectNodeKind {
    fn native_value(self) -> i32 {
        match self {
            Self::Container => 0,
            Self::ModuleScript => 1,
            Self::Script => 2,
            Self::LocalScript => 3,
        }
    }

    fn has_source(self) -> bool {
        !matches!(self, Self::Container)
    }
}

/// One project hierarchy node for static module resolution.
///
/// IDs are opaque to Luau and only need to be unique within this snapshot.
/// They are usually produced from the open project's instance identities.
/// The snapshot includes containers so static paths can walk ancestors and
/// children. A source is required for scripts and forbidden for containers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectNode {
    /// Opaque, project-local identity. Do not use a machine-specific path.
    pub id: String,
    /// Exact Instance name used for static child-path matching.
    pub name: String,
    /// Parent node ID; only the project root has no parent.
    pub parent_id: Option<String>,
    /// Whether this node is a ModuleScript, Script, LocalScript, or container.
    pub kind: ProjectNodeKind,
    /// Exact source for a script node; absent for containers.
    pub source: Option<ScriptDocument>,
}

impl ProjectNode {
    /// Create a non-script hierarchy node.
    pub fn container(
        id: impl Into<String>,
        name: impl Into<String>,
        parent_id: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            parent_id,
            kind: ProjectNodeKind::Container,
            source: None,
        }
    }

    /// Create a ModuleScript node that can be a `require` target.
    pub fn module_script(
        id: impl Into<String>,
        name: impl Into<String>,
        parent_id: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self::with_source(id, name, parent_id, ProjectNodeKind::ModuleScript, source)
    }

    /// Create a server Script node for use as an analysis entry point.
    pub fn script(
        id: impl Into<String>,
        name: impl Into<String>,
        parent_id: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self::with_source(id, name, parent_id, ProjectNodeKind::Script, source)
    }

    /// Create a LocalScript node for use as an analysis entry point.
    pub fn local_script(
        id: impl Into<String>,
        name: impl Into<String>,
        parent_id: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self::with_source(id, name, parent_id, ProjectNodeKind::LocalScript, source)
    }

    fn with_source(
        id: impl Into<String>,
        name: impl Into<String>,
        parent_id: impl Into<String>,
        kind: ProjectNodeKind,
        source: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            parent_id: Some(parent_id.into()),
            kind,
            source: Some(ScriptDocument::new(source)),
        }
    }
}

/// Zero-based line and UTF-8 byte column for editor features.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePosition {
    pub line: u32,
    pub column: u32,
}

/// Completion category returned by the pinned Luau autocomplete API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Property,
    Binding,
    Keyword,
    String,
    Type,
    Module,
    GeneratedFunction,
    RequirePath,
    HotComment,
    Unknown,
}

/// Syntactic context in which Luau requested completions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionContext {
    Unknown,
    Expression,
    Statement,
    Property,
    Type,
    Keyword,
    String,
    HotComment,
}

/// One non-mutating editor suggestion from Luau's Frontend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    pub insert_text: Option<String>,
}

/// Completion suggestions and the source context returned by Luau.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionList {
    pub context: CompletionContext,
    pub items: Vec<CompletionItem>,
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
    /// Project module resolution exceeded its bounded amount of tree work.
    AnalysisLimit,
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
    /// Project node ID when the Frontend can attribute a project diagnostic.
    pub module_id: Option<String>,
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
    /// The supplied project snapshot was malformed or exceeded its graph limits.
    #[error("project analysis input is invalid: {0}")]
    InvalidProject(String),
    /// The project contains more nodes than the bounded analyzer accepts.
    #[error("project contains {actual} analysis nodes; the limit is {limit}")]
    TooManyProjectNodes { actual: usize, limit: usize },
    /// Combined script source exceeds the project analysis limit.
    #[error("project script sources total {actual} bytes; the limit is {limit}")]
    ProjectSourcesTooLarge { actual: usize, limit: usize },
    /// Cursor points outside the entry script or inside a UTF-8 code point.
    #[error("completion cursor is not a valid UTF-8 source position: line {line}, column {column}")]
    InvalidCursorPosition { line: u32, column: u32 },
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

/// Type-check and lint one entry script and its statically resolved modules.
///
/// This consumes an immutable project hierarchy snapshot. It resolves only
/// unique, local `ModuleScript` targets reached through static paths; dynamic
/// expressions, ambiguous child names, and remote asset IDs remain unresolved.
/// The resolver follows the documented Luau paths `script.Parent.Child`,
/// constant-string child lookup, `GetService("...")`, and string paths with
/// `./`, `../`, `@self/`, or `@game/` prefixes. These rules do not implement a
/// runtime DataModel or any Roblox Engine behavior. Resolver tree work is
/// bounded; exhaustion is returned as an `AnalysisLimit` diagnostic. The
/// selected entry remains available as the Frontend root, but a non-ModuleScript
/// entry cannot be loaded as a `require` dependency. The caller must capture the
/// project consistently and run this blocking analysis off the UI thread.
pub fn analyze_project(
    project_root_id: &str,
    entry_script_id: &str,
    nodes: &[ProjectNode],
) -> Result<Vec<Diagnostic>, AnalysisFailure> {
    validate_project_snapshot(project_root_id, entry_script_id, nodes)?;

    let mut native_nodes = Vec::with_capacity(nodes.len());
    for node in nodes {
        let parent_id = node.parent_id.as_deref();
        let source = node
            .source
            .as_ref()
            .map(ScriptDocument::source)
            .unwrap_or("");
        native_nodes.push(NativeAnalysisNode {
            id: node.id.as_ptr().cast::<c_char>(),
            id_size: node.id.len(),
            parent_id: parent_id.map_or(ptr::null(), |value| value.as_ptr().cast::<c_char>()),
            parent_id_size: parent_id.map_or(0, str::len),
            name: node.name.as_ptr().cast::<c_char>(),
            name_size: node.name.len(),
            source: source.as_ptr().cast::<c_char>(),
            source_size: source.len(),
            kind: node.kind.native_value(),
        });
    }

    let mut native_result = NativeCompileResult {
        bytecode: ptr::null_mut(),
        bytecode_size: 0,
        diagnostics: ptr::null_mut(),
        diagnostic_count: 0,
    };
    // SAFETY: all strings and the node array remain alive until the bridge
    // returns; `native_result` is released by its matching free function.
    let status = unsafe {
        studio_luau_analyze_project(
            project_root_id.as_ptr().cast::<c_char>(),
            project_root_id.len(),
            entry_script_id.as_ptr().cast::<c_char>(),
            entry_script_id.len(),
            native_nodes.as_ptr(),
            native_nodes.len(),
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
            "native project analyzer returned status {status} without diagnostics"
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

/// Request editor completions from the official Luau Frontend and pinned
/// partial Engine type surface. This returns suggestions only; it never changes
/// the script source. Run it off the UI thread because project checking is
/// synchronous and has a cooperative deadline.
pub fn complete_project(
    project_root_id: &str,
    entry_script_id: &str,
    nodes: &[ProjectNode],
    position: SourcePosition,
) -> Result<CompletionList, AnalysisFailure> {
    validate_project_snapshot(project_root_id, entry_script_id, nodes)?;
    let Some(entry) = nodes.iter().find(|node| node.id == entry_script_id) else {
        return Err(AnalysisFailure::InvalidProject(
            "entry script disappeared after project validation".to_owned(),
        ));
    };
    let Some(document) = entry.source.as_ref() else {
        return Err(AnalysisFailure::InvalidProject(
            "entry script has no source after project validation".to_owned(),
        ));
    };
    let source = document.source();
    validate_cursor_position(source, position)?;

    let mut native_nodes = Vec::with_capacity(nodes.len());
    for node in nodes {
        let parent_id = node.parent_id.as_deref();
        let source = node
            .source
            .as_ref()
            .map(ScriptDocument::source)
            .unwrap_or("");
        native_nodes.push(NativeAnalysisNode {
            id: node.id.as_ptr().cast::<c_char>(),
            id_size: node.id.len(),
            parent_id: parent_id.map_or(ptr::null(), |value| value.as_ptr().cast::<c_char>()),
            parent_id_size: parent_id.map_or(0, str::len),
            name: node.name.as_ptr().cast::<c_char>(),
            name_size: node.name.len(),
            source: source.as_ptr().cast::<c_char>(),
            source_size: source.len(),
            kind: node.kind.native_value(),
        });
    }

    let mut native_result = NativeCompletionResult {
        items: ptr::null_mut(),
        item_count: 0,
        context: 0,
    };
    // SAFETY: the validated node strings and native node array outlive the call;
    // the matching native free function releases every returned allocation.
    let status = unsafe {
        studio_luau_complete_project(
            project_root_id.as_ptr().cast::<c_char>(),
            project_root_id.len(),
            entry_script_id.as_ptr().cast::<c_char>(),
            entry_script_id.len(),
            native_nodes.as_ptr(),
            native_nodes.len(),
            position.line,
            position.column,
            &mut native_result,
        )
    };
    let guard = CompletionResultGuard(&mut native_result);
    if status != 0 {
        drop(guard);
        let reason = match status {
            -2 => "Luau autocomplete exceeded its cooperative 15-second deadline".to_owned(),
            -3 => "project module resolution exceeded its bounded node-visit budget".to_owned(),
            _ => format!("native Luau autocomplete returned status {status}"),
        };
        return Err(AnalysisFailure::Internal(reason));
    }
    if native_result.item_count > MAX_COMPLETION_ITEMS
        || (native_result.item_count != 0 && native_result.items.is_null())
    {
        drop(guard);
        return Err(AnalysisFailure::Internal(
            "native autocomplete returned an invalid completion list".to_owned(),
        ));
    }

    let context = completion_context(native_result.context);
    // SAFETY: the result came from the bridge and passed pointer/count checks;
    // the result guard remains active until all strings are copied.
    let entries = if native_result.item_count == 0 {
        &[][..]
    } else {
        unsafe { slice::from_raw_parts(native_result.items, native_result.item_count) }
    };
    let items = entries
        .iter()
        .map(|item| CompletionItem {
            label: native_message(item.label),
            kind: completion_kind(item.kind),
            insert_text: native_optional_message(item.insert_text),
        })
        .collect();
    drop(guard);

    Ok(CompletionList { context, items })
}

/// Find the declaration of a local variable or parameter at this source
/// position. Globals, properties, and module navigation are outside this local
/// symbol contract. The returned range uses one-based lines and UTF-8 byte
/// columns, like diagnostics. The API parses the source synchronously; call it
/// on a background worker for large documents.
pub fn find_local_definition(
    document: &ScriptDocument,
    position: SourcePosition,
) -> Result<Option<SourceRange>, AnalysisFailure> {
    let source = document.source().as_bytes();
    if source.len() > MAX_SOURCE_BYTES {
        return Err(AnalysisFailure::SourceTooLarge {
            actual: source.len(),
            limit: MAX_SOURCE_BYTES,
        });
    }
    validate_cursor_position(document.source(), position)?;

    let mut native_result = NativeDefinitionResult::default();
    // SAFETY: the immutable UTF-8 source remains alive for the call and the
    // initialized result pointer refers to a writable C-layout value.
    let status = unsafe {
        studio_luau_find_local_definition(
            source.as_ptr().cast::<c_char>(),
            source.len(),
            position.line,
            position.column,
            &mut native_result,
        )
    };
    if status != 0 {
        return Err(AnalysisFailure::Internal(format!(
            "native local definition lookup returned status {status}"
        )));
    }
    if native_result.found == 0 {
        return Ok(None);
    }

    Ok(Some(SourceRange {
        start_line: native_result.start_line,
        start_column: native_result.start_column,
        end_line: native_result.end_line,
        end_column: native_result.end_column,
    }))
}

fn validate_cursor_position(source: &str, position: SourcePosition) -> Result<(), AnalysisFailure> {
    let valid = source
        .split('\n')
        .nth(position.line as usize)
        .is_some_and(|line| {
            let column = position.column as usize;
            column <= line.len() && line.is_char_boundary(column)
        });
    if valid {
        Ok(())
    } else {
        Err(AnalysisFailure::InvalidCursorPosition {
            line: position.line,
            column: position.column,
        })
    }
}

fn validate_project_snapshot(
    project_root_id: &str,
    entry_script_id: &str,
    nodes: &[ProjectNode],
) -> Result<(), AnalysisFailure> {
    if nodes.is_empty() {
        return Err(AnalysisFailure::InvalidProject(
            "the project snapshot contains no nodes".to_owned(),
        ));
    }
    if nodes.len() > MAX_ANALYSIS_NODES {
        return Err(AnalysisFailure::TooManyProjectNodes {
            actual: nodes.len(),
            limit: MAX_ANALYSIS_NODES,
        });
    }
    validate_project_id(project_root_id, "project root")?;
    validate_project_id(entry_script_id, "entry script")?;

    let mut by_id = HashMap::with_capacity(nodes.len());
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut source_bytes = 0usize;
    let mut module_count = 0usize;
    let mut root_count = 0usize;

    for node in nodes {
        validate_project_id(&node.id, "node")?;
        if node.name.len() > MAX_ANALYSIS_NAME_BYTES || node.name.contains('\0') {
            return Err(AnalysisFailure::InvalidProject(format!(
                "node {:?} has an invalid or oversized name",
                node.id
            )));
        }
        if by_id.insert(node.id.as_str(), node).is_some() {
            return Err(AnalysisFailure::InvalidProject(format!(
                "duplicate node ID {:?}",
                node.id
            )));
        }
        match (node.kind.has_source(), node.source.as_ref()) {
            (true, Some(document)) => {
                let bytes = document.source.len();
                if bytes > MAX_SOURCE_BYTES {
                    return Err(AnalysisFailure::SourceTooLarge {
                        actual: bytes,
                        limit: MAX_SOURCE_BYTES,
                    });
                }
                source_bytes = source_bytes.checked_add(bytes).ok_or({
                    AnalysisFailure::ProjectSourcesTooLarge {
                        actual: usize::MAX,
                        limit: MAX_ANALYSIS_SOURCE_BYTES,
                    }
                })?;
                if node.kind == ProjectNodeKind::ModuleScript {
                    module_count += 1;
                }
            }
            (true, None) => {
                return Err(AnalysisFailure::InvalidProject(format!(
                    "script node {:?} has no source",
                    node.id
                )));
            }
            (false, Some(_)) => {
                return Err(AnalysisFailure::InvalidProject(format!(
                    "container node {:?} must not have source",
                    node.id
                )));
            }
            (false, None) => {}
        }
        if source_bytes > MAX_ANALYSIS_SOURCE_BYTES {
            return Err(AnalysisFailure::ProjectSourcesTooLarge {
                actual: source_bytes,
                limit: MAX_ANALYSIS_SOURCE_BYTES,
            });
        }

        match node.parent_id.as_deref() {
            None => {
                root_count += 1;
                if node.id != project_root_id || node.kind != ProjectNodeKind::Container {
                    return Err(AnalysisFailure::InvalidProject(format!(
                        "only the declared project root may have no parent, and it must be a container (found {:?})",
                        node.id
                    )));
                }
            }
            Some(parent_id) => {
                validate_project_id(parent_id, "parent")?;
                children
                    .entry(parent_id)
                    .or_default()
                    .push(node.id.as_str());
            }
        }
    }

    if root_count != 1 {
        return Err(AnalysisFailure::InvalidProject(format!(
            "expected one root node, found {root_count}"
        )));
    }
    if module_count > MAX_ANALYSIS_MODULES {
        return Err(AnalysisFailure::InvalidProject(format!(
            "project contains {module_count} ModuleScripts; the limit is {MAX_ANALYSIS_MODULES}"
        )));
    }
    for node in nodes {
        if let Some(parent_id) = node.parent_id.as_deref()
            && !by_id.contains_key(parent_id)
        {
            return Err(AnalysisFailure::InvalidProject(format!(
                "node {:?} refers to missing parent {:?}",
                node.id, parent_id
            )));
        }
    }

    let root = by_id.get(project_root_id).ok_or_else(|| {
        AnalysisFailure::InvalidProject(format!(
            "project root ID {:?} is not present",
            project_root_id
        ))
    })?;
    if root.parent_id.is_some() || root.kind != ProjectNodeKind::Container {
        return Err(AnalysisFailure::InvalidProject(
            "declared project root must be a parentless container".to_owned(),
        ));
    }
    let entry = by_id.get(entry_script_id).ok_or_else(|| {
        AnalysisFailure::InvalidProject(format!(
            "entry script ID {:?} is not present",
            entry_script_id
        ))
    })?;
    if !entry.kind.has_source() || entry.source.is_none() {
        return Err(AnalysisFailure::InvalidProject(
            "entry node must be a Script, LocalScript, or ModuleScript with source".to_owned(),
        ));
    }

    let mut reachable = HashSet::with_capacity(nodes.len());
    let mut pending = vec![project_root_id];
    while let Some(id) = pending.pop() {
        if !reachable.insert(id) {
            return Err(AnalysisFailure::InvalidProject(
                "project hierarchy contains a cycle".to_owned(),
            ));
        }
        if let Some(children) = children.get(id) {
            pending.extend(children.iter().copied());
        }
    }
    if reachable.len() != nodes.len() {
        return Err(AnalysisFailure::InvalidProject(
            "every node must belong to the declared project tree".to_owned(),
        ));
    }

    Ok(())
}

fn validate_project_id(id: &str, field: &str) -> Result<(), AnalysisFailure> {
    if id.is_empty() || id.len() > MAX_ANALYSIS_ID_BYTES || id.contains('\0') {
        return Err(AnalysisFailure::InvalidProject(format!(
            "{field} ID must be nonempty, NUL-free, and at most {MAX_ANALYSIS_ID_BYTES} bytes"
        )));
    }
    Ok(())
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
    /// The debugger could not install the requested breakpoint or resume state.
    DebuggerError(String),
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

/// A bounded event from a language-only debug session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugEvent {
    /// Luau stopped at the executable entry-script line selected for a breakpoint.
    BreakpointHit { line: u32 },
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

    /// Start a language-only debug session with breakpoints in the entry script.
    ///
    /// Breakpoint lines are one-based; at most 256 requested entries are
    /// accepted and duplicates are deduplicated. Luau may move a breakpoint to
    /// the next executable line, which is reported in
    /// [`DebugEvent::BreakpointHit`]. The execution deadline includes time spent
    /// paused. Only the entry chunk is supported; stepping, local inspection,
    /// nested module scripts, and coroutine debugging are not implemented.
    /// Poll [`DebugExecutionHandle::try_event`] and [`DebugExecutionHandle::try_result`]
    /// from the UI loop. Do not wait on a paused session from the UI thread.
    pub fn start_debug(
        &self,
        document: &ScriptDocument,
        breakpoint_lines: &[u32],
        limits: ExecutionLimits,
    ) -> Result<DebugExecutionHandle, ExecutionServiceError> {
        validate_limits(limits)?;
        if document.source.len() > MAX_SOURCE_BYTES {
            return Err(ExecutionServiceError::SourceTooLarge {
                actual: document.source.len(),
                limit: MAX_SOURCE_BYTES,
            });
        }
        if breakpoint_lines.len() > MAX_DEBUG_BREAKPOINTS {
            return Err(ExecutionServiceError::TooManyBreakpoints {
                actual: breakpoint_lines.len(),
                limit: MAX_DEBUG_BREAKPOINTS,
            });
        }
        let mut breakpoint_lines = breakpoint_lines.to_vec();
        breakpoint_lines.sort_unstable();
        breakpoint_lines.dedup();
        if let Some(line) = breakpoint_lines
            .iter()
            .copied()
            .find(|line| *line == 0 || *line > i32::MAX as u32)
        {
            return Err(ExecutionServiceError::InvalidBreakpointLine(line));
        }
        self.reserve_worker()?;

        let slot = WorkerSlot(Arc::clone(&self.active_workers));
        let (event_sender, event_receiver) = mpsc::sync_channel(1);
        let (result_sender, result_receiver) = mpsc::channel();
        let control = Arc::new(NativeCancellation::new()?);
        let control_for_worker = Arc::clone(&control);
        let document = document.clone();
        let timeout_millis = limits.timeout.as_millis() as u64;
        let memory_limit_bytes = limits.vm_memory_bytes;

        let worker = thread::Builder::new()
            .name("studio-luau-debug-worker".to_owned())
            .spawn(move || {
                let result = compile_and_debug_run(
                    &document,
                    &breakpoint_lines,
                    timeout_millis,
                    memory_limit_bytes,
                    &control_for_worker,
                    &event_sender,
                );
                let _ = result_sender.send(result);
                drop(slot);
            })?;

        Ok(DebugExecutionHandle {
            event_receiver,
            result_receiver,
            worker: Some(worker),
            control,
            completed: false,
            paused: false,
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

/// Nonblocking control for a bounded language-only debug session.
pub struct DebugExecutionHandle {
    event_receiver: Receiver<DebugEvent>,
    result_receiver: Receiver<ExecutionResult>,
    worker: Option<JoinHandle<()>>,
    control: Arc<NativeCancellation>,
    completed: bool,
    paused: bool,
}

impl DebugExecutionHandle {
    /// Request cancellation. Cancellation wakes a session paused at a breakpoint.
    pub fn cancel(&self) {
        self.control.cancel();
    }

    /// Read a breakpoint event without waiting.
    pub fn try_event(&mut self) -> Option<DebugEvent> {
        if self.completed || self.paused {
            return None;
        }
        match self.event_receiver.try_recv() {
            Ok(event) => {
                self.paused = true;
                Some(event)
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    /// Resume after receiving a breakpoint event.
    pub fn resume(&mut self) -> bool {
        if !self.paused || self.completed {
            return false;
        }
        self.paused = false;
        self.control.resume();
        true
    }

    /// Check for a terminal result without waiting.
    pub fn try_result(&mut self) -> Option<ExecutionResult> {
        if self.completed {
            return None;
        }
        match self.result_receiver.try_recv() {
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
                    "Luau debug worker exited without returning a result",
                ))
            }
        }
    }

    fn release_worker_handle_without_waiting(&mut self) {
        if let Some(worker) = self.worker.take()
            && worker.is_finished()
        {
            let _ = worker.join();
        }
    }
}

impl Drop for DebugExecutionHandle {
    fn drop(&mut self) {
        if !self.completed {
            self.control.cancel();
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
    #[error("debug sessions support at most {limit} breakpoints, got {actual}")]
    TooManyBreakpoints { actual: usize, limit: usize },
    #[error("debug breakpoint line must be between 1 and 2147483647, got {0}")]
    InvalidBreakpointLine(u32),
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

// SAFETY: cancellation uses an atomic flag and debugger commands/events use
// native mutex and condition-variable synchronization.
unsafe impl Send for NativeCancellation {}
// SAFETY: all cross-thread operations are synchronized in the native control.
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
        // function sets the cancellation flag and wakes any debugger pause.
        unsafe { studio_luau_control_cancel(self.raw.as_ptr()) };
    }

    fn resume(&self) {
        // SAFETY: the pointer stays valid while this Arc is alive; the native
        // function updates the resume command under its debugger mutex.
        unsafe { studio_luau_control_resume(self.raw.as_ptr()) };
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

struct CompletionResultGuard(*mut NativeCompletionResult);

impl Drop for CompletionResultGuard {
    fn drop(&mut self) {
        // SAFETY: the bridge accepts its own initialized completion result and
        // frees every label and optional insertion string returned by Luau.
        unsafe { studio_luau_completion_result_free(self.0) };
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

unsafe extern "C" fn debug_event_callback(user_data: *mut c_void, line: u32) {
    if user_data.is_null() {
        return;
    }
    // SAFETY: `compile_and_debug_run` passes a live SyncSender pointer for the
    // full native call; the callback never retains the pointer.
    let sender = unsafe { &*user_data.cast::<SyncSender<DebugEvent>>() };
    let _ = sender.try_send(DebugEvent::BreakpointHit { line });
}

fn compile_and_debug_run(
    document: &ScriptDocument,
    breakpoint_lines: &[u32],
    timeout_millis: u64,
    memory_limit_bytes: u64,
    control: &NativeCancellation,
    event_sender: &SyncSender<DebugEvent>,
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
    let event_user_data = (event_sender as *const SyncSender<DebugEvent>)
        .cast_mut()
        .cast::<c_void>();
    // SAFETY: bytecode, breakpoint lines, event sender, and control remain
    // alive for the full call; the native result is released below.
    let bridge_status = unsafe {
        studio_luau_debug_run(
            compiled.bytecode.as_ptr(),
            compiled.bytecode.len(),
            breakpoint_lines.as_ptr(),
            breakpoint_lines.len(),
            memory_limit_bytes,
            timeout_millis,
            control.raw.as_ptr(),
            debug_event_callback,
            event_user_data,
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
        6 => ExecutionStatus::DebuggerError(format!("breakpoint line {line}: {message}")),
        _ => ExecutionStatus::InternalError(if message.is_empty() {
            format!("native debugger bridge returned status {bridge_status}")
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
            module_id: native_optional_message(entry.module_id),
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
        9 => DiagnosticKind::AnalysisLimit,
        _ => DiagnosticKind::Internal,
    }
}

fn completion_context(context: i32) -> CompletionContext {
    match context {
        1 => CompletionContext::Expression,
        2 => CompletionContext::Statement,
        3 => CompletionContext::Property,
        4 => CompletionContext::Type,
        5 => CompletionContext::Keyword,
        6 => CompletionContext::String,
        7 => CompletionContext::HotComment,
        _ => CompletionContext::Unknown,
    }
}

fn completion_kind(kind: i32) -> CompletionKind {
    match kind {
        1 => CompletionKind::Property,
        2 => CompletionKind::Binding,
        3 => CompletionKind::Keyword,
        4 => CompletionKind::String,
        5 => CompletionKind::Type,
        6 => CompletionKind::Module,
        7 => CompletionKind::GeneratedFunction,
        8 => CompletionKind::RequirePath,
        9 => CompletionKind::HotComment,
        _ => CompletionKind::Unknown,
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

fn native_optional_message(message: *const c_char) -> Option<String> {
    if message.is_null() {
        return None;
    }
    Some(native_message(message))
}

fn runtime_diagnostic(line: u32, message: String) -> Diagnostic {
    Diagnostic {
        module_id: None,
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

    fn project_nodes() -> Vec<ProjectNode> {
        vec![
            ProjectNode::container("DataModel", "game", None),
            ProjectNode::container(
                "ReplicatedStorage",
                "ReplicatedStorage",
                Some("DataModel".to_owned()),
            ),
            ProjectNode::module_script(
                "SharedModule",
                "Shared",
                "ReplicatedStorage",
                "return { answer = 42 }\n",
            ),
            ProjectNode::script(
                "MainScript",
                "Main",
                "ReplicatedStorage",
                "local shared = require(script.Parent.Shared)\nprint(shared.answer)\n",
            ),
        ]
    }

    fn run(source: &str) -> ExecutionResult {
        ExecutionService::default()
            .start(&ScriptDocument::new(source), ExecutionLimits::default())
            .expect("worker should start")
            .wait()
    }

    fn wait_for_debug_event(handle: &mut DebugExecutionHandle) -> DebugEvent {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(event) = handle.try_event() {
                return event;
            }
            if let Some(result) = handle.try_result() {
                panic!("debug session ended before breakpoint: {result:?}");
            }
            assert!(
                std::time::Instant::now() < deadline,
                "debug event timed out"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn wait_for_debug_result(handle: &mut DebugExecutionHandle) -> ExecutionResult {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(result) = handle.try_result() {
                return result;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "debug result timed out"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn format_preview_is_repeatable_and_does_not_mutate_the_document() {
        let document = ScriptDocument::new(
            "--!strict\nlocal function add( left : number,right:number ):number return left+right end\nprint(add(2,3))\n",
        );
        let original = document.clone();

        let preview = format_preview(&document).expect("valid Luau should have a preview");
        assert!(preview.changed);
        assert_eq!(document, original);
        compile(&ScriptDocument::new(preview.formatted_source.clone()))
            .expect("the pinned Luau compiler should accept formatted output");

        let second = format_preview(&ScriptDocument::new(preview.formatted_source.clone()))
            .expect("formatted source should remain formattable");
        assert!(!second.changed);
        assert_eq!(second.formatted_source, preview.formatted_source);
    }

    #[test]
    fn format_preview_keeps_crlf_and_does_not_sort_requires() {
        let document = ScriptDocument::new(
            "local z = require(\"z\")\r\nlocal a = require(\"a\")\r\nprint(z, a)\r\n",
        );
        let preview = format_preview(&document).expect("uniform CRLF Luau should format");

        assert!(preview.formatted_source.contains("\r\n"));
        assert!(!preview.formatted_source.replace("\r\n", "").contains('\n'));
        assert!(
            preview.formatted_source.find("local z").unwrap()
                < preview.formatted_source.find("local a").unwrap()
        );
        assert!(document.source().contains("local z = require(\"z\")\r\n"));
    }

    #[test]
    fn format_preview_rejects_invalid_source_and_mixed_line_endings_without_edits() {
        let invalid = ScriptDocument::new("local value = \n");
        let original_invalid = invalid.clone();
        assert!(matches!(
            format_preview(&invalid),
            Err(FormatError::InvalidLuau(_))
        ));
        assert_eq!(invalid, original_invalid);

        let mixed = ScriptDocument::new("local first = 1\r\nlocal second = 2\n");
        let original_mixed = mixed.clone();
        assert_eq!(
            format_preview(&mixed),
            Err(FormatError::UnsupportedLineEndings)
        );
        assert_eq!(mixed, original_mixed);
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
    fn project_snapshot_rejects_duplicate_ids_before_native_analysis() {
        let mut nodes = project_nodes();
        nodes.push(ProjectNode::container(
            "MainScript",
            "Duplicate",
            Some("ReplicatedStorage".to_owned()),
        ));
        let error = validate_project_snapshot("DataModel", "MainScript", &nodes)
            .expect_err("duplicate project IDs must not cross the FFI boundary");
        assert!(
            matches!(error, AnalysisFailure::InvalidProject(message) if message.contains("duplicate node ID"))
        );
    }

    #[test]
    fn project_snapshot_rejects_disconnected_hierarchies_before_native_analysis() {
        let mut nodes = project_nodes();
        nodes.push(ProjectNode::container("Detached", "Detached", None));
        let error = validate_project_snapshot("DataModel", "MainScript", &nodes)
            .expect_err("disconnected project nodes must be rejected");
        assert!(
            matches!(error, AnalysisFailure::InvalidProject(message) if message.contains("only the declared project root"))
        );
    }

    #[test]
    fn project_snapshot_bounds_combined_source_before_native_analysis() {
        let mut nodes = project_nodes();
        let large_source = " ".repeat(MAX_SOURCE_BYTES);
        nodes[2].source = Some(ScriptDocument::new(large_source.clone()));
        nodes[3].source = Some(ScriptDocument::new(large_source.clone()));
        for index in 0..7 {
            nodes.push(ProjectNode::module_script(
                format!("LargeModule{index}"),
                format!("Large{index}"),
                "ReplicatedStorage",
                large_source.clone(),
            ));
        }
        let error = validate_project_snapshot("DataModel", "MainScript", &nodes)
            .expect_err("combined project source must stay below its limit");
        assert!(matches!(
            error,
            AnalysisFailure::ProjectSourcesTooLarge { .. }
        ));
    }

    #[test]
    fn project_analysis_resolves_static_instance_paths_and_keeps_module_identity() {
        let mut nodes = project_nodes();
        nodes[2].source = Some(ScriptDocument::new("local = false\n"));
        let diagnostics = analyze_project("DataModel", "MainScript", &nodes)
            .expect("valid project snapshots should be analyzed");

        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.kind == DiagnosticKind::Syntax
                    && diagnostic.module_id.as_deref() == Some("SharedModule")
            }),
            "diagnostics: {diagnostics:#?}"
        );
    }

    #[test]
    fn project_analysis_resolves_documented_static_require_string_paths() {
        for require_path in ["./Shared", "@game/ReplicatedStorage/Shared"] {
            let mut nodes = project_nodes();
            nodes[2].source = Some(ScriptDocument::new("local = false\n"));
            nodes[3].source = Some(ScriptDocument::new(format!("require({require_path:?})\n")));

            let diagnostics = analyze_project("DataModel", "MainScript", &nodes)
                .expect("valid project snapshots should be analyzed");

            assert!(
                diagnostics.iter().any(|diagnostic| {
                    diagnostic.kind == DiagnosticKind::Syntax
                        && diagnostic.module_id.as_deref() == Some("SharedModule")
                }),
                "require path {require_path:?} should resolve the ModuleScript"
            );
        }

        let mut self_path_nodes = project_nodes();
        self_path_nodes[2].source = Some(ScriptDocument::new("local = false\n"));
        self_path_nodes[3].source = Some(ScriptDocument::new("require(\"@self/Private\")\n"));
        self_path_nodes.push(ProjectNode::module_script(
            "PrivateModule",
            "Private",
            "MainScript",
            "local = false\n",
        ));
        let diagnostics = analyze_project("DataModel", "MainScript", &self_path_nodes)
            .expect("@self paths should be analyzed");
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.kind == DiagnosticKind::Syntax
                    && diagnostic.module_id.as_deref() == Some("PrivateModule")
            }),
            "@self path should resolve relative to the entry script"
        );

        let mut parent_path_nodes = project_nodes();
        parent_path_nodes[2].source = Some(ScriptDocument::new("local = false\n"));
        parent_path_nodes[3].source = Some(ScriptDocument::new("require(\"../RootSibling\")\n"));
        parent_path_nodes.push(ProjectNode::module_script(
            "RootSibling",
            "RootSibling",
            "DataModel",
            "local = false\n",
        ));
        let diagnostics = analyze_project("DataModel", "MainScript", &parent_path_nodes)
            .expect("parent paths should be analyzed");
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.kind == DiagnosticKind::Syntax
                    && diagnostic.module_id.as_deref() == Some("RootSibling")
            }),
            "../ path should resolve from script.Parent.Parent"
        );
    }

    #[test]
    fn project_analysis_resolves_static_child_lookup_methods() {
        for require_expression in [
            "script.Parent:WaitForChild(\"Shared\")",
            "script.Parent:FindFirstChild(\"Shared\")",
        ] {
            let mut nodes = project_nodes();
            nodes[2].source = Some(ScriptDocument::new("local = false\n"));
            nodes[3].source = Some(ScriptDocument::new(format!(
                "require({require_expression})\n"
            )));
            let diagnostics = analyze_project("DataModel", "MainScript", &nodes)
                .expect("static child lookups should be analyzed");

            assert!(
                diagnostics.iter().any(|diagnostic| {
                    diagnostic.kind == DiagnosticKind::Syntax
                        && diagnostic.module_id.as_deref() == Some("SharedModule")
                }),
                "lookup expression {require_expression:?} should resolve the ModuleScript"
            );
        }

        let mut recursive_nodes = project_nodes();
        recursive_nodes[2].source = Some(ScriptDocument::new("local = false\n"));
        recursive_nodes[3].source = Some(ScriptDocument::new(
            "require(script.Parent:FindFirstChild(\"Deep\", true))\n",
        ));
        recursive_nodes.push(ProjectNode::container(
            "NestedFolder",
            "Nested",
            Some("ReplicatedStorage".to_owned()),
        ));
        recursive_nodes.push(ProjectNode::module_script(
            "DeepModule",
            "Deep",
            "NestedFolder",
            "local = false\n",
        ));
        let diagnostics = analyze_project("DataModel", "MainScript", &recursive_nodes)
            .expect("recursive static child lookup should be analyzed");
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.kind == DiagnosticKind::Syntax
                    && diagnostic.module_id.as_deref() == Some("DeepModule")
            }),
            "recursive FindFirstChild should resolve nested ModuleScripts"
        );
    }

    #[test]
    fn project_analysis_reports_when_static_resolution_reaches_its_work_limit() {
        let mut nodes = project_nodes();
        nodes[2].source = Some(ScriptDocument::new("return true\n"));
        nodes[3].source = Some(ScriptDocument::new(
            "require(script.Parent:FindFirstChild(\"Target\", true))\n".repeat(300),
        ));
        nodes.push(ProjectNode::module_script(
            "TargetModule",
            "Target",
            "ReplicatedStorage",
            "return true\n",
        ));
        for index in 0..1_000 {
            nodes.push(ProjectNode::container(
                format!("Container{index}"),
                format!("Container{index}"),
                Some("ReplicatedStorage".to_owned()),
            ));
        }

        let diagnostics = analyze_project("DataModel", "MainScript", &nodes)
            .expect("bounded resolver exhaustion should be a diagnostic");

        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind == DiagnosticKind::AnalysisLimit),
            "diagnostics: {diagnostics:#?}"
        );
    }

    #[test]
    fn project_analysis_does_not_guess_ambiguous_module_names() {
        let mut nodes = project_nodes();
        nodes[2].source = Some(ScriptDocument::new("local = false\n"));
        nodes[3].source = Some(ScriptDocument::new("require(script.Parent.Shared)\n"));
        nodes.push(ProjectNode::module_script(
            "DuplicateSharedModule",
            "Shared",
            "ReplicatedStorage",
            "local = also_false\n",
        ));

        let diagnostics = analyze_project("DataModel", "MainScript", &nodes)
            .expect("ambiguous paths should remain unresolved without failing analysis");

        assert!(
            diagnostics.iter().all(|diagnostic| {
                !matches!(
                    diagnostic.module_id.as_deref(),
                    Some("SharedModule" | "DuplicateSharedModule")
                )
            }),
            "ambiguous paths must not load either matching module: {diagnostics:#?}"
        );
    }

    #[test]
    fn project_analysis_does_not_resolve_non_module_targets() {
        let mut nodes = project_nodes();
        nodes[2].source = Some(ScriptDocument::new("local = false\n"));
        nodes[3].source = Some(ScriptDocument::new("require(script.Parent.OtherScript)\n"));
        nodes.push(ProjectNode::script(
            "OtherScript",
            "OtherScript",
            "ReplicatedStorage",
            "local = also_false\n",
        ));

        let diagnostics = analyze_project("DataModel", "MainScript", &nodes)
            .expect("non-ModuleScript targets should remain unresolved");

        assert!(
            diagnostics.iter().all(|diagnostic| {
                !matches!(
                    diagnostic.module_id.as_deref(),
                    Some("SharedModule" | "OtherScript")
                )
            }),
            "require must not load a normal Script as a ModuleScript: {diagnostics:#?}"
        );
    }

    #[test]
    fn project_analysis_does_not_require_the_non_module_entry_script() {
        let mut nodes = project_nodes();
        nodes[3].source = Some(ScriptDocument::new("--!strict\nrequire(script)\n"));

        let diagnostics = analyze_project("DataModel", "MainScript", &nodes)
            .expect("the root script should still be analyzed");

        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.kind == DiagnosticKind::Type
                    && diagnostic.module_id.as_deref() == Some("MainScript")
            }),
            "a normal Script entry must not become a ModuleScript dependency: {diagnostics:#?}"
        );
    }

    #[test]
    fn project_analysis_leaves_dynamic_and_asset_id_requires_unresolved() {
        for require_expression in ["loadModule()", "123456"] {
            let mut nodes = project_nodes();
            nodes[2].source = Some(ScriptDocument::new("local = false\n"));
            nodes[3].source = Some(ScriptDocument::new(format!(
                "require({require_expression})\n"
            )));

            let diagnostics = analyze_project("DataModel", "MainScript", &nodes)
                .expect("unsupported require forms should remain unresolved");

            assert!(
                diagnostics
                    .iter()
                    .all(|diagnostic| { diagnostic.module_id.as_deref() != Some("SharedModule") }),
                "require expression {require_expression:?} must not load a guessed module"
            );
        }
    }

    #[test]
    fn project_analysis_resolves_static_service_aliases() {
        let mut nodes = project_nodes();
        nodes[2].source = Some(ScriptDocument::new("local = false\n"));
        nodes[3].source = Some(ScriptDocument::new(
            "local storage = game:GetService(\"ReplicatedStorage\")\nrequire(storage.Shared)\n",
        ));

        let diagnostics = analyze_project("DataModel", "MainScript", &nodes)
            .expect("valid project snapshots should be analyzed");

        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.kind == DiagnosticKind::Syntax
                    && diagnostic.module_id.as_deref() == Some("SharedModule")
            }),
            "diagnostics: {diagnostics:#?}"
        );
    }

    #[test]
    fn project_analysis_loads_the_pinned_partial_engine_types_and_luau_builtins() {
        let mut nodes = project_nodes();
        nodes[3].source = Some(ScriptDocument::new(
            "--!strict\nlocal parent: Instance? = script.Parent\nlocal storage: Instance = game:GetService(\"ReplicatedStorage\")\nprint(parent, storage.Name)\n",
        ));

        let diagnostics = analyze_project("DataModel", "MainScript", &nodes)
            .expect("valid project snapshots should be analyzed");

        assert!(diagnostics.is_empty(), "diagnostics: {diagnostics:#?}");
    }

    #[test]
    fn project_autocomplete_uses_the_pinned_engine_types_without_mutating_source() {
        let mut nodes = project_nodes();
        let source = "--!strict\nlocal instance: Instance\ninstance.\n";
        nodes[3].source = Some(ScriptDocument::new(source));

        let completions = complete_project(
            "DataModel",
            "MainScript",
            &nodes,
            SourcePosition { line: 2, column: 9 },
        )
        .expect("a cursor after an Instance member separator should autocomplete");

        assert!(
            completions
                .items
                .iter()
                .any(|item| item.label == "FindFirstChild"),
            "completions: {completions:#?}"
        );
        assert!(
            completions
                .items
                .iter()
                .any(|item| item.label == "Name" && item.kind == CompletionKind::Property),
            "completions: {completions:#?}"
        );
        assert_eq!(nodes[3].source.as_ref().unwrap().source(), source);
    }

    #[test]
    fn project_autocomplete_rejects_cursor_positions_inside_utf8_characters() {
        let mut nodes = project_nodes();
        let source = "local value = \"🌍\"\n";
        nodes[3].source = Some(ScriptDocument::new(source));
        let emoji_column = source.find('🌍').unwrap() as u32 + 1;

        let error = complete_project(
            "DataModel",
            "MainScript",
            &nodes,
            SourcePosition {
                line: 0,
                column: emoji_column,
            },
        )
        .expect_err("a cursor inside a UTF-8 code point must be rejected");

        assert!(matches!(
            error,
            AnalysisFailure::InvalidCursorPosition { .. }
        ));
    }

    #[test]
    fn local_definition_navigation_resolves_a_reference_to_its_declaration() {
        let document = ScriptDocument::new("local answer = 41\nprint(answer)\n");
        let definition = find_local_definition(&document, SourcePosition { line: 1, column: 8 })
            .expect("local reference lookup should succeed")
            .expect("the cursor is on a local reference");

        assert_eq!(definition.start_line, 1);
        assert_eq!(definition.start_column, 7);
        assert_eq!(definition.end_line, 1);
        assert_eq!(definition.end_column, 13);
    }

    #[test]
    fn local_definition_navigation_respects_shadowing_and_ignores_globals() {
        let shadowed =
            ScriptDocument::new("local value = 1\nfunction read(value)\n    return value\nend\n");
        let definition = find_local_definition(
            &shadowed,
            SourcePosition {
                line: 2,
                column: 12,
            },
        )
        .expect("parameter lookup should succeed")
        .expect("the cursor is on a parameter reference");
        assert_eq!(definition.start_line, 2);
        assert_eq!(definition.start_column, 15);

        let global = ScriptDocument::new("print(1)\n");
        let definition = find_local_definition(&global, SourcePosition { line: 0, column: 1 })
            .expect("global lookup should succeed");
        assert_eq!(definition, None);
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
    fn language_debugger_pauses_at_entry_breakpoint_and_continues() {
        let source = "local value = 1\nprint(\"before\")\nvalue = value + 1\nprint(value)\n";
        let service = ExecutionService::default();
        let mut session = service
            .start_debug(
                &ScriptDocument::new(source),
                &[3, 3],
                ExecutionLimits::default(),
            )
            .expect("debug worker should start");

        assert_eq!(
            wait_for_debug_event(&mut session),
            DebugEvent::BreakpointHit { line: 3 }
        );
        assert!(session.resume());
        assert!(
            !session.resume(),
            "resume is accepted only after a stop event"
        );
        let result = wait_for_debug_result(&mut session);
        assert_eq!(result.status, ExecutionStatus::Completed);
        assert_eq!(result.output, "before\n2\n");
    }

    #[test]
    fn language_debugger_cancellation_wakes_a_paused_session() {
        let service = ExecutionService::default();
        let limits = ExecutionLimits {
            timeout: Duration::from_secs(5),
            ..ExecutionLimits::default()
        };
        for _ in 0..16 {
            let mut session = service
                .start_debug(
                    &ScriptDocument::new("print(\"before\")\nprint(\"after\")\n"),
                    &[2],
                    limits,
                )
                .expect("debug worker should start");
            assert_eq!(
                wait_for_debug_event(&mut session),
                DebugEvent::BreakpointHit { line: 2 }
            );

            let cancelled_at = std::time::Instant::now();
            session.cancel();
            let result = wait_for_debug_result(&mut session);
            assert_eq!(result.status, ExecutionStatus::Cancelled);
            assert_eq!(result.output, "before\n");
            assert!(
                cancelled_at.elapsed() < Duration::from_secs(1),
                "cancellation should wake a paused worker promptly"
            );
        }
    }

    #[test]
    fn language_debugger_deadline_includes_paused_time() {
        let service = ExecutionService::default();
        let limits = ExecutionLimits {
            timeout: Duration::from_millis(250),
            ..ExecutionLimits::default()
        };
        let mut session = service
            .start_debug(
                &ScriptDocument::new("print(\"before\")\nprint(\"after\")\n"),
                &[2],
                limits,
            )
            .expect("debug worker should start");
        assert_eq!(
            wait_for_debug_event(&mut session),
            DebugEvent::BreakpointHit { line: 2 }
        );
        thread::sleep(Duration::from_millis(300));

        let result = wait_for_debug_result(&mut session);
        assert_eq!(result.status, ExecutionStatus::TimedOut);
        assert_eq!(result.output, "before\n");
    }

    #[test]
    fn language_debugger_rejects_invalid_breakpoint_lines_before_scheduling() {
        let service = ExecutionService::default();
        assert!(matches!(
            service.start_debug(
                &ScriptDocument::new("return\n"),
                &[0],
                ExecutionLimits::default()
            ),
            Err(ExecutionServiceError::InvalidBreakpointLine(0))
        ));
        assert!(matches!(
            service.start_debug(
                &ScriptDocument::new("return\n"),
                &[1; MAX_DEBUG_BREAKPOINTS + 1],
                ExecutionLimits::default()
            ),
            Err(ExecutionServiceError::TooManyBreakpoints { .. })
        ));
    }

    #[test]
    fn language_debugger_reports_lines_without_executable_code() {
        let service = ExecutionService::default();
        let mut session = service
            .start_debug(
                &ScriptDocument::new("return\n"),
                &[99],
                ExecutionLimits::default(),
            )
            .expect("in-range breakpoint request should be scheduled");
        let result = wait_for_debug_result(&mut session);
        assert!(matches!(
            result.status,
            ExecutionStatus::DebuggerError(message) if message.contains("line 99")
        ));
    }

    #[test]
    fn language_debugger_discards_queued_events_after_terminal_result() {
        let service = ExecutionService::default();
        let limits = ExecutionLimits {
            timeout: Duration::from_millis(250),
            ..ExecutionLimits::default()
        };
        let mut session = service
            .start_debug(
                &ScriptDocument::new("print(\"before\")\nprint(\"after\")\n"),
                &[2],
                limits,
            )
            .expect("debug worker should start");

        let result = wait_for_debug_result(&mut session);
        assert_eq!(result.status, ExecutionStatus::TimedOut);
        assert!(session.try_event().is_none());
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
