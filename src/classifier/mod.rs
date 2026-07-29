// Thin re-export from seneschal-common.
// The actual classifier implementation now lives there.

// Re-export submodules (implementation moved to common)
pub use seneschal_common::classifier::fallback;
pub use seneschal_common::classifier::heuristic;
pub use seneschal_common::classifier::keyword;
pub use seneschal_common::classifier::pipeline;

// These stub modules were deleted — keep cfg gate for backward compat
#[cfg(feature = "classifier-embedding")]
pub mod embedding {
    compile_error!("classifier-embedding feature is no longer supported");
}
#[cfg(feature = "classifier-embedding")]
pub mod logistic {
    compile_error!("classifier-embedding feature is no longer supported");
}

// Re-export all public types from common
pub use seneschal_common::classifier::{
    ClassifyResult, ClassifierLevel, ClassifierPipeline, Intent,
    DEFAULT_COMPLEX_KEYWORDS, build_classifier, classify,
};
