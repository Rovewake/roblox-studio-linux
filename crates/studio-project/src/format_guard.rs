use std::collections::{HashMap, HashSet};

use rbx_dom_weak::types::{FontStyle, FontWeight, VariantType};
use rbx_reflection::{PropertyKind, PropertySerialization, ReflectionDatabase};
use xml::{
    attribute::OwnedAttribute,
    reader::{EventReader, XmlEvent},
};

use crate::{ProjectError, ReadLimits};

const BINARY_MAGIC: &[u8] = b"<roblox!\x89\xff\r\n\x1a\n";
const MAX_BINARY_CHUNK_BYTES: u64 = 512 * 1024 * 1024;

/// Reject inputs that the pinned `rbx_binary` decoder would silently skip or
/// whose framing could trigger oversized allocations before it reports an error.
pub(super) fn validate_binary(
    bytes: &[u8],
    limits: ReadLimits,
    database: &ReflectionDatabase<'_>,
) -> Result<(), ProjectError> {
    if bytes.len() < 32 || !bytes.starts_with(BINARY_MAGIC) {
        return Err(invalid("binary file header or signature is invalid"));
    }

    let version = u16::from_le_bytes([bytes[14], bytes[15]]);
    if version != 0 {
        return Err(ProjectError::UnsupportedData(format!(
            "binary file version {version} is unsupported"
        )));
    }

    let declared_classes = read_u32(bytes, 16)?;
    let declared_instances = read_u32(bytes, 20)?;
    if bytes[24..32].iter().any(|byte| *byte != 0) {
        return Err(invalid("binary header reserved bytes are nonzero"));
    }
    if declared_instances > limits.max_instances {
        return Err(ProjectError::LimitExceeded {
            resource: "instance count",
            actual: declared_instances.into(),
            limit: limits.max_instances.into(),
        });
    }
    if declared_classes > declared_instances {
        return Err(invalid(
            "binary header declares more classes than instances",
        ));
    }

    let mut offset = 32usize;
    let mut chunks_seen = 0u32;
    let mut total_uncompressed = 0u64;
    let mut class_infos = HashMap::new();
    let mut class_names = HashSet::new();
    let mut instance_referents = HashSet::new();
    let mut property_names = HashSet::new();
    let mut single_chunks = HashSet::new();
    let mut total_instances = 0u64;
    let mut end_seen = false;

    while offset < bytes.len() {
        chunks_seen = chunks_seen.saturating_add(1);
        if chunks_seen > limits.max_binary_chunks {
            return Err(ProjectError::LimitExceeded {
                resource: "binary chunk count",
                actual: chunks_seen.into(),
                limit: limits.max_binary_chunks.into(),
            });
        }
        if bytes.len() - offset < 16 {
            return Err(invalid("binary file ends inside a chunk header"));
        }

        let name: [u8; 4] = bytes[offset..offset + 4]
            .try_into()
            .expect("chunk name length was checked");
        let compressed_len = read_u32(bytes, offset + 4)? as usize;
        let uncompressed_len = read_u32(bytes, offset + 8)? as usize;
        let reserved = read_u32(bytes, offset + 12)?;
        offset += 16;

        if reserved != 0 {
            return Err(invalid(format!(
                "binary chunk {:?} has nonzero reserved bytes",
                printable_chunk_name(name)
            )));
        }
        if uncompressed_len as u64 > MAX_BINARY_CHUNK_BYTES {
            return Err(ProjectError::LimitExceeded {
                resource: "uncompressed chunk byte",
                actual: uncompressed_len as u64,
                limit: MAX_BINARY_CHUNK_BYTES,
            });
        }
        total_uncompressed = total_uncompressed
            .checked_add(uncompressed_len as u64)
            .ok_or_else(|| invalid("binary uncompressed byte count overflowed"))?;
        if total_uncompressed > limits.max_uncompressed_bytes {
            return Err(ProjectError::LimitExceeded {
                resource: "combined uncompressed byte",
                actual: total_uncompressed,
                limit: limits.max_uncompressed_bytes,
            });
        }

        let payload_len = if compressed_len == 0 {
            uncompressed_len
        } else {
            compressed_len
        };
        let payload_end = offset
            .checked_add(payload_len)
            .ok_or_else(|| invalid("binary chunk length overflowed"))?;
        if payload_end > bytes.len() {
            return Err(invalid("binary chunk extends past end of file"));
        }
        let payload = &bytes[offset..payload_end];

        if !is_known_chunk(name) {
            return Err(ProjectError::UnsupportedData(format!(
                "binary chunk {:?} is not understood and cannot be preserved",
                printable_chunk_name(name)
            )));
        }
        if end_seen {
            return Err(invalid("binary chunk appears after the END chunk"));
        }
        if matches!(&name, b"META" | b"SSTR" | b"PRNT") && !single_chunks.insert(name) {
            return Err(invalid(format!(
                "binary file contains more than one {:?} chunk",
                printable_chunk_name(name)
            )));
        }

        let decoded;
        let chunk = if compressed_len == 0 {
            if payload.len() != uncompressed_len {
                return Err(invalid("uncompressed binary chunk length does not match"));
            }
            payload
        } else {
            if compressed_len < 4 {
                return Err(invalid("compressed binary chunk is too short"));
            }
            decoded = decompress_chunk(payload, uncompressed_len)?;
            &decoded
        };

        match &name {
            b"META" => validate_metadata_chunk(chunk)?,
            b"SSTR" => validate_shared_strings_chunk(chunk, limits)?,
            b"INST" => {
                if total_instances > declared_instances as u64 {
                    return Err(invalid(
                        "binary INST chunks exceed the declared instance count",
                    ));
                }
                let remaining_instances = (declared_instances as u64 - total_instances) as u32;
                let (class_id, class_name, count, referents) =
                    validate_instance_chunk(chunk, limits, remaining_instances)?;
                if class_infos
                    .insert(
                        class_id,
                        BinaryClassInfo {
                            name: class_name.clone(),
                            count,
                        },
                    )
                    .is_some()
                {
                    return Err(invalid(format!(
                        "binary file declares duplicate INST class ID {class_id}"
                    )));
                }
                if !class_names.insert(class_name.clone()) {
                    return Err(invalid(format!(
                        "binary file declares class {class_name:?} more than once"
                    )));
                }
                for referent in referents {
                    if referent == -1 || !instance_referents.insert(referent) {
                        return Err(invalid(format!(
                            "binary file contains an invalid or duplicate instance referent {referent}"
                        )));
                    }
                }
                total_instances = total_instances
                    .checked_add(count as u64)
                    .ok_or_else(|| invalid("binary instance count overflowed"))?;
            }
            b"PROP" => validate_property_chunk(
                chunk,
                &class_infos,
                &mut property_names,
                &instance_referents,
                limits,
                database,
            )?,
            b"PRNT" => {
                if total_instances != declared_instances as u64 {
                    return Err(invalid("binary PRNT chunk appears before all INST chunks"));
                }
                validate_parent_chunk(chunk, limits, &instance_referents)?;
            }
            b"END\0" => {
                if compressed_len != 0 || chunk != b"</roblox>" {
                    return Err(invalid("binary END chunk is malformed or compressed"));
                }
                end_seen = true;
                if payload_end != bytes.len() {
                    return Err(invalid("trailing bytes appear after the END chunk"));
                }
            }
            _ => unreachable!("all other binary chunks were rejected above"),
        }

        offset = payload_end;
    }

    if !end_seen {
        return Err(invalid("binary file has no END chunk"));
    }
    if !single_chunks.contains(b"PRNT") {
        return Err(invalid("binary file has no PRNT chunk"));
    }
    if class_infos.len() as u32 != declared_classes {
        return Err(invalid(format!(
            "binary header declares {declared_classes} classes but {} INST chunks were found",
            class_infos.len()
        )));
    }
    if total_instances != declared_instances as u64 {
        return Err(invalid(format!(
            "binary header declares {declared_instances} instances but {total_instances} were found"
        )));
    }

    Ok(())
}

/// Validate the XML tags and references that the selected `rbx_xml` release
/// can preserve. `ReadUnknown` skips unknown value tags and unresolved references.
pub(super) fn validate_xml(bytes: &[u8], limits: ReadLimits) -> Result<(), ProjectError> {
    const SUPPORTED_PROPERTY_TAGS: &[&str] = &[
        "Axes",
        "BinaryString",
        "Color3",
        "Color3uint8",
        "ColorSequence",
        "Content",
        "ContentId",
        "CoordinateFrame",
        "Faces",
        "Font",
        "NumberRange",
        "NumberSequence",
        "NetAssetRef",
        "OptionalCoordinateFrame",
        "PhysicalProperties",
        "ProtectedString",
        "Ray",
        "Rect2D",
        "Ref",
        "SecurityCapabilities",
        "SharedString",
        "UDim",
        "UDim2",
        "UniqueId",
        "Vector2",
        "Vector2int16",
        "Vector3",
        "Vector3int16",
        "bool",
        "double",
        "float",
        "int",
        "int64",
        "string",
        "token",
    ];

    let parser = EventReader::new(std::io::Cursor::new(bytes));
    let mut stack: Vec<String> = Vec::new();
    let mut item_property_names: Vec<HashSet<String>> = Vec::new();
    let mut item_count = 0u32;
    let mut referents = HashSet::new();
    let mut property_references = Vec::new();
    let mut shared_string_ids = HashSet::new();
    let mut shared_string_references = Vec::new();
    let mut pending_value: Option<PendingXmlValue> = None;
    let mut pending_sequence: Option<PendingSequenceValue> = None;

    for event in parser {
        match event.map_err(|error| invalid(format!("invalid XML: {error}")))? {
            XmlEvent::StartElement {
                name, attributes, ..
            } => {
                if name.namespace.is_some() || name.prefix.is_some() {
                    return Err(ProjectError::UnsupportedData(
                        "XML namespaces are not supported in project files".to_owned(),
                    ));
                }
                let local_name = name.local_name;
                if local_name == "Meta" {
                    return Err(ProjectError::UnsupportedData(
                        "XML file-level Meta entries cannot currently be retained".to_owned(),
                    ));
                }
                if local_name == "External" {
                    return Err(ProjectError::UnsupportedData(
                        "XML External reference entries cannot currently be retained".to_owned(),
                    ));
                }

                validate_xml_attributes(&local_name, &stack, &attributes)?;
                if stack.is_empty() {
                    if local_name != "roblox" {
                        return Err(invalid("XML project root is not named roblox"));
                    }
                    let version = attribute(&attributes, "version").ok_or_else(|| {
                        invalid("XML project root does not declare a format version")
                    })?;
                    if version != "4" {
                        return Err(ProjectError::UnsupportedData(format!(
                            "XML project format version {version:?} is unsupported"
                        )));
                    }
                }

                let parent = stack.last().map(String::as_str);
                let is_property_value = parent == Some("Properties");
                if is_property_value {
                    if !SUPPORTED_PROPERTY_TAGS.contains(&local_name.as_str()) {
                        return Err(ProjectError::UnsupportedData(format!(
                            "XML property value tag {local_name:?} is not supported by the selected decoder"
                        )));
                    }
                    let property_name = attribute(&attributes, "name")
                        .ok_or_else(|| invalid("XML property value has no name attribute"))?;
                    let item_properties = item_property_names
                        .last_mut()
                        .ok_or_else(|| invalid("XML Properties element is outside an Item"))?;
                    if !item_properties.insert(property_name.to_owned()) {
                        return Err(ProjectError::UnsupportedData(format!(
                            "XML Item has duplicate property {property_name:?}"
                        )));
                    }

                    if matches!(local_name.as_str(), "Ref" | "SharedString" | "NetAssetRef") {
                        pending_value = Some(PendingXmlValue {
                            tag: local_name.clone(),
                            text: String::new(),
                        });
                    }
                    let values_per_keypoint = match local_name.as_str() {
                        "NumberSequence" => Some(3),
                        "ColorSequence" => Some(5),
                        _ => None,
                    };
                    if let Some(values_per_keypoint) = values_per_keypoint {
                        pending_sequence = Some(PendingSequenceValue {
                            tag: local_name.clone(),
                            values_per_keypoint,
                            token_count: 0,
                            in_token: false,
                        });
                    }
                }

                if local_name == "Item" {
                    item_count = item_count.saturating_add(1);
                    if item_count > limits.max_instances {
                        return Err(ProjectError::LimitExceeded {
                            resource: "instance count",
                            actual: item_count.into(),
                            limit: limits.max_instances.into(),
                        });
                    }
                    item_property_names.push(HashSet::new());
                    if let Some(referent) = attribute(&attributes, "referent")
                        && !referents.insert(referent.to_owned())
                    {
                        return Err(ProjectError::UnsupportedData(format!(
                            "XML file contains duplicate referent {referent:?}"
                        )));
                    }
                }

                if local_name == "SharedString" && parent == Some("SharedStrings") {
                    let hash = attribute(&attributes, "md5").ok_or_else(|| {
                        invalid("XML SharedString dictionary entry has no md5 attribute")
                    })?;
                    if !shared_string_ids.insert(hash.to_owned()) {
                        return Err(ProjectError::UnsupportedData(format!(
                            "XML SharedStrings dictionary contains duplicate hash {hash:?}"
                        )));
                    }
                }

                if stack.len() >= limits.max_xml_depth {
                    return Err(ProjectError::LimitExceeded {
                        resource: "XML nesting depth",
                        actual: stack.len() as u64 + 1,
                        limit: limits.max_xml_depth as u64,
                    });
                }
                stack.push(local_name);
            }
            XmlEvent::Characters(text) | XmlEvent::CData(text) => {
                if let Some(value) = pending_value.as_mut() {
                    value.text.push_str(&text);
                }
                if let Some(sequence) = pending_sequence.as_mut() {
                    sequence.consume(&text);
                }
            }
            // `rbx_xml` filters these whitespace-only nodes before reading tag
            // contents, so the preflight must use the same interpretation.
            XmlEvent::Whitespace(_) => {
                if let Some(sequence) = pending_sequence.as_mut() {
                    sequence.finish_token();
                }
            }
            XmlEvent::EndElement { name } => {
                if pending_sequence
                    .as_ref()
                    .is_some_and(|sequence| sequence.tag == name.local_name)
                {
                    let mut sequence = pending_sequence
                        .take()
                        .expect("pending sequence was checked");
                    sequence.finish_token();
                    let keypoints = sequence.token_count.div_ceil(sequence.values_per_keypoint);
                    if keypoints > u64::from(limits.max_property_elements) {
                        return Err(ProjectError::LimitExceeded {
                            resource: "sequence keypoint count",
                            actual: keypoints,
                            limit: u64::from(limits.max_property_elements),
                        });
                    }
                }
                if let Some(value) = pending_value.as_ref()
                    && value.tag == name.local_name
                {
                    let value = pending_value.take().expect("pending value was checked");
                    match value.tag.as_str() {
                        "Ref" if value.text != "null" => {
                            property_references.push(value.text);
                        }
                        "SharedString" | "NetAssetRef" => {
                            shared_string_references.push(value.text);
                        }
                        _ => {}
                    }
                }
                if stack.pop().is_none() {
                    return Err(invalid("XML element stack underflow"));
                }
                if name.local_name == "Item" && item_property_names.pop().is_none() {
                    return Err(invalid("XML Item stack underflow"));
                }
            }
            XmlEvent::Doctype { .. } => {
                return Err(ProjectError::UnsupportedData(
                    "XML document type declarations are not accepted".to_owned(),
                ));
            }
            _ => {}
        }
    }

    for referent in property_references {
        if !referents.contains(&referent) {
            return Err(ProjectError::UnsupportedData(format!(
                "XML Ref property points to missing referent {referent:?}"
            )));
        }
    }
    for hash in shared_string_references {
        if !shared_string_ids.contains(&hash) {
            return Err(ProjectError::UnsupportedData(format!(
                "XML shared-string property points to missing hash {hash:?}"
            )));
        }
    }

    Ok(())
}

struct PendingXmlValue {
    tag: String,
    text: String,
}

struct PendingSequenceValue {
    tag: String,
    values_per_keypoint: u64,
    token_count: u64,
    in_token: bool,
}

impl PendingSequenceValue {
    fn consume(&mut self, text: &str) {
        for byte in text.bytes() {
            if byte.is_ascii_whitespace() {
                self.finish_token();
            } else {
                self.in_token = true;
            }
        }
    }

    fn finish_token(&mut self) {
        if self.in_token {
            self.token_count = self.token_count.saturating_add(1);
            self.in_token = false;
        }
    }
}

fn validate_xml_attributes(
    element: &str,
    stack: &[String],
    attributes: &[OwnedAttribute],
) -> Result<(), ProjectError> {
    let parent = stack.last().map(String::as_str);
    let allowed = match element {
        "roblox" if stack.is_empty() => &["version"][..],
        "Item" => &["class", "referent"][..],
        "SharedString" if parent == Some("SharedStrings") => &["md5"][..],
        _ if parent == Some("Properties") => &["name"][..],
        _ => &[][..],
    };

    for attribute in attributes {
        if attribute.name.namespace.is_some() || attribute.name.prefix.is_some() {
            return Err(ProjectError::UnsupportedData(
                "XML attributes with namespaces are not supported".to_owned(),
            ));
        }
        if !allowed.contains(&attribute.name.local_name.as_str()) {
            return Err(ProjectError::UnsupportedData(format!(
                "XML attribute {:?} on element {element:?} cannot be preserved",
                attribute.name.local_name
            )));
        }
    }
    Ok(())
}

fn attribute<'a>(attributes: &'a [OwnedAttribute], name: &str) -> Option<&'a str> {
    attributes
        .iter()
        .find(|attribute| attribute.name.local_name == name)
        .map(|attribute| attribute.value.as_str())
}

#[derive(Debug)]
struct BinaryClassInfo {
    name: String,
    count: u32,
}

fn is_known_chunk(name: [u8; 4]) -> bool {
    matches!(
        &name,
        b"META" | b"SSTR" | b"INST" | b"PROP" | b"PRNT" | b"END\0"
    )
}

fn printable_chunk_name(name: [u8; 4]) -> String {
    String::from_utf8_lossy(&name)
        .trim_end_matches('\0')
        .to_owned()
}

fn decompress_chunk(payload: &[u8], expected_len: usize) -> Result<Vec<u8>, ProjectError> {
    let output = if payload.starts_with(b"\x28\xb5\x2f\xfd") {
        zstd::bulk::decompress(payload, expected_len)
            .map_err(|error| invalid(format!("invalid Zstandard chunk: {error}")))?
    } else {
        lz4_flex::block::decompress(payload, expected_len)
            .map_err(|error| invalid(format!("invalid LZ4 chunk: {error}")))?
    };
    if output.len() != expected_len {
        return Err(invalid(format!(
            "decompressed chunk has {} bytes, expected {expected_len}",
            output.len()
        )));
    }
    Ok(output)
}

fn validate_metadata_chunk(data: &[u8]) -> Result<(), ProjectError> {
    let mut offset = 0;
    let count = read_u32(data, offset)?;
    offset += 4;
    if count != 0 {
        return Err(ProjectError::UnsupportedData(
            "binary META entries cannot currently be retained".to_owned(),
        ));
    }
    if offset != data.len() {
        return Err(invalid("binary META chunk has trailing data"));
    }
    Ok(())
}

fn validate_shared_strings_chunk(data: &[u8], limits: ReadLimits) -> Result<(), ProjectError> {
    let mut offset = 0;
    let version = read_u32(data, offset)?;
    offset += 4;
    if version != 0 {
        return Err(ProjectError::UnsupportedData(format!(
            "binary SSTR version {version} is unsupported"
        )));
    }
    let count = read_u32(data, offset)?;
    offset += 4;
    if count > limits.max_instances {
        return Err(ProjectError::LimitExceeded {
            resource: "shared string count",
            actual: count.into(),
            limit: limits.max_instances.into(),
        });
    }
    let minimum_bytes = (count as u64)
        .checked_mul(20)
        .and_then(|minimum| minimum.checked_add(8))
        .ok_or_else(|| invalid("binary shared string count overflowed"))?;
    if minimum_bytes > data.len() as u64 {
        return Err(invalid(
            "binary SSTR chunk is too short for its entry count",
        ));
    }
    for _ in 0..count {
        skip(data, &mut offset, 16)?;
        let _ = read_binary_string(data, &mut offset)?;
    }
    if offset != data.len() {
        return Err(invalid("binary SSTR chunk has trailing data"));
    }
    Ok(())
}

fn validate_instance_chunk(
    data: &[u8],
    limits: ReadLimits,
    remaining_instances: u32,
) -> Result<(u32, String, u32, Vec<i32>), ProjectError> {
    let mut offset = 0;
    let class_id = read_u32(data, offset)?;
    offset += 4;
    let class_name = read_binary_string(data, &mut offset)?;
    let class_name = std::str::from_utf8(class_name)
        .map_err(|_| invalid("binary INST class name is not UTF-8"))?
        .to_owned();
    let object_format = *data
        .get(offset)
        .ok_or_else(|| invalid("binary INST chunk omits object format"))?;
    offset += 1;
    if object_format > 1 {
        return Err(ProjectError::UnsupportedData(format!(
            "binary INST object format {object_format} is unsupported"
        )));
    }
    let count = read_u32(data, offset)?;
    offset += 4;
    if count > limits.max_instances {
        return Err(ProjectError::LimitExceeded {
            resource: "instance count",
            actual: count.into(),
            limit: limits.max_instances.into(),
        });
    }
    if count > remaining_instances {
        return Err(invalid(
            "binary INST chunk exceeds the remaining declared instance count",
        ));
    }
    let referents = read_referent_array(data, &mut offset, count)?;
    let service_bytes = if object_format == 1 { count as u64 } else { 0 };
    let required = (offset as u64)
        .checked_add(service_bytes)
        .ok_or_else(|| invalid("binary INST chunk size overflowed"))?;
    if required != data.len() as u64 {
        return Err(invalid(
            "binary INST chunk length does not match its instance count",
        ));
    }
    Ok((class_id, class_name, count, referents))
}

fn validate_property_chunk(
    data: &[u8],
    class_infos: &HashMap<u32, BinaryClassInfo>,
    property_names: &mut HashSet<(u32, String)>,
    instance_referents: &HashSet<i32>,
    limits: ReadLimits,
    database: &ReflectionDatabase<'_>,
) -> Result<(), ProjectError> {
    let mut offset = 0;
    let class_id = read_u32(data, offset)?;
    offset += 4;
    let property_name = read_binary_string(data, &mut offset)?;
    let property_name = std::str::from_utf8(property_name)
        .map_err(|_| invalid("binary PROP property name is not UTF-8"))?;
    if !property_names.insert((class_id, property_name.to_owned())) {
        return Err(invalid(format!(
            "binary file contains duplicate PROP chunks for {property_name:?}"
        )));
    }
    let value_type = *data.get(offset).ok_or_else(|| {
        ProjectError::UnsupportedData(format!(
            "binary property {property_name:?} has no type byte; this decoder cannot preserve that format"
        ))
    })?;
    offset += 1;

    if !is_supported_property_type(value_type) {
        return Err(ProjectError::UnsupportedData(format!(
            "binary property {property_name:?} uses unsupported value type 0x{value_type:02x}"
        )));
    }

    let class_info = class_infos.get(&class_id).ok_or_else(|| {
        invalid(format!(
            "binary PROP refers to undeclared class ID {class_id}"
        ))
    })?;
    let reflected_type = reflected_property_type(database, &class_info.name, property_name);
    if property_name != "Name"
        && reflected_type.is_some_and(|_| {
            !is_serialized_reflected_property(database, &class_info.name, property_name)
        })
    {
        return Err(ProjectError::UnsupportedData(format!(
            "binary property {}.{} is known to reflection but is not serialized by this decoder",
            class_info.name, property_name
        )));
    }

    if property_name == "Name" && value_type != 0x01 {
        return Err(invalid(
            "binary Name property does not use the String wire type",
        ));
    }

    // Unknown property names use the wire type's default Variant, which is
    // BinaryString for type 0x01. The pinned decoder only performs a lossy
    // conversion when reflection says the canonical property is a String.
    let must_be_utf8 = property_name == "Name" || reflected_type == Some(VariantType::String);
    if value_type == 0x01 {
        for _ in 0..class_info.count {
            let value = read_binary_string(data, &mut offset)?;
            if must_be_utf8 {
                std::str::from_utf8(value).map_err(|_| {
                    ProjectError::UnsupportedData(format!(
                        "binary string property {}.{} contains non-UTF-8 bytes that the selected decoder would replace",
                        class_info.name, property_name
                    ))
                })?;
            }
        }
        if offset != data.len() {
            return Err(invalid(format!(
                "binary string property {property_name:?} has trailing data"
            )));
        }
    } else if value_type == 0x13 {
        for referent in read_referent_array(data, &mut offset, class_info.count)? {
            if referent != -1 && !instance_referents.contains(&referent) {
                return Err(ProjectError::UnsupportedData(format!(
                    "binary Ref property {}.{} points to missing referent {referent}",
                    class_info.name, property_name
                )));
            }
        }
        if offset != data.len() {
            return Err(invalid(format!(
                "binary Ref property {property_name:?} has trailing data"
            )));
        }
    } else if value_type == 0x15 || value_type == 0x16 {
        validate_sequence_property(data, offset, class_info.count, value_type, limits)?;
    } else if value_type == 0x22 {
        validate_content_property(data, offset, class_info.count, instance_referents, limits)?;
    } else {
        validate_fixed_property(data, offset, class_info.count, value_type)?;
    }
    Ok(())
}

fn validate_fixed_property(
    data: &[u8],
    mut offset: usize,
    instance_count: u32,
    value_type: u8,
) -> Result<(), ProjectError> {
    if value_type == 0x10 {
        skip_cframe_rotations(data, &mut offset, instance_count)?;
        skip(data, &mut offset, array_bytes(instance_count, 12)?)?;
    } else if value_type == 0x19 {
        for _ in 0..instance_count {
            let discriminator = *data
                .get(offset)
                .ok_or_else(|| invalid("binary PhysicalProperties value is truncated"))?;
            skip(data, &mut offset, 1)?;
            match discriminator {
                0b00 | 0b10 => {}
                0b01 => skip(data, &mut offset, 20)?,
                0b11 => skip(data, &mut offset, 24)?,
                _ => {
                    return Err(invalid(format!(
                        "binary PhysicalProperties discriminator {discriminator} is invalid"
                    )));
                }
            }
        }
    } else if value_type == 0x1e {
        let cframe_marker = *data
            .get(offset)
            .ok_or_else(|| invalid("binary OptionalCFrame omits its CFrame marker"))?;
        if cframe_marker != 0x10 {
            return Err(invalid(
                "binary OptionalCFrame has an invalid CFrame marker",
            ));
        }
        skip(data, &mut offset, 1)?;
        skip_cframe_rotations(data, &mut offset, instance_count)?;
        skip(data, &mut offset, array_bytes(instance_count, 12)?)?;

        let bool_marker = *data
            .get(offset)
            .ok_or_else(|| invalid("binary OptionalCFrame omits its Bool marker"))?;
        if bool_marker != 0x02 {
            return Err(invalid("binary OptionalCFrame has an invalid Bool marker"));
        }
        skip(data, &mut offset, 1)?;
        let bool_count = array_bytes(instance_count, 1)?;
        let bool_end = offset
            .checked_add(bool_count)
            .ok_or_else(|| invalid("binary OptionalCFrame length overflowed"))?;
        let bool_values = data
            .get(offset..bool_end)
            .ok_or_else(|| invalid("binary OptionalCFrame values are truncated"))?;
        if bool_values.iter().any(|value| *value > 1) {
            return Err(invalid(
                "binary OptionalCFrame contains a non-canonical Bool value",
            ));
        }
        offset = bool_end;
    } else if value_type == 0x20 {
        for _ in 0..instance_count {
            let _ = read_binary_string(data, &mut offset)?;

            let weight_bytes = data
                .get(offset..offset + 2)
                .ok_or_else(|| invalid("binary Font weight is truncated"))?;
            let weight = u16::from_le_bytes([weight_bytes[0], weight_bytes[1]]);
            if FontWeight::from_u16(weight).is_none() {
                return Err(invalid(format!(
                    "binary Font weight {weight} is unsupported"
                )));
            }
            skip(data, &mut offset, 2)?;

            let style = *data
                .get(offset)
                .ok_or_else(|| invalid("binary Font style is truncated"))?;
            if FontStyle::from_u8(style).is_none() {
                return Err(invalid(format!("binary Font style {style} is unsupported")));
            }
            skip(data, &mut offset, 1)?;
            let _ = read_binary_string(data, &mut offset)?;
        }
    } else {
        let bytes_per_value = match value_type {
            0x02 | 0x09 | 0x0a => 1,
            0x03 | 0x04 | 0x0b | 0x12 | 0x1c => 4,
            0x05 | 0x06 | 0x17 | 0x21 => 8,
            0x07 => 16,
            0x08 => 24,
            0x0c | 0x0e => 12,
            0x0d => 8,
            0x14 => 6,
            0x18 => 16,
            0x1a => 3,
            0x1b => 8,
            0x1f => 16,
            _ => {
                return Err(ProjectError::UnsupportedData(format!(
                    "binary property type 0x{value_type:02x} has no guarded payload parser"
                )));
            }
        };

        if value_type == 0x02 {
            let bool_count = array_bytes(instance_count, bytes_per_value)?;
            let bool_end = offset
                .checked_add(bool_count)
                .ok_or_else(|| invalid("binary Bool property length overflowed"))?;
            let bool_values = data
                .get(offset..bool_end)
                .ok_or_else(|| invalid("binary Bool property is truncated"))?;
            if bool_values.iter().any(|value| *value > 1) {
                return Err(invalid(
                    "binary Bool property contains a non-canonical Bool value",
                ));
            }
            offset = bool_end;
        } else {
            skip(
                data,
                &mut offset,
                array_bytes(instance_count, bytes_per_value)?,
            )?;
        }
    }

    if offset != data.len() {
        return Err(invalid(format!(
            "binary property type 0x{value_type:02x} has trailing data"
        )));
    }
    Ok(())
}

fn skip_cframe_rotations(
    data: &[u8],
    offset: &mut usize,
    instance_count: u32,
) -> Result<(), ProjectError> {
    for _ in 0..instance_count {
        let rotation_id = *data
            .get(*offset)
            .ok_or_else(|| invalid("binary CFrame rotation identifier is truncated"))?;
        skip(data, offset, 1)?;
        if rotation_id == 0 {
            skip(data, offset, 36)?;
        }
    }
    Ok(())
}

fn array_bytes(count: u32, bytes_per_value: u64) -> Result<usize, ProjectError> {
    let bytes = u64::from(count)
        .checked_mul(bytes_per_value)
        .ok_or_else(|| invalid("binary property array size overflowed"))?;
    usize::try_from(bytes).map_err(|_| invalid("binary property exceeds the host address space"))
}

fn validate_sequence_property(
    data: &[u8],
    mut offset: usize,
    instance_count: u32,
    value_type: u8,
    limits: ReadLimits,
) -> Result<(), ProjectError> {
    // NumberSequence has three f32 values per keypoint. ColorSequence stores
    // time, RGB, and envelope as five f32 values.
    let bytes_per_element = if value_type == 0x15 { 12u64 } else { 20u64 };
    for _ in 0..instance_count {
        let count = read_u32(data, offset)?;
        offset = offset
            .checked_add(4)
            .ok_or_else(|| invalid("binary sequence offset overflowed"))?;
        if count > limits.max_property_elements {
            return Err(ProjectError::LimitExceeded {
                resource: "sequence keypoint count",
                actual: count.into(),
                limit: limits.max_property_elements.into(),
            });
        }
        let block_bytes = (count as u64)
            .checked_mul(bytes_per_element)
            .ok_or_else(|| invalid("binary sequence size overflowed"))?;
        let block_bytes = usize::try_from(block_bytes)
            .map_err(|_| invalid("binary sequence exceeds the host address space"))?;
        skip(data, &mut offset, block_bytes)?;
    }
    if offset != data.len() {
        return Err(invalid("binary sequence property has trailing data"));
    }
    Ok(())
}

fn validate_content_property(
    data: &[u8],
    mut offset: usize,
    instance_count: u32,
    instance_referents: &HashSet<i32>,
    limits: ReadLimits,
) -> Result<(), ProjectError> {
    let kinds = read_interleaved_i32_array(data, &mut offset, instance_count)?;
    let mut expected_uri_count = 0u32;
    let mut expected_object_count = 0u32;
    for kind in kinds {
        match kind {
            0 => {}
            1 => expected_uri_count = expected_uri_count.saturating_add(1),
            2 => expected_object_count = expected_object_count.saturating_add(1),
            other => {
                return Err(ProjectError::UnsupportedData(format!(
                    "binary Content property contains unsupported value discriminator {other}"
                )));
            }
        }
    }

    let uri_count = read_u32(data, offset)?;
    offset += 4;
    if uri_count > limits.max_property_elements {
        return Err(ProjectError::LimitExceeded {
            resource: "Content URI count",
            actual: uri_count.into(),
            limit: limits.max_property_elements.into(),
        });
    }
    if uri_count != expected_uri_count {
        return Err(invalid(
            "binary Content URI count does not match its instance values",
        ));
    }
    for _ in 0..uri_count {
        let value = read_binary_string(data, &mut offset)?;
        std::str::from_utf8(value).map_err(|_| invalid("binary Content URI is not UTF-8"))?;
    }

    let object_count = read_u32(data, offset)?;
    offset += 4;
    if object_count > limits.max_property_elements {
        return Err(ProjectError::LimitExceeded {
            resource: "Content object count",
            actual: object_count.into(),
            limit: limits.max_property_elements.into(),
        });
    }
    if object_count != expected_object_count {
        return Err(invalid(
            "binary Content object count does not match its instance values",
        ));
    }
    for referent in read_referent_array(data, &mut offset, object_count)? {
        if !instance_referents.contains(&referent) {
            return Err(ProjectError::UnsupportedData(format!(
                "binary Content property points to missing referent {referent}"
            )));
        }
    }

    let external_count = read_u32(data, offset)?;
    offset += 4;
    if external_count != 0 {
        return Err(ProjectError::UnsupportedData(
            "binary Content external references cannot currently be retained".to_owned(),
        ));
    }
    if offset != data.len() {
        return Err(invalid("binary Content property has trailing data"));
    }
    Ok(())
}

fn reflected_property_type(
    database: &ReflectionDatabase<'_>,
    class_name: &str,
    property_name: &str,
) -> Option<VariantType> {
    let mut class = database.classes.get(class_name)?;
    loop {
        if let Some(property) = class.properties.get(property_name) {
            let canonical = match &property.kind {
                PropertyKind::Canonical { .. } => property,
                PropertyKind::Alias { alias_for } => class.properties.get(*alias_for)?,
                _ => return None,
            };
            return Some(canonical.data_type.ty());
        }
        class = database.classes.get(class.superclass?)?;
    }
}

fn is_serialized_reflected_property(
    database: &ReflectionDatabase<'_>,
    class_name: &str,
    property_name: &str,
) -> bool {
    let Some(mut class) = database.classes.get(class_name) else {
        return false;
    };
    loop {
        if let Some(property) = class.properties.get(property_name) {
            let canonical = match &property.kind {
                PropertyKind::Canonical { .. } => property,
                PropertyKind::Alias { alias_for } => {
                    let Some(canonical) = class.properties.get(*alias_for) else {
                        return false;
                    };
                    canonical
                }
                _ => return false,
            };
            return match &canonical.kind {
                PropertyKind::Canonical { serialization } => {
                    matches!(
                        serialization,
                        PropertySerialization::Serializes
                            | PropertySerialization::SerializesAs(_)
                            | PropertySerialization::Migrate(_)
                    )
                }
                _ => false,
            };
        }
        let Some(superclass) = class.superclass else {
            return false;
        };
        let Some(parent) = database.classes.get(superclass) else {
            return false;
        };
        class = parent;
    }
}

fn validate_parent_chunk(
    data: &[u8],
    limits: ReadLimits,
    instance_referents: &HashSet<i32>,
) -> Result<(), ProjectError> {
    let version = *data
        .first()
        .ok_or_else(|| invalid("binary PRNT chunk is empty"))?;
    if version != 0 {
        return Err(ProjectError::UnsupportedData(format!(
            "binary PRNT version {version} is unsupported"
        )));
    }
    let count = read_u32(data, 1)?;
    if count > limits.max_instances {
        return Err(ProjectError::LimitExceeded {
            resource: "parent relationship count",
            actual: count.into(),
            limit: limits.max_instances.into(),
        });
    }
    if count as usize != instance_referents.len() {
        return Err(invalid(
            "binary PRNT relationship count does not match the INST referents",
        ));
    }

    let mut offset = 5;
    let subjects = read_referent_array(data, &mut offset, count)?;
    let parents = read_referent_array(data, &mut offset, count)?;
    if offset != data.len() {
        return Err(invalid(
            "binary PRNT chunk length does not match its relationship count",
        ));
    }

    let mut parent_by_instance = HashMap::with_capacity(subjects.len());
    for (subject, parent) in subjects.into_iter().zip(parents) {
        if !instance_referents.contains(&subject) {
            return Err(invalid(format!(
                "binary PRNT refers to missing subject {subject}"
            )));
        }
        if parent != -1 && !instance_referents.contains(&parent) {
            return Err(invalid(format!(
                "binary PRNT refers to missing parent {parent}"
            )));
        }
        if subject == parent {
            return Err(invalid("binary PRNT instance is its own parent"));
        }
        if parent_by_instance.insert(subject, parent).is_some() {
            return Err(invalid(format!(
                "binary PRNT contains duplicate subject {subject}"
            )));
        }
    }
    if parent_by_instance.len() != instance_referents.len() {
        return Err(invalid("binary PRNT omits one or more INST referents"));
    }

    // `rbx_binary` assumes every instance belongs to one rooted tree while
    // finalizing the DOM. Reject cycles before that code can silently orphan
    // their instances or panic when it removes a missing referent.
    let mut visit_state = HashMap::with_capacity(parent_by_instance.len());
    for &start in instance_referents {
        let mut path = Vec::new();
        let mut current = start;
        loop {
            if current == -1 {
                break;
            }
            match visit_state.get(&current) {
                Some(1) => return Err(invalid("binary PRNT contains a parent cycle")),
                Some(2) => break,
                _ => {}
            }
            visit_state.insert(current, 1u8);
            path.push(current);
            current = *parent_by_instance
                .get(&current)
                .ok_or_else(|| invalid("binary PRNT does not define every instance"))?;
        }
        for referent in path {
            visit_state.insert(referent, 2u8);
        }
    }

    Ok(())
}

fn is_supported_property_type(value_type: u8) -> bool {
    matches!(
        value_type,
        0x01 | 0x02
            | 0x03
            | 0x04
            | 0x05
            | 0x06
            | 0x07
            | 0x08
            | 0x09
            | 0x0a
            | 0x0b
            | 0x0c
            | 0x0d
            | 0x0e
            | 0x10
            | 0x12
            | 0x13
            | 0x14
            | 0x15
            | 0x16
            | 0x17
            | 0x18
            | 0x19
            | 0x1a
            | 0x1b
            | 0x1c
            | 0x1e
            | 0x1f
            | 0x20
            | 0x21
            | 0x22
    )
}

fn read_interleaved_i32_array(
    data: &[u8],
    offset: &mut usize,
    count: u32,
) -> Result<Vec<i32>, ProjectError> {
    let count = usize::try_from(count)
        .map_err(|_| invalid("binary array count exceeds the host address space"))?;
    let byte_count = count
        .checked_mul(4)
        .ok_or_else(|| invalid("binary interleaved array size overflowed"))?;
    let end = offset
        .checked_add(byte_count)
        .ok_or_else(|| invalid("binary interleaved array offset overflowed"))?;
    let interleaved = data
        .get(*offset..end)
        .ok_or_else(|| invalid("binary interleaved array is truncated"))?;
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        let transformed = i32::from_be_bytes([
            interleaved[index],
            interleaved[index + count],
            interleaved[index + count * 2],
            interleaved[index + count * 3],
        ]);
        values.push(untransform_i32(transformed));
    }
    *offset = end;
    Ok(values)
}

fn read_referent_array(
    data: &[u8],
    offset: &mut usize,
    count: u32,
) -> Result<Vec<i32>, ProjectError> {
    let deltas = read_interleaved_i32_array(data, offset, count)?;
    let mut referents = Vec::with_capacity(deltas.len());
    let mut previous = 0i32;
    for delta in deltas {
        previous = previous
            .checked_add(delta)
            .ok_or_else(|| invalid("binary referent delta overflows signed 32-bit range"))?;
        referents.push(previous);
    }
    Ok(referents)
}

fn untransform_i32(value: i32) -> i32 {
    ((value as u32) >> 1) as i32 ^ -(value & 1)
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, ProjectError> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| invalid("binary field offset overflowed"))?;
    let bytes: [u8; 4] = data
        .get(offset..end)
        .ok_or_else(|| invalid("binary field is truncated"))?
        .try_into()
        .expect("four-byte field length was checked");
    Ok(u32::from_le_bytes(bytes))
}

fn read_binary_string<'a>(data: &'a [u8], offset: &mut usize) -> Result<&'a [u8], ProjectError> {
    let length = read_u32(data, *offset)? as usize;
    *offset = offset
        .checked_add(4)
        .ok_or_else(|| invalid("binary string offset overflowed"))?;
    let end = offset
        .checked_add(length)
        .ok_or_else(|| invalid("binary string length overflowed"))?;
    let value = data
        .get(*offset..end)
        .ok_or_else(|| invalid("binary string is truncated"))?;
    *offset = end;
    Ok(value)
}

fn skip(data: &[u8], offset: &mut usize, count: usize) -> Result<(), ProjectError> {
    let end = offset
        .checked_add(count)
        .ok_or_else(|| invalid("binary field length overflowed"))?;
    if end > data.len() {
        return Err(invalid("binary field is truncated"));
    }
    *offset = end;
    Ok(())
}

fn invalid(message: impl Into<String>) -> ProjectError {
    ProjectError::UnsupportedData(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_limits_reject_untrusted_counts_before_allocating() {
        let data = u32::MAX.to_le_bytes();
        assert!(matches!(
            validate_sequence_property(&data, 0, 1, 0x15, ReadLimits::default()),
            Err(ProjectError::LimitExceeded {
                resource: "sequence keypoint count",
                ..
            })
        ));
    }

    #[test]
    fn content_limits_reject_untrusted_uri_counts_before_allocating() {
        // One interleaved i32 value of 1 (zigzag encoding: 2), followed by an
        // excessive URI count. No URI buffers are allocated before this check.
        let data = [0, 0, 0, 2, 0xff, 0xff, 0xff, 0xff];
        assert!(matches!(
            validate_content_property(&data, 0, 1, &HashSet::new(), ReadLimits::default()),
            Err(ProjectError::LimitExceeded {
                resource: "Content URI count",
                ..
            })
        ));
    }
}
