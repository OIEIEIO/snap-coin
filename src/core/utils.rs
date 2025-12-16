use num_bigint::BigUint;

/// Clamp float between a min float and max float
pub fn clamp_f(x: f64, minv: f64, maxv: f64) -> f64 {
    x.max(minv).min(maxv)
}

/// Clamp integer between a min integer and max integer
pub fn clamp_i(x: i64, minv: i64, maxv: i64) -> i64 {
    x.max(minv).min(maxv)
}

/// Max value of a BigUint 256 bit
pub fn max_256_bui() -> BigUint {
    BigUint::from_bytes_be(&max_256_buf())
}

/// Max value of a BigUint 256 bit in a byte buffer
pub fn max_256_buf() -> [u8; 32] {
    [0xFFu8; 32]
}

pub fn slice_vec<T>(v: &[T], start: usize, end: usize) -> &[T] {
    if start >= v.len() {
        &[] // start is out of bounds → empty slice
    } else {
        let end = end.min(v.len()); // clamp end to vector length
        &v[start..end]
    }
}
