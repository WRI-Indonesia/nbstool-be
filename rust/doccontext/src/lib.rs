//! Database rows (as JSON) -> the exact docxtpl contexts the Python builds, for `nbstool-docx`.
//! See README.md.

pub mod ccb;
mod py;
pub mod v3;

pub use v3::{Analyzer, build_context, centroid_words, feasibility_prefill, load_draft, py_str};
