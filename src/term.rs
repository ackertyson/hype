//! Terminal queries via libc FFI, without pulling in a crate.

#[repr(C)]
#[derive(Default)]
struct Winsize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
const TIOCGWINSZ: std::ffi::c_ulong = 0x4008_7468;

#[cfg(any(target_os = "linux", target_os = "android"))]
const TIOCGWINSZ: std::ffi::c_ulong = 0x5413;

unsafe extern "C" {
    fn ioctl(fd: std::ffi::c_int, request: std::ffi::c_ulong, ...) -> std::ffi::c_int;
    fn isatty(fd: std::ffi::c_int) -> std::ffi::c_int;
}

/// Whether `fd` is a terminal.
pub fn is_tty(fd: std::ffi::c_int) -> bool {
    // SAFETY: isatty only inspects the descriptor.
    unsafe { isatty(fd) == 1 }
}

/// Width in columns of the terminal attached to stdout, stderr, or stdin
/// (checked in that order), or `None` if none of them is a terminal.
pub fn width() -> Option<usize> {
    [1, 2, 0].into_iter().find_map(|fd| {
        let mut ws = Winsize::default();
        // SAFETY: TIOCGWINSZ writes a `struct winsize` through the pointer,
        // which matches `Winsize`'s C layout; on a non-terminal fd it fails
        // without writing.
        let ok = unsafe { ioctl(fd, TIOCGWINSZ, &mut ws as *mut Winsize) } == 0;
        (ok && ws.ws_col > 0).then_some(ws.ws_col as usize)
    })
}
