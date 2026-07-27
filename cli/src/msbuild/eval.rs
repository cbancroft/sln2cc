//! Macro expansion and `Condition` evaluation.
//!
//! Handles MSBuild property references such as `$(Configuration)`, `$(Platform)`,
//! `$(SolutionDir)`, and conditions like
//! `Condition="'$(Configuration)|$(Platform)'=='Debug|x64'"`.
//!
//! Unsupported constructs must be surfaced as warnings rather than silently producing
//! incorrect flags — see the engineering requirements in docs/engineering_spec.md.

use std::collections::HashMap;

use crate::Result;

/// The property bag built up during evaluation. Later assignments shadow earlier ones.
#[derive(Debug, Default, Clone)]
pub struct Properties(pub HashMap<String, String>);

/// Expand `$(...)` references in `input` against the current property bag.
pub fn expand(_input: &str, _props: &Properties) -> Result<String> {
    todo!("expand MSBuild macros")
}

/// Evaluate an MSBuild `Condition` expression to a boolean.
pub fn condition_holds(_condition: &str, _props: &Properties) -> Result<bool> {
    todo!("evaluate MSBuild condition")
}
