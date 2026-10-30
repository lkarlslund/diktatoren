//! Editing in place needs an accessibility API; Windows (UI Automation) and macOS (the
//! Accessibility API) are not supported yet, so no field is ever offered and dictation uses
//! the dictation window.

use anyhow::{Result, anyhow};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub text: String,
    pub caret: usize,
    pub selection: Option<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {}

pub fn start() {}

pub fn focused() -> Option<Target> {
    None
}

pub fn read(_: &Target) -> Result<Snapshot> {
    Err(anyhow!("editing in place is not supported on this platform"))
}

pub fn write(_: &Target, _: Vec<Edit>) -> Result<()> {
    Err(anyhow!("editing in place is not supported on this platform"))
}

pub fn diff(_: &str, _: &str, _: std::ops::Range<usize>) -> Vec<Edit> {
    Vec::new()
}

pub fn byte_offset(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map_or(text.len(), |(i, _)| i)
}
