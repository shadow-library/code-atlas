mod apply;
mod diff;
mod error;
mod indent;
mod lock;
mod occurrence;
mod plan;
mod region;
mod scope;
mod symbol;
mod text;

pub use apply::{AppliedChange, ApplyOptions, ApplyReport, DEFAULT_LOCK_TIMEOUT, TEMP_PREFIX, apply, sweep_stale_temps, temp_prefix};
pub use diff::{DiffLine, FileDiff, Hunk, LineKind, file_diff};
pub use error::{EditError, EditResult, NotTextReason, PlanViolation};
pub use indent::{FirstLine, reindent};
pub use lock::EditLock;
pub use occurrence::{Occurrence, OccurrenceId};
pub use plan::{EditPlan, FileEdits, FileTarget, ResourceOp, TextEdit};
pub use region::{LeadingRegion, Syntax, TriviaSummary, leading_region, leading_trivia};
pub use scope::{EditScope, Refusal, RefusalReason, ScopedPath};
pub use symbol::{Nesting, SymbolOp, SymbolPlan, SymbolSite, SymbolTarget, plan_symbol_edit, symbol_edit};
pub use text::{
  Disposition, FilesPlan, LineEnding, Pattern, ReplaceContent, ReplaceInFiles, SkipReason, SkippedFile, TextPlan, UTF8_BOM, bom_len, replace_content, replace_in_files,
  replace_in_text,
};
