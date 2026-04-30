//! Test utilities for the transport crate.
//!
//! Provides in-process loopback pairs using two iroh endpoints with relay disabled.
//! Both endpoints bind explicitly on localhost so they can connect via direct IP.

use std::net::Ipv4Addr;

use iroh::{Endpoint, EndpointAddr};

use crate::{ALPN, connection::PairwireConnection};

/// Create a pair of connected `PairwireConnection`s using loopback QUIC.
///
/// Binds two iroh endpoints on 127.0.0.1 (relay disabled), connects them
/// to each other directly, and returns both sides. The iroh `Endpoint` objects
/// are intentionally leaked to keep the underlying QUIC state alive.
///
/// # Panics
///
/// Panics on bind or connect failure — this is test-only code.
pub async fn loopback_pair() -> (PairwireConnection, PairwireConnection) {
    // Bind each endpoint on loopback-only, relay disabled (empty_builder default).
    let ep_a = Endpoint::empty_builder()
        .clear_ip_transports()
        .bind_addr((Ipv4Addr::LOCALHOST, 0))
        .expect("endpoint A bind_addr failed")
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .expect("endpoint A bind failed");

    let ep_b = Endpoint::empty_builder()
        .clear_ip_transports()
        .bind_addr((Ipv4Addr::LOCALHOST, 0))
        .expect("endpoint B bind_addr failed")
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .expect("endpoint B bind failed");

    // Get endpoint B's actual bound address for A to connect to.
    let b_addr = {
        let sockets = ep_b.bound_sockets();
        let sock = sockets
            .into_iter()
            .next()
            .expect("endpoint B has no bound sockets");
        EndpointAddr::new(ep_b.id()).with_ip_addr(sock)
    };

    // Connect A → B and accept concurrently.
    let connect_fut = ep_a.connect(b_addr, ALPN);
    let accept_fut = async {
        ep_b.accept()
            .await
            .expect("B: no incoming connection")
            .await
            .expect("B: QUIC handshake failed")
    };

    let (conn_a_result, conn_b) = tokio::join!(connect_fut, accept_fut);
    let conn_a = conn_a_result.expect("A: connect failed");

    // Keep the endpoints alive: dropping them would close all QUIC connections.
    // In tests, this small leak is acceptable.
    std::mem::forget(ep_a);
    std::mem::forget(ep_b);

    (
        PairwireConnection::new(conn_a),
        PairwireConnection::new(conn_b),
    )
}
