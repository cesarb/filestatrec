use std::error::Error;
use std::fmt::{self, Display, Formatter};

#[derive(Debug)]
pub struct ErrorWithPath<E> {
    error: E,
    path: String,
}

impl<E> ErrorWithPath<E> {
    fn new(error: E, path: String) -> Self {
        ErrorWithPath { error, path }
    }
}

impl<E: Display> Display for ErrorWithPath<E> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.error)
    }
}

impl<E: Error + 'static> Error for ErrorWithPath<E> {
    #[inline]
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.error)
    }
}

pub trait ErrorPath {
    fn into_error_path(self) -> String;
}

impl ErrorPath for &str {
    fn into_error_path(self) -> String {
        self.to_owned()
    }
}

impl ErrorPath for &[u8] {
    fn into_error_path(self) -> String {
        String::from_utf8_lossy(self).into_owned()
    }
}

fn error_with_path<E, P>(error: E, path: P) -> ErrorWithPath<E>
where
    P: ErrorPath,
{
    ErrorWithPath::new(error, path.into_error_path())
}

#[inline]
pub fn with_error_path<P, F, T, E>(path: P, f: F) -> Result<T, ErrorWithPath<E>>
where
    P: ErrorPath,
    F: FnOnce() -> Result<T, E>,
{
    f().map_err(|err| error_with_path(err, path))
}
