//! The double-click desktop shell: a native window hosting the web interface
//! through the WebView2 runtime, driven directly over its published C API so the
//! executable stays one small self-contained binary.
#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;
use std::net::SocketAddr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use windows_sys::core::{GUID, PCWSTR};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows_sys::Win32::System::LibraryLoader::{
  GetProcAddress, GetModuleHandleW, LoadLibraryW,
};
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::WindowsAndMessaging::{
  CreateWindowExW, CS_HREDRAW, CS_VREDRAW, DefWindowProcW, DispatchMessageW,
  GetClientRect, GetMessageW, GetSystemMetrics, GetWindowPlacement, IDC_ARROW, LoadCursorW, MSG,
  PostMessageW, PostQuitMessage, RegisterClassW, SetWindowPlacement, ShowWindow, SW_SHOW,
  SM_CXSCREEN, SM_CYSCREEN, TranslateMessage, WINDOWPLACEMENT, WM_CLOSE, WM_DESTROY, WM_SIZE,
  WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

const S_OK: i32 = 0;

/// Stage-stamped diagnostics: every step of the window host lands in one file
/// under the temporary directory, so a failed run still tells its story.
fn log(text: &str) {
  use std::io::Write as _;
  let path = std::env::temp_dir().join("wish-window.log");
  if std::fs::metadata(&path).map(|meta| meta.len() > 1 << 20).unwrap_or(false) {
    let _ = std::fs::remove_file(&path);
  }
  if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
    let _ = writeln!(file, "{text}");
  }
}

fn format_guid(guid: &GUID) -> String {
  format!(
    "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{}",
    guid.data1,
    guid.data2,
    guid.data3,
    guid.data4[0],
    guid.data4[1],
    guid.data4.iter().skip(2).map(|byte| format!("{byte:02x}")).collect::<String>()
  )
}

/// The window the interface lives in, and the WebView2 pieces once they exist.
static WINDOW: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static CONTROLLER: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static CONTROLLER_HANDLER: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static READY: AtomicBool = AtomicBool::new(false);
static FAILURE: OnceLock<String> = OnceLock::new();
static URL: OnceLock<String> = OnceLock::new();
static PLACEMENT_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Window geometry remembered between runs, written beside the configuration.
#[derive(serde::Deserialize, serde::Serialize)]
struct Placement {
  flags: u32,
  show_cmd: u32,
  min: (i32, i32),
  max: (i32, i32),
  normal: (i32, i32, i32, i32),
}

fn load_placement(path: &Path) -> Option<WINDOWPLACEMENT> {
  let saved: Placement = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
  log("window: placement restored");
  Some(WINDOWPLACEMENT {
    length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
    flags: saved.flags,
    ptMinPosition: POINT { x: saved.min.0, y: saved.min.1 },
    ptMaxPosition: POINT { x: saved.max.0, y: saved.max.1 },
    rcNormalPosition: RECT {
      left: saved.normal.0,
      top: saved.normal.1,
      right: saved.normal.2,
      bottom: saved.normal.3,
    },
    showCmd: saved.show_cmd,
  })
}

fn save_placement(path: &Path, hwnd: HWND) {
  let placement = unsafe {
    let mut placement: WINDOWPLACEMENT = std::mem::zeroed();
    placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
    if GetWindowPlacement(hwnd, &mut placement) == 0 {
      return;
    }
    placement
  };
  // Never restore into a minimized start; keep restored and maximized only.
  let show_cmd = if placement.showCmd == 2 { 5 } else { placement.showCmd };
  let saved = Placement {
    flags: placement.flags,
    show_cmd,
    min: (placement.ptMinPosition.x, placement.ptMinPosition.y),
    max: (placement.ptMaxPosition.x, placement.ptMaxPosition.y),
    normal: (
      placement.rcNormalPosition.left,
      placement.rcNormalPosition.top,
      placement.rcNormalPosition.right,
      placement.rcNormalPosition.bottom,
    ),
  };
  if let Ok(text) = serde_json::to_string_pretty(&saved) {
    let _ = std::fs::write(path, text);
    log("window: placement saved");
  }
}

/// Open the interface window and block until the user closes it.
pub(crate) fn run(addr: SocketAddr, profile_dir: Option<PathBuf>, open_session: Option<String>) -> Result<(), String> {
  // The interface uses hash routing, so one session can be addressed directly.
  let url = match &open_session {
    Some(id) if !id.is_empty() => format!("{}/#/s/{}", super::host_url(addr), id),
    _ => super::host_url(addr),
  };
  log(&format!("window: starting for {url}"));
  let _ = URL.set(url);
  let Some(profile_dir) = profile_dir else {
    return Err("no profile directory is available beside the configuration".into());
  };
  std::fs::create_dir_all(&profile_dir).map_err(|error| format!("creating the profile directory failed: {error}"))?;
  let placement_path = profile_dir
    .parent()
    .unwrap_or(&profile_dir)
    .join("window-placement.json");
  let _ = PLACEMENT_PATH.set(placement_path);
  unsafe {
    // Crisp text on scaled displays: the window measures itself in physical pixels.
    let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    // WebView2 completions arrive on the thread that created the environment.
    CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED as u32);
    let module = GetModuleHandleW(std::ptr::null());
    let class: Vec<u16> = "WishShellWindow\0".encode_utf16().collect();
    let class = WNDCLASSW {
      style: CS_HREDRAW | CS_VREDRAW,
      lpfnWndProc: Some(wnd_proc),
      cbClsExtra: 0,
      cbWndExtra: 0,
      hInstance: module,
      hIcon: std::ptr::null_mut(),
      hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
      hbrBackground: 6usize as *mut c_void, // COLOR_WINDOW + 1
      lpszMenuName: std::ptr::null(),
      lpszClassName: class.as_ptr(),
    };
    if RegisterClassW(&class) == 0 {
      return Err("registering the window class failed".into());
    }
    // A first run centers on the primary display; later runs restore saved geometry.
    let width = 1360;
    let height = 900;
    let screen_width = GetSystemMetrics(SM_CXSCREEN).max(width);
    let screen_height = GetSystemMetrics(SM_CYSCREEN).max(height);
    let title: Vec<u16> = "Wish\0".encode_utf16().collect();
    let window = CreateWindowExW(
      0,
      class.lpszClassName,
      title.as_ptr(),
      WS_OVERLAPPEDWINDOW,
      (screen_width - width) / 2,
      (screen_height - height) / 2,
      width,
      height,
      std::ptr::null_mut(),
      std::ptr::null_mut(),
      module,
      std::ptr::null_mut(),
    );
    if window.is_null() {
      return Err("creating the window failed".into());
    }
    WINDOW.store(window, Ordering::SeqCst);
    // The frame belongs to the paper: ask DWM for a caption in the light
    // theme colour instead of the system accent. Best effort.
    super::caption::tint(window);
    // Remembered geometry, or a sensible first show.
    match load_placement(PLACEMENT_PATH.get().expect("placement path set")) {
      Some(mut placement) => {
        SetWindowPlacement(window, &mut placement);
      }
      None => {
        ShowWindow(window, SW_SHOW);
      }
    }
    let loader = load_loader()?;
    log("window: loader found");
    let create_environment = match GetProcAddress(
      loader,
      c"CreateCoreWebView2EnvironmentWithOptions".as_ptr().cast::<u8>(),
    ) {
      Some(proc) => Some(std::mem::transmute::<
        unsafe extern "system" fn() -> isize,
        unsafe extern "system" fn(PCWSTR, PCWSTR, *mut c_void, *mut c_void) -> i32,
      >(proc)),
      None => None,
    };
    let Some(create_environment) = create_environment else {
      return Err("the WebView2 loader exports no CreateCoreWebView2EnvironmentWithOptions".into());
    };
    let environment_handler = Box::leak(Box::new(Handler::new(Role::Environment))) as *mut Handler as *mut c_void;
    CONTROLLER_HANDLER.store(Box::leak(Box::new(Handler::new(Role::Controller))) as *mut Handler as *mut c_void, Ordering::SeqCst);
    let profile: Vec<u16> = profile_dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let code = create_environment(std::ptr::null(), profile.as_ptr(), std::ptr::null_mut(), environment_handler);
    log(&format!("window: create_environment -> {code:#010x}"));
    if code != S_OK {
      return Err(format!(
        "creating the WebView2 environment failed ({code:#010x}); is the WebView2 runtime installed?"
      ));
    }
    // If the web view never comes up, close the window so run reports the
    // failure instead of hanging forever.
    let watchdog = window as usize;
    std::thread::spawn(move || {
      for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(500));
        if READY.load(Ordering::SeqCst) {
          return;
        }
      }
      log("window: watchdog closing the window");
      PostMessageW(watchdog as HWND, WM_CLOSE, 0, 0);
    });
    let mut message: MSG = std::mem::zeroed();
    while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
      TranslateMessage(&message);
      DispatchMessageW(&message);
    }
    log("window: message loop exited");
  }
  if let Some(failure) = FAILURE.get() {
    return Err(failure.clone());
  }
  if !READY.load(Ordering::SeqCst) {
    return Err("the interface window closed before the web view was ready".into());
  }
  Ok(())
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
  match message {
    WM_SIZE => {
      let controller = CONTROLLER.load(Ordering::SeqCst);
      if !controller.is_null() {
        let width = (lparam & 0xffff) as i32;
        let height = ((lparam >> 16) & 0xffff) as i32;
        let put_bounds: unsafe extern "system" fn(*mut c_void, RECT) -> i32 =
          std::mem::transmute(vtfn(controller, 6));
        put_bounds(controller, RECT { left: 0, top: 0, right: width, bottom: height });
      }
      0
    }
    WM_DESTROY => {
      if let Some(path) = PLACEMENT_PATH.get() {
        save_placement(path, hwnd);
      }
      let controller = CONTROLLER.swap(std::ptr::null_mut(), Ordering::SeqCst);
      if !controller.is_null() {
        let close: unsafe extern "system" fn(*mut c_void) -> i32 =
          std::mem::transmute(vtfn(controller, 24));
        close(controller);
      }
      PostQuitMessage(0);
      0
    }
    _ => DefWindowProcW(hwnd, message, wparam, lparam),
  }
}

/// The completion callbacks WebView2 calls: one shared shape for the
/// environment and the controller handlers.
#[repr(C)]
struct Handler {
  vtable: *const HandlerVtable,
  role: Role,
}
#[derive(Clone, Copy, PartialEq)]
enum Role {
  Environment,
  Controller,
}
impl Handler {
  fn new(role: Role) -> Self {
    Self { vtable: &HANDLER_VTABLE, role }
  }
}
#[repr(C)]
struct HandlerVtable {
  query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
  add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
  release: unsafe extern "system" fn(*mut c_void) -> u32,
  invoke: unsafe extern "system" fn(*mut c_void, i32, *mut c_void) -> i32,
}
static HANDLER_VTABLE: HandlerVtable = HandlerVtable {
  query_interface: handler_query_interface,
  add_ref: handler_add_ref,
  release: handler_release,
  invoke: handler_invoke,
};

// The handlers live for the whole process, so refcounts stay fixed at one and
// QueryInterface answers yes to the runtime's own callback interface (whose
// GUID this binary deliberately does not carry).
/// The completion-handler interface identifiers, transcribed from WebView2.h.
const IID_IUNKNOWN: GUID =
  GUID { data1: 0x00000000, data2: 0x0000, data3: 0x0000, data4: [0xc0, 0, 0, 0, 0, 0, 0, 0x46] };
const IID_ENVIRONMENT_HANDLER: GUID =
  GUID { data1: 0x4e8a3389, data2: 0xc9d8, data3: 0x4bd2, data4: [0xb6, 0xb5, 0x12, 0x4f, 0xee, 0x6c, 0xc1, 0x4d] };
const IID_CONTROLLER_HANDLER: GUID =
  GUID { data1: 0x6c4819f3, data2: 0xc9b7, data3: 0x4260, data4: [0x81, 0x27, 0xc9, 0xf5, 0xbd, 0xe7, 0xf6, 0x8c] };

fn guid_eq(left: &GUID, right: &GUID) -> bool {
  left.data1 == right.data1
    && left.data2 == right.data2
    && left.data3 == right.data3
    && left.data4 == right.data4
}

unsafe extern "system" fn handler_query_interface(this: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> i32 {
  log(&format!("window: QI {}", format_guid(&*iid)));
  if out.is_null() {
    return -2147467261; // E_POINTER
  }
  let handler = &*(this as *const Handler);
  let known = guid_eq(&*iid, &IID_IUNKNOWN)
    || (handler.role == Role::Environment && guid_eq(&*iid, &IID_ENVIRONMENT_HANDLER))
    || (handler.role == Role::Controller && guid_eq(&*iid, &IID_CONTROLLER_HANDLER));
  if !known {
    return -2147467260; // E_NOINTERFACE
  }
  handler_add_ref(this);
  *out = this;
  S_OK
}
unsafe extern "system" fn handler_add_ref(_this: *mut c_void) -> u32 {
  1
}
unsafe extern "system" fn handler_release(_this: *mut c_void) -> u32 {
  1
}

unsafe extern "system" fn handler_invoke(this: *mut c_void, error: i32, result: *mut c_void) -> i32 {
  log(&format!("window: invoke error={error:#010x} result={result:p}"));
  if result.is_null() {
    let _ = FAILURE.set("the WebView2 runtime completed without an interface".into());
    let window = WINDOW.load(Ordering::SeqCst);
    if !window.is_null() {
      PostMessageW(window, WM_CLOSE, 0, 0);
    }
    return -2147467261; // E_POINTER
  }
  if error != S_OK {
    let _ = FAILURE.set(format!("the WebView2 runtime reported error {error:#010x}"));
    let window = WINDOW.load(Ordering::SeqCst);
    if !window.is_null() {
      PostMessageW(window, WM_CLOSE, 0, 0);
    }
    return error;
  }
  let handler = &*(this as *const Handler);
  match handler.role {
    Role::Environment => {
      // Hold the environment alive, then create the controller bound to the window.
      let add_ref: unsafe extern "system" fn(*mut c_void) -> u32 =
        std::mem::transmute(vtfn(result, 1));
      add_ref(result);
      let create_controller: unsafe extern "system" fn(*mut c_void, HWND, *mut c_void) -> i32 =
        std::mem::transmute(vtfn(result, 3));
      let code = create_controller(result, WINDOW.load(Ordering::SeqCst), CONTROLLER_HANDLER.load(Ordering::SeqCst));
      log(&format!("window: create_controller -> {code:#010x}"));
    }
    Role::Controller => {
      // Hold the controller alive, size the view to the window and navigate.
      let add_ref: unsafe extern "system" fn(*mut c_void) -> u32 =
        std::mem::transmute(vtfn(result, 1));
      add_ref(result);
      CONTROLLER.store(result, Ordering::SeqCst);
      let mut webview: *mut c_void = std::ptr::null_mut();
      let get_webview: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 =
        std::mem::transmute(vtfn(result, 25));
      let code = get_webview(result, &mut webview);
      log(&format!("window: get_webview -> {code:#010x} ptr={webview:p}"));
      if webview.is_null() {
        let _ = FAILURE.set("the web view could not be reached from its controller".into());
        let window = WINDOW.load(Ordering::SeqCst);
        if !window.is_null() {
          PostMessageW(window, WM_CLOSE, 0, 0);
        }
        return -2147467259; // E_FAIL
      }
      let window = WINDOW.load(Ordering::SeqCst);
      let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
      if GetClientRect(window, &mut client) != 0 {
        let put_bounds: unsafe extern "system" fn(*mut c_void, RECT) -> i32 =
          std::mem::transmute(vtfn(result, 6));
        let code = put_bounds(result, client);
        log(&format!("window: put_bounds -> {code:#010x}"));
      }
      let add_ref: unsafe extern "system" fn(*mut c_void) -> u32 =
        std::mem::transmute(vtfn(webview, 1));
      add_ref(webview);
      let url: Vec<u16> = URL
        .get()
        .map(|url| url.encode_utf16().chain(std::iter::once(0)).collect())
        .unwrap_or_default();
      let navigate: unsafe extern "system" fn(*mut c_void, PCWSTR) -> i32 =
        std::mem::transmute(vtfn(webview, 5));
      let code = navigate(webview, url.as_ptr());
      log(&format!("window: navigate -> {code:#010x}"));
      READY.store(true, Ordering::SeqCst);
    }
  }
  S_OK
}

/// The vtable entry at `slot` (IUnknown occupies 0 through 2).
unsafe fn vtfn(this: *mut c_void, slot: usize) -> *mut c_void {
  let vtable = *(this as *mut *mut *mut c_void);
  *vtable.add(slot)
}

/// Locate WebView2Loader.dll: beside the executable first (the installer ships
/// one), then the copies that ship with Office, OneDrive and WSL.
unsafe fn load_loader() -> Result<*mut c_void, String> {
  let mut candidates: Vec<PathBuf> = Vec::new();
  if let Ok(exe) = std::env::current_exe() {
    if let Some(dir) = exe.parent() {
      candidates.push(dir.join("WebView2Loader.dll"));
    }
  }
  if let Some(base) = std::env::var_os("ProgramFiles").map(PathBuf::from) {
    candidates.push(base.join(r"Microsoft Office\root\Office16\WebView2Loader.dll"));
    candidates.push(base.join(r"WSL\wslsettings\WebView2Loader.dll"));
  }
  if let Some(base) = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from) {
    // OneDrive and the WebView2 runtime nest the loader inside version folders.
    for parent in [base.join("Microsoft OneDrive"), base.join(r"Microsoft\EdgeWebView\Application")] {
      if let Ok(entries) = std::fs::read_dir(&parent) {
        for entry in entries.flatten() {
          let candidate = entry.path().join("WebView2Loader.dll");
          if candidate.is_file() {
            candidates.push(candidate);
          }
        }
      }
    }
  }
  for candidate in &candidates {
    if !candidate.is_file() {
      continue;
    }
    let wide: Vec<u16> = candidate.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let module = LoadLibraryW(wide.as_ptr());
    if !module.is_null() {
      return Ok(module);
    }
  }
  Err(format!(
    "WebView2Loader.dll was not found (looked in {} places); the installer should place one beside wish.exe",
    candidates.len()
  ))
}
