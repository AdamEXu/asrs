// Ported from three.js r168 (MIT, Copyright 2010-2024 three.js authors). See THIRD_PARTY_NOTICES.md.

use super::{ArrayData, ArrayKind, BufferAttribute, BufferGeometry};

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

/// `BufferGeometryUtils.mergeGeometries` for geometries without morph attributes. Each source
/// array is freed once it is merged.
pub fn merge_geometries(
    mut geometries: Vec<BufferGeometry>,
    use_groups: bool,
) -> Result<BufferGeometry, MergeError> {
    let first = &geometries[0];
    let is_indexed = first.index.is_some();
    let used: Vec<String> = first.attribute_names().map(str::to_owned).collect();
    let mut names: Vec<String> = Vec::new();
    let mut merged = BufferGeometry::new();
    let mut offset = 0;

    for (i, geometry) in geometries.iter().enumerate() {
        if is_indexed != geometry.index.is_some() {
            return Err(MergeError::IndexMismatch(i));
        }
        let mut count = 0;
        for name in geometry.attribute_names() {
            if !used.iter().any(|n| n == name) {
                return Err(MergeError::UnknownAttribute(i, name.to_owned()));
            }
            if !names.iter().any(|n| n == name) {
                names.push(name.to_owned());
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
        merged.index = Some(merge_indices(&geometries)?);
        for geometry in &mut geometries {
            geometry.index = None;
        }
    }

    for name in names {
        let list: Vec<BufferAttribute> = geometries
            .iter_mut()
            .filter_map(|g| g.take_attribute(&name))
            .collect();
        match merge_attributes(&list.iter().collect::<Vec<_>>()) {
            Some(attribute) => merged.set_attribute(&name, attribute),
            None => return Err(MergeError::Attribute(name)),
        }
    }
    Ok(merged)
}

/// The concatenated index, each geometry's offset by the vertices before it, in the typed array
/// `setIndex` picks.
fn merge_indices(geometries: &[BufferGeometry]) -> Result<BufferAttribute, MergeError> {
    let mut parts = Vec::with_capacity(geometries.len());
    let mut length = 0;
    let mut wide = false;
    let mut index_offset = 0.0;
    for (i, geometry) in geometries.iter().enumerate() {
        let Some(index) = &geometry.index else {
            unreachable!("checked above");
        };
        wide = wide || (0..index.count()).any(|j| index.get_x(j) + index_offset >= 65535.0);
        length += index.count();
        parts.push((index, index_offset));
        let Some(position) = geometry.attribute("position") else {
            return Err(MergeError::IndexedWithoutPosition(i));
        };
        index_offset += position.count() as f64;
    }
    let kind = if wide {
        ArrayKind::Uint32
    } else {
        ArrayKind::Uint16
    };
    let mut array = ArrayData::zeroed(kind, length);
    let mut at = 0;
    for (index, offset) in parts {
        for j in 0..index.count() {
            array.set(at, index.get_x(j) + offset);
            at += 1;
        }
    }
    Ok(BufferAttribute::new(array, 1, false))
}

/// `BufferGeometryUtils.mergeAttributes`. The result is never interleaved.
pub fn merge_attributes(attributes: &[&BufferAttribute]) -> Option<BufferAttribute> {
    let first = attributes.first()?;
    let kind = first.array.kind();
    let mut length = 0;
    for attribute in attributes {
        if attribute.array.kind() != kind
            || attribute.item_size != first.item_size
            || attribute.normalized != first.normalized
            || attribute.interleaved != first.interleaved
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
