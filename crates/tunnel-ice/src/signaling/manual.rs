//! Manual stdin/stdout signaling helpers.
//!
//! Provides functions for displaying and reading signaling payloads
//! in a PEM-like format for manual copy-paste exchange.

use anyhow::{anyhow, Result};
use std::io::BufRead;

use super::codec::{wrap_lines, LINE_WIDTH};
use super::{decode_answer, decode_offer, encode_answer, encode_offer, ManualAnswer, ManualOffer};

// Custom mode markers (v1)
const OFFER_BEGIN_MARKER: &str = "-----BEGIN TUNNEL-RS MANUAL OFFER-----";
const OFFER_END_MARKER: &str = "-----END TUNNEL-RS MANUAL OFFER-----";
const ANSWER_BEGIN_MARKER: &str = "-----BEGIN TUNNEL-RS MANUAL ANSWER-----";
const ANSWER_END_MARKER: &str = "-----END TUNNEL-RS MANUAL ANSWER-----";

// ============================================================================
// Custom Mode (v1) Display/Read
// ============================================================================

pub fn display_offer(offer: &ManualOffer) -> Result<()> {
    display_payload(encode_offer(offer)?, OFFER_BEGIN_MARKER, OFFER_END_MARKER)
}

pub fn display_answer(answer: &ManualAnswer) -> Result<()> {
    display_payload(
        encode_answer(answer)?,
        ANSWER_BEGIN_MARKER,
        ANSWER_END_MARKER,
    )
}

pub fn read_offer_from_stdin() -> Result<ManualOffer> {
    let payload = read_marked_payload(OFFER_BEGIN_MARKER, OFFER_END_MARKER)?;
    decode_offer(&payload)
}

pub fn read_answer_from_stdin() -> Result<ManualAnswer> {
    let payload = read_marked_payload(ANSWER_BEGIN_MARKER, ANSWER_END_MARKER)?;
    decode_answer(&payload)
}

// ============================================================================
// Internal Helpers
// ============================================================================

fn display_payload(payload: String, begin: &str, end: &str) -> Result<()> {
    let wrapped = wrap_lines(&payload, LINE_WIDTH);
    println!();
    println!("{}", begin);
    println!("{}", wrapped);
    println!("{}", end);
    println!();
    Ok(())
}

fn read_marked_payload(begin: &str, end: &str) -> Result<String> {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut collected = Vec::new();

    loop {
        let line = lines
            .next()
            .ok_or_else(|| anyhow!("Missing BEGIN marker"))??;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == begin {
            break;
        }
        return Err(anyhow!("Unexpected text before BEGIN marker"));
    }

    let mut found_end = false;
    for line in lines {
        let line = line?;
        let trimmed = line.trim();
        if trimmed == end {
            found_end = true;
            break;
        }
        if !trimmed.is_empty() {
            collected.push(trimmed.to_string());
        }
    }

    if !found_end {
        return Err(anyhow!("END marker not found"));
    }

    if collected.is_empty() {
        return Err(anyhow!("No payload found between markers"));
    }

    Ok(collected.join(""))
}
