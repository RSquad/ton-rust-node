/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Unit tests for `adnl_overlay.rs` helpers.

use super::*;
use ton_api::{
    serialize_boxed,
    ton::consensus::{
        simplex::candidateandcert::CandidateAndCert, RequestError as ConsensusRequestError,
    },
    IntoBoxed,
};

#[test]
fn test_extract_query_response_error_recognizes_request_error() {
    let bytes = serialize_boxed(&ConsensusRequestError::Consensus_RequestError)
        .expect("serialize requestError");

    let error_name =
        extract_query_response_error(bytes.as_slice()).expect("requestError must be extracted");
    assert_eq!(error_name, "consensus.requestError");
}

#[test]
fn test_normalize_query_response_payload_rejects_request_error() {
    let bytes = serialize_boxed(&ConsensusRequestError::Consensus_RequestError)
        .expect("serialize requestError");

    let err =
        normalize_query_response_payload(bytes).expect_err("requestError must become query error");
    assert!(err.to_string().contains("consensus.requestError"), "unexpected error: {err}");
}

#[test]
fn test_normalize_query_response_payload_accepts_candidate_and_cert() {
    let bytes = serialize_boxed(
        &CandidateAndCert { candidate: Vec::<u8>::new().into(), notar: Vec::<u8>::new().into() }
            .into_boxed(),
    )
    .expect("serialize CandidateAndCert");

    let payload =
        normalize_query_response_payload(bytes.clone()).expect("candidateAndCert must pass");
    assert_eq!(payload.data(), &bytes);
}
