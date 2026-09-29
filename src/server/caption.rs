//! Tint the native window caption towards the interface paper.
//!
//! The window keeps a real Win32 caption bar, and Windows paints it with the
//! system accent — which arrives in a colour that has nothing to do with the
//! paper the interface is made of. Windows 11 lets an application choose the
//! caption colour, so the frame joins the theme instead of fighting it.
//!
//! Every step is best effort: an older build without DwmSetWindowAttribute
//! simply keeps its default frame, which is preferable to a failed launch.
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

type SetWindowAttribute = unsafe extern "system" fn(HWND, u32, *const core::ffi::c_void, u32) -> i32;

/// DWMWA_CAPTION_COLOR / DWMWA_TEXT_COLOR from dwmapi.h.
const DWMWA_CAPTION_COLOR: u32 = 35;
const DWMWA_TEXT_COLOR: u32 = 36;

/// hanako light paper F5EFE4 and its ink 2A2622, as COLORREF (0x00BBGGRR).
const CAPTION_COLOR: u32 = 0x00E4_EFF5;
const CAPTION_TEXT: u32 = 0x0022_262A;

pub(crate) fn tint(window: HWND) {
  let library = unsafe { LoadLibraryW("dwmapi.dll\0".encode_utf16().collect::<Vec<u16>>().as_ptr()) };
  if library.is_null() {
    return;
  }
  let Some(address) = (unsafe { GetProcAddress(library, c"DwmSetWindowAttribute".as_ptr().cast::<u8>()) })
  else {
    return;
  };
  // The signature is fixed by the API; the pointer only ever names this function.
  let set: SetWindowAttribute = unsafe { std::mem::transmute(address) };
  unsafe {
    let _ = set(window, DWMWA_CAPTION_COLOR, (&CAPTION_COLOR as *const u32).cast(), 4);
    let _ = set(window, DWMWA_TEXT_COLOR, (&CAPTION_TEXT as *const u32).cast(), 4);
  }
}
