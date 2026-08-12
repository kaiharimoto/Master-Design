use thiserror::Error;

#[derive(Debug, Error)]
pub enum GeomError {
    #[error("could not parse path data: {0}")]
    ParsePath(String),

    #[error("path is empty")]
    EmptyPath,

    #[error("invalid parameter: {0}")]
    InvalidParam(String),
}
