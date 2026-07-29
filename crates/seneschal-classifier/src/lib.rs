// seneschal-classifier — Intent classification cascade.
//
// Thin wrapper crate. The full implementation lives in seneschal-common.
// This crate exists so the classifier can be feature-flagged independently.

pub use seneschal_common::classifier::{
    ClassifierLevel, ClassifierPipeline, ClassifyResult, DEFAULT_COMPLEX_KEYWORDS, Intent,
    build_classifier, classify, fallback, heuristic, keyword, pipeline,
};
