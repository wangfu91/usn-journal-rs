//! Shared helpers used by the rich and lean raw-MFT entry builders.

use std::ffi::OsString;

use crate::{
    Fid,
    raw_mft::{
        attribute::{FileNameNamespace, NtfsAttribute},
        entry::{AttributeListInfo, RawMftLink},
    },
};

/// Borrowed view of the currently selected `$FILE_NAME`.
#[derive(Clone, Copy)]
pub(super) struct CurrentFileName<'a> {
    /// Namespace of the current best `$FILE_NAME`.
    pub(super) namespace: FileNameNamespace,
    /// Parent directory reference of the current best `$FILE_NAME`.
    pub(super) parent_reference: Fid,
    /// Leaf name of the current best `$FILE_NAME`.
    pub(super) file_name: &'a OsString,
}

/// Shared state for choosing the best `$FILE_NAME` and retaining any
/// additional hard-link names.
pub(super) struct FileNameTracker {
    best_namespace_score: i32,
    links: Vec<RawMftLink>,
    collect_dos_file_name_links: bool,
}

impl FileNameTracker {
    /// Create a fresh tracker configured for the caller's DOS-link policy.
    pub(super) fn new(collect_dos_file_name_links: bool) -> Self {
        Self {
            best_namespace_score: -1,
            links: Vec::new(),
            collect_dos_file_name_links,
        }
    }

    /// Fold one `$FILE_NAME` candidate into the retained-link set and report
    /// whether it becomes the new best name.
    pub(super) fn consider(
        &mut self,
        current: Option<CurrentFileName<'_>>,
        candidate_namespace: FileNameNamespace,
        candidate_parent: Fid,
        candidate_name: &OsString,
    ) -> bool {
        if !self.collect_dos_file_name_links
            && candidate_namespace == FileNameNamespace::Dos
            && self.has_non_dos_file_name_link(current, candidate_parent)
        {
            return false;
        }

        if !self.collect_dos_file_name_links && candidate_namespace != FileNameNamespace::Dos {
            self.links.retain(|link| {
                link.namespace != FileNameNamespace::Dos
                    || link.parent_reference != candidate_parent
            });
        }

        if let Some(current) = current {
            if self.links.is_empty()
                && self.should_retain_file_name_link(
                    current.namespace,
                    current.parent_reference,
                    candidate_namespace,
                    candidate_parent,
                )
            {
                self.links.push(RawMftLink {
                    parent_reference: current.parent_reference,
                    namespace: current.namespace,
                    file_name: current.file_name.clone(),
                });
            }

            if self.should_retain_file_name_link(
                candidate_namespace,
                candidate_parent,
                candidate_namespace,
                candidate_parent,
            ) {
                self.links.push(RawMftLink {
                    parent_reference: candidate_parent,
                    namespace: candidate_namespace,
                    file_name: candidate_name.clone(),
                });
            }
        }

        let score = candidate_namespace.score();
        if score > self.best_namespace_score {
            self.best_namespace_score = score;
            return true;
        }

        false
    }

    /// Finalize the retained links into the public entry representation.
    pub(super) fn into_links(self) -> Box<[RawMftLink]> {
        self.links.into_boxed_slice()
    }

    /// Return `true` when the current best name or any retained link already
    /// carries a non-DOS name for the same parent directory.
    fn has_non_dos_file_name_link(
        &self,
        current: Option<CurrentFileName<'_>>,
        parent_reference: Fid,
    ) -> bool {
        current.is_some_and(|current| {
            current.parent_reference == parent_reference
                && current.namespace != FileNameNamespace::Dos
        }) || self.links.iter().any(|link| {
            link.parent_reference == parent_reference && link.namespace != FileNameNamespace::Dos
        })
    }

    /// Decide whether a specific link should be retained when DOS aliases are
    /// being filtered.
    fn should_retain_file_name_link(
        &self,
        link_namespace: FileNameNamespace,
        link_parent: Fid,
        current_namespace: FileNameNamespace,
        current_parent: Fid,
    ) -> bool {
        if self.collect_dos_file_name_links || link_namespace != FileNameNamespace::Dos {
            return true;
        }

        let current_shadows_link =
            current_namespace != FileNameNamespace::Dos && current_parent == link_parent;
        let existing_link_shadows = self.links.iter().any(|link| {
            link.parent_reference == link_parent && link.namespace != FileNameNamespace::Dos
        });
        !current_shadows_link && !existing_link_shadows
    }
}

/// Borrow the currently selected file name when one exists.
pub(super) fn current_file_name(
    namespace: FileNameNamespace,
    parent_reference: Fid,
    file_name: &OsString,
) -> Option<CurrentFileName<'_>> {
    if file_name.is_empty() {
        None
    } else {
        Some(CurrentFileName {
            namespace,
            parent_reference,
            file_name,
        })
    }
}

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