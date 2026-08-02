#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::MacOsPermissionManager;
#[cfg(windows)]
pub use windows::WindowsPermissionManager;
