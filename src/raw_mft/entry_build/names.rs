//! File-name namespace selection and retained-link policy.

use std::ffi::OsString;

use crate::{Fid, raw_mft::ondisk::attribute::FileNameNamespace};

use super::entry::RawMftLink;

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

/// Chooses the best `$FILE_NAME` and retains additional hard-link names.
pub(super) struct FileNameSelector {
    best_namespace_score: i32,
    links: Vec<RawMftLink>,
    collect_dos_file_name_links: bool,
}

impl FileNameSelector {
    /// Create a selector configured for the caller's DOS-link policy.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn name(value: &str) -> OsString {
        OsString::from(value)
    }

    #[test]
    fn higher_namespace_score_replaces_selected_name() {
        let parent = Fid::new(5);
        let mut selector = FileNameSelector::new(true);
        assert!(selector.consider(None, FileNameNamespace::Dos, parent, &name("FILE~1.TXT")));
        let current_name = name("FILE~1.TXT");
        assert!(selector.consider(
            current_file_name(FileNameNamespace::Dos, parent, &current_name),
            FileNameNamespace::Win32,
            parent,
            &name("file.txt"),
        ));
    }

    #[test]
    fn dos_link_is_suppressed_when_non_dos_same_parent_exists() {
        let parent = Fid::new(5);
        let current_name = name("file.txt");
        let mut selector = FileNameSelector::new(false);
        assert!(selector.consider(None, FileNameNamespace::Win32, parent, &current_name));
        assert!(!selector.consider(
            current_file_name(FileNameNamespace::Win32, parent, &current_name),
            FileNameNamespace::Dos,
            parent,
            &name("FILE~1.TXT"),
        ));
        assert!(selector.into_links().is_empty());
    }

    #[test]
    fn hard_link_candidates_are_retained() {
        let first_parent = Fid::new(5);
        let second_parent = Fid::new(9);
        let current_name = name("first.txt");
        let mut selector = FileNameSelector::new(true);
        assert!(selector.consider(None, FileNameNamespace::Win32, first_parent, &current_name));
        assert!(!selector.consider(
            current_file_name(FileNameNamespace::Win32, first_parent, &current_name),
            FileNameNamespace::Win32,
            second_parent,
            &name("second.txt"),
        ));
        assert_eq!(selector.into_links().len(), 2);
    }
}
