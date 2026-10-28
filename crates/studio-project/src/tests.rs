use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use rbx_dom_weak::types::{
    BinaryString, CFrame, Matrix3, NumberSequence, NumberSequenceKeypoint, Variant, Vector3,
};

use crate::{FileFormat, ProjectDocument, ProjectError, ReadLimits};

const PLACE_FIXTURE: &str = include_str!("../tests/fixtures/place_roundtrip.rbxlx");
static TEMP_DIR_ID: AtomicU64 = AtomicU64::new(0);

#[test]
fn xml_fixture_preserves_hierarchy_unknown_fields_and_forward_references() {
    let document = ProjectDocument::from_bytes(FileFormat::PlaceXml, PLACE_FIXTURE.as_bytes())
        .expect("synthetic XML place fixture should decode");
    let workspace_id = document.root_children().next().expect("Workspace root");
    let workspace = document.instance(workspace_id).expect("Workspace exists");
    assert_eq!(workspace.name, "Workspace");

    let unknown = workspace
        .properties
        .iter()
        .find(|(name, _)| name.as_str() == "FutureStringProperty")
        .map(|(_, value)| value)
        .expect("unknown typed property is retained");
    assert_eq!(unknown, &Variant::String("retain this value".to_owned()));

    let child_ids = workspace.children().to_vec();
    let link_id = child_ids
        .iter()
        .copied()
        .find(|id| {
            document
                .weak_dom()
                .get_by_ref(*id)
                .is_some_and(|instance| instance.class.as_str() == "ObjectValue")
        })
        .expect("ObjectValue child exists");
    let part_id = child_ids
        .iter()
        .copied()
        .find(|id| {
            document
                .weak_dom()
                .get_by_ref(*id)
                .is_some_and(|instance| instance.class.as_str() == "Part")
        })
        .expect("Part child exists");
    let link = document
        .weak_dom()
        .get_by_ref(link_id)
        .expect("link resolves");
    let value = link
        .properties
        .iter()
        .find(|(name, _)| name.as_str() == "Value")
        .map(|(_, value)| value)
        .expect("ObjectValue.Value exists");
    assert_eq!(value, &Variant::Ref(part_id));

    let same_name_count = child_ids
        .iter()
        .filter(|id| {
            document
                .weak_dom()
                .get_by_ref(**id)
                .is_some_and(|instance| instance.name == "Repeated")
        })
        .count();
    assert_eq!(same_name_count, 2);

    let encoded = document
        .to_bytes(FileFormat::PlaceXml)
        .expect("supported XML subset round-trips semantically");
    ProjectDocument::from_bytes(FileFormat::PlaceXml, &encoded).expect("serialized XML reopens");
}

#[test]
fn all_four_extensions_round_trip_the_supported_binary_and_xml_subset() {
    let mut document = ProjectDocument::new(FileFormat::PlaceXml);
    let root = document.root_id();
    let part_id = document
        .insert(root, "Part", "Measured Part")
        .expect("insert Part");
    document
        .set_property(part_id, "Anchored", true.into())
        .expect("set Anchored");
    document
        .set_property(
            part_id,
            "CFrame",
            CFrame::new(Vector3::new(1.0, 2.0, 3.0), Matrix3::identity()).into(),
        )
        .expect("set CFrame");

    let object_id = document
        .insert(root, "ObjectValue", "Reference")
        .expect("insert ObjectValue");
    document
        .set_name(object_id, "Renamed Reference")
        .expect("rename ObjectValue");
    document
        .set_property(object_id, "Value", Variant::Ref(part_id.as_ref()))
        .expect("set ObjectValue.Value");

    for format in [
        FileFormat::PlaceBinary,
        FileFormat::PlaceXml,
        FileFormat::ModelBinary,
        FileFormat::ModelXml,
    ] {
        let bytes = document
            .to_bytes(format)
            .unwrap_or_else(|error| panic!("serialize and verify {format:?}: {error}"));
        let reopened = ProjectDocument::from_bytes(format, &bytes).expect("reopen serialized data");
        assert_eq!(reopened.root_children().len(), 2, "format {format:?}");
        let renamed = reopened
            .root_children()
            .map(|id| reopened.instance(id).expect("root child exists"))
            .find(|instance| instance.class.as_str() == "ObjectValue")
            .expect("ObjectValue remains in tree");
        assert_eq!(renamed.name, "Renamed Reference", "format {format:?}");
    }
}

#[test]
fn encoding_rejects_references_outside_the_serialized_tree() {
    let mut document = ProjectDocument::new(FileFormat::PlaceXml);
    let object = document
        .insert(document.root_id(), "ObjectValue", "ObjectValue")
        .expect("insert ObjectValue");
    document
        .set_property(
            object,
            "Value",
            Variant::Ref(rbx_dom_weak::types::Ref::some(123)),
        )
        .expect("set dangling reference");

    for format in [
        FileFormat::PlaceBinary,
        FileFormat::PlaceXml,
        FileFormat::ModelBinary,
        FileFormat::ModelXml,
    ] {
        assert!(matches!(
            document.to_bytes(format),
            Err(ProjectError::UnsupportedData(_))
        ));
    }
}

#[test]
fn encoding_enforces_the_configured_output_size_limit_during_writes() {
    let limits = ReadLimits {
        max_file_bytes: 64,
        ..ReadLimits::default()
    };
    let mut document = ProjectDocument::from_bytes_with_limits(
        FileFormat::PlaceXml,
        b"<roblox version=\"4\"></roblox>",
        limits,
    )
    .expect("empty XML project should decode");
    document
        .insert(document.root_id(), "Folder", "Folder")
        .expect("insert Folder");

    assert!(matches!(
        document.to_bytes(FileFormat::PlaceXml),
        Err(ProjectError::LimitExceeded {
            resource: "serialized byte",
            limit: 64,
            ..
        })
    ));
}

#[test]
fn encoding_enforces_the_configured_sequence_limit() {
    let limits = ReadLimits {
        max_property_elements: 2,
        ..ReadLimits::default()
    };
    let mut document = ProjectDocument::from_bytes_with_limits(
        FileFormat::PlaceXml,
        b"<roblox version=\"4\"></roblox>",
        limits,
    )
    .expect("empty XML project should decode");
    let part = document
        .insert(document.root_id(), "Part", "Part")
        .expect("insert Part");
    document
        .set_property(
            part,
            "FutureSequence",
            Variant::NumberSequence(NumberSequence {
                keypoints: vec![
                    NumberSequenceKeypoint::new(0.0, 0.0, 0.0),
                    NumberSequenceKeypoint::new(0.5, 1.0, 0.0),
                    NumberSequenceKeypoint::new(1.0, 0.0, 0.0),
                ],
            }),
        )
        .expect("set sequence");

    assert!(matches!(
        document.to_bytes(FileFormat::ModelXml),
        Err(ProjectError::LimitExceeded {
            resource: "serialized sequence keypoint count",
            limit: 2,
            ..
        })
    ));
}

#[test]
fn binary_unknown_binary_string_keeps_non_utf8_payload() {
    let mut document = ProjectDocument::new(FileFormat::ModelBinary);
    let part = document
        .insert(document.root_id(), "Part", "Part")
        .expect("insert Part");
    let payload = BinaryString::from(vec![0x00, 0xff, 0x80, 0x41]);
    document
        .set_property(
            part,
            "FutureBinaryProperty",
            Variant::BinaryString(payload.clone()),
        )
        .expect("set binary property");

    let bytes = document
        .to_bytes(FileFormat::ModelBinary)
        .expect("unknown binary property is retained using the wire default");
    let reopened =
        ProjectDocument::from_bytes(FileFormat::ModelBinary, &bytes).expect("binary data reopens");
    let reopened_part = reopened
        .instance(reopened.root_children().next().expect("Part root"))
        .expect("Part resolves");
    let actual = reopened_part
        .properties
        .iter()
        .find(|(name, _)| name.as_str() == "FutureBinaryProperty")
        .map(|(_, value)| value)
        .expect("binary unknown property is present");
    assert_eq!(actual, &Variant::BinaryString(payload));
}

#[test]
fn binary_guard_rejects_unknown_chunks_and_missing_parent_referents() {
    let mut document = ProjectDocument::new(FileFormat::ModelBinary);
    document
        .insert(document.root_id(), "Part", "Part")
        .expect("insert Part");
    let valid = uncompressed_binary(&document);

    let mut unknown_chunk = valid.clone();
    unknown_chunk[32..36].copy_from_slice(b"JUNK");
    assert!(matches!(
        ProjectDocument::from_bytes(FileFormat::ModelBinary, &unknown_chunk),
        Err(ProjectError::UnsupportedData(_))
    ));

    let mut missing_parent = valid;
    let parent = binary_chunk_payload(&missing_parent, b"PRNT");
    let count = u32::from_le_bytes(
        missing_parent[parent.start + 1..parent.start + 5]
            .try_into()
            .expect("PRNT count is four bytes"),
    );
    assert_eq!(count, 1, "single Part fixture has one parent row");
    let parent_value = parent.start + 5 + 4;
    missing_parent[parent_value..parent_value + 4].copy_from_slice(&[0, 0, 0, 246]);
    assert!(matches!(
        ProjectDocument::from_bytes(FileFormat::ModelBinary, &missing_parent),
        Err(ProjectError::UnsupportedData(_))
    ));
}

#[test]
fn binary_guard_rejects_instance_chunks_over_the_declared_total() {
    let mut document = ProjectDocument::new(FileFormat::ModelBinary);
    for class_name in ["Part", "Folder"] {
        for _ in 0..2 {
            document
                .insert(document.root_id(), class_name, class_name)
                .expect("insert instance");
        }
    }
    let mut bytes = uncompressed_binary(&document);
    bytes[20..24].copy_from_slice(&2u32.to_le_bytes());

    assert!(matches!(
        ProjectDocument::from_bytes(FileFormat::ModelBinary, &bytes),
        Err(ProjectError::UnsupportedData(_))
    ));
}

#[test]
fn binary_guard_rejects_trailing_bytes_in_fixed_width_properties() {
    let mut document = ProjectDocument::new(FileFormat::ModelBinary);
    let part = document
        .insert(document.root_id(), "Part", "Part")
        .expect("insert Part");
    document
        .set_property(part, "Anchored", true.into())
        .expect("set Anchored");
    let mut bytes = uncompressed_binary(&document);
    append_property_trailing_byte(&mut bytes, "Anchored");

    assert!(matches!(
        ProjectDocument::from_bytes(FileFormat::ModelBinary, &bytes),
        Err(ProjectError::UnsupportedData(_))
    ));
}

#[test]
fn xml_guard_rejects_data_the_decoder_would_discard() {
    let cases = [
        (
            "unknown value tag",
            b"<roblox version=\"4\"><Item class=\"Folder\"><Properties><FutureType name=\"Data\">x</FutureType></Properties></Item></roblox>".as_slice(),
        ),
        (
            "metadata",
            b"<roblox version=\"4\"><Meta name=\"Mode\">x</Meta></roblox>".as_slice(),
        ),
        (
            "external entry",
            b"<roblox version=\"4\"><External>null</External></roblox>".as_slice(),
        ),
        (
            "unresolved reference",
            b"<roblox version=\"4\"><Item class=\"ObjectValue\" referent=\"RBX0\"><Properties><Ref name=\"Value\">MISSING</Ref></Properties></Item></roblox>".as_slice(),
        ),
        (
            "unresolved shared string",
            b"<roblox version=\"4\"><Item class=\"Part\" referent=\"RBX0\"><Properties><SharedString name=\"FutureValue\">missing-hash</SharedString></Properties></Item></roblox>".as_slice(),
        ),
        (
            "duplicate property",
            b"<roblox version=\"4\"><Item class=\"Folder\"><Properties><string name=\"Name\">One</string><string name=\"Name\">Two</string></Properties></Item></roblox>".as_slice(),
        ),
        (
            "duplicate referent",
            b"<roblox version=\"4\"><Item class=\"Folder\" referent=\"RBX0\"/><Item class=\"Folder\" referent=\"RBX0\"/></roblox>".as_slice(),
        ),
        (
            "unknown attribute",
            b"<roblox version=\"4\"><Item class=\"Folder\" future=\"x\"/></roblox>".as_slice(),
        ),
    ];

    for (case, bytes) in cases {
        let error = match ProjectDocument::from_bytes(FileFormat::ModelXml, bytes) {
            Err(error) => error,
            Ok(_) => panic!("{case}: input was unexpectedly accepted"),
        };
        assert!(
            matches!(error, ProjectError::UnsupportedData(_)),
            "{case}: {error}"
        );
    }
}

#[test]
fn xml_guard_rejects_bad_version_doctype_and_resource_limits() {
    let wrong_version = b"<roblox version=\"5\"/>";
    assert!(matches!(
        ProjectDocument::from_bytes(FileFormat::PlaceXml, wrong_version),
        Err(ProjectError::UnsupportedData(_))
    ));

    let doctype = b"<!DOCTYPE roblox><roblox version=\"4\"/>";
    assert!(matches!(
        ProjectDocument::from_bytes(FileFormat::PlaceXml, doctype),
        Err(ProjectError::UnsupportedData(_))
    ));

    let limits = ReadLimits {
        max_instances: 1,
        ..ReadLimits::default()
    };
    assert!(matches!(
        ProjectDocument::from_bytes_with_limits(
            FileFormat::PlaceXml,
            PLACE_FIXTURE.as_bytes(),
            limits
        ),
        Err(ProjectError::LimitExceeded {
            resource: "instance count",
            ..
        })
    ));
}

#[test]
fn xml_guard_enforces_number_and_color_sequence_keypoint_limits() {
    let limits = ReadLimits {
        max_property_elements: 2,
        ..ReadLimits::default()
    };
    let cases = [
        ("NumberSequence", "0 0 0 0.5 1 0 1 0 0"),
        ("ColorSequence", "0 0 0 0 0 0.5 1 0 0 0 1 0 0 0 0"),
    ];

    for (tag, values) in cases {
        let xml = format!(
            "<roblox version=\"4\"><Item class=\"Folder\"><Properties><{tag} name=\"FutureSequence\">{values}</{tag}></Properties></Item></roblox>"
        );
        assert!(matches!(
            ProjectDocument::from_bytes_with_limits(FileFormat::ModelXml, xml.as_bytes(), limits),
            Err(ProjectError::LimitExceeded {
                resource: "sequence keypoint count",
                limit: 2,
                ..
            })
        ));
    }
}

#[test]
fn extension_mismatch_and_malformed_data_return_controlled_errors() {
    assert!(matches!(
        ProjectDocument::from_bytes(FileFormat::PlaceBinary, PLACE_FIXTURE.as_bytes()),
        Err(ProjectError::FormatMismatch { .. })
    ));
    assert!(matches!(
        ProjectDocument::from_bytes(FileFormat::PlaceXml, b"<roblox version=\"4\">"),
        Err(ProjectError::UnsupportedData(_) | ProjectError::Decode(_))
    ));
}

#[test]
fn atomic_save_keeps_recovery_copy_and_does_not_overwrite_invalid_input() {
    let directory = TestDirectory::new();
    let project_path = directory.path().join("fixture.rbxlx");
    fs::write(&project_path, PLACE_FIXTURE).expect("seed original project");
    let original = fs::read(&project_path).expect("read original project");
    let mut document = ProjectDocument::open(&project_path).expect("open fixture");
    let workspace = document.root_children().next().expect("Workspace");
    document
        .set_property(workspace, "FutureStringProperty", "edited".into())
        .expect("edit property");

    let receipt = document.save_atomic(&project_path).expect("atomic save");
    assert_eq!(receipt.path, project_path);
    assert!(receipt.bytes_written > 0);
    let backup = receipt.backup_path.expect("existing file has a backup");
    assert_eq!(fs::read(backup).expect("read backup"), original);
    ProjectDocument::open(&project_path).expect("saved project reopens");

    let invalid_path = directory.path().join("invalid.rbxlx");
    let invalid_bytes = b"not a Roblox project";
    fs::write(&invalid_path, invalid_bytes).expect("write invalid destination");
    assert!(document.save_atomic(&invalid_path).is_err());
    assert_eq!(
        fs::read(invalid_path).expect("invalid file survives"),
        invalid_bytes
    );
}

#[test]
fn save_detects_a_destination_changed_after_its_snapshot() {
    let directory = TestDirectory::new();
    let destination = directory.path().join("concurrent.rbxlx");
    fs::write(&destination, b"original bytes").expect("write initial destination");
    let original_metadata = fs::symlink_metadata(&destination).expect("snapshot metadata");
    let original_bytes = fs::read(&destination).expect("snapshot bytes");

    fs::write(&destination, b"updated  bytes").expect("simulate another writer");
    assert!(matches!(
        crate::ensure_destination_unchanged(
            &destination,
            Some(&original_metadata),
            Some(&original_bytes),
            1024,
        ),
        Err(ProjectError::ConcurrentModification(_))
    ));

    let absent_destination = directory.path().join("created.rbxlx");
    fs::write(&absent_destination, b"created elsewhere").expect("simulate concurrent create");
    assert!(matches!(
        crate::ensure_destination_unchanged(&absent_destination, None, None, 1024),
        Err(ProjectError::ConcurrentModification(_))
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn failed_snapshot_check_restores_the_displaced_destination() {
    let directory = TestDirectory::new();
    let destination = directory.path().join("concurrent.rbxlx");
    let original_bytes = b"original snapshot";
    fs::write(&destination, original_bytes).expect("write initial destination");

    let mut staged =
        crate::create_temporary_file(directory.path(), "stage").expect("create staged output");
    let staged_bytes = b"new verified output";
    staged
        .file
        .write_all(staged_bytes)
        .expect("write staged output");
    staged.file.sync_all().expect("sync staged output");

    let parent_directory = fs::File::open(directory.path()).expect("open parent directory");
    crate::exchange_staged_with_destination(&parent_directory, &destination, &mut staged)
        .expect("atomically exchange destination and staged output");

    // Simulate a writer that changed the displaced inode after the snapshot.
    fs::write(&staged.path, b"other writer's bytes").expect("simulate concurrent writer");
    assert!(
        crate::restore_exchanged_destination(
            &parent_directory,
            &destination,
            &staged,
            staged_bytes,
            1024,
        )
        .expect("restore destination")
    );

    assert_eq!(
        fs::read(&destination).expect("read restored destination"),
        b"other writer's bytes"
    );
    assert!(
        !staged.path.exists(),
        "verified staged output is cleaned up"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn failed_rollback_io_preserves_the_displaced_file_for_recovery() {
    let directory = TestDirectory::new();
    let destination = directory.path().join("concurrent.rbxlx");
    let original_bytes = b"original project data that must survive";
    fs::write(&destination, original_bytes).expect("write initial destination");

    let mut staged =
        crate::create_temporary_file(directory.path(), "stage").expect("create staged output");
    let staged_bytes = b"verified output";
    staged
        .file
        .write_all(staged_bytes)
        .expect("write staged output");
    staged.file.sync_all().expect("sync staged output");
    let parent_directory = fs::File::open(directory.path()).expect("open parent directory");
    crate::exchange_staged_with_destination(&parent_directory, &destination, &mut staged)
        .expect("exchange staged output with destination");

    let restore_error = crate::restore_exchanged_destination(
        &parent_directory,
        &destination,
        &staged,
        staged_bytes,
        1,
    )
    .expect_err("a one-byte read limit must prevent rollback verification");
    assert!(matches!(restore_error, ProjectError::LimitExceeded { .. }));

    let recovery_path = staged.path.clone();
    drop(staged);
    assert_eq!(
        fs::read(recovery_path).expect("displaced original remains available"),
        original_bytes
    );
}

#[cfg(target_os = "linux")]
#[test]
fn displaced_file_is_moved_to_a_unique_backup_without_copying() {
    let directory = TestDirectory::new();
    let destination = directory.path().join("project.rbxlx");
    let displaced = directory.path().join("temporary-stage.tmp");
    let original = b"recoverable prior version";
    fs::write(&displaced, original).expect("write displaced version");
    let parent_directory = fs::File::open(directory.path()).expect("open parent directory");

    let backup = crate::move_displaced_file_to_backup(&parent_directory, &displaced, &destination)
        .expect("move displaced file to backup");

    assert!(!displaced.exists());
    assert_eq!(fs::read(backup).expect("read backup"), original);
}

#[test]
fn atomic_save_creates_a_missing_destination_without_a_backup() {
    let directory = TestDirectory::new();
    let destination = directory.path().join("new-project.rbxlx");
    let document = ProjectDocument::new(FileFormat::PlaceXml);

    let receipt = document
        .save_atomic(&destination)
        .expect("save new project");

    assert_eq!(receipt.path, destination);
    assert!(receipt.backup_path.is_none());
    ProjectDocument::open(destination).expect("new project reopens");
}

#[test]
fn atomic_save_rejects_a_directory_destination() {
    let directory = TestDirectory::new();
    let destination = directory.path().join("existing.rbxlx");
    fs::create_dir(&destination).expect("create directory destination");
    let document = ProjectDocument::new(FileFormat::PlaceXml);
    assert!(matches!(
        document.save_atomic(&destination),
        Err(ProjectError::InvalidDestination(_))
    ));
}

#[cfg(unix)]
#[test]
fn atomic_save_rejects_a_symlink_destination() {
    use std::os::unix::fs::symlink;

    let directory = TestDirectory::new();
    let target = directory.path().join("target.rbxlx");
    let link = directory.path().join("link.rbxlx");
    fs::write(&target, PLACE_FIXTURE).expect("create target file");
    symlink(&target, &link).expect("create symlink");
    let document = ProjectDocument::new(FileFormat::PlaceXml);
    assert!(matches!(
        document.save_atomic(&link),
        Err(ProjectError::InvalidDestination(_))
    ));
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let id = TEMP_DIR_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("studio-project-test-{}-{id}", std::process::id()));
        fs::create_dir(&path).expect("create isolated temporary test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn uncompressed_binary(document: &ProjectDocument) -> Vec<u8> {
    let database = rbx_reflection_database::get_bundled();
    let serializer = rbx_binary::Serializer::new()
        .reflection_database(database)
        .compression_type(rbx_binary::CompressionType::None);
    let roots = document.weak_dom().root().children().to_vec();
    let mut bytes = Vec::new();
    serializer
        .serialize(&mut bytes, document.weak_dom(), &roots)
        .expect("serialize uncompressed test fixture");
    bytes
}

fn binary_chunk_payload(bytes: &[u8], requested_name: &[u8; 4]) -> std::ops::Range<usize> {
    let mut offset = 32;
    while offset + 16 <= bytes.len() {
        let name = &bytes[offset..offset + 4];
        let compressed = u32::from_le_bytes(
            bytes[offset + 4..offset + 8]
                .try_into()
                .expect("compressed length is four bytes"),
        ) as usize;
        let uncompressed = u32::from_le_bytes(
            bytes[offset + 8..offset + 12]
                .try_into()
                .expect("uncompressed length is four bytes"),
        ) as usize;
        assert_eq!(compressed, 0, "test binary uses uncompressed chunks");
        let start = offset + 16;
        let end = start + uncompressed;
        if name == requested_name {
            return start..end;
        }
        offset = end;
    }
    panic!("requested binary chunk {requested_name:?} was not found");
}

fn append_property_trailing_byte(bytes: &mut Vec<u8>, requested_property: &str) {
    let mut header_offset = 32;
    while header_offset + 16 <= bytes.len() {
        let name = &bytes[header_offset..header_offset + 4];
        let compressed = u32::from_le_bytes(
            bytes[header_offset + 4..header_offset + 8]
                .try_into()
                .expect("compressed length is four bytes"),
        );
        let uncompressed = u32::from_le_bytes(
            bytes[header_offset + 8..header_offset + 12]
                .try_into()
                .expect("uncompressed length is four bytes"),
        ) as usize;
        assert_eq!(compressed, 0, "test binary uses uncompressed chunks");
        let payload_start = header_offset + 16;
        let payload_end = payload_start + uncompressed;

        if name == b"PROP" {
            let property_name_length = u32::from_le_bytes(
                bytes[payload_start + 4..payload_start + 8]
                    .try_into()
                    .expect("property name length is four bytes"),
            ) as usize;
            let property_name_start = payload_start + 8;
            let property_name_end = property_name_start + property_name_length;
            if &bytes[property_name_start..property_name_end] == requested_property.as_bytes() {
                bytes.insert(payload_end, 0xaa);
                let new_length = (uncompressed as u32 + 1).to_le_bytes();
                bytes[header_offset + 8..header_offset + 12].copy_from_slice(&new_length);
                return;
            }
        }

        header_offset = payload_end;
    }
    panic!("PROP chunk {requested_property:?} was not found");
}
