use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("buffer ends at {have} bytes, needed {need}")]
    Short { need: usize, have: usize },
    #[error("varint longer than 5 bytes")]
    VarIntTooLong,
    #[error("negative value cannot be a varint")]
    VarIntNegative,
    #[error("frame length {0} out of range")]
    BadLength(i64),
    #[error("unknown packet id {0}")]
    UnknownId(u32),
    #[error("packet {name} is {len} bytes, max {max}")]
    TooBig {
        name: &'static str,
        len: usize,
        max: usize,
    },
    #[error("field {field}: offset slot is {got}, canonical layout needs {want}")]
    NonCanonical {
        field: &'static str,
        got: i32,
        want: i32,
    },
    #[error("field {field}: length {len} over limit {max}")]
    FieldTooLong {
        field: &'static str,
        len: usize,
        max: usize,
    },
    #[error("field {field}: not valid {what}")]
    BadText {
        field: &'static str,
        what: &'static str,
    },
    #[error("field {field}: unknown enum value {value}")]
    BadEnum { field: &'static str, value: i64 },
    #[error("zstd: {0}")]
    Zstd(String),
}
