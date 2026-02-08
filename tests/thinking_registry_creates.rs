mod common;

use anyclaude::proxy::thinking::TransformerRegistry;

#[test]
fn registry_creates_native_transformer() {
    let registry = TransformerRegistry::new();
    assert_eq!(registry.transformer().name(), "native");
}
