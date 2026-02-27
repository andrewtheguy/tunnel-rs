//! QUIC setup helpers for manual mode.

use anyhow::{Context, Result};
use log::info;
use quinn::congestion::{BbrConfig, ControllerFactory, CubicConfig, NewRenoConfig};
use quinn::crypto::rustls::QuicClientConfig;
use quinn::{
    AsyncUdpSocket, ClientConfig, Endpoint, EndpointConfig, IdleTimeout, Runtime, ServerConfig,
};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use crate::defaults::{DEFAULT_ICE_RECEIVE_WINDOW, DEFAULT_ICE_SEND_WINDOW};
use tunnel_common::config::{CongestionController, TransportTuning};

/// Ensure rustls crypto provider is installed.
/// This must be called before using any rustls functionality.
pub fn ensure_crypto_provider() {
    // Install aws-lc-rs as the default crypto provider for rustls.
    // This is separate from str0m's crypto provider.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// Create base transport config with common settings for both server and client.
///
/// Configures 5 minute idle timeout with 15s keepalive. Active connections send
/// pings every 15s, so idle timeout only triggers for truly dead/unresponsive
/// connections.
fn create_base_transport_config(transport_tuning: &TransportTuning) -> quinn::TransportConfig {
    let mut transport = quinn::TransportConfig::default();
    transport.max_idle_timeout(Some(
        IdleTimeout::try_from(Duration::from_secs(300))
            .expect("300s is a valid IdleTimeout duration"),
    ));
    transport.keep_alive_interval(Some(Duration::from_secs(15)));
    apply_transport_tuning(&mut transport, transport_tuning);
    transport
}

pub struct QuicServerIdentity {
    pub server_config: ServerConfig,
    pub fingerprint: String,
}

pub fn generate_server_identity(transport_tuning: &TransportTuning) -> Result<QuicServerIdentity> {
    // Ensure rustls has a crypto provider
    ensure_crypto_provider();

    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(vec!["manual.tunnel".into()])
            .context("Failed to generate self-signed certificate")?;
    let cert_der = cert.der().to_vec();
    let key = PrivatePkcs8KeyDer::from(key_pair.serialize_der());

    let fingerprint = cert_fingerprint_hex(&cert_der);
    let cert_chain = vec![CertificateDer::from(cert_der.clone())];

    let mut server_config =
        ServerConfig::with_single_cert(cert_chain, key.into()).context("Invalid TLS config")?;

    let mut transport = create_base_transport_config(transport_tuning);
    // Server only accepts bidirectional streams for tunnel data; disable
    // unidirectional streams since this protocol doesn't use them.
    transport.max_concurrent_uni_streams(0_u8.into());
    server_config.transport_config(Arc::new(transport));

    Ok(QuicServerIdentity {
        server_config,
        fingerprint,
    })
}

pub fn make_server_endpoint(
    socket: Arc<dyn AsyncUdpSocket>,
    server_config: ServerConfig,
) -> Result<Endpoint> {
    let runtime: Arc<dyn Runtime> = Arc::new(quinn::TokioRuntime);
    let endpoint = Endpoint::new_with_abstract_socket(
        EndpointConfig::default(),
        Some(server_config),
        socket,
        runtime,
    )
    .context("Failed to create QUIC server endpoint")?;
    Ok(endpoint)
}

pub fn make_client_endpoint(
    socket: Arc<dyn AsyncUdpSocket>,
    expected_fingerprint: &str,
    transport_tuning: &TransportTuning,
) -> Result<Endpoint> {
    // Ensure rustls has a crypto provider
    ensure_crypto_provider();

    let runtime: Arc<dyn Runtime> = Arc::new(quinn::TokioRuntime);
    let mut endpoint =
        Endpoint::new_with_abstract_socket(EndpointConfig::default(), None, socket, runtime)
            .context("Failed to create QUIC client endpoint")?;

    let client_cfg = build_client_config(expected_fingerprint, transport_tuning)?;
    endpoint.set_default_client_config(client_cfg);
    Ok(endpoint)
}

fn build_client_config(
    expected_fingerprint: &str,
    transport_tuning: &TransportTuning,
) -> Result<ClientConfig> {
    let verifier = FingerprintVerifier::new(expected_fingerprint);
    let rustls_config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();

    let quic_cfg =
        QuicClientConfig::try_from(rustls_config).context("Failed to build QUIC client config")?;

    let mut client_config = ClientConfig::new(Arc::new(quic_cfg));

    let mut transport = create_base_transport_config(transport_tuning);
    // Client only uses bidirectional streams for tunnel data; disable
    // unidirectional streams since this protocol doesn't use them.
    transport.max_concurrent_uni_streams(0_u8.into());
    client_config.transport_config(Arc::new(transport));

    Ok(client_config)
}

fn create_congestion_controller_factory(
    controller: CongestionController,
) -> Arc<dyn ControllerFactory + Send + Sync> {
    match controller {
        CongestionController::Cubic => Arc::new(CubicConfig::default()),
        CongestionController::Bbr => Arc::new(BbrConfig::default()),
        CongestionController::NewReno => Arc::new(NewRenoConfig::default()),
    }
}

fn apply_transport_tuning(transport: &mut quinn::TransportConfig, tuning: &TransportTuning) {
    let factory = create_congestion_controller_factory(tuning.congestion_controller);
    transport.congestion_controller_factory(factory);

    let receive_window = tuning
        .receive_window
        .unwrap_or(DEFAULT_ICE_RECEIVE_WINDOW);
    transport.stream_receive_window(receive_window.into());
    transport.receive_window(receive_window.into());

    let send_window = tuning
        .send_window
        .unwrap_or(DEFAULT_ICE_SEND_WINDOW.max(receive_window));
    transport.send_window(send_window.into());

    info!(
        "ICE QUIC transport: cc={:?}, stream/receive={}KB, send={}KB",
        tuning.congestion_controller,
        receive_window / 1024,
        send_window / 1024
    );
}

fn cert_fingerprint_hex(cert_der: &[u8]) -> String {
    let digest = Sha256::digest(cert_der);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}

#[derive(Debug)]
struct FingerprintVerifier {
    expected: String,
    crypto: Arc<rustls::crypto::CryptoProvider>,
}

impl FingerprintVerifier {
    fn new(expected: &str) -> Arc<Self> {
        let expected = expected.to_lowercase();
        let crypto = rustls::crypto::aws_lc_rs::default_provider();
        Arc::new(Self {
            expected,
            crypto: Arc::new(crypto),
        })
    }

    fn matches(&self, cert: &CertificateDer<'_>) -> bool {
        let actual = cert_fingerprint_hex(cert.as_ref());
        actual == self.expected
    }
}

impl rustls::client::danger::ServerCertVerifier for FingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if !self.matches(end_entity) {
            return Err(rustls::Error::General("manual fingerprint mismatch".into()));
        }
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.crypto.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.crypto.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.crypto
            .signature_verification_algorithms
            .supported_schemes()
    }
}
