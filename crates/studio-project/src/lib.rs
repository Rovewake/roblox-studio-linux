//! A guarded, Linux-native project data layer for the Studio reimplementation.
//!
//! `rbx-dom` supplies the current in-memory Roblox value model and file
//! serializers. This crate owns the file boundary, explicit compatibility
//! limits, safe replacement behavior, and a semantic round-trip check.

mod format_guard;

#[cfg(test)]
mod tests;

use std::{
    collections::HashMap,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use rbx_dom_weak::{
    Instance, InstanceBuilder, WeakDom,
    types::{Ref, Variant},
    ustr,
};
use rustix::{fs::RenameFlags, io::Errno};
use thiserror::Error;

use crate::format_guard::{validate_binary, validate_xml};

static TEMPORARY_FILE_ID: AtomicU64 = AtomicU64::new(0);

/// The four local Roblox place/model encodings supported by this data layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileFormat {
    /// Binary place file (`.rbxl`).
    PlaceBinary,
    /// XML place file (`.rbxlx`).
    PlaceXml,
    /// Binary model file (`.rbxm`).
    ModelBinary,
    /// XML model file (`.rbxmx`).
    ModelXml,
}

impl FileFormat {
    /// Determine a file format from a path's extension.
    pub fn from_path(path: &Path) -> Result<Self, ProjectError> {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .ok_or_else(|| ProjectError::UnsupportedExtension(path.display().to_string()))?
            .to_ascii_lowercase();

        match extension.as_str() {
            "rbxl" => Ok(Self::PlaceBinary),
            "rbxlx" => Ok(Self::PlaceXml),
            "rbxm" => Ok(Self::ModelBinary),
            "rbxmx" => Ok(Self::ModelXml),
            _ => Err(ProjectError::UnsupportedExtension(
                path.display().to_string(),
            )),
        }
    }

    /// The canonical lower-case extension, without a leading dot.
    pub const fn extension(self) -> &'static str {
        match self {
            Self::PlaceBinary => "rbxl",
            Self::PlaceXml => "rbxlx",
            Self::ModelBinary => "rbxm",
            Self::ModelXml => "rbxmx",
        }
    }

    const fn is_binary(self) -> bool {
        matches!(self, Self::PlaceBinary | Self::ModelBinary)
    }

    const fn is_xml(self) -> bool {
        matches!(self, Self::PlaceXml | Self::ModelXml)
    }
}

/// A stable ID wrapper for an instance within one open project document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InstanceId(Ref);

impl InstanceId {
    /// The library-level referent, for integrations that need `rbx-dom` APIs.
    pub const fn as_ref(self) -> Ref {
        self.0
    }
}

/// A typed property value supported by the pinned `rbx-dom` data model.
pub type PropertyValue = Variant;

/// Resource limits for reading untrusted or unusually large local files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadLimits {
    /// Maximum compressed/input file size. Defaults to 512 MiB.
    pub max_file_bytes: u64,
    /// Maximum combined decompressed binary chunk size. Defaults to 512 MiB.
    pub max_uncompressed_bytes: u64,
    /// Maximum number of serialized instances. Defaults to one million.
    pub max_instances: u32,
    /// Maximum sequence keypoints stored in one property value. Defaults to 65,536.
    pub max_property_elements: u32,
    /// Maximum nesting depth for XML elements. Defaults to 1024.
    pub max_xml_depth: usize,
    /// Maximum number of binary chunks. Defaults to one million.
    pub max_binary_chunks: u32,
}

impl Default for ReadLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 512 * 1024 * 1024,
            max_uncompressed_bytes: 512 * 1024 * 1024,
            max_instances: 1_000_000,
            max_property_elements: 65_536,
            max_xml_depth: 1024,
            max_binary_chunks: 1_000_000,
        }
    }
}

/// Errors returned while reading, validating, or saving a project document.
#[derive(Debug, Error)]
pub enum ProjectError {
    /// A filesystem operation failed.
    #[error("filesystem operation failed: {0}")]
    Io(#[from] io::Error),

    /// The path extension is not one of the four supported formats.
    #[error("unsupported project path or extension: {0}")]
    UnsupportedExtension(String),

    /// The file bytes do not use the encoding implied by the extension.
    #[error("project extension .{extension} does not match detected {detected} data")]
    FormatMismatch {
        /// Extension-derived format expected for the file.
        extension: &'static str,
        /// Detected byte encoding.
        detected: &'static str,
    },

    /// A configured read limit was exceeded.
    #[error("project exceeds configured {resource} limit ({actual} > {limit})")]
    LimitExceeded {
        /// Resource limit that was exceeded.
        resource: &'static str,
        /// Observed amount.
        actual: u64,
        /// Configured maximum.
        limit: u64,
    },

    /// The file is structurally invalid or outside this implementation's
    /// explicitly supported subset.
    #[error("project file is not safely supported: {0}")]
    UnsupportedData(String),

    /// A format decoder rejected the input.
    #[error("project format decode failed: {0}")]
    Decode(#[from] DecodeFailure),

    /// A format encoder rejected a value in the project model.
    #[error("project format encode failed: {0}")]
    Encode(#[from] EncodeFailure),

    /// A saved temporary file did not preserve the in-memory semantic model.
    #[error("serialized project did not match the in-memory semantic model")]
    SemanticRoundTripMismatch,

    /// A requested instance ID is not part of this document.
    #[error("instance {0:?} is not part of this project")]
    MissingInstance(InstanceId),

    /// A save destination already exists but is not a regular file.
    #[error("save destination is not a regular file: {0}")]
    InvalidDestination(String),

    /// The save destination changed after the save operation captured it.
    #[error("save destination changed while the project was being saved: {0}")]
    ConcurrentModification(String),
}

/// The binary/XML decoder errors kept as distinct source types.
#[derive(Debug, Error)]
pub enum DecodeFailure {
    /// Binary parser error.
    #[error(transparent)]
    Binary(#[from] rbx_binary::DecodeError),
    /// XML parser error.
    #[error(transparent)]
    Xml(#[from] rbx_xml::DecodeError),
}

/// The binary/XML encoder errors kept as distinct source types.
#[derive(Debug, Error)]
pub enum EncodeFailure {
    /// Binary serializer error.
    #[error(transparent)]
    Binary(#[from] rbx_binary::EncodeError),
    /// XML serializer error.
    #[error(transparent)]
    Xml(#[from] rbx_xml::EncodeError),
}

/// A completed atomic save and its recovery copy, if one was needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveReceipt {
    /// The destination path that was atomically replaced or created.
    pub path: PathBuf,
    /// A unique adjacent copy of the previous file, when the destination existed.
    pub backup_path: Option<PathBuf>,
    /// Number of serialized bytes written.
    pub bytes_written: u64,
}

/// A parsed project tree plus the file-format adapter that opened it.
///
/// The root is always a synthetic `DataModel`; the serialized roots are its
/// ordered children. Instance IDs are only stable for the lifetime of this
/// document and must not be persisted outside it.
pub struct ProjectDocument {
    dom: WeakDom,
    format: FileFormat,
    limits: ReadLimits,
}

impl ProjectDocument {
    /// Construct an empty local project or model tree.
    pub fn new(format: FileFormat) -> Self {
        Self {
            dom: WeakDom::new(InstanceBuilder::new("DataModel")),
            format,
            limits: ReadLimits::default(),
        }
    }

    /// Open a supported file using the default resource limits.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ProjectError> {
        Self::open_with_limits(path, ReadLimits::default())
    }

    /// Open a supported file using caller-selected resource limits.
    pub fn open_with_limits(
        path: impl AsRef<Path>,
        limits: ReadLimits,
    ) -> Result<Self, ProjectError> {
        let path = path.as_ref();
        let format = FileFormat::from_path(path)?;
        let bytes = read_bounded(path, limits.max_file_bytes)?;
        Self::from_bytes_with_limits(format, &bytes, limits)
    }

    /// Decode bytes whose place/model format is supplied by the caller.
    pub fn from_bytes(format: FileFormat, bytes: &[u8]) -> Result<Self, ProjectError> {
        Self::from_bytes_with_limits(format, bytes, ReadLimits::default())
    }

    /// Decode bytes under explicit resource limits.
    pub fn from_bytes_with_limits(
        format: FileFormat,
        bytes: &[u8],
        limits: ReadLimits,
    ) -> Result<Self, ProjectError> {
        if bytes.len() as u64 > limits.max_file_bytes {
            return Err(ProjectError::LimitExceeded {
                resource: "input byte",
                actual: bytes.len() as u64,
                limit: limits.max_file_bytes,
            });
        }

        match detect_encoding(bytes)? {
            Encoding::Binary if format.is_binary() => {
                let database = rbx_reflection_database::get_bundled();
                validate_binary(bytes, limits, database)?;
                let dom = rbx_binary::Deserializer::new()
                    .reflection_database(database)
                    .deserialize(bytes)
                    .map_err(DecodeFailure::Binary)?;
                Ok(Self {
                    dom,
                    format,
                    limits,
                })
            }
            Encoding::Xml if format.is_xml() => {
                validate_xml(bytes, limits)?;
                let database = rbx_reflection_database::get_bundled();
                let options = rbx_xml::DecodeOptions::new()
                    .reflection_database(database)
                    .property_behavior(rbx_xml::DecodePropertyBehavior::ReadUnknown);
                let dom = rbx_xml::from_reader(bytes, options).map_err(DecodeFailure::Xml)?;
                Ok(Self {
                    dom,
                    format,
                    limits,
                })
            }
            Encoding::Binary => Err(ProjectError::FormatMismatch {
                extension: format.extension(),
                detected: "binary",
            }),
            Encoding::Xml => Err(ProjectError::FormatMismatch {
                extension: format.extension(),
                detected: "XML",
            }),
        }
    }

    /// The encoding and place/model mode originally selected for this document.
    pub const fn format(&self) -> FileFormat {
        self.format
    }

    /// The document's synthetic root identity.
    pub fn root_id(&self) -> InstanceId {
        InstanceId(self.dom.root_ref())
    }

    /// The ordered top-level instances that will be serialized.
    pub fn root_children(&self) -> impl ExactSizeIterator<Item = InstanceId> + '_ {
        self.dom.root().children().iter().copied().map(InstanceId)
    }

    /// Look up an instance by its document-scoped identity.
    pub fn instance(&self, id: InstanceId) -> Option<&Instance> {
        self.dom.get_by_ref(id.0)
    }

    /// Insert a child under an existing instance and return its document ID.
    pub fn insert(
        &mut self,
        parent: InstanceId,
        class: impl AsRef<str>,
        name: impl Into<String>,
    ) -> Result<InstanceId, ProjectError> {
        if self.dom.get_by_ref(parent.0).is_none() {
            return Err(ProjectError::MissingInstance(parent));
        }
        let builder = InstanceBuilder::new(class.as_ref()).with_name(name);
        Ok(InstanceId(self.dom.insert(parent.0, builder)))
    }

    /// Rename an instance without exposing mutable access to the owned tree.
    pub fn set_name(
        &mut self,
        id: InstanceId,
        name: impl Into<String>,
    ) -> Result<(), ProjectError> {
        let instance = self
            .dom
            .get_by_ref_mut(id.0)
            .ok_or(ProjectError::MissingInstance(id))?;
        instance.name = name.into();
        Ok(())
    }

    /// Change one typed property on an existing instance.
    pub fn set_property(
        &mut self,
        id: InstanceId,
        name: impl AsRef<str>,
        value: PropertyValue,
    ) -> Result<(), ProjectError> {
        let instance = self
            .dom
            .get_by_ref_mut(id.0)
            .ok_or(ProjectError::MissingInstance(id))?;
        instance.properties.insert(ustr(name.as_ref()), value);
        Ok(())
    }

    /// Borrow the `rbx-dom` tree for adapters that need lower-level operations.
    pub const fn weak_dom(&self) -> &WeakDom {
        &self.dom
    }

    /// Serialize and verify a semantic round trip into the selected format.
    pub fn to_bytes(&self, format: FileFormat) -> Result<Vec<u8>, ProjectError> {
        if self.dom.root().children().len() as u64 > self.limits.max_instances as u64 {
            return Err(ProjectError::LimitExceeded {
                resource: "serialized instance count",
                actual: self.dom.root().children().len() as u64,
                limit: self.limits.max_instances as u64,
            });
        }
        let roots: Vec<Ref> = self.dom.root().children().to_vec();
        validate_serializable_tree(&self.dom, &roots, self.limits)?;

        let mut output = LimitedVecWriter::new(self.limits.max_file_bytes);
        if let Err(error) = self.encode_to(&mut output, format) {
            if let Some(actual) = output.limit_exceeded {
                return Err(ProjectError::LimitExceeded {
                    resource: "serialized byte",
                    actual,
                    limit: self.limits.max_file_bytes,
                });
            }
            return Err(error);
        }
        let bytes = output.bytes;

        let reparsed = Self::from_bytes_with_limits(format, &bytes, self.limits)?;
        let expected = semantic_snapshot(&self.dom);
        let actual = semantic_snapshot(&reparsed.dom);
        if expected != actual {
            return Err(ProjectError::SemanticRoundTripMismatch);
        }
        Ok(bytes)
    }

    /// Save to the path's selected format with a recovery copy and atomic replace.
    ///
    /// Existing regular files are parsed before replacement. The staged file
    /// is exchanged atomically with the destination and the displaced file is
    /// moved to a unique adjacent backup. New destinations use no-replace
    /// rename semantics. Concurrent changes are reported.
    pub fn save_atomic(&self, path: impl AsRef<Path>) -> Result<SaveReceipt, ProjectError> {
        let path = path.as_ref();
        let format = FileFormat::from_path(path)?;
        let parent = usable_parent(path)?;
        let destination_state = match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() => Some(metadata),
            Ok(_) => {
                return Err(ProjectError::InvalidDestination(path.display().to_string()));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };

        let original_bytes = if destination_state.is_some() {
            let bytes = read_bounded(path, self.limits.max_file_bytes)?;
            // Refuse to overwrite a corrupt, mismatched, or unsupported file.
            let _ = Self::from_bytes_with_limits(format, &bytes, self.limits)?;
            Some(bytes)
        } else {
            None
        };

        let bytes = self.to_bytes(format)?;
        let mut staged = create_temporary_file(parent, "stage")?;
        staged.file.write_all(&bytes)?;
        staged.file.flush()?;
        if let Some(metadata) = &destination_state {
            staged.file.set_permissions(metadata.permissions())?;
        }
        staged.file.sync_all()?;
        let staged_bytes = read_bounded(&staged.path, self.limits.max_file_bytes)?;
        let verified = Self::from_bytes_with_limits(format, &staged_bytes, self.limits)?;
        if semantic_snapshot(&self.dom) != semantic_snapshot(&verified.dom) {
            return Err(ProjectError::SemanticRoundTripMismatch);
        }

        ensure_destination_unchanged(
            path,
            destination_state.as_ref(),
            original_bytes.as_deref(),
            self.limits.max_file_bytes,
        )?;
        let parent_directory = File::open(parent)?;
        let backup_path = if destination_state.is_some() {
            exchange_staged_with_destination(&parent_directory, path, &mut staged)?;

            let displaced_matches_snapshot = matches!(
                ensure_destination_unchanged(
                    &staged.path,
                    destination_state.as_ref(),
                    original_bytes.as_deref(),
                    self.limits.max_file_bytes,
                ),
                Ok(())
            );
            if !displaced_matches_snapshot {
                match restore_exchanged_destination(
                    &parent_directory,
                    path,
                    &staged,
                    &bytes,
                    self.limits.max_file_bytes,
                ) {
                    Ok(true) => {
                        return Err(ProjectError::ConcurrentModification(
                            path.display().to_string(),
                        ));
                    }
                    Ok(false) => {}
                    Err(error) => {
                        staged.keep = true;
                        return Err(ProjectError::ConcurrentModification(format!(
                            "{}; the displaced file was preserved at {} because rollback failed: {error}",
                            path.display(),
                            staged.path.display()
                        )));
                    }
                }
                staged.keep = true;
                return Err(ProjectError::ConcurrentModification(format!(
                    "{}; the displaced file was preserved at {}",
                    path.display(),
                    staged.path.display()
                )));
            }

            match move_displaced_file_to_backup(&parent_directory, &staged.path, path) {
                Ok(backup_path) => {
                    staged.keep = true;
                    Some(backup_path)
                }
                Err(error) => {
                    match restore_exchanged_destination(
                        &parent_directory,
                        path,
                        &staged,
                        &bytes,
                        self.limits.max_file_bytes,
                    ) {
                        Ok(true) => return Err(error),
                        Ok(false) => {}
                        Err(restore_error) => {
                            staged.keep = true;
                            return Err(ProjectError::ConcurrentModification(format!(
                                "{}; the displaced file was preserved at {} after backup creation failed ({error}) and rollback failed ({restore_error})",
                                path.display(),
                                staged.path.display()
                            )));
                        }
                    }
                    staged.keep = true;
                    return Err(ProjectError::ConcurrentModification(format!(
                        "{}; the displaced file was preserved at {} after backup creation failed: {error}",
                        path.display(),
                        staged.path.display()
                    )));
                }
            }
        } else {
            let staged_name = staged.path.file_name().ok_or_else(|| {
                ProjectError::InvalidDestination(staged.path.display().to_string())
            })?;
            let destination_name = path
                .file_name()
                .ok_or_else(|| ProjectError::InvalidDestination(path.display().to_string()))?;
            match rustix::fs::renameat_with(
                &parent_directory,
                staged_name,
                &parent_directory,
                destination_name,
                RenameFlags::NOREPLACE,
            ) {
                Ok(()) => staged.keep = true,
                Err(error) if error == Errno::EXIST => {
                    return Err(ProjectError::ConcurrentModification(
                        path.display().to_string(),
                    ));
                }
                Err(error) => return Err(io::Error::from(error).into()),
            }
            None
        };

        parent_directory.sync_all()?;

        Ok(SaveReceipt {
            path: path.to_path_buf(),
            backup_path,
            bytes_written: bytes.len() as u64,
        })
    }

    fn encode_to<W: Write>(&self, writer: &mut W, format: FileFormat) -> Result<(), ProjectError> {
        let refs: Vec<Ref> = self.dom.root().children().to_vec();
        let database = rbx_reflection_database::get_bundled();
        if format.is_binary() {
            let serializer = rbx_binary::Serializer::new().reflection_database(database);
            serializer
                .serialize(writer, &self.dom, &refs)
                .map_err(EncodeFailure::Binary)?;
        } else {
            let options = rbx_xml::EncodeOptions::new()
                .reflection_database(database)
                .property_behavior(rbx_xml::EncodePropertyBehavior::WriteUnknown);
            rbx_xml::to_writer(writer, &self.dom, &refs, options).map_err(EncodeFailure::Xml)?;
        }
        Ok(())
    }
}

fn ensure_destination_unchanged(
    path: &Path,
    before_metadata: Option<&fs::Metadata>,
    before_bytes: Option<&[u8]>,
    max_bytes: u64,
) -> Result<(), ProjectError> {
    let after_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };

    match (before_metadata, after_metadata.as_ref(), before_bytes) {
        (None, None, None) => Ok(()),
        (Some(before), Some(after), Some(bytes))
            if after.file_type().is_file() && same_file_metadata(before, after) =>
        {
            let current_bytes = match read_bounded(path, max_bytes) {
                Ok(bytes) => bytes,
                Err(ProjectError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(ProjectError::ConcurrentModification(
                        path.display().to_string(),
                    ));
                }
                Err(error) => return Err(error),
            };
            if current_bytes == bytes {
                Ok(())
            } else {
                Err(ProjectError::ConcurrentModification(
                    path.display().to_string(),
                ))
            }
        }
        _ => Err(ProjectError::ConcurrentModification(
            path.display().to_string(),
        )),
    }
}

fn same_file_metadata(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        before.dev() == after.dev() && before.ino() == after.ino() && before.mode() == after.mode()
    }
    #[cfg(not(unix))]
    {
        before.permissions().readonly() == after.permissions().readonly()
    }
}

fn validate_serializable_tree(
    dom: &WeakDom,
    roots: &[Ref],
    limits: ReadLimits,
) -> Result<(), ProjectError> {
    let mut refs = Vec::new();
    let mut cursor = 0;
    refs.extend_from_slice(roots);
    while cursor < refs.len() {
        if refs.len() as u64 > limits.max_instances as u64 {
            return Err(ProjectError::LimitExceeded {
                resource: "serialized instance count",
                actual: refs.len() as u64,
                limit: limits.max_instances as u64,
            });
        }
        let instance = dom.get_by_ref(refs[cursor]).ok_or_else(|| {
            ProjectError::UnsupportedData(
                "serialized hierarchy contains an instance without a valid identity".to_owned(),
            )
        })?;
        let projected_count = refs.len().saturating_add(instance.children().len());
        if projected_count as u64 > limits.max_instances as u64 {
            return Err(ProjectError::LimitExceeded {
                resource: "serialized instance count",
                actual: projected_count as u64,
                limit: limits.max_instances as u64,
            });
        }
        refs.extend_from_slice(instance.children());
        cursor += 1;
    }

    let serialized_refs: std::collections::HashSet<Ref> = refs.iter().copied().collect();
    for referent in refs {
        let instance = dom.get_by_ref(referent).ok_or_else(|| {
            ProjectError::UnsupportedData(
                "serialized hierarchy contains an instance without a valid identity".to_owned(),
            )
        })?;
        for (name, value) in &instance.properties {
            if let Variant::Ref(target) = value
                && !target.is_none()
                && !serialized_refs.contains(target)
            {
                return Err(ProjectError::UnsupportedData(format!(
                    "property {name:?} on {:?} references an instance outside the serialized tree",
                    instance.class
                )));
            }
            let sequence_keypoints = match value {
                Variant::NumberSequence(sequence) => Some(sequence.keypoints.len()),
                Variant::ColorSequence(sequence) => Some(sequence.keypoints.len()),
                _ => None,
            };
            if let Some(actual) = sequence_keypoints
                && actual as u64 > limits.max_property_elements as u64
            {
                return Err(ProjectError::LimitExceeded {
                    resource: "serialized sequence keypoint count",
                    actual: actual as u64,
                    limit: limits.max_property_elements as u64,
                });
            }
        }
    }

    Ok(())
}

struct LimitedVecWriter {
    bytes: Vec<u8>,
    max_bytes: u64,
    limit_exceeded: Option<u64>,
}

impl LimitedVecWriter {
    fn new(max_bytes: u64) -> Self {
        Self {
            bytes: Vec::new(),
            max_bytes,
            limit_exceeded: None,
        }
    }
}

impl Write for LimitedVecWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let current = self.bytes.len() as u64;
        let requested = current.saturating_add(bytes.len() as u64);
        if requested > self.max_bytes {
            self.limit_exceeded = Some(requested);
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "serialized project exceeds the configured byte limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Encoding {
    Binary,
    Xml,
}

fn detect_encoding(bytes: &[u8]) -> Result<Encoding, ProjectError> {
    const BINARY_MAGIC: &[u8] = b"<roblox!\x89\xff\r\n\x1a\n";
    if bytes.starts_with(BINARY_MAGIC) {
        return Ok(Encoding::Binary);
    }
    let xml_bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let xml_start = xml_bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map_or(&xml_bytes[xml_bytes.len()..], |index| &xml_bytes[index..]);
    if xml_start.starts_with(b"<") {
        return Ok(Encoding::Xml);
    }
    Err(ProjectError::UnsupportedData(
        "file signature is not a supported Roblox place/model format".to_owned(),
    ))
}

fn read_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>, ProjectError> {
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    if size > max_bytes {
        return Err(ProjectError::LimitExceeded {
            resource: "input byte",
            actual: size,
            limit: max_bytes,
        });
    }

    let capacity = usize::try_from(size).map_err(|_| ProjectError::LimitExceeded {
        resource: "input byte address space",
        actual: size,
        limit: usize::MAX as u64,
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(ProjectError::LimitExceeded {
            resource: "input byte",
            actual: bytes.len() as u64,
            limit: max_bytes,
        });
    }
    Ok(bytes)
}

fn usable_parent(path: &Path) -> Result<&Path, ProjectError> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => Ok(parent),
        Some(_) => Ok(Path::new(".")),
        None => Err(ProjectError::InvalidDestination(path.display().to_string())),
    }
}

struct TemporaryFile {
    path: PathBuf,
    file: File,
    keep: bool,
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn create_temporary_file(parent: &Path, purpose: &str) -> Result<TemporaryFile, ProjectError> {
    for _ in 0..100 {
        let id = TEMPORARY_FILE_ID.fetch_add(1, Ordering::Relaxed);
        let name = format!(".studio-project-{}-{id}-{purpose}.tmp", std::process::id());
        let path = parent.join(name);
        let file = create_new_file(&path);
        match file {
            Ok(file) => {
                return Ok(TemporaryFile {
                    path,
                    file,
                    keep: false,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique temporary project file after 100 attempts",
    )
    .into())
}

fn path_matches_file(
    path: &Path,
    expected_file: &File,
    expected_bytes: &[u8],
    max_bytes: u64,
) -> Result<bool, ProjectError> {
    let current_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if !same_file_metadata(&expected_file.metadata()?, &current_metadata) {
        return Ok(false);
    }
    Ok(read_bounded(path, max_bytes)? == expected_bytes)
}

fn exchange_staged_with_destination(
    parent_directory: &File,
    destination: &Path,
    staged: &mut TemporaryFile,
) -> Result<(), ProjectError> {
    let staged_name = staged
        .path
        .file_name()
        .ok_or_else(|| ProjectError::InvalidDestination(staged.path.display().to_string()))?;
    let destination_name = destination
        .file_name()
        .ok_or_else(|| ProjectError::InvalidDestination(destination.display().to_string()))?;
    match rustix::fs::renameat_with(
        parent_directory,
        staged_name,
        parent_directory,
        destination_name,
        RenameFlags::EXCHANGE,
    ) {
        Ok(()) => {
            // Until the displaced original is backed up or rollback finishes,
            // TemporaryFile must never delete the file at this path.
            staged.keep = true;
            Ok(())
        }
        Err(Errno::NOENT | Errno::EXIST) => Err(ProjectError::ConcurrentModification(
            destination.display().to_string(),
        )),
        Err(error) => Err(io::Error::from(error).into()),
    }
}

fn restore_exchanged_destination(
    parent_directory: &File,
    destination: &Path,
    staged: &TemporaryFile,
    staged_bytes: &[u8],
    max_bytes: u64,
) -> Result<bool, ProjectError> {
    if !path_matches_file(destination, &staged.file, staged_bytes, max_bytes)? {
        return Ok(false);
    }

    let staged_name = staged
        .path
        .file_name()
        .ok_or_else(|| ProjectError::InvalidDestination(staged.path.display().to_string()))?;
    let destination_name = destination
        .file_name()
        .ok_or_else(|| ProjectError::InvalidDestination(destination.display().to_string()))?;
    match rustix::fs::renameat_with(
        parent_directory,
        staged_name,
        parent_directory,
        destination_name,
        RenameFlags::EXCHANGE,
    ) {
        Ok(()) => {}
        Err(Errno::NOENT | Errno::EXIST) => return Ok(false),
        Err(error) => return Err(io::Error::from(error).into()),
    }

    if path_matches_file(&staged.path, &staged.file, staged_bytes, max_bytes)? {
        fs::remove_file(&staged.path)?;
        return Ok(true);
    }

    // Preserve anything another writer displaced during rollback.
    Ok(false)
}

fn move_displaced_file_to_backup(
    parent_directory: &File,
    displaced_path: &Path,
    destination: &Path,
) -> Result<PathBuf, ProjectError> {
    for _ in 0..100 {
        let id = TEMPORARY_FILE_ID.fetch_add(1, Ordering::Relaxed);
        let backup_path = backup_name(destination, id)?;
        let displaced_name = displaced_path.file_name().ok_or_else(|| {
            ProjectError::InvalidDestination(displaced_path.display().to_string())
        })?;
        let backup_name = backup_path
            .file_name()
            .ok_or_else(|| ProjectError::InvalidDestination(backup_path.display().to_string()))?;
        match rustix::fs::renameat_with(
            parent_directory,
            displaced_name,
            parent_directory,
            backup_name,
            RenameFlags::NOREPLACE,
        ) {
            Ok(()) => return Ok(backup_path),
            Err(error) if error == Errno::EXIST => continue,
            Err(error) => return Err(io::Error::from(error).into()),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique project backup after 100 attempts",
    )
    .into())
}

fn create_new_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).read(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o666);
    }
    options.open(path)
}

#[derive(Debug, PartialEq)]
struct SemanticSnapshot(Vec<NodeSnapshot>);

#[derive(Debug, PartialEq)]
struct NodeSnapshot {
    class: String,
    name: String,
    children: Vec<usize>,
    properties: Vec<(String, SemanticValue)>,
}

#[derive(Debug, PartialEq)]
enum SemanticValue {
    NullReference,
    InternalReference(usize),
    ExternalReference(Ref),
    Value(Variant),
}

fn semantic_snapshot(dom: &WeakDom) -> SemanticSnapshot {
    let mut refs = vec![dom.root_ref()];
    let mut cursor = 0;
    while cursor < refs.len() {
        let instance = dom
            .get_by_ref(refs[cursor])
            .expect("DOM child identities must resolve");
        refs.extend_from_slice(instance.children());
        cursor += 1;
    }

    let indices: HashMap<Ref, usize> = refs
        .iter()
        .copied()
        .enumerate()
        .map(|(index, referent)| (referent, index))
        .collect();

    let nodes = refs
        .iter()
        .map(|referent| {
            let instance = dom
                .get_by_ref(*referent)
                .expect("DOM child identities must resolve");
            let mut properties: Vec<_> = instance
                .properties
                .iter()
                .map(|(name, value)| {
                    let value = match value {
                        Variant::Ref(target) if target.is_none() => SemanticValue::NullReference,
                        Variant::Ref(target) => match indices.get(target).copied() {
                            Some(index) => SemanticValue::InternalReference(index),
                            None => SemanticValue::ExternalReference(*target),
                        },
                        _ => SemanticValue::Value(value.clone()),
                    };
                    (name.to_string(), value)
                })
                .collect();
            properties.sort_by(|left, right| left.0.cmp(&right.0));

            NodeSnapshot {
                class: instance.class.to_string(),
                name: instance.name.clone(),
                children: instance
                    .children()
                    .iter()
                    .map(|child| indices[child])
                    .collect(),
                properties,
            }
        })
        .collect();

    SemanticSnapshot(nodes)
}

fn backup_name(path: &Path, id: u64) -> Result<PathBuf, ProjectError> {
    let file_name = path
        .file_name()
        .ok_or_else(|| ProjectError::InvalidDestination(path.display().to_string()))?;
    let mut backup_name = OsString::from(file_name);
    backup_name.push(format!(".bak-{}-{id}", std::process::id()));
    Ok(usable_parent(path)?.join(backup_name))
}
