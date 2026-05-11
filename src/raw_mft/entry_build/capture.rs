//! Small attribute payload helpers used during entry construction.

use crate::raw_mft::layout::attribute::NtfsAttribute;

use super::entry::AttributeListInfo;

/// Capture raw `$ATTRIBUTE_LIST` bytes from either a resident or non-resident
/// attribute payload.
pub(super) fn capture_attribute_list(attr: &NtfsAttribute<'_>) -> Option<AttributeListInfo> {
    if attr.is_non_resident() {
        let header = attr.nonresident_header()?;
        let runs_offset = header.data_runs_offset as usize;
        let attr_bytes = attr.data();
        if runs_offset > attr_bytes.len() {
            return None;
        }

        Some(AttributeListInfo::NonResident {
            runs_data: attr_bytes[runs_offset..].to_vec(),
            data_size: header.data_size,
        })
    } else {
        attr.resident_value()
            .map(|value| AttributeListInfo::Resident(value.to_vec()))
    }
}

/// Decode the reparse tag stored in a resident `$REPARSE_POINT` value.
pub(super) fn resident_reparse_tag(attr: &NtfsAttribute<'_>) -> Option<u32> {
    let value = attr.resident_value()?;
    let tag_bytes = value.get(..4)?;
    Some(u32::from_le_bytes([
        tag_bytes[0],
        tag_bytes[1],
        tag_bytes[2],
        tag_bytes[3],
    ]))
}
