mod common;

use anyclaude::metrics::StreamError;

#[test]
fn stream_error_display() {
    let err = StreamError::IdleTimeout { duration: 60 };
    assert_eq!(err.to_string(), "idle timeout after 60s of inactivity");
}
