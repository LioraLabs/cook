//! Terminal progress rendering for the Cook build system.

pub mod driver;
pub mod event;
pub mod log_reader;
pub mod log_store;
pub mod model;
pub mod naming;
pub mod render;
pub mod style;
pub mod wire;

pub use driver::Driver;
pub use event::{
    NodeId, NodeKind, PROGRESS_SCHEMA_VERSION, ProgressEvent, RecipeId, RecipeTopo, SkipReason,
    Stream,
};
pub use log_reader::{BuildSummary, BuildView, LoadDiagnostics, LogLine, NodeView, RecipeView};
pub use log_store::{LogConfig, LogStore};
pub use model::{BuildState, Counters, NodeState, NodeStatus, RecipeState, Status};
pub use render::Renderer;
pub use render::event_writer::EventWriterOptions;
pub use render::inline::{InlineOptions, InlineRenderer};
pub use render::json::{JsonWriter, SchemaCheckError, check_schema_version};
pub use render::plain::PlainRenderer;
pub use render::snapshot::StatusLineOptions;
pub use style::{LineKind, VERB_COL_WIDTH, Verb, VerbColor, format_verb, verb_for};
