use thiserror::Error;

pub type Result<T> = std::result::Result<T, AnimError>;

#[derive(Debug, Error)]
pub enum AnimError {
    #[error("no animation package '{0}' is installed")]
    NotFound(String),

    #[error("animation package '{id}' is malformed: {reason}")]
    BadManifest { id: String, reason: String },

    #[error("parameter '{key}': {reason}")]
    BadParam { key: String, reason: String },

    #[error("'{key}' is referenced by the generator but is not a parameter of the package")]
    UnknownParamReference { key: String },

    #[error("{id} cannot be applied here: {reason}")]
    TargetMismatch { id: String, reason: String },

    #[error("could not bake {id}: {reason}")]
    Bake { id: String, reason: String },

    #[error(
        "{id} uses a script generator ({entry}), which only the studio can run — \
         headless baking needs a declarative generator"
    )]
    ScriptGenerator { id: String, entry: String },

    // The field is `expr` rather than `source`: thiserror treats a field named
    // `source` as the underlying error in the chain, which a plain String is not.
    #[error("in expression '{expr}': {reason}")]
    Expression { expr: String, reason: String },

    #[error("{0}")]
    Io(String),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Doc(#[from] md_doc::DocError),
}

impl From<std::io::Error> for AnimError {
    fn from(e: std::io::Error) -> Self {
        AnimError::Io(e.to_string())
    }
}
