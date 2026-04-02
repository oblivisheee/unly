//! Tests for unly-memory similarity functions.

use unly_memory::similarity::{cosine_similarity, deserialize_embedding, serialize_embedding};

#[test]
fn identical_vectors_are_maximally_similar() {
    let v = vec![0.1f32, 0.5, -0.3, 1.0];
    assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
}

#[test]
fn orthogonal_vectors_have_zero_similarity() {
    let a = vec![1.0f32, 0.0, 0.0];
    let b = vec![0.0f32, 1.0, 0.0];
    assert!(cosine_similarity(&a, &b).abs() < 1e-6);
}

#[test]
fn opposite_vectors_have_negative_similarity() {
    let a = vec![1.0f32, 0.0];
    let b = vec![-1.0f32, 0.0];
    assert!((cosine_similarity(&a, &b) - (-1.0)).abs() < 1e-6);
}

#[test]
fn empty_vectors_return_zero() {
    let a: Vec<f32> = vec![];
    let b: Vec<f32> = vec![];
    assert_eq!(cosine_similarity(&a, &b), 0.0);
}

#[test]
fn zero_vector_returns_zero() {
    let a = vec![0.0f32, 0.0, 0.0];
    let b = vec![1.0f32, 2.0, 3.0];
    assert_eq!(cosine_similarity(&a, &b), 0.0);
}

#[test]
fn dimension_mismatch_returns_zero() {
    let a = vec![1.0f32, 2.0];
    let b = vec![1.0f32, 2.0, 3.0];
    assert_eq!(cosine_similarity(&a, &b), 0.0);
}

#[test]
fn serialization_roundtrip_preserves_values() {
    let original = vec![0.1f32, -0.5, 1.3, 0.0, f32::MAX, f32::MIN_POSITIVE];
    let bytes = serialize_embedding(&original);
    let recovered = deserialize_embedding(&bytes);
    assert_eq!(original.len(), recovered.len());
    for (a, b) in original.iter().zip(recovered.iter()) {
        assert!((a - b).abs() < 1e-7, "expected {}, got {}", a, b);
    }
}

#[test]
fn empty_embedding_serializes_to_empty_bytes() {
    let bytes = serialize_embedding(&[]);
    assert!(bytes.is_empty());
    let recovered = deserialize_embedding(&bytes);
    assert!(recovered.is_empty());
}
