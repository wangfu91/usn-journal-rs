//! Shared FILE-record attribute dispatch for raw-MFT entry builders.

use crate::raw_mft::ondisk::{
    attribute::{NtfsAttribute, NtfsAttributeType, for_each_attribute},
    record::FileRecord,
};

use super::{capture::capture_attribute_list, entry::AttributeListInfo};

/// Consumer of typed NTFS attributes from one FILE record.
pub(super) trait AttributeConsumer {
    /// Fold a `$STANDARD_INFORMATION` attribute.
    fn on_standard_information(&mut self, attr: &NtfsAttribute<'_>);

    /// Fold a `$FILE_NAME` attribute.
    fn on_file_name(&mut self, attr: &NtfsAttribute<'_>);

    /// Fold a `$DATA` attribute.
    fn on_data(&mut self, attr: &NtfsAttribute<'_>);

    /// Fold a `$REPARSE_POINT` attribute.
    fn on_reparse_point(&mut self, attr: &NtfsAttribute<'_>);

    /// Capture a `$ATTRIBUTE_LIST` payload for later enrichment.
    fn on_attribute_list(&mut self, attr_list: AttributeListInfo);
}

/// Walk the attributes of a parsed FILE record and dispatch the attribute
/// types used by raw-MFT entry construction.
pub(super) fn fold_record_attributes<C>(record: &FileRecord<'_>, consumer: &mut C)
where
    C: AttributeConsumer,
{
    let (attrs_off, used) = record.attrs_range();
    for_each_attribute(record.data, attrs_off, used, |attr| {
        let type_id = attr.type_id();
        if type_id == NtfsAttributeType::StandardInformation as u32 {
            consumer.on_standard_information(attr);
        } else if type_id == NtfsAttributeType::FileName as u32 {
            consumer.on_file_name(attr);
        } else if type_id == NtfsAttributeType::Data as u32 {
            consumer.on_data(attr);
        } else if type_id == NtfsAttributeType::ReparsePoint as u32 {
            consumer.on_reparse_point(attr);
        } else if type_id == NtfsAttributeType::AttributeList as u32
            && let Some(attr_list) = capture_attribute_list(attr)
        {
            consumer.on_attribute_list(attr_list);
        }
    });
}
