// This file is Copyright its original authors, visible in version control history.
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. You may not use this file except in
// accordance with one or both of these licenses.

//! LSPS4-aware router that creates LSPS4 blinded payment paths when LSPS4 config is set.

use std::sync::{Arc, Mutex, RwLock};

use bitcoin::secp256k1::{self, PublicKey, Secp256k1};

use lightning::blinded_path::payment::{
	BlindedPaymentPath, ForwardTlvs, PaymentConstraints, PaymentForwardNode, PaymentRelay,
	ReceiveTlvs,
};
use lightning::ln::channel_state::ChannelDetails;
use lightning::ln::channelmanager::{PaymentId, MIN_FINAL_CLTV_EXPIRY_DELTA};
use lightning::ln::msgs::DecodeError;
use lightning::routing::router::{DefaultRouter, InFlightHtlcs, Route, RouteParameters, Router};
use lightning::routing::scoring::ProbabilisticScoringFeeParameters;
use lightning::types::features::BlindedHopFeatures;
use lightning::types::payment::PaymentHash;
use lightning::util::ser::{Readable, Writeable, Writer};

use crate::logger::Logger;
use crate::types::{Graph, KeysManager, Scorer};

/// Configuration for creating LSPS4 blinded payment paths.
///
/// This is populated after LSPS4 registration with the LSP.
#[derive(Debug, Clone)]
pub struct LSPS4BlindedPathConfig {
	/// The LSP's node ID.
	pub lsp_node_id: PublicKey,
	/// The intercept SCID provided by the LSP during registration.
	pub intercept_scid: u64,
	/// The CLTV expiry delta for the LSPS4 channel.
	pub cltv_expiry_delta: u32,
}

impl Writeable for LSPS4BlindedPathConfig {
	fn write<W: Writer>(&self, writer: &mut W) -> Result<(), lightning::io::Error> {
		self.lsp_node_id.write(writer)?;
		self.intercept_scid.write(writer)?;
		self.cltv_expiry_delta.write(writer)?;
		Ok(())
	}
}

impl Readable for LSPS4BlindedPathConfig {
	fn read<R: lightning::io::Read>(reader: &mut R) -> Result<Self, DecodeError> {
		let lsp_node_id = Readable::read(reader)?;
		let intercept_scid = Readable::read(reader)?;
		let cltv_expiry_delta = Readable::read(reader)?;
		Ok(Self { lsp_node_id, intercept_scid, cltv_expiry_delta })
	}
}

/// The inner DefaultRouter type used by ldk-node.
type InnerRouter = DefaultRouter<
	Arc<Graph>,
	Arc<Logger>,
	Arc<KeysManager>,
	Arc<Mutex<Scorer>>,
	ProbabilisticScoringFeeParameters,
	Scorer,
>;

/// A router that wraps [`DefaultRouter`] and uses LSPS4 blinded payment paths
/// when LSPS4 config is set.
///
/// This enables BOLT12 offers to work with LSPS4 JIT channels, matching BOLT11 LSPS4
/// behavior: payments always route through the LSP's intercept SCID, and the LSP
/// decides at payment time whether to use an existing channel or open a new one.
pub struct LSPS4Router {
	inner: InnerRouter,
	lsps4_config: Arc<RwLock<Option<LSPS4BlindedPathConfig>>>,
	entropy_source: Arc<KeysManager>,
}

impl LSPS4Router {
	/// Creates a new LSPS4Router wrapping the given DefaultRouter.
	pub fn new(
		inner: InnerRouter, lsps4_config: Arc<RwLock<Option<LSPS4BlindedPathConfig>>>,
		entropy_source: Arc<KeysManager>,
	) -> Self {
		Self { inner, lsps4_config, entropy_source }
	}

	/// Returns a reference to the shared LSPS4 config.
	pub fn lsps4_config(&self) -> Arc<RwLock<Option<LSPS4BlindedPathConfig>>> {
		Arc::clone(&self.lsps4_config)
	}

	fn create_lsps4_blinded_path<T: secp256k1::Signing + secp256k1::Verification>(
		&self, config: &LSPS4BlindedPathConfig, recipient: PublicKey, tlvs: ReceiveTlvs,
		secp_ctx: &Secp256k1<T>,
	) -> Result<Vec<BlindedPaymentPath>, ()> {
		let forward_node = PaymentForwardNode {
			node_id: config.lsp_node_id,
			tlvs: ForwardTlvs {
				short_channel_id: config.intercept_scid,
				payment_relay: PaymentRelay {
					cltv_expiry_delta: config.cltv_expiry_delta as u16,
					// LSPS4 charges via channel opening, not routing fees
					fee_base_msat: 0,
					fee_proportional_millionths: 0,
				},
				payment_constraints: PaymentConstraints {
					max_cltv_expiry: tlvs
						.tlvs()
						.payment_constraints
						.max_cltv_expiry
						.saturating_add(config.cltv_expiry_delta),
					htlc_minimum_msat: 0,
				},
				next_blinding_override: None,
				features: BlindedHopFeatures::empty(),
			},
			htlc_maximum_msat: u64::MAX,
		};

		BlindedPaymentPath::new(
			&[forward_node],
			recipient,
			tlvs,
			u64::MAX,
			MIN_FINAL_CLTV_EXPIRY_DELTA,
			&*self.entropy_source,
			secp_ctx,
		)
		.map(|path| vec![path])
	}
}

impl Router for LSPS4Router {
	fn find_route(
		&self, payer: &PublicKey, route_params: &RouteParameters,
		first_hops: Option<&[&ChannelDetails]>, inflight_htlcs: InFlightHtlcs,
	) -> Result<Route, &'static str> {
		self.inner.find_route(payer, route_params, first_hops, inflight_htlcs)
	}

	fn find_route_with_id(
		&self, payer: &PublicKey, route_params: &RouteParameters,
		first_hops: Option<&[&ChannelDetails]>, inflight_htlcs: InFlightHtlcs,
		payment_hash: PaymentHash, payment_id: PaymentId,
	) -> Result<Route, &'static str> {
		self.inner.find_route_with_id(
			payer,
			route_params,
			first_hops,
			inflight_htlcs,
			payment_hash,
			payment_id,
		)
	}

	fn create_blinded_payment_paths<T: secp256k1::Signing + secp256k1::Verification>(
		&self, recipient: PublicKey, first_hops: Vec<ChannelDetails>, tlvs: ReceiveTlvs,
		amount_msats: Option<u64>, secp_ctx: &Secp256k1<T>,
	) -> Result<Vec<BlindedPaymentPath>, ()> {
		// If LSPS4 config is set, ALWAYS use LSPS4 blinded paths.
		// This matches BOLT11 LSPS4 behavior where payments always route through
		// the intercept_scid, and the LSP decides whether to use an existing
		// channel or open a new one.
		if let Some(config) = self.lsps4_config.read().unwrap().as_ref() {
			return self.create_lsps4_blinded_path(config, recipient, tlvs, secp_ctx);
		}

		// No LSPS4 config, use normal path creation
		self.inner.create_blinded_payment_paths(recipient, first_hops, tlvs, amount_msats, secp_ctx)
	}
}
