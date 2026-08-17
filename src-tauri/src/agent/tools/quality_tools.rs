//! Agent 质量/安全/运行时/媒体工具（facade，按职责拆到 4 个子文件）。
//!
//! 调用方式不变：quality_tools::code_metrics(...) / quality_tools::api_test(...) 等，
//! 通过下面 pub use 把子文件函数全部 re-export 出来。

#[path = "quality_metrics.rs"]
mod quality_metrics;
#[path = "quality_security.rs"]
mod quality_security;
#[path = "quality_runtime.rs"]
mod quality_runtime;
#[path = "quality_media.rs"]
mod quality_media;

pub use quality_metrics::*;
pub use quality_security::*;
pub use quality_runtime::*;
pub use quality_media::*;
