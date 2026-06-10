//! Shared helper functions for iroh-based tunnels.
//!
//! This module contains stream and connection helpers used by
//! iroh mode.

use anyhow::{Context, Result};
use bytes::{Buf, Bytes, BytesMut};
use std::future::poll_fn;
use std::io::{self, IoSlice};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::Mutex;

use crate::buffer::uninitialized_vec;
use crate::net::{
    order_udp_addresses, retry_with_backoff, tune_tcp_stream, STREAM_OPEN_BASE_DELAY_MS,
    STREAM_OPEN_MAX_ATTEMPTS,
};

/// Read exactly enough bytes to fill `read_buf` to its capacity from a QUIC
/// stream — our own `read_exact` over uninitialized memory.
///
/// The stock `RecvStream::read_exact` requires an initialized `&mut [u8]`, which
/// forces zeroing the buffer first. This reads through the `AsyncRead` impl into
/// a `ReadBuf::uninit` instead, skipping the memset. `read_buf`'s capacity bounds
/// the read, so frame boundaries are preserved (no over-read into the next
/// frame). Errors if the stream ends before the buffer is full.
async fn read_exact_uninit(
    stream: &mut iroh::endpoint::RecvStream,
    read_buf: &mut ReadBuf<'_>,
) -> Result<()> {
    while read_buf.remaining() > 0 {
        let before = read_buf.filled().len();
        poll_fn(|cx| Pin::new(&mut *stream).poll_read(cx, read_buf))
            .await
            .context("Failed to read frame payload")?;
        if read_buf.filled().len() == before {
            anyhow::bail!("QUIC stream ended mid-frame");
        }
    }
    Ok(())
}

// ============================================================================
// QUIC Stream Helpers
// ============================================================================

/// Open an iroh QUIC bidirectional stream with retry and exponential backoff.
pub(super) async fn open_bi_with_retry(
    conn: &iroh::endpoint::Connection,
) -> Result<(iroh::endpoint::SendStream, iroh::endpoint::RecvStream)> {
    retry_with_backoff(
        |_| async {
            conn.open_bi()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to open QUIC stream: {}", e))
        },
        STREAM_OPEN_MAX_ATTEMPTS,
        STREAM_OPEN_BASE_DELAY_MS,
    )
    .await
}

/// Bridge a QUIC stream bidirectionally with a TCP stream.
pub(super) async fn bridge_streams(
    mut quic_recv: iroh::endpoint::RecvStream,
    mut quic_send: iroh::endpoint::SendStream,
    tcp_stream: TcpStream,
) -> Result<()> {
    tune_tcp_stream(&tcp_stream);
    let (mut tcp_read, mut tcp_write) = tcp_stream.into_split();

    tokio::select! {
        result = copy_quic_to_tcp(&mut quic_recv, &mut tcp_write) => {
            if let Err(e) = result
                && !e.to_string().contains("reset") {
                    log::warn!("QUIC->TCP error: {}", e);
                }
        }
        result = copy_tcp_to_quic(&mut tcp_read, &mut quic_send) => {
            if let Err(e) = result
                && !e.to_string().contains("reset") {
                    log::warn!("TCP->QUIC error: {}", e);
                }
        }
    }

    // Signal EOF on the QUIC send stream for graceful shutdown
    let _ = quic_send.finish();

    Ok(())
}

const TCP_TO_QUIC_CHUNK_SIZE: usize = 256 * 1024;
const QUIC_TO_TCP_CHUNKS: usize = 64;

async fn copy_tcp_to_quic<R>(
    reader: &mut R,
    writer: &mut iroh::endpoint::SendStream,
) -> Result<()>
where
    R: AsyncRead + Unpin,
{
    // Single-write-per-read: coalescing multiple reads into one large QUIC write
    // was measured to increase iperf3 retransmits by making the sender bursty.
    loop {
        let mut buf = BytesMut::with_capacity(TCP_TO_QUIC_CHUNK_SIZE);
        let read_len = reader
            .read_buf(&mut buf)
            .await
            .context("Failed to read from TCP stream")?;
        if read_len == 0 {
            break;
        }

        writer
            .write_chunk(buf.freeze())
            .await
            .context("Failed to write to QUIC stream")?;
    }

    Ok(())
}

async fn copy_quic_to_tcp<W>(
    reader: &mut iroh::endpoint::RecvStream,
    writer: &mut W,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut chunks: [Bytes; QUIC_TO_TCP_CHUNKS] = std::array::from_fn(|_| Bytes::new());

    while let Some(chunk_count) = reader
        .read_many_chunks(&mut chunks)
        .await
        .context("Failed to read from QUIC stream")?
    {
        write_all_chunks_vectored(writer, &mut chunks[..chunk_count])
            .await
            .context("Failed to write to TCP stream")?;
    }

    writer.flush().await.context("Failed to flush TCP stream")?;
    Ok(())
}

async fn write_all_chunks_vectored<W>(writer: &mut W, chunks: &mut [Bytes]) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut first = 0;

    while first < chunks.len() {
        while first < chunks.len() && chunks[first].is_empty() {
            first += 1;
        }
        if first == chunks.len() {
            break;
        }

        let slices: Vec<IoSlice<'_>> = chunks[first..]
            .iter()
            .filter(|chunk| !chunk.is_empty())
            .map(|chunk| IoSlice::new(chunk.as_ref()))
            .collect();

        let written = writer.write_vectored(&slices).await?;
        if written == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }

        let mut remaining = written;
        while remaining > 0 && first < chunks.len() {
            let chunk_len = chunks[first].len();
            if remaining >= chunk_len {
                remaining -= chunk_len;
                chunks[first] = Bytes::new();
                first += 1;
            } else {
                chunks[first].advance(remaining);
                break;
            }
        }
    }

    Ok(())
}

// ============================================================================
// UDP Stream Helpers
// ============================================================================

/// Read UDP packets from local socket and forward to iroh stream.
pub(super) async fn forward_udp_to_stream(
    udp_socket: Arc<UdpSocket>,
    mut send_stream: iroh::endpoint::SendStream,
    peer_addr: Arc<Mutex<Option<SocketAddr>>>,
) -> Result<()> {
    let mut storage = uninitialized_vec(65535);

    loop {
        let mut read_buf = ReadBuf::uninit(&mut storage);
        let addr = poll_fn(|cx| udp_socket.poll_recv_from(cx, &mut read_buf))
            .await
            .context("Failed to receive UDP packet")?;
        let data = read_buf.filled();
        let len = data.len();

        *peer_addr.lock().await = Some(addr);

        let frame_len = (len as u16).to_be_bytes();
        send_stream
            .write_all(&frame_len)
            .await
            .context("Failed to write frame length")?;
        send_stream
            .write_all(data)
            .await
            .context("Failed to write frame payload")?;

        log::debug!("-> Forwarded {} bytes from {}", len, addr);
    }
}

/// Read from iroh stream, forward to UDP target, and send responses back (server mode).
///
/// Supports multiple target addresses with fallback:
/// - Addresses are tried in Happy Eyeballs order (IPv6 first)
/// - On send error, falls back to the next address
/// - Aggregates errors if all addresses fail
pub(super) async fn forward_stream_to_udp_server(
    mut recv_stream: iroh::endpoint::RecvStream,
    mut send_stream: iroh::endpoint::SendStream,
    udp_socket: Arc<UdpSocket>,
    target_addrs: Arc<Vec<SocketAddr>>,
) -> Result<()> {
    if target_addrs.is_empty() {
        anyhow::bail!("No target addresses provided for UDP forwarding");
    }

    // Order addresses for connection attempts
    let ordered_addrs = order_udp_addresses(&target_addrs);

    let udp_clone = udp_socket.clone();
    let response_task = tokio::spawn(async move {
        let mut storage = uninitialized_vec(65535);
        loop {
            let mut read_buf = ReadBuf::uninit(&mut storage);
            if poll_fn(|cx| udp_clone.poll_recv_from(cx, &mut read_buf))
                .await
                .is_err()
            {
                break;
            }
            let data = read_buf.filled();
            let len = data.len();
            let frame_len = (len as u16).to_be_bytes();
            if send_stream.write_all(&frame_len).await.is_err() {
                break;
            }
            if send_stream.write_all(data).await.is_err() {
                break;
            }
            log::debug!("-> Sent {} bytes back to client", len);
        }
    });

    let mut active_addr_idx = 0;
    let mut logged_active = false;
    let mut storage = uninitialized_vec(u16::MAX as usize);

    loop {
        // Track errors for each address for aggregate reporting - fresh for each packet
        let mut errors: Vec<(SocketAddr, std::io::Error)> = Vec::new();
        let mut len_buf = [0u8; 2];
        match recv_stream.read_exact(&mut len_buf).await {
            Ok(()) => {}
            Err(iroh::endpoint::ReadExactError::FinishedEarly(_)) => {
                // Clean EOF - stream finished at frame boundary
                break;
            }
            Err(e) => {
                log::warn!("Failed to read frame length: {}", e);
                break;
            }
        }
        let len = u16::from_be_bytes(len_buf) as usize;

        let mut read_buf = ReadBuf::uninit(&mut storage[..len]);
        read_exact_uninit(&mut recv_stream, &mut read_buf).await?;
        let payload = read_buf.filled();

        // Try to send to current address, falling back on error
        let mut sent = false;
        while active_addr_idx < ordered_addrs.len() {
            let target_addr = ordered_addrs[active_addr_idx];

            match udp_socket.send_to(payload, target_addr).await {
                Ok(_) => {
                    if !logged_active {
                        if active_addr_idx > 0 {
                            log::info!(
                                "UDP fallback: using {} after {} failed address(es)",
                                target_addr,
                                active_addr_idx
                            );
                        }
                        logged_active = true;
                    }
                    log::debug!("<- Forwarded {} bytes to {}", len, target_addr);
                    sent = true;
                    break;
                }
                Err(e) => {
                    log::warn!("UDP send to {} failed: {}", target_addr, e);
                    errors.push((target_addr, e));
                    active_addr_idx += 1;
                    logged_active = false;
                }
            }
        }

        if !sent {
            // All addresses failed for this packet; reset so the next packet retries from the start.
            if active_addr_idx >= ordered_addrs.len() {
                active_addr_idx = 0;
                logged_active = false;
            }
            if errors.len() == 1 {
                let (addr, e) = errors.remove(0);
                log::warn!("Failed to send UDP packet to {}: {}", addr, e);
            } else if !errors.is_empty() {
                let error_details: Vec<String> = errors
                    .iter()
                    .map(|(addr, e)| format!("{}: {}", addr, e))
                    .collect();
                log::warn!(
                    "Failed to send UDP packet to any address:\n  {}",
                    error_details.join("\n  ")
                );
            } else {
                log::warn!("Failed to send UDP packet: no target addresses available");
            }
        }
    }

    response_task.abort();
    Ok(())
}

/// Read from iroh stream and forward to local UDP client (client mode).
pub(super) async fn forward_stream_to_udp_client(
    mut recv_stream: iroh::endpoint::RecvStream,
    udp_socket: Arc<UdpSocket>,
    client_addr: Arc<Mutex<Option<SocketAddr>>>,
) -> Result<()> {
    let mut storage = uninitialized_vec(u16::MAX as usize);
    loop {
        let mut len_buf = [0u8; 2];
        match recv_stream.read_exact(&mut len_buf).await {
            Ok(()) => {}
            Err(iroh::endpoint::ReadExactError::FinishedEarly(_)) => {
                // Clean EOF - stream finished at frame boundary
                break;
            }
            Err(e) => {
                log::warn!("Failed to read frame length: {}", e);
                break;
            }
        }
        let len = u16::from_be_bytes(len_buf) as usize;

        let mut read_buf = ReadBuf::uninit(&mut storage[..len]);
        read_exact_uninit(&mut recv_stream, &mut read_buf).await?;
        let payload = read_buf.filled();

        if let Some(addr) = *client_addr.lock().await {
            udp_socket
                .send_to(payload, addr)
                .await
                .context("Failed to send UDP packet to client")?;
            log::debug!("<- Forwarded {} bytes to client {}", len, addr);
        } else {
            log::debug!("<- Received {} bytes but no client connected yet", len);
        }
    }

    Ok(())
}
