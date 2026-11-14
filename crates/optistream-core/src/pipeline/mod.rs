//! Analysis and optimisation of a whole resource folder with any [`Method`](crate::Method).

mod size;
mod progress;
mod files;
mod analyze;
mod report;
mod optimize;

pub use size::{mib, FIVEM_WARN_BYTES, SizeInfo};
pub use progress::Progress;
pub use files::{contexts, Selection};
pub use analyze::{analyze, Analysis, AnalyzedFile};
pub use report::{FileReport, FileStatus, Output, Report, Totals};
pub use optimize::optimize;

