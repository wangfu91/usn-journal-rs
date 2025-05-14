use std::mem::size_of;

use windows::Win32::{
    Foundation::HANDLE,
    Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

pub(crate) fn is_elevated() -> windows::core::Result<bool> {
    let mut handle: HANDLE = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle)? };

    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned_length = 0;

    unsafe {
        GetTokenInformation(
            handle,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned_length,
        )?
    };

    Ok(elevation.TokenIsElevated != 0)
}
