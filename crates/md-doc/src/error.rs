use crate::id::NodeId;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, DocError>;

#[derive(Debug, Error)]
pub enum DocError {
    #[error("no node with id {0}")]
    NodeNotFound(NodeId),

    #[error("no page with id or slug {0}")]
    PageNotFound(String),

    #[error("node {0} already exists in this document")]
    DuplicateNode(NodeId),

    #[error("{0} cannot contain children")]
    NotAContainer(NodeId),

    #[error("cannot move {parent} into its own descendant {child}")]
    CyclicMove { parent: NodeId, child: NodeId },

    #[error("the page root cannot be removed or reparented")]
    RootIsImmovable,

    #[error("index {index} is out of range for a parent with {len} children")]
    IndexOutOfRange { index: usize, len: usize },

    #[error("no property at '{0}'")]
    NoSuchProperty(String),

    #[error("value at '{path}' is not valid: {reason}")]
    InvalidValue { path: String, reason: String },

    #[error("invalid identifier '{0}'")]
    InvalidId(String),

    #[error("invalid color '{0}': expected #rgb, #rrggbb or #rrggbbaa")]
    InvalidColor(String),

    #[error("invalid selector '{selector}': {reason}")]
    InvalidSelector { selector: String, reason: String },

    #[error("nothing to undo")]
    NothingToUndo,

    #[error("nothing to redo")]
    NothingToRedo,

    #[error("geometry error: {0}")]
    Geom(#[from] md_geom::GeomError),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Io(String),
}

impl From<std::io::Error> for DocError {
    fn from(e: std::io::Error) -> Self {
        DocError::Io(e.to_string())
    }
}
