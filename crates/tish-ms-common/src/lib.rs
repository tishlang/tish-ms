//! Shared, platform-free pieces of the Windows Tish UI hosts: tag names, prop helpers, and the
//! retained node tree with its layout (the same rules as tish-macos, so one Tish UI lays out the
//! same on both). Everything here runs and is tested on any OS.

pub mod layout;
pub mod style;
pub mod tag;
pub mod tree;
