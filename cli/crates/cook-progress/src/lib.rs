//! Terminal progress rendering for the Cook build system.

pub mod driver;
pub mod event;
pub mod log_store;
pub mod model;
pub mod render;
pub mod strip;
pub mod style;

pub use driver::Driver;
pub use event::{NodeId, NodeKind, ProgressEvent, RecipeId, RecipeTopo, SkipReason, Stream, PROGRESS_SCHEMA_VERSION};
pub use log_store::{LogConfig, LogStore};
pub use model::{BuildState, Counters, NodeState, NodeStatus, RecipeState, Status};
pub use render::Renderer;
pub use render::inline::{InlineOptions, InlineRenderer};
pub use render::event_writer::EventWriterOptions;
pub use render::snapshot::StatusLineOptions;
pub use render::json::{check_schema_version, JsonWriter, SchemaCheckError};
pub use render::plain::PlainRenderer;
pub use style::{format_verb, verb_for, LineKind, Verb, VerbColor, VERB_COL_WIDTH};
