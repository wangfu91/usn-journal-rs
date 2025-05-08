// Unwraps
#![warn(clippy::unwrap_used)] // Discourage using .unwrap() which can cause panics
#![warn(clippy::expect_used)] // Discourage using .expect() which can cause panics
#![warn(clippy::panicking_unwrap)] // Prevent unwrap on values known to cause panics
#![warn(clippy::option_env_unwrap)] // Prevent unwrapping environment variables which might be absent

// Array indexing
#![warn(clippy::indexing_slicing)] // Avoid direct array indexing and use safer methods like .get()

// Path handling
#![warn(clippy::join_absolute_paths)] // Prevent issues when joining paths with absolute paths

// Serialization issues
#![warn(clippy::serde_api_misuse)] // Prevent incorrect usage of Serde's serialization/deserialization API

// Unbounded input
#![warn(clippy::uninit_vec)] // Prevent creating uninitialized vectors which is unsafe

// Unsafe code detection
#![warn(clippy::transmute_int_to_char)] // Prevent unsafe transmutation from integers to characters
#![warn(clippy::transmute_int_to_float)] // Prevent unsafe transmutation from integers to floats
#![warn(clippy::transmute_ptr_to_ref)] // Prevent unsafe transmutation from pointers to references
#![warn(clippy::transmute_undefined_repr)] // Detect transmutes with potentially undefined representations

pub mod mft;
pub mod path_resolver;
mod tests_utils;
pub mod usn_entry;
pub mod usn_journal;
pub mod utils;

pub type Usn = i64;

pub(crate) const DEFAULT_BUFFER_SIZE: usize = 64 * 1024; // 64KB

pub const DEFAULT_JOURNAL_MAX_SIZE: u64 = 32 * 1024 * 1024; // 32MB
pub const DEFAULT_JOURNAL_ALLOCATION_DELTA: u64 = 8 * 1024 * 1024; // 4MB

pub const USN_REASON_MASK_ALL: u32 = 0xFFFFFFFF;
