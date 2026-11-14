//! OptiStream core: the [`Method`] extension point, schema-driven settings and the
//! resource-level pipeline (analysis, optimisation to an export folder or in place).

pub mod method;
pub mod pipeline;
pub mod settings;

pub use method::{
    normalize_rel, resource_prefix, FileAnalysis, FileCtx, FileOutcome, Issue, Method, PreviewContent,
    PreviewImage, PreviewItem, PreviewMesh, Severity, MANIFESTS,
};
pub use pipeline::{analyze, contexts, mib, optimize, AnalyzedFile, FileReport, FileStatus, Selection, Totals, Analysis, Output, Progress, Report, SizeInfo};
pub use settings::{ParamKind, ParamSpec, Preset, Settings, Value};
