//! Buffer utilities for high-performance I/O.
//!
//! This module provides buffer allocation helpers using `MaybeUninit` and
//! `ReadBuf` to skip zeroing overhead for buffers that will be immediately
//! overwritten.

use std::mem::MaybeUninit;

/// Allocate an uninitialized byte buffer of the specified capacity.
///
/// Returns a `Vec<MaybeUninit<u8>>` where all bytes are uninitialized.
/// This is type-safe because `MaybeUninit<u8>` explicitly represents
/// that the bytes may contain uninitialized memory.
///
/// # Usage
///
/// ```ignore
/// let mut buf = uninitialized_vec(65535);
/// let mut read_buf = tokio::io::ReadBuf::uninit(&mut buf);
/// udp_socket.recv_buf_from(&mut read_buf).await?;
/// let packet = read_buf.filled();
/// ```
///
/// # Performance
///
/// Avoids zeroing buffers that are about to be overwritten by a read. For
/// large or frequently allocated buffers this removes the up-front memset.
#[inline]
pub fn uninitialized_vec(capacity: usize) -> Vec<MaybeUninit<u8>> {
    let mut buf = Vec::with_capacity(capacity);
    // MaybeUninit::uninit() returns uninitialized memory.
    // resize_with extends the Vec to capacity, filling with uninitialized values.
    // This avoids the unsafe set_len() by using the safe resize_with API.
    buf.resize_with(capacity, MaybeUninit::uninit);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uninitialized_vec_capacity() {
        let buf = uninitialized_vec(1500);
        assert_eq!(buf.len(), 1500);
        assert!(buf.capacity() >= 1500);
    }

    #[test]
    fn test_uninitialized_vec_zero_capacity() {
        let buf = uninitialized_vec(0);
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.capacity(), 0);
    }

    #[test]
    fn test_uninitialized_vec_write_then_read() {
        let mut buf = uninitialized_vec(100);
        let mut read_buf = tokio::io::ReadBuf::uninit(&mut buf);
        let data = b"hello world";
        read_buf.put_slice(data);
        assert_eq!(read_buf.filled(), data);
    }
}
