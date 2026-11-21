// Ported from three.js r168 (MIT, Copyright 2010-2024 three.js authors). See THIRD_PARTY_NOTICES.md.

use super::{ArrayData, BufferAttribute, BufferGeometry};

/// Why `mergeGeometries` returned `null`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeError {
    IndexMismatch(usize),
    UnknownAttribute(usize, String),
    AttributeCount(usize),
    NoIndexOrPosition(usize),
    /// Upstream throws a `TypeError` reading `attributes.position.count` here.
    IndexedWithoutPosition(usize),
    Attribute(String),
}

/// `BufferGeometryUtils.mergeGeometries` for geometries without morph attributes.
pub fn merge_geometries(
    geometries: &[BufferGeometry],
    use_groups: bool,
) -> Result<BufferGeometry, MergeError> {
    let first = &geometries[0];
    let is_indexed = first.index.is_some();
    let used: Vec<&str> = first.attribute_names().collect();
    let mut attributes: Vec<(&str, Vec<&BufferAttribute>)> = Vec::new();
    let mut merged = BufferGeometry::new();
    let mut offset = 0;

    for (i, geometry) in geometries.iter().enumerate() {
        if is_indexed != geometry.index.is_some() {
            return Err(MergeError::IndexMismatch(i));
        }
        let mut count = 0;
        for (name, attribute) in geometry.attributes() {
            if !used.contains(&name) {
                return Err(MergeError::UnknownAttribute(i, name.to_owned()));
            }
            match attributes.iter_mut().find(|(n, _)| *n == name) {
                Some((_, list)) => list.push(attribute),
                None => attributes.push((name, vec![attribute])),
            }
            count += 1;
        }
        if count != used.len() {
            return Err(MergeError::AttributeCount(i));
        }
        if use_groups {
            let count = match (&geometry.index, geometry.attribute("position")) {
                (Some(index), _) if is_indexed => index.count(),
                (_, Some(position)) => position.count(),
                _ => return Err(MergeError::NoIndexOrPosition(i)),
            };
            merged.add_group(offset, count, i);
            offset += count;
        }
    }

    if is_indexed {
        let mut index_offset = 0.0;
        let mut merged_index = Vec::new();
        for (i, geometry) in geometries.iter().enumerate() {
            let Some(index) = &geometry.index else {
                unreachable!("checked above");
            };
            for j in 0..index.count() {
                merged_index.push(index.get_x(j) + index_offset);
            }
            let Some(position) = geometry.attribute("position") else {
                return Err(MergeError::IndexedWithoutPosition(i));
            };
            index_offset += position.count() as f64;
        }
        merged.set_index(&merged_index);
    }

    for (name, list) in attributes {
        let attribute =
            merge_attributes(&list).ok_or_else(|| MergeError::Attribute(name.to_owned()))?;
        merged.set_attribute(name, attribute);
    }
    Ok(merged)
}

/// `BufferGeometryUtils.mergeAttributes` for non-interleaved attributes.
pub fn merge_attributes(attributes: &[&BufferAttribute]) -> Option<BufferAttribute> {
    let first = attributes.first()?;
    let kind = first.array.kind();
    let mut length = 0;
    for attribute in attributes {
        if attribute.array.kind() != kind
            || attribute.item_size != first.item_size
            || attribute.normalized != first.normalized
        {
            return None;
        }
        length += attribute.array.len();
    }
    let mut array = ArrayData::zeroed(kind, length);
    let mut offset = 0;
    for attribute in attributes {
        array.copy_from(&attribute.array, offset);
        offset += attribute.array.len();
    }
    Some(BufferAttribute::new(
        array,
        first.item_size,
        first.normalized,
    ))
}
